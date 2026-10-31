//! Resolve local-time and leap-second overhead from the static config, the
//! host's IANA zone, or a user-specified IANA zone string.
//!
//! `LTM_OFF` is a 6-bit two's complement value in **half-hours** (per
//! C.S0005 §3.7.2.3.2.21); the range `[-32, 31]` covers `[-16h, +15.5h]`,
//! i.e. every real-world IANA zone with margin to spare. We still clamp
//! defensively in case a future tzdata transition drifts further.

use chrono::{DateTime, Datelike, NaiveDate, Offset, TimeZone, Utc};
use chrono_tz::{OffsetComponents, Tz};
use serde::{Deserialize, Serialize};

use crate::overhead::OverheadParameters;

/// Where the broadcast `LTM_OFF` / `DAYLT` come from.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum TimezoneSource {
    /// Use the values configured under `overhead.{ltm_off, daylt, lp_sec}`.
    #[default]
    Overhead,
    /// Resolve the host's IANA zone at sync-frame build time.
    System,
    /// Resolve a user-specified IANA zone (e.g. `"America/Los_Angeles"`).
    User { tz: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimezoneConfig {
    pub source: TimezoneSource,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct TimezoneConfigRaw {
    source: Option<String>,
    tz: Option<String>,
}

impl Serialize for TimezoneConfig {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let (source, tz) = match &self.source {
            TimezoneSource::Overhead => ("overhead", None),
            TimezoneSource::System => ("system", None),
            TimezoneSource::User { tz } => ("user", Some(tz.clone())),
        };
        TimezoneConfigRaw {
            source: Some(source.into()),
            tz,
        }
        .serialize(ser)
    }
}

impl<'de> Deserialize<'de> for TimezoneConfig {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let raw = TimezoneConfigRaw::deserialize(de)?;
        let source = match raw.source.as_deref().unwrap_or("overhead") {
            "overhead" => {
                if raw.tz.is_some() {
                    return Err(D::Error::custom(
                        "timezone.tz only allowed when source = \"user\"",
                    ));
                }
                TimezoneSource::Overhead
            }
            "system" => {
                if raw.tz.is_some() {
                    return Err(D::Error::custom(
                        "timezone.tz only allowed when source = \"user\"",
                    ));
                }
                TimezoneSource::System
            }
            "user" => {
                let tz = raw.tz.ok_or_else(|| {
                    D::Error::custom("timezone.tz is required when source = \"user\"")
                })?;
                TimezoneSource::User { tz }
            }
            other => {
                return Err(D::Error::custom(format!(
                    "timezone.source must be one of \"overhead\", \"system\", \"user\" (got {other:?})"
                )));
            }
        };
        Ok(TimezoneConfig { source })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedTimezone {
    pub ltm_off: i8,
    pub daylt: u8,
    pub lp_sec: u8,
    pub local_time_offset_minutes: i16,
}

/// Errors raised while validating or resolving a `TimezoneConfig`.
#[derive(Debug)]
pub enum TimezoneError {
    InvalidIana(String),
    SystemUnavailable(String),
}

impl std::fmt::Display for TimezoneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIana(s) => write!(f, "invalid IANA timezone: {s}"),
            Self::SystemUnavailable(s) => write!(f, "could not determine system timezone: {s}"),
        }
    }
}

impl std::error::Error for TimezoneError {}

/// Look up the host's IANA timezone name (e.g. `"America/Los_Angeles"`).
///
/// Returns `None` if the platform doesn't expose one.
pub fn host_iana_name() -> Option<String> {
    iana_time_zone::get_timezone().ok()
}

/// Validate that the configured source can be resolved at runtime. Called
/// at config-load time so misconfiguration fails fast — bad IANA names and
/// missing host tzdata are deployment errors, not runtime conditions.
///
/// - `Overhead`: always valid (uses static config values).
/// - `System`: probes the host IANA name; fails if unavailable.
/// - `User { tz }`: parses the tz string against `chrono-tz`'s embedded
///   tzdata.
pub fn validate(cfg: &TimezoneConfig) -> Result<(), TimezoneError> {
    match &cfg.source {
        TimezoneSource::Overhead => Ok(()),
        TimezoneSource::System => {
            let name = iana_time_zone::get_timezone()
                .map_err(|e| TimezoneError::SystemUnavailable(e.to_string()))?;
            name.parse::<Tz>()
                .map(|_| ())
                .map_err(|_| TimezoneError::InvalidIana(name))
        }
        TimezoneSource::User { tz } => tz
            .parse::<Tz>()
            .map(|_| ())
            .map_err(|_| TimezoneError::InvalidIana(tz.clone())),
    }
}

/// Resolve the broadcast values for the given moment.
///
/// `Overhead`: copy from the static overhead config (current pre-feature
/// behavior). `System` and `User` share one path: look up the IANA zone via
/// `chrono-tz` and read the standard + DST offset components directly from
/// the tzdata transitions baked into the crate.
pub fn resolve(
    cfg: &TimezoneConfig,
    overhead: &OverheadParameters,
    now_utc: DateTime<Utc>,
) -> ResolvedTimezone {
    let lp_sec = overhead.lp_sec;
    match &cfg.source {
        TimezoneSource::Overhead => ResolvedTimezone {
            ltm_off: overhead.ltm_off,
            daylt: overhead.daylt,
            lp_sec,
            local_time_offset_minutes: i16::from(overhead.ltm_off) * 30,
        },
        TimezoneSource::System => {
            // validate() guarantees host IANA db is available; treat
            // post-boot loss of tzdata as unrecoverable.
            let (ltm_off, daylt, local_time_offset_minutes) = resolve_system(now_utc)
                .expect("System tz lookup failed after validate() succeeded");
            ResolvedTimezone {
                ltm_off,
                daylt,
                lp_sec,
                local_time_offset_minutes,
            }
        }
        TimezoneSource::User { tz } => {
            // validate() guarantees the string parses; programmatic misuse
            // (constructing TimezoneConfig without validating) crashes loud.
            let zone: Tz = tz
                .parse()
                .unwrap_or_else(|_| panic!("invalid IANA timezone {tz:?} reached resolve()"));
            let (ltm_off, daylt, local_time_offset_minutes) = resolve_zone(zone, now_utc);
            ResolvedTimezone {
                ltm_off,
                daylt,
                lp_sec,
                local_time_offset_minutes,
            }
        }
    }
}

fn resolve_system(now_utc: DateTime<Utc>) -> Result<(i8, u8, i16), TimezoneError> {
    let name = iana_time_zone::get_timezone()
        .map_err(|e| TimezoneError::SystemUnavailable(e.to_string()))?;
    let zone: Tz = name.parse().map_err(|_| TimezoneError::InvalidIana(name))?;
    Ok(resolve_zone(zone, now_utc))
}

fn resolve_zone(zone: Tz, now_utc: DateTime<Utc>) -> (i8, u8, i16) {
    let local = zone.from_utc_datetime(&now_utc.naive_utc());
    let offset = local.offset();
    let total_secs = offset.fix().local_minus_utc();
    let dst_secs = offset.dst_offset().num_seconds();
    (
        half_hours_clamped(total_secs),
        if dst_secs != 0 { 1 } else { 0 },
        minutes_clamped(total_secs),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneDst {
    /// Standard-time offset in seconds, DST never applied. The FAP adds the DST
    /// hour itself from the fields below, so this stays constant all year.
    pub standard_offset_secs: i32,
    pub daylight_savings_used: bool,
    /// Local date of the spring-forward transition, if the zone has one.
    pub dst_start: Option<NaiveDate>,
    /// Local date of the fall-back transition, if the zone has one.
    pub dst_end: Option<NaiveDate>,
}

/// Standard offset and DST window for `zone` at `now_utc`. Transitions are
/// found by sampling each day at 12:00 UTC, clear of the overnight change.
pub fn zone_dst(zone: Tz, now_utc: DateTime<Utc>) -> ZoneDst {
    let local_now = zone.from_utc_datetime(&now_utc.naive_utc());
    let offset_now = local_now.offset();
    let standard_offset_secs =
        offset_now.fix().local_minus_utc() - offset_now.dst_offset().num_seconds() as i32;

    let year = local_now.year();
    let mut used = false;
    let mut dst_start = None;
    let mut dst_end = None;
    let mut prev: Option<bool> = None;
    let mut day = NaiveDate::from_ymd_opt(year, 1, 1).expect("valid Jan 1");
    let last = NaiveDate::from_ymd_opt(year, 12, 31).expect("valid Dec 31");
    while day <= last {
        let noon = day.and_hms_opt(12, 0, 0).expect("valid noon");
        let active = zone
            .from_utc_datetime(&noon)
            .offset()
            .dst_offset()
            .num_seconds()
            != 0;
        used |= active;
        if let Some(was) = prev {
            if active && !was {
                dst_start.get_or_insert(day);
            } else if !active && was {
                dst_end.get_or_insert(day);
            }
        }
        prev = Some(active);
        day = day.succ_opt().expect("date within year");
    }

    ZoneDst {
        standard_offset_secs,
        daylight_savings_used: used,
        dst_start,
        dst_end,
    }
}

/// [`zone_dst`] for an IANA zone name, returning `None` if it does not
/// parse against the embedded tzdata.
pub fn zone_dst_by_name(name: &str, now_utc: DateTime<Utc>) -> Option<ZoneDst> {
    Some(zone_dst(name.parse::<Tz>().ok()?, now_utc))
}

/// [`zone_dst_by_name`] evaluated at the current instant.
pub fn zone_dst_now(name: &str) -> Option<ZoneDst> {
    zone_dst_by_name(name, Utc::now())
}

fn minutes_clamped(secs: i32) -> i16 {
    let minutes = (secs as f64 / 60.0).round() as i32;
    minutes.clamp(-1024, 1023) as i16
}

/// Convert a UTC offset in seconds to LTM_OFF half-hours, rounded to
/// nearest and clamped to the 6-bit signed range `[-32, 31]`. The clamp is
/// defensive — every IANA zone fits inside `[-16h, +15.5h]` (Baker Island
/// through Kiribati).
fn half_hours_clamped(secs: i32) -> i8 {
    let hh = (secs as f64 / 1800.0).round() as i32;
    hh.clamp(-32, 31) as i8
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn overhead_with(ltm_off: i8, daylt: u8, lp_sec: u8) -> OverheadParameters {
        let mut o = OverheadParameters::default();
        o.ltm_off = ltm_off;
        o.daylt = daylt;
        o.lp_sec = lp_sec;
        o
    }

    #[test]
    fn overhead_source_returns_overhead_values() {
        let cfg = TimezoneConfig {
            source: TimezoneSource::Overhead,
        };
        let oh = overhead_with(-14, 0, 7);
        let r = resolve(
            &cfg,
            &oh,
            Utc.with_ymd_and_hms(2026, 5, 7, 0, 0, 0).unwrap(),
        );
        assert_eq!(
            r,
            ResolvedTimezone {
                ltm_off: -14,
                daylt: 0,
                lp_sec: 7,
                local_time_offset_minutes: -420,
            }
        );
    }

    #[test]
    fn user_la_january_is_pst() {
        let cfg = TimezoneConfig {
            source: TimezoneSource::User {
                tz: "America/Los_Angeles".into(),
            },
        };
        let oh = overhead_with(0, 0, 0);
        let r = resolve(&cfg, &oh, at("2026-01-15T12:00:00Z"));
        // PST = UTC-8 = -16 half-hours, no DST.
        assert_eq!(r.ltm_off, -16);
        assert_eq!(r.daylt, 0);
        assert_eq!(r.local_time_offset_minutes, -480);
    }

    #[test]
    fn user_la_july_is_pdt() {
        let cfg = TimezoneConfig {
            source: TimezoneSource::User {
                tz: "America/Los_Angeles".into(),
            },
        };
        let oh = overhead_with(0, 0, 0);
        let r = resolve(&cfg, &oh, at("2026-07-15T12:00:00Z"));
        // PDT = UTC-7 = -14 half-hours, DST active.
        assert_eq!(r.ltm_off, -14);
        assert_eq!(r.daylt, 1);
        assert_eq!(r.local_time_offset_minutes, -420);
    }

    #[test]
    fn user_southern_hemisphere_dst_in_january() {
        // Sydney AEDT = UTC+11 = +22 half-hours, AEST = UTC+10 = +20.
        // Both fit comfortably; verify daylt also flips with tzdata.
        let cfg = TimezoneConfig {
            source: TimezoneSource::User {
                tz: "Australia/Sydney".into(),
            },
        };
        let oh = overhead_with(0, 0, 0);
        let r_jan = resolve(&cfg, &oh, at("2026-01-15T12:00:00Z"));
        assert_eq!(r_jan.ltm_off, 22);
        assert_eq!(r_jan.daylt, 1);
        assert_eq!(r_jan.local_time_offset_minutes, 660);
        let r_jul = resolve(&cfg, &oh, at("2026-07-15T12:00:00Z"));
        assert_eq!(r_jul.ltm_off, 20);
        assert_eq!(r_jul.daylt, 0);
        assert_eq!(r_jul.local_time_offset_minutes, 600);
    }

    #[test]
    fn zone_dst_denver_has_mountain_dst() {
        let r = zone_dst(
            "America/Denver".parse().unwrap(),
            at("2026-10-30T18:30:00Z"),
        );
        assert_eq!(r.standard_offset_secs, -7 * 3600);
        assert!(r.daylight_savings_used);
        assert_eq!(r.dst_start, NaiveDate::from_ymd_opt(2026, 3, 8));
        assert_eq!(r.dst_end, NaiveDate::from_ymd_opt(2026, 11, 1));
    }

    #[test]
    fn zone_dst_phoenix_has_no_dst() {
        let r = zone_dst(
            "America/Phoenix".parse().unwrap(),
            at("2026-07-15T12:00:00Z"),
        );
        assert_eq!(r.standard_offset_secs, -7 * 3600);
        assert!(!r.daylight_savings_used);
        assert_eq!(r.dst_start, None);
        assert_eq!(r.dst_end, None);
    }

    #[test]
    fn hrpd_offset_preserves_whole_minutes() {
        let cfg = TimezoneConfig {
            source: TimezoneSource::User {
                tz: "Asia/Kathmandu".into(),
            },
        };
        let r = resolve(&cfg, &overhead_with(0, 0, 18), at("2026-07-15T12:00:00Z"));
        assert_eq!(r.ltm_off, 12);
        assert_eq!(r.local_time_offset_minutes, 345);
    }

    #[test]
    #[should_panic(expected = "invalid IANA timezone")]
    fn user_invalid_tz_panics_in_resolve() {
        // Programmatic misuse: bypass validate() with an unparseable name.
        // resolve() must crash loud rather than silently broadcast wrong
        // local time.
        let cfg = TimezoneConfig {
            source: TimezoneSource::User {
                tz: "Not/AZone".into(),
            },
        };
        let oh = overhead_with(-14, 1, 3);
        let _ = resolve(&cfg, &oh, at("2026-05-07T12:00:00Z"));
    }

    #[test]
    fn lp_sec_always_comes_from_overhead() {
        // LP_SEC lives only on the static overhead config; the timezone
        // section never overrides it.
        let cfg = TimezoneConfig {
            source: TimezoneSource::Overhead,
        };
        let oh = overhead_with(-14, 0, 18);
        let r = resolve(&cfg, &oh, at("2026-05-07T12:00:00Z"));
        assert_eq!(r.lp_sec, 18);
    }

    #[test]
    fn validate_rejects_bad_iana() {
        let cfg = TimezoneConfig {
            source: TimezoneSource::User { tz: "bogus".into() },
        };
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn deserializes_all_documented_json_shapes() {
        let c: TimezoneConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(c.source, TimezoneSource::Overhead);

        let c: TimezoneConfig = serde_json::from_str(r#"{"source":"overhead"}"#).unwrap();
        assert_eq!(c.source, TimezoneSource::Overhead);

        let c: TimezoneConfig = serde_json::from_str(r#"{"source":"system"}"#).unwrap();
        assert_eq!(c.source, TimezoneSource::System);

        let c: TimezoneConfig =
            serde_json::from_str(r#"{"source":"user","tz":"America/Los_Angeles"}"#).unwrap();
        assert_eq!(
            c.source,
            TimezoneSource::User {
                tz: "America/Los_Angeles".into()
            }
        );
    }

    #[test]
    fn rejects_unknown_lp_sec_field_on_timezone_block() {
        // LP_SEC is not a timezone field. A user setting it under
        // `timezone` is misconfigured and should fail at load.
        let err = serde_json::from_str::<TimezoneConfig>(
            r#"{"source":"user","tz":"America/New_York","lp_sec":18}"#,
        )
        .expect_err("expected unknown-field error");
        assert!(
            err.to_string().contains("lp_sec"),
            "error should mention the offending field: {err}"
        );
    }

    #[test]
    fn validate_accepts_overhead_unconditionally() {
        // System is now host-dependent (probes iana_time_zone), so we don't
        // assert it here — CI hosts may or may not have tzdata configured.
        assert!(
            validate(&TimezoneConfig {
                source: TimezoneSource::Overhead,
            })
            .is_ok()
        );
    }

    #[test]
    fn half_hours_rounds_and_clamps() {
        assert_eq!(half_hours_clamped(0), 0);
        assert_eq!(half_hours_clamped(-8 * 3600), -16); // PST
        assert_eq!(half_hours_clamped(-7 * 3600), -14); // PDT
        assert_eq!(half_hours_clamped(5 * 3600 + 30 * 60), 11); // India +5:30
        assert_eq!(half_hours_clamped(5 * 3600 + 45 * 60), 12); // Nepal +5:45 rounds up
        assert_eq!(half_hours_clamped(14 * 3600), 28); // Kiribati Line +14
        assert_eq!(half_hours_clamped(-12 * 3600), -24); // Baker -12
        // The 6-bit signed range itself ([-32,31] = [-16h,+15.5h]) covers
        // every real zone; clamp paths shown for defensiveness.
        assert_eq!(half_hours_clamped(20 * 3600), 31);
        assert_eq!(half_hours_clamped(-20 * 3600), -32);
    }
}
