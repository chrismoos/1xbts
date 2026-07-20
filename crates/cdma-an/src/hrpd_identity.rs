use std::sync::Arc;

use cdma_common::hrpd::air as hrpd_air;
use log::{info, warn};

const HRPD_DERIVED_IMSI_ESN_DOMAIN: &[u8] = b"hrpd-derived-esn-imsi-v1";
const HRPD_DERIVED_IMSI_MEID_DOMAIN: &[u8] = b"hrpd-derived-meid-imsi-v1";
const HRPD_DERIVED_IMSI_SUFFIX_MODULUS: u64 = 10_000_000_000;

/// HardwareIDResponse HardwareIDType codes (C.S0024 §7.4, 24-bit field).
const HARDWARE_ID_TYPE_ESN: u32 = 0x010000;
const HARDWARE_ID_TYPE_MEID: u32 = 0x00ffff;
/// The AT reports no hardware identifier.
const HARDWARE_ID_TYPE_NONE: u32 = 0xffffff;
const ESN_HARDWARE_ID_OCTETS: usize = 4;
const MEID_HARDWARE_ID_OCTETS: usize = 7;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HrpdA9MobileIdentity {
    pub imsi: Option<String>,
    pub esn: Option<u32>,
    pub meid: Option<cdma_a9::Meid>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HrpdDerivedImsiConfig {
    pub mcc: String,
    pub imsi_11_12: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HrpdHardwareIdentity {
    Esn(u32),
    Meid(cdma_a9::Meid),
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

pub fn hardware_identity_from_response(
    response: &hrpd_air::HrpdHardwareIdResponse,
) -> Option<HrpdHardwareIdentity> {
    match response.hardware_id_type {
        HARDWARE_ID_TYPE_ESN if response.hardware_id_value.len() == ESN_HARDWARE_ID_OCTETS => {
            Some(HrpdHardwareIdentity::Esn(u32::from_be_bytes([
                response.hardware_id_value[0],
                response.hardware_id_value[1],
                response.hardware_id_value[2],
                response.hardware_id_value[3],
            ])))
        }
        HARDWARE_ID_TYPE_MEID if response.hardware_id_value.len() == MEID_HARDWARE_ID_OCTETS => {
            let mut bytes = [0u8; MEID_HARDWARE_ID_OCTETS];
            bytes.copy_from_slice(&response.hardware_id_value);
            Some(HrpdHardwareIdentity::Meid(cdma_a9::Meid(bytes)))
        }
        HARDWARE_ID_TYPE_NONE => None,
        _ => None,
    }
}

pub async fn resolve_hrpd_a9_identity(
    hlr_repo: Option<&Arc<dyn cdma_hlr::repository::HlrRepository>>,
    derived_imsi_config: &HrpdDerivedImsiConfig,
    hardware: &HrpdHardwareIdentity,
) -> HrpdA9MobileIdentity {
    let mut identity = match hardware {
        HrpdHardwareIdentity::Esn(esn) => HrpdA9MobileIdentity {
            imsi: None,
            esn: Some(*esn),
            meid: None,
        },
        HrpdHardwareIdentity::Meid(meid) => HrpdA9MobileIdentity {
            imsi: None,
            esn: None,
            meid: Some(*meid),
        },
    };

    let Some(hlr_repo) = hlr_repo else {
        identity.imsi = Some(derive_hrpd_imsi(derived_imsi_config, hardware));
        info!(
            "HRPD AN bridge: derived IMSI-format MN ID for hardware identity {hardware:?}; \
             no HLR repository is configured, and A12/AN-AAA, roaming/VLR, or an authenticated \
             subscriber database should provide the authoritative MN ID when available"
        );
        return identity;
    };
    let (esn, meid_hex) = match hardware {
        HrpdHardwareIdentity::Esn(esn) => (Some(*esn), None),
        HrpdHardwareIdentity::Meid(meid) => (None, Some(hex_lower(&meid.0))),
    };
    match hlr_repo
        .resolve_by_hardware_identity(esn, meid_hex.as_deref())
        .await
    {
        Ok(Some(resolved)) => {
            if let Some(primary) = resolved.primary_identity {
                identity.imsi = primary.imsi;
                if identity.esn.is_none() {
                    identity.esn = primary.esn;
                }
                if identity.meid.is_none() {
                    if let Some(meid) = primary.meid.as_deref() {
                        if let Some(parsed) = meid_from_hex(meid) {
                            identity.meid = Some(parsed);
                        }
                    }
                }
            }
        }
        Ok(None) => {
            warn!("HRPD AN bridge: HLR has no subscriber for hardware identity {hardware:?}");
        }
        Err(err) => warn!("HRPD AN bridge: HLR hardware identity resolution failed: {err}"),
    }
    if identity.imsi.is_none() {
        identity.imsi = Some(derive_hrpd_imsi(derived_imsi_config, hardware));
        info!(
            "HRPD AN bridge: derived IMSI-format MN ID for hardware identity {hardware:?}; \
             A12/AN-AAA, roaming/VLR, or an authenticated subscriber database should provide \
             the authoritative MN ID when available"
        );
    }
    identity
}

pub fn derive_hrpd_imsi(config: &HrpdDerivedImsiConfig, hardware: &HrpdHardwareIdentity) -> String {
    // HRPD IOS requires an IMSI-format MN ID in A9/A11 before the A10 bearer
    // can be registered. When no HLR subscriber resolves the hardware identity,
    // derive a stable local MN ID from ESN/MEID so packet bearer setup is not
    // blocked by provisioning. This value is not a subscriber credential; future
    // A12/AN-AAA, roaming/VLR, or authenticated subscriber identity sources must
    // take precedence over it. Do not infer subscriber ownership from unrelated
    // registration timing.
    let mut hash = Fnv1a64::new();
    match hardware {
        HrpdHardwareIdentity::Esn(esn) => {
            hash.update(HRPD_DERIVED_IMSI_ESN_DOMAIN);
            hash.update(&esn.to_be_bytes());
        }
        HrpdHardwareIdentity::Meid(meid) => {
            hash.update(HRPD_DERIVED_IMSI_MEID_DOMAIN);
            hash.update(&meid.0);
        }
    }
    let suffix = hash.finish() % HRPD_DERIVED_IMSI_SUFFIX_MODULUS;
    format!("{}{}{:010}", config.mcc, config.imsi_11_12, suffix)
}

struct Fnv1a64(u64);

impl Fnv1a64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn finish(self) -> u64 {
        self.0
    }
}

fn meid_from_hex(value: &str) -> Option<cdma_a9::Meid> {
    let value = value.trim();
    if value.len() != 14 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0u8; 7];
    for (idx, chunk) in value.as_bytes().chunks(2).enumerate() {
        let high = (chunk[0] as char).to_digit(16)? as u8;
        let low = (chunk[1] as char).to_digit(16)? as u8;
        bytes[idx] = (high << 4) | low;
    }
    Some(cdma_a9::Meid(bytes))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use cdma_hlr::model::{
        MobileIdentityKey, MobileSeenUpsert, NumberPlan, NumberType, OtaspSessionFilter,
        OtaspSessionRow, Prl, PrlDeleteBlocked, PrlListFilter, RegistrationBinding,
        ResolvedSubscriber, SetRingtoneOutcome, Subscriber, SubscriberIdentity,
        SubscriberRingtoneCodecBlob,
    };
    use cdma_hlr::repository::HlrRepository;
    use uuid::Uuid;

    use super::*;

    #[derive(Default)]
    struct HardwareMissRepo {
        resolve_by_hardware_calls: AtomicUsize,
    }

    fn unexpected<T>(name: &str) -> Result<T, String> {
        panic!("unexpected HLR call during HRPD identity resolution: {name}")
    }

    #[tonic::async_trait]
    impl HlrRepository for HardwareMissRepo {
        async fn upsert_subscriber(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: NumberType,
            _: NumberPlan,
        ) -> Result<Subscriber, String> {
            unexpected("upsert_subscriber")
        }

        async fn set_subscriber_firstchp_override(
            &self,
            _: Uuid,
            _: Option<u16>,
        ) -> Result<(), String> {
            unexpected("set_subscriber_firstchp_override")
        }

        async fn get_subscriber_by_phone_number(
            &self,
            _: &str,
        ) -> Result<Option<ResolvedSubscriber>, String> {
            unexpected("get_subscriber_by_phone_number")
        }

        async fn get_subscriber_by_id(
            &self,
            _: Uuid,
        ) -> Result<Option<ResolvedSubscriber>, String> {
            unexpected("get_subscriber_by_id")
        }

        async fn update_subscriber(
            &self,
            _: Uuid,
            _: &str,
            _: &str,
            _: &str,
            _: NumberType,
            _: NumberPlan,
        ) -> Result<Option<Subscriber>, String> {
            unexpected("update_subscriber")
        }

        async fn list_subscribers(&self, _: u32, _: u32) -> Result<(Vec<Subscriber>, u32), String> {
            unexpected("list_subscribers")
        }

        async fn delete_subscriber(&self, _: Uuid) -> Result<bool, String> {
            unexpected("delete_subscriber")
        }

        async fn upsert_identity(
            &self,
            _: Uuid,
            _: Option<&str>,
            _: Option<u32>,
            _: Option<&str>,
        ) -> Result<SubscriberIdentity, String> {
            unexpected("upsert_identity")
        }

        async fn replace_primary_identity(
            &self,
            _: Uuid,
            _: Option<&str>,
            _: Option<u32>,
            _: Option<&str>,
        ) -> Result<SubscriberIdentity, String> {
            unexpected("replace_primary_identity")
        }

        async fn get_identities_for_subscriber(
            &self,
            _: Uuid,
        ) -> Result<Vec<SubscriberIdentity>, String> {
            unexpected("get_identities_for_subscriber")
        }

        async fn resolve_by_identity(
            &self,
            _: &MobileIdentityKey,
        ) -> Result<Option<ResolvedSubscriber>, String> {
            unexpected("resolve_by_identity")
        }

        async fn resolve_by_hardware_identity(
            &self,
            esn: Option<u32>,
            meid: Option<&str>,
        ) -> Result<Option<ResolvedSubscriber>, String> {
            assert_eq!(esn, None);
            assert_eq!(meid, Some("35512606023434"));
            self.resolve_by_hardware_calls
                .fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }

        async fn upsert_mobile_seen(
            &self,
            _: &MobileIdentityKey,
            _: Option<u8>,
        ) -> Result<MobileSeenUpsert, String> {
            unexpected("upsert_mobile_seen")
        }

        async fn upsert_registration_binding(
            &self,
            _: RegistrationBinding,
        ) -> Result<RegistrationBinding, String> {
            unexpected("upsert_registration_binding")
        }

        async fn get_registration_binding(
            &self,
            _: Uuid,
        ) -> Result<Option<RegistrationBinding>, String> {
            unexpected("get_registration_binding")
        }

        async fn set_ringtone(
            &self,
            _: Uuid,
            _: Vec<u8>,
            _: &str,
        ) -> Result<SetRingtoneOutcome, String> {
            unexpected("set_ringtone")
        }

        async fn clear_ringtone(&self, _: Uuid) -> Result<(), String> {
            unexpected("clear_ringtone")
        }

        async fn get_ringtone_codec(
            &self,
            _: Uuid,
            _: &str,
        ) -> Result<Option<SubscriberRingtoneCodecBlob>, String> {
            unexpected("get_ringtone_codec")
        }

        async fn list_prls(
            &self,
            _: u32,
            _: u32,
            _: PrlListFilter,
        ) -> Result<(Vec<Prl>, u32), String> {
            unexpected("list_prls")
        }

        async fn get_prl(&self, _: Uuid) -> Result<Option<Prl>, String> {
            unexpected("get_prl")
        }

        async fn get_default_prl(&self) -> Result<Option<Prl>, String> {
            unexpected("get_default_prl")
        }

        async fn create_prl(
            &self,
            _: &str,
            _: &[u8],
            _: i32,
            _: i16,
            _: &str,
        ) -> Result<Prl, String> {
            unexpected("create_prl")
        }

        async fn update_prl(
            &self,
            _: Uuid,
            _: Option<&str>,
            _: Option<&[u8]>,
            _: Option<(i32, i16)>,
            _: Option<&str>,
        ) -> Result<Prl, String> {
            unexpected("update_prl")
        }

        async fn soft_delete_prl(&self, _: Uuid) -> Result<Result<(), PrlDeleteBlocked>, String> {
            unexpected("soft_delete_prl")
        }

        async fn set_default_prl(&self, _: Uuid) -> Result<(), String> {
            unexpected("set_default_prl")
        }

        async fn set_subscriber_prl_override(
            &self,
            _: Uuid,
            _: Option<Uuid>,
        ) -> Result<(), String> {
            unexpected("set_subscriber_prl_override")
        }

        async fn set_subscriber_spc(&self, _: Uuid, _: Option<String>) -> Result<(), String> {
            unexpected("set_subscriber_spc")
        }

        async fn save_otasp_session(&self, _: &OtaspSessionRow) -> Result<(), String> {
            unexpected("save_otasp_session")
        }

        async fn list_otasp_sessions(
            &self,
            _: OtaspSessionFilter,
            _: u32,
            _: u32,
        ) -> Result<(Vec<OtaspSessionRow>, u32), String> {
            unexpected("list_otasp_sessions")
        }

        async fn get_otasp_session(&self, _: Uuid) -> Result<Option<OtaspSessionRow>, String> {
            unexpected("get_otasp_session")
        }
    }

    #[tokio::test]
    async fn hardware_hlr_miss_does_not_associate_to_registered_subscriber() {
        let repo = Arc::new(HardwareMissRepo::default());
        let hlr_repo: Arc<dyn HlrRepository> = repo.clone();
        let config = HrpdDerivedImsiConfig {
            mcc: "310".to_string(),
            imsi_11_12: "55".to_string(),
        };

        let identity = resolve_hrpd_a9_identity(
            Some(&hlr_repo),
            &config,
            &HrpdHardwareIdentity::Meid(cdma_a9::Meid([0x35, 0x51, 0x26, 0x06, 0x02, 0x34, 0x34])),
        )
        .await;

        assert_eq!(
            repo.resolve_by_hardware_calls.load(Ordering::SeqCst),
            1,
            "resolver should perform one direct hardware lookup"
        );
        assert_eq!(identity.imsi.as_deref(), Some("310556898017332"));
        assert_eq!(identity.esn, None);
        assert_eq!(
            identity.meid,
            Some(cdma_a9::Meid([0x35, 0x51, 0x26, 0x06, 0x02, 0x34, 0x34]))
        );
    }
}
