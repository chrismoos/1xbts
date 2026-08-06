use cdma_common::consts::{
    SERVICE_OPTION_BASIC_VOICE, SERVICE_OPTION_HIGH_RATE_PACKET_DATA, SERVICE_OPTION_PACKET_DATA,
    SERVICE_OPTION_QCELP13,
};
use cdma_common::lac::paging_messages::{self as paging_messages, MsAddress, MsPageAddress};
use log::{info, warn};

use crate::config::TrafficAssignmentConfig;

pub(crate) fn is_packet_data_so(so: u16) -> bool {
    matches!(
        so,
        SERVICE_OPTION_PACKET_DATA | SERVICE_OPTION_HIGH_RATE_PACKET_DATA
    )
}

/// F-SCH eligibility predicate.
///
/// Returns `true` when an F-SCH should be advertised in Service Connect and
/// later allocated through the Abis Burst procedure. All conditions must hold:
/// the operator has flipped `enable_f_sch=true`, the call is SO33 packet data,
/// the negotiated RC is RC3+, the mobile reports MOB_P_REV ≥ 6, and the mobile
/// actually advertises RC3 in its capabilities.
pub(crate) fn ms_eligible_for_fsch_phase1(
    enable_f_sch: bool,
    service_option: u16,
    for_rc: u8,
    mob_p_rev: u8,
    for_preferred_rc: Option<u8>,
    for_supported_rcs: &[u8],
) -> bool {
    enable_f_sch
        && service_option == SERVICE_OPTION_HIGH_RATE_PACKET_DATA
        && for_rc >= 3
        && mob_p_rev >= 6
        && (for_preferred_rc == Some(3) || for_supported_rcs.contains(&3))
}

/// Select the initial explicit RC pair for ECAM based on the mobile's
/// supported Radio Configurations, stated preferences, and the configured
/// base-station policy.
///
/// Returns `None` if no implemented pair is allowed by both the mobile and
/// the configured policy.
pub(crate) fn select_initial_traffic_rcs(
    policy: &TrafficAssignmentConfig,
    for_rcs: &[u8],
    rev_rcs: &[u8],
    for_pref: Option<u8>,
    rev_pref: Option<u8>,
    mob_p_rev: u8,
) -> Option<(u8, u8)> {
    // Force IS-95 into RC1
    let pre_is2000 = mob_p_rev < 6;
    let for_has = |rc: u8| {
        if pre_is2000 {
            rc == 1
        } else {
            for_rcs.is_empty() || for_rcs.contains(&rc)
        }
    };
    let rev_has = |rc: u8| {
        if pre_is2000 {
            rc == 1
        } else {
            rev_rcs.is_empty() || rev_rcs.contains(&rc)
        }
    };
    let allowed_by_policy = |pair: (u8, u8)| {
        matches!(pair, (1, 1) | (2, 2) | (3, 3))
            && policy.supported_for_rcs.contains(&pair.0)
            && policy.supported_rev_rcs.contains(&pair.1)
    };
    let supported_by_mobile = |pair: (u8, u8)| for_has(pair.0) && rev_has(pair.1);

    // Pass 1: try explicit RC negotiation from the mobile's advertised capabilities.
    for pair in policy
        .preferred_pairs
        .iter()
        .map(|pair| (pair.for_rc, pair.rev_rc))
    {
        if allowed_by_policy(pair) && supported_by_mobile(pair) {
            return Some(pair);
        }
    }

    if let (Some(for_pref), Some(rev_pref)) = (for_pref, rev_pref) {
        let pair = (for_pref, rev_pref);
        if allowed_by_policy(pair) && supported_by_mobile(pair) {
            return Some(pair);
        }
    }

    for pair in [(1, 1), (2, 2), (3, 3)] {
        if allowed_by_policy(pair) && supported_by_mobile(pair) {
            return Some(pair);
        }
    }

    // Pass 2: mob_p_rev >= 6 mobiles implicitly support RC1 and RC3 as
    // baseline configurations even when the FCH capability record only lists
    // higher-rate RCs.  Fall back to the first preferred pair that the policy
    // allows.
    if mob_p_rev >= 6 {
        for pair in policy.preferred_pairs.iter().map(|p| (p.for_rc, p.rev_rc)) {
            if allowed_by_policy(pair) {
                info!(
                    "BSC: mobile for_rcs={:?} rev_rcs={:?} did not explicitly list RC{}/{}, \
                     falling back to preferred pair (mob_p_rev={} implies baseline support)",
                    for_rcs, rev_rcs, pair.0, pair.1, mob_p_rev
                );
                return Some(pair);
            }
        }
    }

    warn!(
        "BSC: no matching implemented RC pair between policy for_rcs={:?} rev_rcs={:?} preferred_pairs={:?} and mobile for_rcs={:?} rev_rcs={:?} prefs=({:?},{:?}) mob_p_rev={}",
        policy.supported_for_rcs,
        policy.supported_rev_rcs,
        policy.preferred_pairs,
        for_rcs,
        rev_rcs,
        for_pref,
        rev_pref,
        mob_p_rev,
    );
    None
}

/// Service-option-aware wrapper around [`select_initial_traffic_rcs`].
///
/// SO1 uses the legacy RC1 configuration. QCELP-13K requires RC2 in both
/// directions.
pub(crate) fn select_initial_traffic_rcs_for_so(
    policy: &TrafficAssignmentConfig,
    for_rcs: &[u8],
    rev_rcs: &[u8],
    for_pref: Option<u8>,
    rev_pref: Option<u8>,
    mob_p_rev: u8,
    service_option: Option<u16>,
) -> Option<(u8, u8)> {
    if service_option == Some(SERVICE_OPTION_BASIC_VOICE) {
        if policy.supported_for_rcs.contains(&1) && policy.supported_rev_rcs.contains(&1) {
            return Some((1, 1));
        }
        warn!(
            "BSC: basic voice (SO1) requested but RC1 is disabled by policy (for={:?} rev={:?})",
            policy.supported_for_rcs, policy.supported_rev_rcs,
        );
        return None;
    }
    if service_option == Some(SERVICE_OPTION_QCELP13) {
        let admits_rc2_policy =
            policy.supported_for_rcs.contains(&2) && policy.supported_rev_rcs.contains(&2);
        let admits_rc2_mobile = mob_p_rev >= 3
            && (mob_p_rev < 6
                || ((for_rcs.is_empty() || for_rcs.contains(&2))
                    && (rev_rcs.is_empty() || rev_rcs.contains(&2))));
        if admits_rc2_policy && admits_rc2_mobile {
            return Some((2, 2));
        }
        warn!(
            "BSC: QCELP-13K (SO {}) requested but RC2 not admissible (policy for={:?} rev={:?} mobile for={:?} rev={:?} mob_p_rev={})",
            SERVICE_OPTION_QCELP13,
            policy.supported_for_rcs,
            policy.supported_rev_rcs,
            for_rcs,
            rev_rcs,
            mob_p_rev,
        );
        return None;
    }
    select_initial_traffic_rcs(policy, for_rcs, rev_rcs, for_pref, rev_pref, mob_p_rev)
}

/// Resolve IMSI class-0 forward-link address from access event fields.
///
/// Per C.S0005-E 2.6.2.2.5: when the mobile omits MCC or IMSI_11_12
/// (`None`), it means the value equals current overhead — the mobile
/// compared its MCC_O_S / IMSI_O_11_12_S against MCC_S / IMSI_11_12_S
/// and found a match. We reconstruct by substituting overhead.
///
/// A roaming mobile whose MCC_O differs from overhead will always send
/// MCC explicitly (`Some(foreign_mcc)`), per C.S0004-E 2.1.1.3.1.3
/// IMSI_CLASS_0_TYPE = '10' or '11'.
pub(crate) fn select_imsi_class0_forward_address(
    imsi_m_s1: u32,
    imsi_m_s2: u16,
    imsi_mcc: Option<u16>,
    imsi_11_12: Option<u8>,
    overhead_mcc: u16,
    overhead_imsi_11_12: u8,
) -> MsAddress {
    let resolved_mcc = imsi_mcc.unwrap_or(overhead_mcc);
    let resolved_imsi_11_12 = imsi_11_12.unwrap_or(overhead_imsi_11_12);
    paging_messages::select_imsi_class0_forward_address(
        imsi_m_s1,
        imsi_m_s2,
        resolved_mcc,
        resolved_imsi_11_12,
    )
}

pub(crate) fn format_ms_address(addr: &MsAddress) -> String {
    match addr {
        MsAddress::Esn(esn) => format!("ESN:0x{:08X}", esn),
        MsAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
        } => format!("IMSI_S:s1={},s2={}", imsi_m_s1, imsi_m_s2),
        MsAddress::ImsiClass0 {
            imsi_m_s1,
            imsi_m_s2,
            mcc,
            imsi_11_12,
        } => {
            format!(
                "IMSI_CLASS0:s1={},s2={},mcc={},imsi_11_12={}",
                imsi_m_s1, imsi_m_s2, mcc, imsi_11_12
            )
        }
    }
}

pub(crate) fn parse_sms_target_address(target: &str) -> Option<MsAddress> {
    let target = target.trim();

    if let Some(rest) = target.strip_prefix("ESN:0x") {
        let esn = u32::from_str_radix(rest, 16).ok()?;
        return Some(MsAddress::Esn(esn));
    }

    if let Some(rest) = target.strip_prefix("ESN:") {
        let esn = rest.parse::<u32>().ok()?;
        return Some(MsAddress::Esn(esn));
    }

    if let Some(rest) = target.strip_prefix("IMSI_S:") {
        let mut s1 = None;
        let mut s2 = None;
        for part in rest.split(',') {
            let mut kv = part.splitn(2, '=');
            let key = kv.next()?.trim();
            let value = kv.next()?.trim();
            match key {
                "s1" => s1 = value.parse::<u32>().ok(),
                "s2" => s2 = value.parse::<u16>().ok(),
                _ => {}
            }
        }
        return Some(MsAddress::ImsiS {
            imsi_m_s1: s1?,
            imsi_m_s2: s2?,
        });
    }

    if let Some(rest) = target.strip_prefix("IMSI_CLASS0:") {
        let mut s1 = None;
        let mut s2 = None;
        let mut mcc = None;
        let mut imsi_11_12 = None;
        for part in rest.split(',') {
            let mut kv = part.splitn(2, '=');
            let key = kv.next()?.trim();
            let value = kv.next()?.trim();
            match key {
                "s1" => s1 = value.parse::<u32>().ok(),
                "s2" => s2 = value.parse::<u16>().ok(),
                "mcc" => mcc = value.parse::<u16>().ok(),
                "imsi_11_12" => imsi_11_12 = value.parse::<u8>().ok(),
                _ => {}
            }
        }
        return Some(MsAddress::ImsiClass0 {
            imsi_m_s1: s1?,
            imsi_m_s2: s2?,
            mcc: mcc?,
            imsi_11_12: imsi_11_12?,
        });
    }

    None
}

pub(crate) fn format_ms_page_address(addr: &MsPageAddress) -> String {
    match addr {
        MsPageAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
            mcc,
            imsi_11_12,
        } => {
            let imsi_s = ((*imsi_m_s2 as u64) << 24) | (*imsi_m_s1 as u64);
            let mut parts = vec![format!("IMSI_S:0x{:09X}", imsi_s)];
            if let Some(mcc) = mcc {
                parts.push(format!("mcc={}", mcc));
            }
            if let Some(imsi_11_12) = imsi_11_12 {
                parts.push(format!("imsi_11_12={}", imsi_11_12));
            }
            parts.join(" ")
        }
        MsPageAddress::Esn(esn) => format!("ESN:0x{:08X}", esn),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RcPairConfig, TrafficAssignmentConfig};

    // ---- is_packet_data_so ----

    #[test]
    fn packet_data_so_7_and_33() {
        assert!(is_packet_data_so(7));
        assert!(is_packet_data_so(33));
    }

    #[test]
    fn non_packet_data_sos() {
        assert!(!is_packet_data_so(0));
        assert!(!is_packet_data_so(1));
        assert!(!is_packet_data_so(6));
        assert!(!is_packet_data_so(32));
        assert!(!is_packet_data_so(34));
    }

    // ---- format_ms_address / parse_sms_target_address round-trips ----

    #[test]
    fn format_and_parse_esn_hex() {
        let addr = MsAddress::Esn(0xDEADBEEF);
        let formatted = format_ms_address(&addr);
        assert_eq!(formatted, "ESN:0xDEADBEEF");
        let parsed = parse_sms_target_address(&formatted).unwrap();
        assert_eq!(parsed, addr);
    }

    #[test]
    fn format_and_parse_imsi_s() {
        let addr = MsAddress::ImsiS {
            imsi_m_s1: 123456,
            imsi_m_s2: 789,
        };
        let formatted = format_ms_address(&addr);
        assert_eq!(formatted, "IMSI_S:s1=123456,s2=789");
        let parsed = parse_sms_target_address(&formatted).unwrap();
        assert_eq!(parsed, addr);
    }

    #[test]
    fn format_and_parse_imsi_class0_minimal() {
        let addr = MsAddress::ImsiClass0 {
            imsi_m_s1: 100,
            imsi_m_s2: 200,
            mcc: 310,
            imsi_11_12: 0,
        };
        let formatted = format_ms_address(&addr);
        assert!(formatted.contains("mcc=310"));
        assert!(formatted.contains("imsi_11_12=0"));
        let parsed = parse_sms_target_address(&formatted).unwrap();
        assert_eq!(parsed, addr);
    }

    #[test]
    fn format_and_parse_imsi_class0_with_mcc_and_imsi_11_12() {
        let addr = MsAddress::ImsiClass0 {
            imsi_m_s1: 100,
            imsi_m_s2: 200,
            mcc: 310,
            imsi_11_12: 15,
        };
        let formatted = format_ms_address(&addr);
        assert!(formatted.contains("mcc=310"));
        assert!(formatted.contains("imsi_11_12=15"));
        let parsed = parse_sms_target_address(&formatted).unwrap();
        assert_eq!(parsed, addr);
    }

    #[test]
    fn parse_esn_decimal() {
        let parsed = parse_sms_target_address("ESN:12345").unwrap();
        assert_eq!(parsed, MsAddress::Esn(12345));
    }

    #[test]
    fn parse_whitespace_trimmed() {
        let parsed = parse_sms_target_address("  ESN:0xAB  ").unwrap();
        assert_eq!(parsed, MsAddress::Esn(0xAB));
    }

    #[test]
    fn parse_unknown_prefix_returns_none() {
        assert!(parse_sms_target_address("TMSI:1234").is_none());
        assert!(parse_sms_target_address("").is_none());
        assert!(parse_sms_target_address("garbage").is_none());
    }

    #[test]
    fn parse_imsi_s_missing_field_returns_none() {
        assert!(parse_sms_target_address("IMSI_S:s1=100").is_none());
        assert!(parse_sms_target_address("IMSI_S:s2=200").is_none());
    }

    // ---- format_ms_page_address ----

    #[test]
    fn format_page_address_esn() {
        let addr = MsPageAddress::Esn(0x12345678);
        assert_eq!(format_ms_page_address(&addr), "ESN:0x12345678");
    }

    #[test]
    fn format_page_address_imsi_s() {
        let addr = MsPageAddress::ImsiS {
            imsi_m_s1: 0x00ABCDEF,
            imsi_m_s2: 0x0123,
            mcc: None,
            imsi_11_12: None,
        };
        let formatted = format_ms_page_address(&addr);
        assert!(formatted.starts_with("IMSI_S:0x"));
    }

    // ---- select_imsi_class0_forward_address ----
    //
    // Per C.S0004-E 2.1.1.3.1.3 and C.S0005-E 2.6.2.2.5:
    //
    // The mobile compares its operational IMSI_O fields (MCC_O_S,
    // IMSI_O_11_12_S) against stored overhead (MCC_S, IMSI_11_12_S)
    // and omits fields that match.  None in access event = "equals
    // overhead."  The BSC wrapper resolves None→overhead before
    // calling the core function, which is now pure compression.
    //
    // C.S0004-E Table 2.1.1.3.1.1-2:
    //   type 00 — IMSI_S only             (both implied by overhead)
    //   type 01 — IMSI_S + IMSI_11_12     (MCC implied, IMSI_11_12 differs)
    //   type 10 — IMSI_S + MCC            (IMSI_11_12 implied, MCC differs)
    //   type 11 — IMSI_S + MCC + IMSI_11_12 (both differ)

    #[test]
    fn class0_type00_both_implied_by_wildcard_overhead() {
        // MS sends MCC=310, IMSI_11_12=15; overhead is all-wildcard.
        // Resolved address stores actual values; OTA compression deferred to write_to.
        let addr = select_imsi_class0_forward_address(100, 200, Some(310), Some(15), 0x03ff, 0x7f);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 310,
                imsi_11_12: 15,
            }
        );
    }

    #[test]
    fn class0_type00_both_implied_by_matching_overhead() {
        // MS MCC and IMSI_11_12 match non-wildcard overhead exactly.
        let addr = select_imsi_class0_forward_address(100, 200, Some(310), Some(15), 310, 15);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 310,
                imsi_11_12: 15,
            }
        );
    }

    #[test]
    fn class0_type00_home_subscriber_omits_both_non_wildcard_overhead() {
        // Per C.S0004-E 2.1.1.3.1.3: home subscriber on non-wildcard cell.
        // Mobile omits MCC and IMSI_11_12 (None = "equals overhead").
        // BSC resolves None→overhead(310,15).
        let addr = select_imsi_class0_forward_address(100, 200, None, None, 310, 15);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 310,
                imsi_11_12: 15,
            }
        );
    }

    #[test]
    fn class0_type01_imsi_11_12_differs_from_overhead() {
        // MS MCC matches overhead but IMSI_11_12 differs.
        let addr = select_imsi_class0_forward_address(100, 200, Some(310), Some(42), 310, 15);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 310,
                imsi_11_12: 42,
            }
        );
    }

    #[test]
    fn class0_type10_roamer_mcc_differs_imsi_11_12_matches() {
        // Per C.S0004-E 2.1.1.3.1.3 IMSI_CLASS_0_TYPE='10':
        // Roaming MS (MCC_O=450, Korean) on US cell (MCC=310).
        // MCC_O ≠ MCC_S → MS sends MCC=450 explicitly.
        let addr = select_imsi_class0_forward_address(100, 200, Some(450), Some(15), 310, 0x7f);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 450,
                imsi_11_12: 15,
            }
        );
    }

    #[test]
    fn class0_type10_roamer_mcc_differs_imsi_11_12_matches_non_wildcard() {
        // Roamer MCC differs, but IMSI_11_12 happens to match overhead.
        let addr = select_imsi_class0_forward_address(100, 200, Some(450), Some(15), 310, 15);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 450,
                imsi_11_12: 15,
            }
        );
    }

    #[test]
    fn class0_type11_roamer_both_differ() {
        // Roaming MS: MCC and IMSI_11_12 both differ from overhead.
        let addr = select_imsi_class0_forward_address(100, 200, Some(450), Some(42), 310, 15);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 450,
                imsi_11_12: 42,
            }
        );
    }

    #[test]
    fn class0_roamer_omits_imsi_11_12_only() {
        // Roamer sends MCC explicitly (differs), omits IMSI_11_12 (matches overhead).
        // None for IMSI_11_12 resolves to overhead=15.
        let addr = select_imsi_class0_forward_address(100, 200, Some(450), None, 310, 15);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 450,
                imsi_11_12: 15,
            }
        );
    }

    #[test]
    fn class0_wildcard_overhead_mobile_omits_both() {
        // Per C.S0005-E 2.6.2.2.5: when both MCC_r and IMSI_11_12_r are
        // wildcard, mobile uses programmed IMSI_M values and always sends
        // them explicitly. But if the mobile somehow omits them, resolve
        // to wildcard overhead values.
        let addr = select_imsi_class0_forward_address(100, 200, None, None, 0x03ff, 0x7f);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 0x03ff,
                imsi_11_12: 0x7f,
            }
        );
    }

    // ---- select_initial_traffic_rcs ----

    fn default_policy() -> TrafficAssignmentConfig {
        TrafficAssignmentConfig::default()
    }

    fn rc1_only_policy() -> TrafficAssignmentConfig {
        TrafficAssignmentConfig {
            supported_for_rcs: vec![1],
            supported_rev_rcs: vec![1],
            preferred_pairs: vec![RcPairConfig::new(1, 1)],
            ..Default::default()
        }
    }

    fn rc3_only_policy() -> TrafficAssignmentConfig {
        TrafficAssignmentConfig {
            supported_for_rcs: vec![3],
            supported_rev_rcs: vec![3],
            preferred_pairs: vec![RcPairConfig::new(3, 3)],
            ..Default::default()
        }
    }

    #[test]
    fn rc_selection_preferred_pair_wins() {
        let policy = default_policy();
        // Mobile supports both RC1 and RC3, policy prefers RC1 first.
        let result = select_initial_traffic_rcs(&policy, &[1, 3], &[1, 3], None, None, 6);
        assert_eq!(result, Some((1, 1)));
    }

    #[test]
    fn rc_selection_mobile_only_supports_rc3() {
        let policy = default_policy();
        let result = select_initial_traffic_rcs(&policy, &[3], &[3], None, None, 6);
        assert_eq!(result, Some((3, 3)));
    }

    #[test]
    fn rc_selection_policy_only_allows_rc1() {
        let policy = rc1_only_policy();
        let result = select_initial_traffic_rcs(&policy, &[1, 3], &[1, 3], None, None, 6);
        assert_eq!(result, Some((1, 1)));
    }

    #[test]
    fn rc_selection_policy_only_allows_rc3() {
        let policy = rc3_only_policy();
        let result = select_initial_traffic_rcs(&policy, &[1, 3], &[1, 3], None, None, 6);
        assert_eq!(result, Some((3, 3)));
    }

    #[test]
    fn rc_selection_empty_mobile_caps_treated_as_unrestricted() {
        let policy = default_policy();
        let result = select_initial_traffic_rcs(&policy, &[], &[], None, None, 6);
        assert_eq!(result, Some((1, 1)));
    }

    #[test]
    fn rc_selection_no_overlap_returns_none() {
        let policy = rc3_only_policy();
        // Mobile only supports RC1, policy only allows RC3, mob_p_rev < 6.
        let result = select_initial_traffic_rcs(&policy, &[1], &[1], None, None, 5);
        assert!(result.is_none());
    }

    #[test]
    fn rc_selection_mob_p_rev_6_fallback() {
        let policy = rc3_only_policy();
        // Mobile only lists RC1, but mob_p_rev >= 6 implies baseline RC3 support.
        let result = select_initial_traffic_rcs(&policy, &[1], &[1], None, None, 6);
        assert_eq!(result, Some((3, 3)));
    }

    #[test]
    fn rc_selection_pre_is2000_mobile_with_rc3_policy_falls_back_to_rc1() {
        let policy = default_policy();
        let result = select_initial_traffic_rcs(&policy, &[], &[], None, None, 3);
        assert_eq!(result, Some((1, 1)));
    }

    #[test]
    fn rc_selection_pre_is2000_returns_none_when_policy_excludes_rc1() {
        // If the operator policy excludes RC1, a pre-IS-2000 mobile cannot be
        // assigned at all — RC3 is not decodable by these handsets.
        let policy = rc3_only_policy();
        let result = select_initial_traffic_rcs(&policy, &[], &[], None, None, 3);
        assert!(result.is_none());
    }

    #[test]
    fn qcelp_p_rev3_selects_rc2_despite_legacy_capability_vectors() {
        let result = select_initial_traffic_rcs_for_so(
            &default_policy(),
            &[1],
            &[1],
            None,
            None,
            3,
            Some(SERVICE_OPTION_QCELP13),
        );
        assert_eq!(result, Some((2, 2)));
    }

    #[test]
    fn basic_voice_selects_rc1_even_for_is2000_mobile() {
        let result = select_initial_traffic_rcs_for_so(
            &default_policy(),
            &[2, 3],
            &[2, 3],
            Some(3),
            Some(3),
            6,
            Some(SERVICE_OPTION_BASIC_VOICE),
        );
        assert_eq!(result, Some((1, 1)));
    }

    #[test]
    fn qcelp_rejects_policy_without_rc2() {
        let result = select_initial_traffic_rcs_for_so(
            &rc1_only_policy(),
            &[1],
            &[1],
            None,
            None,
            3,
            Some(SERVICE_OPTION_QCELP13),
        );
        assert!(result.is_none());
    }

    #[test]
    fn qcelp_p_rev6_honors_explicit_rc2_capability() {
        let supported = select_initial_traffic_rcs_for_so(
            &default_policy(),
            &[1, 2],
            &[1, 2],
            None,
            None,
            6,
            Some(SERVICE_OPTION_QCELP13),
        );
        let unsupported = select_initial_traffic_rcs_for_so(
            &default_policy(),
            &[1],
            &[1],
            None,
            None,
            6,
            Some(SERVICE_OPTION_QCELP13),
        );
        assert_eq!(supported, Some((2, 2)));
        assert!(unsupported.is_none());
    }

    #[test]
    fn rc_selection_prefers_mobile_stated_preference() {
        let policy = default_policy();
        // Mobile states preference for RC3.
        let result = select_initial_traffic_rcs(&policy, &[1, 3], &[1, 3], Some(3), Some(3), 6);
        // Policy preferred_pairs has RC1 first, but mobile's stated pref is checked
        // after preferred_pairs. Since both pass, preferred_pairs wins.
        assert_eq!(result, Some((1, 1)));
    }
}
