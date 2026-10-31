use super::*;
use crate::config::TimezoneField;
use chrono::NaiveDate;
use std::sync::OnceLock;
use tzf_rs::DefaultFinder;

pub(super) fn leaf_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

pub(super) fn gps_degrees_minutes_to_decimal(value: &str) -> Option<String> {
    let signed = value.parse::<f64>().ok()?;
    if !signed.is_finite() {
        return None;
    }
    let absolute = signed.abs();
    let degrees = (absolute / 100.0).floor();
    let minutes = absolute - degrees * 100.0;
    if minutes >= 60.0 {
        return None;
    }
    let decimal = degrees + minutes / 60.0;
    let sign = if signed.is_sign_negative() { '-' } else { '+' };
    Some(format!("{sign}{decimal:.6}"))
}

pub(super) fn gps_longitude_to_timezone(value: &str) -> Option<String> {
    let decimal = gps_degrees_minutes_to_decimal(value)?.parse::<f64>().ok()?;
    let offset = (decimal / DEGREES_PER_TIMEZONE_HOUR).round() as i32;
    Some(format!("{offset:+03}:00 {}", timezone_abbreviation(offset)))
}

fn timezone_abbreviation(offset: i32) -> &'static str {
    match offset {
        -10 => "HST",
        -9 => "AKST",
        -8 => "PST",
        -7 => "MST",
        -6 => "CST",
        -5 => "EST",
        _ => "UTC",
    }
}

/// Format a UTC offset in seconds as a CWMP `LocalTimeZone` string
/// (`"+HH:MM ABBR"`, or `"+HH:MM"` for a sub-hour offset). The Airvana
/// format-validates the label length then derives the displayed label from the
/// numeric offset, so the abbreviation here is cosmetic.
pub(super) fn offset_secs_to_cwmp_timezone(secs: i32) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let abs = secs.unsigned_abs();
    let offset = format!("{sign}{:02}:{:02}", abs / 3600, (abs % 3600) / 60);
    if abs % 3600 == 0 {
        format!("{offset} {}", timezone_abbreviation(secs / 3600))
    } else {
        offset
    }
}

/// Format a DST transition date as the CWMP `DaylightSavings{Start,End}`
/// dateTime. The Airvana stores the local wall-clock transition, 02:00 by US
/// convention, carried with a `Z` suffix.
pub(super) fn dst_date_to_cwmp(date: NaiveDate) -> String {
    format!("{}T02:00:00.000Z", date.format("%Y-%m-%d"))
}

static TZ_FINDER: OnceLock<DefaultFinder> = OnceLock::new();

/// Resolve the IANA zone for a FAP GPS position given as Airvana
/// degrees-minutes strings (latitude then longitude). Returns `None` if either
/// coordinate does not parse or the position falls outside the boundary set.
pub(super) fn gps_latlon_to_iana(latitude: &str, longitude: &str) -> Option<String> {
    let lat = gps_degrees_minutes_to_decimal(latitude)?
        .parse::<f64>()
        .ok()?;
    let lon = gps_degrees_minutes_to_decimal(longitude)?
        .parse::<f64>()
        .ok()?;
    let name = TZ_FINDER
        .get_or_init(DefaultFinder::new)
        .get_tz_name(lon, lat);
    (!name.is_empty()).then(|| name.to_string())
}

/// Derive one field of the CWMP `InternetGatewayDevice.Time.*` set from an IANA
/// zone name, evaluated now so the result tracks DST transitions across
/// provisioning cycles.
pub(super) fn cwmp_timezone_value(zone_name: &str, field: TimezoneField) -> Option<String> {
    let info = cdma_common::timezone::zone_dst_now(zone_name)?;
    Some(match field {
        TimezoneField::LocalTimeZone => offset_secs_to_cwmp_timezone(info.standard_offset_secs),
        TimezoneField::DaylightSavingsUsed => {
            if info.daylight_savings_used { "1" } else { "0" }.to_string()
        }
        TimezoneField::DaylightSavingsStart => dst_date_to_cwmp(info.dst_start?),
        TimezoneField::DaylightSavingsEnd => dst_date_to_cwmp(info.dst_end?),
    })
}

pub(super) fn gps_distance_meters(
    latitude: f64,
    longitude: f64,
    reference_latitude: f64,
    reference_longitude: f64,
) -> f64 {
    const EARTH_RADIUS_METERS: f64 = 6_371_000.0;
    let latitude = latitude.to_radians();
    let reference_latitude = reference_latitude.to_radians();
    let latitude_delta = reference_latitude - latitude;
    let longitude_delta = (reference_longitude - longitude).to_radians();
    let haversine = (latitude_delta / 2.0).sin().powi(2)
        + latitude.cos() * reference_latitude.cos() * (longitude_delta / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_METERS * haversine.sqrt().asin()
}

pub(super) fn cwmp_values_match(name: &str, expected: &str, actual: &str) -> bool {
    let expected = expected.trim();
    let actual = actual.trim();
    if expected == actual {
        return true;
    }
    if !name.ends_with(ADMIN_STATE_SUFFIX) {
        return false;
    }
    let normalized = |value: &str| match value.to_ascii_lowercase().as_str() {
        "lock" | "locked" => Some(false),
        "unlock" | "unlocked" => Some(true),
        _ => None,
    };
    normalized(expected).is_some_and(|value| normalized(actual) == Some(value))
}

pub(super) fn administrative_states(expected: &str, actual: &str) -> Option<(bool, bool)> {
    let state = |value: &str| match value.trim().to_ascii_lowercase().as_str() {
        "lock" | "locked" => Some(false),
        "unlock" | "unlocked" => Some(true),
        _ => None,
    };
    state(expected).zip(state(actual))
}

pub(super) fn is_sensitive_parameter(name: &str) -> bool {
    let leaf = name.rsplit('.').next().unwrap_or(name).to_ascii_lowercase();
    leaf.contains("password") || leaf.ends_with("securitykey") || leaf.ends_with("psk")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gps_longitude_maps_to_rough_local_time_zone() {
        assert_eq!(
            gps_longitude_to_timezone("-11814.400000").as_deref(),
            Some("-08:00 PST")
        );
        assert_eq!(
            gps_longitude_to_timezone("-11142.963867").as_deref(),
            Some("-07:00 MST")
        );
        assert_eq!(
            gps_longitude_to_timezone("+00000.000000").as_deref(),
            Some("+00:00 UTC")
        );
        assert_eq!(gps_longitude_to_timezone("not-a-number"), None);
    }

    #[test]
    fn rejects_invalid_gps_degrees_minutes() {
        assert_eq!(gps_degrees_minutes_to_decimal("+3360.000000"), None);
        assert_eq!(
            gps_degrees_minutes_to_decimal("-07530.000000").as_deref(),
            Some("-75.500000")
        );
    }

    #[test]
    fn formats_offset_as_cwmp_timezone() {
        assert_eq!(offset_secs_to_cwmp_timezone(-7 * 3600), "-07:00 MST");
        assert_eq!(offset_secs_to_cwmp_timezone(-6 * 3600), "-06:00 CST");
        assert_eq!(offset_secs_to_cwmp_timezone(0), "+00:00 UTC");
        assert_eq!(offset_secs_to_cwmp_timezone(5 * 3600 + 1800), "+05:30");
    }

    #[test]
    fn formats_dst_transition_date() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
        assert_eq!(dst_date_to_cwmp(date), "2026-03-08T02:00:00.000Z");
    }

    #[test]
    fn resolves_gps_position_to_iana_zone() {
        assert_eq!(
            gps_latlon_to_iana("+4042.000000", "-7400.000000").as_deref(),
            Some("America/New_York")
        );
        assert_eq!(gps_latlon_to_iana("not-a-number", "-7400.000000"), None);
    }

    #[test]
    fn compares_airave_administrative_states_semantically() {
        let one_x = "Device.OneX.AdminState";
        let sector = "Device.OneX.OneXSectorAdminState";

        assert!(cwmp_values_match(one_x, "unlock", "Unlocked"));
        assert!(cwmp_values_match(sector, "LOCK", "locked"));
        assert!(!cwmp_values_match(one_x, "unlock", "locked"));
        assert!(!cwmp_values_match("Device.Mode", "unlock", "Unlocked"));
    }
}
