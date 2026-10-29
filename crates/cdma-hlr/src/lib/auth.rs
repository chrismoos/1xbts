//! CAVE Authentication Center decisions over stored credentials.
//!
//! The A-key and SSD stay inside the HLR. Callers pass the challenge inputs
//! and receive a verdict, plus the per-call CMEA key and private long code
//! mask when the authentication sets up a call.

use cdma_cave::{AKey, Esn, RandSsd, Ssd};

use crate::model::AuthAlgorithm;

/// COUNTs-p is a modulo-64 call history count (C.S0005-E §2.3.12.1.3).
pub const AUTH_COUNT_MODULUS: u8 = 64;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct CaveCredentials {
    pub auth_algo: AuthAlgorithm,
    pub a_key: Option<[u8; 8]>,
    pub ssd: Option<Ssd>,
    pub auth_count: u8,
    pub randssd: Option<[u8; 7]>,
    /// A-key from an OTASP key exchange awaiting the mobile's Commit Response.
    pub pending_a_key: Option<[u8; 8]>,
    /// SSD generated from `pending_a_key`, committed with it.
    pub pending_ssd: Option<Ssd>,
}

impl std::fmt::Debug for CaveCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaveCredentials")
            .field("auth_algo", &self.auth_algo)
            .field("a_key_provisioned", &self.a_key.is_some())
            .field("ssd_provisioned", &self.ssd.is_some())
            .field("auth_count", &self.auth_count)
            .field("ssd_update_pending", &self.randssd.is_some())
            .field("key_exchange_pending", &self.pending_a_key.is_some())
            .finish()
    }
}

impl CaveCredentials {
    /// A-key verification and OTASP A-key commit zero the SSD (S.S0053-0 §2.2.2), so a
    /// provisioned A-key without stored SSD authenticates against an all-zero SSD.
    fn current_ssd(&self) -> Option<Ssd> {
        if self.auth_algo != AuthAlgorithm::Cave {
            return None;
        }
        match (self.ssd, self.a_key) {
            (Some(ssd), _) => Some(ssd),
            (None, Some(_)) => Some(Ssd {
                ssd_a: [0; 8],
                ssd_b: [0; 8],
            }),
            (None, None) => None,
        }
    }

    /// An OTASP key exchange updates the SSD from its exchanged A-key before the commit,
    /// whether or not an A-key is on file.
    pub fn can_update_ssd(&self) -> bool {
        self.pending_a_key.is_some()
            || (self.a_key.is_some() && self.auth_algo == AuthAlgorithm::Cave)
    }

    /// C.S0016-D §3.3.2: after an OTASP key exchange the mobile authenticates with the SSD
    /// generated from A_KEY_TEMP, and with its committed SSD until then.
    fn otasp_ssd(&self) -> Option<Ssd> {
        self.pending_ssd.or_else(|| self.current_ssd())
    }

    /// SSD_NEW for the running SSD update. During an OTASP session it derives from the
    /// exchanged A-key (C.S0016-D §3.3.2).
    fn ssd_update_result(&self, esn: Esn) -> Option<Ssd> {
        let a_key = self.pending_a_key.or(self.a_key)?;
        let randssd = self.randssd?;
        Some(cdma_cave::generate_ssd(
            &AKey(a_key),
            esn,
            &RandSsd(randssd),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaveAuthOutcome {
    Success,
    SignatureMismatch,
    CountMismatch,
    NotProvisioned,
}

impl CaveAuthOutcome {
    pub fn is_success(self) -> bool {
        self == Self::Success
    }
}

/// CMEA key and PVTLCM_42 from Key_VPM_Generation (S.S0053-0 §2.5.1).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CallKeyMaterial {
    pub cmea_key: [u8; 8],
    pub private_long_code_mask: u64,
}

impl std::fmt::Debug for CallKeyMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CallKeyMaterial(..)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaveVerdict {
    pub outcome: CaveAuthOutcome,
    pub call_keys: Option<CallKeyMaterial>,
}

impl CaveVerdict {
    fn without_keys(outcome: CaveAuthOutcome) -> Self {
        Self {
            outcome,
            call_keys: None,
        }
    }
}

/// Global-challenge inputs from a Registration, Origination, Page Response or
/// Data Burst (C.S0004-E §2.1.1.1.2.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GlobalChallenge {
    pub esn: u32,
    pub rand: u32,
    pub authr: u32,
    /// The MS COUNT. `None` skips the COUNT comparison.
    pub count: Option<u8>,
    pub auth_data: u32,
    pub generate_call_keys: bool,
    pub otasp_reauthentication: bool,
}

/// Unique-challenge inputs (C.S0005-E §2.3.12.1.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UniqueChallenge {
    pub esn: u32,
    pub randu: u32,
    pub authu: u32,
    pub imsi_s1: u32,
    pub imsi_s2: u16,
    pub generate_call_keys: bool,
}

fn call_keys(saved: &cdma_cave::SavedRegisters, ssd: &Ssd, esn: Esn) -> CallKeyMaterial {
    let keys = cdma_cave::generate_call_keys(saved, &ssd.ssd_b, esn);
    CallKeyMaterial {
        cmea_key: keys.cmea_key.0,
        private_long_code_mask: keys.private_long_code_mask(),
    }
}

pub fn verify_global_challenge(
    creds: &CaveCredentials,
    challenge: &GlobalChallenge,
) -> Result<CaveVerdict, String> {
    let ssd = if challenge.otasp_reauthentication {
        creds.otasp_ssd()
    } else {
        creds.current_ssd()
    };
    let Some(ssd) = ssd else {
        return Ok(CaveVerdict::without_keys(CaveAuthOutcome::NotProvisioned));
    };
    let esn = Esn(challenge.esn);
    let auth = cdma_cave::auth_signature(challenge.rand, challenge.auth_data, &ssd.ssd_a, esn)
        .map_err(|e| e.to_string())?;
    if auth.signature != challenge.authr {
        return Ok(CaveVerdict::without_keys(
            CaveAuthOutcome::SignatureMismatch,
        ));
    }
    Ok(CaveVerdict {
        outcome: CaveAuthOutcome::Success,
        call_keys: challenge
            .generate_call_keys
            .then(|| call_keys(&auth.saved, &ssd, esn)),
    })
}

pub fn verify_unique_challenge(
    creds: &CaveCredentials,
    challenge: &UniqueChallenge,
) -> Result<CaveVerdict, String> {
    let Some(ssd) = creds.current_ssd() else {
        return Ok(CaveVerdict::without_keys(CaveAuthOutcome::NotProvisioned));
    };
    let esn = Esn(challenge.esn);
    let auth = cdma_cave::authu(
        &ssd.ssd_a,
        challenge.randu,
        challenge.imsi_s1,
        challenge.imsi_s2,
        esn,
    )
    .map_err(|e| e.to_string())?;
    if auth.signature != challenge.authu {
        return Ok(CaveVerdict::without_keys(
            CaveAuthOutcome::SignatureMismatch,
        ));
    }
    Ok(CaveVerdict {
        outcome: CaveAuthOutcome::Success,
        call_keys: challenge
            .generate_call_keys
            .then(|| call_keys(&auth.saved, &ssd, esn)),
    })
}

/// AUTHBS over the pending SSD_A_NEW (C.S0005-E §2.3.12.1.5). Fails when no SSD
/// update is in progress.
pub fn base_station_challenge(
    creds: &CaveCredentials,
    esn: u32,
    randbs: u32,
    imsi_s1: u32,
) -> Result<u32, String> {
    let ssd_new = creds
        .ssd_update_result(Esn(esn))
        .ok_or_else(|| "no SSD update in progress".to_string())?;
    cdma_cave::authbs(&ssd_new.ssd_a, randbs, imsi_s1, Esn(esn)).map_err(|e| e.to_string())
}

/// The SSD that replaces the stored SSD once the MS confirms the update.
pub fn committed_ssd(creds: &CaveCredentials, esn: u32) -> Result<Ssd, String> {
    creds
        .ssd_update_result(Esn(esn))
        .ok_or_else(|| "no SSD update in progress".to_string())
}

/// The COUNT to store after a verified global challenge, when it differs from
/// the stored one. The HLR adopts the mobile's COUNT rather than rejecting the
/// access, so a lost Parameter Update Confirmation never locks a mobile out. A
/// mismatch is still a clone indicator and is logged by the caller.
pub fn resynced_count(creds: &CaveCredentials, challenge: &GlobalChallenge) -> Option<u8> {
    challenge.count.filter(|&count| count != creds.auth_count)
}

pub fn next_auth_count(count: u8) -> u8 {
    (count + 1) % AUTH_COUNT_MODULUS
}

#[cfg(test)]
mod tests {
    use super::*;

    const ESN: u32 = 0xD75A_96EC;
    const MIN1: u32 = 0x79_2971;
    const A_KEY: u64 = 0xC442_F56B_E9E1_7158;
    const RANDSSD: [u8; 7] = [0x4d, 0x18, 0xee, 0xaa, 0x05, 0x89, 0x5c];

    fn creds() -> CaveCredentials {
        let ssd = cdma_cave::generate_ssd(&AKey::from_u64(A_KEY), Esn(ESN), &RandSsd(RANDSSD));
        CaveCredentials {
            auth_algo: AuthAlgorithm::Cave,
            a_key: Some(A_KEY.to_be_bytes()),
            ssd: Some(ssd),
            auth_count: 5,
            randssd: None,
            ..CaveCredentials::default()
        }
    }

    fn challenge(authr: u32, count: Option<u8>) -> GlobalChallenge {
        GlobalChallenge {
            esn: ESN,
            rand: 0x34A2_B05F,
            authr,
            count,
            auth_data: MIN1,
            generate_call_keys: true,
            otasp_reauthentication: false,
        }
    }

    #[test]
    fn global_challenge_accepts_the_s_s0053_vector_and_derives_keys() {
        let verdict = verify_global_challenge(&creds(), &challenge(0x3_66F6, Some(5))).unwrap();
        assert_eq!(verdict.outcome, CaveAuthOutcome::Success);
        let keys = verdict.call_keys.unwrap();
        assert_eq!(
            keys.cmea_key,
            [0xA0, 0x7B, 0x1C, 0xD1, 0x02, 0x75, 0x69, 0x14]
        );
        assert_eq!(keys.private_long_code_mask, 0x147_80CC_B8DF);
    }

    #[test]
    fn global_challenge_rejects_a_wrong_signature_and_resyncs_count() {
        let c = creds();
        assert_eq!(
            verify_global_challenge(&c, &challenge(0x3_66F7, Some(5)))
                .unwrap()
                .outcome,
            CaveAuthOutcome::SignatureMismatch
        );
        assert!(
            verify_global_challenge(&c, &challenge(0x3_66F6, Some(6)))
                .unwrap()
                .outcome
                .is_success()
        );
        assert_eq!(resynced_count(&c, &challenge(0x3_66F6, Some(6))), Some(6));
        assert_eq!(resynced_count(&c, &challenge(0x3_66F6, Some(5))), None);
        assert_eq!(resynced_count(&c, &challenge(0x3_66F6, None)), None);
        assert!(
            verify_global_challenge(&c, &challenge(0x3_66F6, None))
                .unwrap()
                .outcome
                .is_success()
        );
    }

    #[test]
    fn unprovisioned_identity_is_reported() {
        let c = CaveCredentials {
            auth_algo: AuthAlgorithm::None,
            a_key: None,
            ssd: None,
            auth_count: 0,
            randssd: None,
            ..CaveCredentials::default()
        };
        assert_eq!(
            verify_global_challenge(&c, &challenge(0, None)).unwrap(),
            CaveVerdict::without_keys(CaveAuthOutcome::NotProvisioned)
        );
    }

    #[test]
    fn a_key_without_ssd_authenticates_against_zero_ssd() {
        let mut c = creds();
        c.ssd = None;
        let zero = [0u8; 8];
        let expected = cdma_cave::auth_signature(0x1234_5678, MIN1, &zero, Esn(ESN))
            .unwrap()
            .signature;
        let mut ch = challenge(expected, None);
        ch.rand = 0x1234_5678;
        assert!(
            verify_global_challenge(&c, &ch)
                .unwrap()
                .outcome
                .is_success()
        );
    }

    #[test]
    fn unique_challenge_round_trip() {
        let c = creds();
        let ssd = c.ssd.unwrap();
        let authu = cdma_cave::authu(&ssd.ssd_a, 0xABCDEF, MIN1, 0x2A5, Esn(ESN))
            .unwrap()
            .signature;
        let mut ch = UniqueChallenge {
            esn: ESN,
            randu: 0xABCDEF,
            authu,
            imsi_s1: MIN1,
            imsi_s2: 0x2A5,
            generate_call_keys: false,
        };
        assert!(
            verify_unique_challenge(&c, &ch)
                .unwrap()
                .outcome
                .is_success()
        );
        ch.authu ^= 1;
        assert_eq!(
            verify_unique_challenge(&c, &ch).unwrap().outcome,
            CaveAuthOutcome::SignatureMismatch
        );
    }

    #[test]
    fn ssd_update_uses_pending_ssd_for_authbs_and_commit() {
        let mut c = creds();
        c.ssd = None;
        assert!(base_station_challenge(&c, ESN, 1, MIN1).is_err());
        c.randssd = Some(RANDSSD);
        let ssd_new = committed_ssd(&c, ESN).unwrap();
        assert_eq!(
            ssd_new.ssd_a,
            [0xCC, 0x38, 0x12, 0x94, 0x9F, 0x4D, 0xCD, 0x0D]
        );
        let authbs = base_station_challenge(&c, ESN, 0xDEAD_BEEF, MIN1).unwrap();
        assert_eq!(
            authbs,
            cdma_cave::authbs(&ssd_new.ssd_a, 0xDEAD_BEEF, MIN1, Esn(ESN)).unwrap()
        );
    }

    #[test]
    fn auth_count_wraps_modulo_64() {
        assert_eq!(next_auth_count(62), 63);
        assert_eq!(next_auth_count(63), 0);
    }
}

#[cfg(test)]
mod otasp_tests {
    use super::*;

    const ESN: u32 = 0x8000_1234;
    const RANDSSD: [u8; 7] = [1, 2, 3, 4, 5, 6, 7];

    #[test]
    fn otasp_ssd_update_derives_from_the_exchanged_a_key() {
        let old = [0x11; 8];
        let new = [0x22; 8];
        let creds = CaveCredentials {
            auth_algo: AuthAlgorithm::Cave,
            a_key: Some(old),
            randssd: Some(RANDSSD),
            pending_a_key: Some(new),
            ..CaveCredentials::default()
        };
        assert_eq!(
            committed_ssd(&creds, ESN).unwrap(),
            cdma_cave::generate_ssd(&AKey(new), Esn(ESN), &RandSsd(RANDSSD))
        );
    }

    #[test]
    fn otasp_key_exchange_updates_ssd_without_a_key_on_file() {
        let creds = CaveCredentials {
            pending_a_key: Some([0x22; 8]),
            randssd: Some(RANDSSD),
            ..CaveCredentials::default()
        };
        assert!(creds.can_update_ssd());
        assert_eq!(
            committed_ssd(&creds, ESN).unwrap(),
            cdma_cave::generate_ssd(&AKey([0x22; 8]), Esn(ESN), &RandSsd(RANDSSD))
        );
        assert!(!CaveCredentials::default().can_update_ssd());
    }

    #[test]
    fn reauthentication_uses_the_session_ssd() {
        let session = cdma_cave::generate_ssd(&AKey([0x22; 8]), Esn(ESN), &RandSsd(RANDSSD));
        let creds = CaveCredentials {
            auth_algo: AuthAlgorithm::Cave,
            a_key: Some([0x11; 8]),
            pending_a_key: Some([0x22; 8]),
            pending_ssd: Some(session),
            ..CaveCredentials::default()
        };
        let authr = cdma_cave::auth_signature(7, 0x12_3456, &session.ssd_a, Esn(ESN))
            .unwrap()
            .signature;
        let mut challenge = GlobalChallenge {
            esn: ESN,
            rand: 7,
            authr,
            auth_data: 0x12_3456,
            otasp_reauthentication: true,
            ..GlobalChallenge::default()
        };
        assert!(
            verify_global_challenge(&creds, &challenge)
                .unwrap()
                .outcome
                .is_success()
        );
        challenge.otasp_reauthentication = false;
        assert_eq!(
            verify_global_challenge(&creds, &challenge).unwrap().outcome,
            CaveAuthOutcome::SignatureMismatch
        );
    }
}
