use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "FemtoConfigRaw")]
pub struct FemtoConfig {
    pub home_domain: String,
    pub event_bus_addr: String,
    pub segw: SegwConfig,
    pub acs: AcsConfig,
    pub sip: SipConfig,
    #[serde(default)]
    pub fcs: FcsConfig,
    pub packet: PacketConfig,
    #[serde(skip)]
    pub a11_provisioning: Option<A11Provisioning>,
    #[serde(default)]
    pub fap_profiles: BTreeMap<String, FapProfileConfig>,
    #[serde(default)]
    pub sites: BTreeMap<String, UnifiedSite>,
    #[serde(skip)]
    pub faps: BTreeMap<String, FapDefinition>,
    #[serde(skip)]
    pub flat_sites: BTreeMap<String, FemtoSiteConfig>,
    /// Single per-AAA-server secret. Must match `a12.radius_secret` on the PDSN
    /// side, or EV-DO terminal authentication fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub term_auth_psk: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct FemtoConfigRaw {
    home_domain: String,
    event_bus_addr: String,
    segw: SegwConfig,
    acs: AcsConfig,
    sip: SipConfig,
    #[serde(default)]
    fcs: FcsConfig,
    packet: PacketConfig,
    #[serde(default)]
    fap_profiles: BTreeMap<String, FapProfileConfig>,
    #[serde(default)]
    sites: BTreeMap<String, UnifiedSite>,
    #[serde(default)]
    term_auth_psk: Option<String>,
}

impl From<FemtoConfigRaw> for FemtoConfig {
    fn from(raw: FemtoConfigRaw) -> Self {
        let mut config = FemtoConfig {
            home_domain: raw.home_domain,
            event_bus_addr: raw.event_bus_addr,
            segw: raw.segw,
            acs: raw.acs,
            sip: raw.sip,
            fcs: raw.fcs,
            packet: raw.packet,
            a11_provisioning: None,
            fap_profiles: raw.fap_profiles,
            sites: raw.sites,
            faps: BTreeMap::new(),
            flat_sites: BTreeMap::new(),
            term_auth_psk: raw.term_auth_psk,
        };
        config.normalize();
        config
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FapImplementationKind {
    StandardXs0059,
    AirvanaAirave2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FapProfileConfig {
    pub implementation: FapImplementationKind,
    #[serde(default)]
    pub acs: Option<FapAcsConfig>,
    #[serde(default)]
    pub connection_request: Option<ConnectionRequestConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionRequestConfig {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FapAcsConfig {
    #[serde(default)]
    pub ensure_objects: Vec<CwmpObject>,
    #[serde(default)]
    pub stages: Vec<ProvisioningStage>,
    #[serde(default)]
    pub reapply_stages: Vec<String>,
    #[serde(default)]
    pub discovery_query: Vec<CwmpObject>,
    #[serde(default)]
    pub status_query: Vec<String>,
    #[serde(default)]
    pub batch_queries: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FapDefinition {
    pub profile: String,
    pub site: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cwmp_inform_response_style: CwmpInformResponseStyle,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CwmpInformResponseStyle {
    #[default]
    Standard,
    LegacyAirave2,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FemtoSiteConfig {
    pub mcc: String,
    pub imsi_11_12: String,
    pub cell_id: u16,
    pub sector: u8,
    pub sid: u32,
    pub nid: u32,
    pub band_class: u32,
    pub cdma_channel: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_sector_power_dbm: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sip_password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pn_offset: Option<u32>,
    #[serde(default)]
    pub evdo_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evdo_band_class: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evdo_channel: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evdo_cell_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evdo_sector_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evdo_color_code: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evdo_tx_power: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evdo_pn_offset: Option<u32>,
    #[serde(
        default,
        rename = "1x_enabled",
        skip_serializing_if = "Option::is_none"
    )]
    pub onex_enabled: Option<bool>,
    /// IANA zone (e.g. `"America/Denver"`) overriding GPS-derived timezone and
    /// DST provisioning. When absent, the zone is resolved from the FAP's GPS
    /// position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedSite {
    pub feid: String,
    pub profile: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub inform_response_style: CwmpInformResponseStyle,
    pub cell: CellIdentity,
    #[serde(rename = "1x")]
    pub onex: OneXConfig,
    #[serde(default)]
    pub evdo: EvdoConfig,
    pub acs: SiteAcsCreds,
    pub sip: SiteSipCreds,
    pub segw: SiteSegwCreds,
    /// IANA zone overriding GPS-derived timezone/DST provisioning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellIdentity {
    pub mcc: String,
    pub imsi_11_12: String,
    pub cell_id: u16,
    #[serde(default)]
    pub sector: u8,
}

/// Airvana FAP hard floor for 1x MaxSectorPowerIndBm. Lower values fault 9002.
const MIN_SECTOR_POWER_DBM: i32 = -30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OneXConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub sid: u32,
    pub nid: u32,
    pub band_class: u32,
    pub channel: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pn_offset: Option<u32>,
    /// Max 1x sector Tx power (dBm).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_sector_power_dbm: Option<i32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EvdoConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub band_class: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sector_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_code: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_power: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pn_offset: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteAcsCreds {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteSipCreds {
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impu: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impi: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteSegwCreds {
    #[serde(default = "default_security_profile")]
    pub security_profile: SecurityProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ike_group_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eap_password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eap_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ike_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fap_ca_cert: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segw_cert: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segw_key: Option<String>,
}

impl UnifiedSite {
    pub fn flat_site(&self) -> FemtoSiteConfig {
        FemtoSiteConfig {
            mcc: self.cell.mcc.clone(),
            imsi_11_12: self.cell.imsi_11_12.clone(),
            cell_id: self.cell.cell_id,
            sector: self.cell.sector,
            sid: self.onex.sid,
            nid: self.onex.nid,
            band_class: self.onex.band_class,
            cdma_channel: self.onex.channel,
            max_sector_power_dbm: self.onex.max_sector_power_dbm,
            sip_password: Some(self.sip.password.clone()),
            pn_offset: self.onex.pn_offset,
            onex_enabled: Some(self.onex.enabled),
            evdo_enabled: self.evdo.enabled,
            evdo_band_class: self.evdo.band_class,
            evdo_channel: self.evdo.channel,
            evdo_cell_id: self.evdo.cell_id,
            evdo_sector_id: self.evdo.sector_id.clone(),
            evdo_color_code: self.evdo.color_code,
            evdo_tx_power: self.evdo.tx_power,
            evdo_pn_offset: self.evdo.pn_offset,
            timezone: self.timezone.clone(),
        }
    }

    pub fn fap_definition(&self, site_name: &str) -> FapDefinition {
        FapDefinition {
            profile: self.profile.clone(),
            site: site_name.to_string(),
            name: self.name.clone(),
            cwmp_inform_response_style: self.inform_response_style,
        }
    }

    pub fn cpe_credential(&self) -> CpeCredential {
        CpeCredential {
            username: self.acs.username.clone(),
            password: self.acs.password.clone(),
            feid: Some(self.feid.clone()),
        }
    }

    pub fn sip_user(&self, realm: &str) -> SipUser {
        SipUser {
            impu: self
                .sip
                .impu
                .clone()
                .unwrap_or_else(|| format!("sip:{}@{}", self.feid, realm)),
            impi: self.sip.impi.clone().unwrap_or_else(|| self.feid.clone()),
            password: self.sip.password.clone(),
            feid: Some(self.feid.clone()),
        }
    }

    pub fn segw_connection(&self, ike_domain: Option<&str>) -> SegwConnection {
        SegwConnection {
            feid: self.feid.clone(),
            profile: self.segw.security_profile,
            proposal: self.segw.proposal.clone(),
            fap_ca_cert: self.segw.fap_ca_cert.clone(),
            segw_cert: self.segw.segw_cert.clone(),
            segw_key: self.segw.segw_key.clone(),
            ike_group_key: self.segw.ike_group_key.clone(),
            eap_identity: self.segw.eap_identity.clone(),
            ike_identity: self
                .segw
                .ike_identity
                .clone()
                .or_else(|| ike_domain.map(|domain| format!("{}@{}", self.feid, domain))),
            eap_password: self.segw.eap_password.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegwConfig {
    #[serde(default = "default_security_profile")]
    pub profile_default: SecurityProfile,
    #[serde(default = "default_segw_listen")]
    pub listen: String,
    #[serde(default = "default_swanctl_dir")]
    pub swanctl_dir: String,
    #[serde(default)]
    pub connections: Vec<SegwConnection>,
    /// Inner virtual-IP pool (CIDR) the SeGW assigns to a FAP via the IKEv2
    /// configuration payload. Without a pool the SeGW answers
    /// INTERNAL_ADDRESS_FAILURE and no CHILD_SA is built, so no inner traffic
    /// flows.
    #[serde(default)]
    pub vip_subnet: Option<String>,
    #[serde(default)]
    pub vip_dns: Option<String>,
    /// INTERNAL_IP4_NETMASK returned with the virtual IP, required by Airave 2.5+.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vip_netmask: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ike_identity_domain: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SegwConfigDocument {
    segw: SegwConfig,
    #[serde(default)]
    sites: BTreeMap<String, UnifiedSite>,
}

impl SegwConfig {
    pub fn load(path: &str) -> Result<Self, ConfigError> {
        let value = cdma_common::config_load::load_json_with_local_override(Path::new(path))
            .map_err(|source| ConfigError::Io {
                path: path.to_string(),
                source,
            })?;
        let document = serde_json::from_value::<SegwConfigDocument>(value).map_err(|source| {
            ConfigError::Parse {
                path: path.to_string(),
                source,
            }
        })?;
        let mut segw = document.segw;
        let ike_domain = segw.ike_identity_domain.clone();
        segw.connections.extend(derive_segw_connections(
            &document.sites,
            ike_domain.as_deref(),
        ));
        Ok(segw)
    }
}

fn default_swanctl_dir() -> String {
    "config/femto/segw".to_string()
}

fn default_security_profile() -> SecurityProfile {
    SecurityProfile::Standard
}

fn default_segw_listen() -> String {
    "0.0.0.0:500".to_string()
}

/// The two profiles are never negotiated down into each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityProfile {
    /// Certificate-authenticated IKEv2 with AES-based suites (A.S0024 / S.S0132).
    Standard,
    Legacy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegwConnection {
    pub feid: String,
    pub profile: SecurityProfile,
    #[serde(default)]
    pub proposal: Option<String>,
    #[serde(default)]
    pub fap_ca_cert: Option<String>,
    #[serde(default)]
    pub segw_cert: Option<String>,
    #[serde(default)]
    pub segw_key: Option<String>,
    #[serde(default)]
    pub ike_group_key: Option<String>,
    #[serde(default)]
    pub eap_identity: Option<String>,
    #[serde(default)]
    pub ike_identity: Option<String>,
    #[serde(default)]
    pub eap_password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcsConfig {
    pub listen: String,
    #[serde(default = "default_acs_realm")]
    pub realm: String,
    #[serde(default = "default_true")]
    pub require_site_approval: bool,
    #[serde(default)]
    pub cpe_credentials: Vec<CpeCredential>,
    #[serde(default)]
    pub reboot: bool,
    #[serde(default)]
    pub background_sync: AcsBackgroundSyncConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcsBackgroundSyncConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_acs_sync_interval_seconds")]
    pub interval_seconds: u64,
    #[serde(default = "default_acs_sync_timeout_seconds")]
    pub per_fap_timeout_seconds: u64,
    #[serde(default = "default_acs_sync_startup_delay_seconds")]
    pub startup_delay_seconds: u64,
}

impl Default for AcsBackgroundSyncConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: default_acs_sync_interval_seconds(),
            per_fap_timeout_seconds: default_acs_sync_timeout_seconds(),
            startup_delay_seconds: default_acs_sync_startup_delay_seconds(),
        }
    }
}

fn default_acs_sync_interval_seconds() -> u64 {
    30 * 60
}

fn default_acs_sync_timeout_seconds() -> u64 {
    60
}

fn default_acs_sync_startup_delay_seconds() -> u64 {
    3
}

fn default_acs_realm() -> String {
    "1xbts-acs".to_string()
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CwmpParam {
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub value_from: Option<CwmpValueSource>,
    #[serde(default = "default_xsd_string")]
    pub xsd_type: String,
    #[serde(default = "default_true")]
    pub verify: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CwmpValueSource {
    CpeParameter {
        parameter: String,
        transform: CwmpValueTransform,
    },
    FapContext {
        context: CwmpFapValue,
    },
    /// One field of the timezone/DST set, resolved from the FAP's GPS
    /// position (CPE `latitude`/`longitude`) or the site timezone override.
    Timezone {
        latitude: String,
        longitude: String,
        field: TimezoneField,
    },
}

impl CwmpValueSource {
    pub fn cpe_parameters(&self) -> Vec<&str> {
        match self {
            Self::CpeParameter { parameter, .. } => vec![parameter],
            Self::FapContext { .. } => Vec::new(),
            Self::Timezone {
                latitude,
                longitude,
                ..
            } => vec![latitude, longitude],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CwmpValueTransform {
    GpsDegreesMinutesToDecimalDegrees,
    GpsLongitudeToTimezone,
}

/// Which value of the CWMP `InternetGatewayDevice.Time.*` set a
/// [`CwmpValueSource::Timezone`] produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimezoneField {
    /// `LocalTimeZone` as the standard-time offset, DST never applied. The FAP
    /// adds the DST hour from the `DaylightSavings*` fields.
    LocalTimeZone,
    /// `DaylightSavingsUsed` ("1" if the zone observes DST this year).
    DaylightSavingsUsed,
    /// `DaylightSavingsStart` (spring-forward, local 02:00).
    DaylightSavingsStart,
    /// `DaylightSavingsEnd` (fall-back, local 02:00).
    DaylightSavingsEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CwmpFapValue {
    Feid,
    SiteSid,
    SiteNid,
    SiteBandClass,
    SiteCdmaChannel,
    SiteMaxSectorPowerDbm,
    SiteSipPassword,
    SiteLocationString,
    SitePnOffset,
    SiteEvdoBandClass,
    SiteEvdoChannel,
    SiteEvdoCellId,
    SiteEvdoSectorId,
    SiteEvdoColorCode,
    SiteEvdoTxPower,
    SiteEvdoPnOffset,
    SiteEvdoAdminStatus,
    TermAuthPsk,
    A11Spi,
    A11SharedSecretHex,
    #[serde(rename = "site_onex_admin_state")]
    SiteOneXAdminState,
}

fn default_xsd_string() -> String {
    "xsd:string".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisioningStage {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub atomic: bool,
    #[serde(default)]
    pub replace_object: Option<CwmpObject>,
    #[serde(default)]
    pub params: Vec<CwmpParam>,
    /// Bracket this stage's SetParameterValues with an administrative down/up.
    /// Some CPE parameters (e.g. an HRPD sector element's identity) only commit
    /// while the element is locked, so when the stage has real config to send
    /// the ACS locks this parameter down first and restores it up afterwards.
    #[serde(default)]
    pub bracket_admin: Option<BracketAdmin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BracketAdmin {
    pub parameter: String,
    pub down_value: String,
    pub up_value: String,
    #[serde(default = "default_xsd_string")]
    pub xsd_type: String,
    /// Read-only status parameter polled after the admin-down. The admin-down is
    /// asynchronous — config that needs a down element faults 9002 if it lands before
    /// the radio finishes tearing it down. When unset, config is sent immediately.
    #[serde(default)]
    pub settle_parameter: Option<String>,
    /// The `settle_parameter` value meaning "still operating" — config waits until the poll reads anything else.
    #[serde(default)]
    pub settle_operating_value: Option<String>,
    /// Maximum number of settle polls before applying config regardless.
    #[serde(default = "default_settle_polls")]
    pub settle_polls: u32,
}

fn default_settle_polls() -> u32 {
    20
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CwmpObject {
    pub name: String,
    #[serde(default)]
    pub next_level: bool,
    #[serde(default)]
    pub required_instance: Option<u32>,
    /// Leaf parameter used to test whether `required_instance` exists.
    ///
    /// When set (with `required_instance`), the ACS probes existence with a
    /// single `GetParameterValues` of `{name}{required_instance}.{leaf}` rather
    /// than a `GetParameterNames` enumeration: a value means the instance is
    /// present, a 9005 fault means it is absent. Some legacy firmware faults
    /// `GetParameterNames` on a table/instance even when it exists, so a name
    /// probe would wrongly recreate (and leak) the instance every session.
    #[serde(default)]
    pub existence_probe_leaf: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpeCredential {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub feid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SipConfig {
    pub listen: String,
    #[serde(default = "default_sip_realm")]
    pub realm: String,
    #[serde(default)]
    pub allow_unauthenticated: bool,
    #[serde(default)]
    pub users: Vec<SipUser>,
}

fn default_sip_realm() -> String {
    "femto.local.1xbts.org".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SipUser {
    pub impu: String,
    pub impi: String,
    pub password: String,
    #[serde(default)]
    pub feid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcsConfig {
    #[serde(default)]
    pub msc_a1_addr: Option<String>,
    #[serde(default = "default_fcs_a1_listen")]
    pub a1_listen: String,
    #[serde(default = "default_fcs_a1_advertise_addr")]
    pub a1_advertise_addr: String,
    #[serde(default = "default_fcs_node_id")]
    pub node_id: String,
    #[serde(default = "default_fcs_management_listen")]
    pub management_listen: String,
    #[serde(default = "default_fcs_cell_id")]
    pub cell_id: u16,
    #[serde(default)]
    pub sector: u8,
    #[serde(default = "default_fcs_mcc")]
    pub mcc: String,
    #[serde(default = "default_fcs_imsi_11_12")]
    pub imsi_11_12: String,
    #[serde(default = "default_fcs_rtp_bind_ip")]
    pub rtp_bind_ip: std::net::Ipv4Addr,
    #[serde(default)]
    pub rtp_msc_bind_ip: Option<std::net::Ipv4Addr>,
    #[serde(default = "default_fcs_rtp_port_start")]
    pub rtp_port_start: u16,
    #[serde(default = "default_fcs_rtp_port_end")]
    pub rtp_port_end: u16,
}

impl Default for FcsConfig {
    fn default() -> Self {
        Self {
            msc_a1_addr: None,
            a1_listen: default_fcs_a1_listen(),
            a1_advertise_addr: default_fcs_a1_advertise_addr(),
            node_id: default_fcs_node_id(),
            management_listen: default_fcs_management_listen(),
            cell_id: default_fcs_cell_id(),
            sector: 0,
            mcc: default_fcs_mcc(),
            imsi_11_12: default_fcs_imsi_11_12(),
            rtp_bind_ip: default_fcs_rtp_bind_ip(),
            rtp_msc_bind_ip: None,
            rtp_port_start: default_fcs_rtp_port_start(),
            rtp_port_end: default_fcs_rtp_port_end(),
        }
    }
}

fn default_fcs_a1_listen() -> String {
    "0.0.0.0:17013".to_string()
}

fn default_fcs_a1_advertise_addr() -> String {
    "127.0.0.1:17013".to_string()
}

fn default_fcs_node_id() -> String {
    "femto-fcs".to_string()
}

fn default_fcs_mcc() -> String {
    "310".to_string()
}

fn default_fcs_imsi_11_12() -> String {
    "00".to_string()
}

fn default_fcs_cell_id() -> u16 {
    0x193
}

fn default_fcs_management_listen() -> String {
    "0.0.0.0:17024".to_string()
}

fn default_fcs_rtp_bind_ip() -> std::net::Ipv4Addr {
    std::net::Ipv4Addr::UNSPECIFIED
}

fn default_fcs_rtp_port_start() -> u16 {
    18_000
}

fn default_fcs_rtp_port_end() -> u16 {
    18_199
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PacketConfig {
    pub adapter: PacketAdapter,
    #[serde(default)]
    pub pdsn_config: Option<std::path::PathBuf>,
    #[serde(default)]
    pub a11_spi: Option<u32>,
    #[serde(default)]
    pub direct_ip: Option<DirectIpConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PacketAdapter {
    DirectIp,
    ExternalPdsn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectIpConfig {
    #[serde(default)]
    pub allowed_source_cidrs: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading femto config {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("parsing femto config {path}: {source}")]
    Parse {
        path: String,
        source: serde_json::Error,
    },
    #[error("invalid femto config: {0}")]
    Validation(String),
}

#[derive(Clone)]
pub struct A11Provisioning(pub cdma_a11::A11SecurityConfig);

impl std::fmt::Debug for A11Provisioning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("A11Provisioning")
            .field("spi", &self.0.spi)
            .field("shared_secret_hex", &"<redacted>")
            .finish()
    }
}

#[derive(Deserialize)]
struct PdsnA11Config {
    a11_security: cdma_a11::A11SecurityConfig,
    #[serde(default)]
    a11_peers: Vec<cdma_a11::A11SecurityConfig>,
}

impl FemtoConfig {
    fn load_a11_provisioning(&mut self, femto_path: &Path) -> Result<(), ConfigError> {
        let required = self
            .fap_profiles
            .values()
            .filter_map(|p| p.acs.as_ref())
            .flat_map(|a| &a.stages)
            .flat_map(|s| &s.params)
            .any(|p| {
                matches!(
                    p.value_from,
                    Some(CwmpValueSource::FapContext {
                        context: CwmpFapValue::A11Spi | CwmpFapValue::A11SharedSecretHex
                    })
                )
            });
        if !required {
            return Ok(());
        }
        let parent = femto_path.parent().unwrap_or_else(|| Path::new("."));
        let path = parent.join(
            self.packet
                .pdsn_config
                .as_deref()
                .unwrap_or_else(|| Path::new("pdsn.json")),
        );
        let value =
            cdma_common::config_load::load_json_with_local_override(&path).map_err(|source| {
                ConfigError::Io {
                    path: path.display().to_string(),
                    source,
                }
            })?;
        let source: PdsnA11Config =
            serde_json::from_value(value).map_err(|source| ConfigError::Parse {
                path: path.display().to_string(),
                source,
            })?;
        let mut seen = std::collections::HashSet::new();
        for association in std::iter::once(&source.a11_security).chain(&source.a11_peers) {
            association
                .validate("PDSN A11 provisioning")
                .map_err(ConfigError::Validation)?;
            if !seen.insert(association.spi) {
                return Err(ConfigError::Validation(format!(
                    "duplicate PDSN A11 SPI {}",
                    association.spi
                )));
            }
        }
        let mut association = match self.packet.a11_spi {
            None => source.a11_security,
            Some(spi) if spi == source.a11_security.spi => source.a11_security,
            Some(spi) => source
                .a11_peers
                .into_iter()
                .find(|a| a.spi == spi)
                .ok_or_else(|| {
                    ConfigError::Validation(format!(
                        "packet.a11_spi {spi} is not configured on the PDSN"
                    ))
                })?,
        };
        association.shared_secret_hex = association.shared_secret_hex.trim().to_ascii_lowercase();
        self.a11_provisioning = Some(A11Provisioning(association));
        Ok(())
    }

    pub fn load(path: &str) -> Result<Self, ConfigError> {
        let value = cdma_common::config_load::load_json_with_local_override(Path::new(path))
            .map_err(|source| ConfigError::Io {
                path: path.to_string(),
                source,
            })?;
        let mut config: Self =
            serde_json::from_value(value).map_err(|source| ConfigError::Parse {
                path: path.to_string(),
                source,
            })?;
        config.load_a11_provisioning(Path::new(path))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let mut feids: BTreeMap<&str, &str> = BTreeMap::new();
        let mut cells: BTreeMap<(&str, &str, u16, u8), &str> = BTreeMap::new();
        for (name, site) in &self.sites {
            if site.feid.len() < 6 || !site.feid.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(ConfigError::Validation(format!(
                    "site {name}: invalid FEID {:?} (expected at least 6 hex digits)",
                    site.feid
                )));
            }
            if let Some(other) = feids.insert(&site.feid, name) {
                return Err(ConfigError::Validation(format!(
                    "FEID {} is used by both sites {other} and {name}",
                    site.feid
                )));
            }
            let cell = &site.cell;
            if let Some(other) = cells.insert(
                (
                    cell.mcc.as_str(),
                    cell.imsi_11_12.as_str(),
                    cell.cell_id,
                    cell.sector,
                ),
                name,
            ) {
                return Err(ConfigError::Validation(format!(
                    "cell identity mcc={} imsi_11_12={} cell_id={} sector={} is used by both sites {other} and {name}",
                    cell.mcc, cell.imsi_11_12, cell.cell_id, cell.sector
                )));
            }
            if !self.fap_profiles.contains_key(&site.profile) {
                return Err(ConfigError::Validation(format!(
                    "site {name}: unknown profile {:?}",
                    site.profile
                )));
            }
            for (field, value) in [
                ("acs.username", &site.acs.username),
                ("acs.password", &site.acs.password),
                ("sip.password", &site.sip.password),
            ] {
                if value.is_empty() {
                    return Err(ConfigError::Validation(format!(
                        "site {name}: empty {field}"
                    )));
                }
            }
            if let Some(dbm) = site.onex.max_sector_power_dbm {
                if dbm < MIN_SECTOR_POWER_DBM {
                    return Err(ConfigError::Validation(format!(
                        "site {name}: 1x max_sector_power_dbm {dbm} is below the FAP floor of {MIN_SECTOR_POWER_DBM} dBm"
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn normalize(&mut self) {
        let realm = self.sip.realm.clone();
        let ike_domain = self.segw.ike_identity_domain.clone();
        let connections = derive_segw_connections(&self.sites, ike_domain.as_deref());
        self.faps.clear();
        self.flat_sites.clear();
        self.acs.cpe_credentials.clear();
        self.sip.users.clear();
        self.segw.connections = connections;
        let sites: Vec<(String, UnifiedSite)> = self
            .sites
            .iter()
            .map(|(name, site)| (name.clone(), site.clone()))
            .collect();
        for (name, site) in sites {
            self.flat_sites.insert(name.clone(), site.flat_site());
            self.faps
                .insert(site.feid.clone(), site.fap_definition(&name));
            self.acs.cpe_credentials.push(site.cpe_credential());
            self.sip.users.push(site.sip_user(&realm));
        }
    }
}

fn derive_segw_connections(
    sites: &BTreeMap<String, UnifiedSite>,
    ike_domain: Option<&str>,
) -> Vec<SegwConnection> {
    sites
        .values()
        .map(|site| site.segw_connection(ike_domain))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_shipped_config() {
        let json = r#"{
            "home_domain": "femto.local.1xbts.org",
            "event_bus_addr": "http://127.0.0.1:17023",
            "segw": { "profile_default": "legacy", "listen": "0.0.0.0:500" },
            "acs": { "listen": "0.0.0.0:7003" },
            "sip": { "listen": "0.0.0.0:5060" },
            "packet": { "adapter": "external_pdsn" }
        }"#;
        let cfg: FemtoConfig = serde_json::from_str(json).expect("parse");
        assert_eq!(cfg.home_domain, "femto.local.1xbts.org");
        assert_eq!(cfg.segw.profile_default, SecurityProfile::Legacy);
        assert_eq!(cfg.packet.adapter, PacketAdapter::ExternalPdsn);
        assert!(cfg.acs.require_site_approval);
        assert_eq!(cfg.sip.realm, "femto.local.1xbts.org");
    }

    #[test]
    fn cwmp_value_source_variants_deserialize_unambiguously() {
        let tz: CwmpValueSource = serde_json::from_str(
            r#"{"latitude":"a.Latitude","longitude":"a.Longitude","field":"local_time_zone"}"#,
        )
        .expect("timezone source");
        assert_eq!(
            tz,
            CwmpValueSource::Timezone {
                latitude: "a.Latitude".into(),
                longitude: "a.Longitude".into(),
                field: TimezoneField::LocalTimeZone,
            }
        );
        assert_eq!(tz.cpe_parameters(), vec!["a.Latitude", "a.Longitude"]);

        let cpe: CwmpValueSource =
            serde_json::from_str(r#"{"parameter":"p","transform":"gps_longitude_to_timezone"}"#)
                .expect("cpe source");
        assert!(matches!(cpe, CwmpValueSource::CpeParameter { .. }));

        let fap: CwmpValueSource =
            serde_json::from_str(r#"{"context":"site_sid"}"#).expect("fap source");
        assert!(matches!(fap, CwmpValueSource::FapContext { .. }));
    }

    #[test]
    fn omitted_segw_profile_defaults_to_standard_security() {
        let config: SegwConfig = serde_json::from_value(serde_json::json!({})).unwrap();

        assert_eq!(config.profile_default, SecurityProfile::Standard);
    }

    #[test]
    fn unified_site_normalizes_into_internal_structures() {
        let json = r#"{
            "home_domain": "femto.local.1xbts.org",
            "event_bus_addr": "http://127.0.0.1:17023",
            "segw": { "listen": "0.0.0.0:500", "ike_identity_domain": "segw.example" },
            "acs": { "listen": "0.0.0.0:7003" },
            "sip": { "listen": "0.0.0.0:5060", "realm": "ims.example" },
            "packet": { "adapter": "external_pdsn" },
            "fap_profiles": { "p": { "implementation": "airvana_airave2" } },
            "sites": {
                "test-fap": {
                    "feid": "0005B9AABBCC", "profile": "p", "name": "TestFAP",
                    "cell": { "mcc": "310", "imsi_11_12": "00", "cell_id": 403 },
                    "1x": { "sid": 4107, "nid": 65535, "band_class": 1, "channel": 50 },
                    "acs": { "username": "0005B9-cdmaFAP3-0005B9AABBCC", "password": "acspw" },
                    "sip": { "password": "sippw" },
                    "segw": { "security_profile": "legacy", "ike_group_key": "gk", "eap_password": "ep" }
                }
            }
        }"#;
        let cfg: FemtoConfig = serde_json::from_str(json).expect("parse");
        assert_eq!(cfg.faps["0005B9AABBCC"].profile, "p");
        assert_eq!(cfg.flat_sites["test-fap"].sid, 4107);
        assert_eq!(
            cfg.acs.cpe_credentials[0].username,
            "0005B9-cdmaFAP3-0005B9AABBCC"
        );
        assert_eq!(cfg.acs.cpe_credentials[0].password, "acspw");
        assert_eq!(cfg.segw.connections[0].feid, "0005B9AABBCC");
        assert_eq!(
            cfg.segw.connections[0].ike_identity.as_deref(),
            Some("0005B9AABBCC@segw.example")
        );
        assert_eq!(cfg.sip.users[0].impi, "0005B9AABBCC");
        assert_eq!(cfg.sip.users[0].impu, "sip:0005B9AABBCC@ims.example");
        assert_eq!(cfg.packet.adapter, PacketAdapter::ExternalPdsn);
    }

    #[test]
    fn rejects_sites_sharing_a_cell_identity() {
        let json = r#"{
            "home_domain": "femto.local.1xbts.org",
            "event_bus_addr": "http://127.0.0.1:17023",
            "segw": { "listen": "0.0.0.0:500" },
            "acs": { "listen": "0.0.0.0:7003" },
            "sip": { "listen": "0.0.0.0:5060", "realm": "ims.example" },
            "packet": { "adapter": "external_pdsn" },
            "fap_profiles": { "p": { "implementation": "airvana_airave2" } },
            "sites": {
                "a": {
                    "feid": "0005B90000AA", "profile": "p",
                    "cell": { "mcc": "310", "imsi_11_12": "00", "cell_id": 403 },
                    "1x": { "sid": 1, "nid": 65535, "band_class": 1, "channel": 50 },
                    "acs": { "username": "x", "password": "x" }, "sip": { "password": "x" }, "segw": {}
                },
                "b": {
                    "feid": "0005B90000BB", "profile": "p",
                    "cell": { "mcc": "310", "imsi_11_12": "00", "cell_id": 403 },
                    "1x": { "sid": 1, "nid": 65535, "band_class": 1, "channel": 50 },
                    "acs": { "username": "x", "password": "x" }, "sip": { "password": "x" }, "segw": {}
                }
            }
        }"#;
        let cfg: FemtoConfig = serde_json::from_str(json).expect("parse");
        let error = cfg
            .validate()
            .expect_err("two sites sharing a cell identity must fail validation");
        assert!(
            matches!(error, ConfigError::Validation(ref m) if m.contains("cell identity")),
            "got {error:?}"
        );
    }

    #[test]
    fn rejects_sector_power_below_the_fap_floor() {
        let json = r#"{
            "home_domain": "femto.local.1xbts.org",
            "event_bus_addr": "http://127.0.0.1:17023",
            "segw": { "listen": "0.0.0.0:500" },
            "acs": { "listen": "0.0.0.0:7003" },
            "sip": { "listen": "0.0.0.0:5060", "realm": "ims.example" },
            "packet": { "adapter": "external_pdsn" },
            "fap_profiles": { "p": { "implementation": "airvana_airave2" } },
            "sites": {
                "a": {
                    "feid": "0005B90000AA", "profile": "p",
                    "cell": { "mcc": "310", "imsi_11_12": "00", "cell_id": 403 },
                    "1x": { "sid": 1, "nid": 65535, "band_class": 1, "channel": 50, "max_sector_power_dbm": -43 },
                    "acs": { "username": "x", "password": "x" }, "sip": { "password": "x" }, "segw": {}
                }
            }
        }"#;
        let cfg: FemtoConfig = serde_json::from_str(json).expect("parse");
        let error = cfg
            .validate()
            .expect_err("sub-floor sector power must fail validation");
        assert!(
            matches!(error, ConfigError::Validation(ref m) if m.contains("floor")),
            "got {error:?}"
        );
    }

    fn site_credentials_config(acs: &str, sip: &str) -> String {
        format!(
            r#"{{
            "home_domain": "femto.local.1xbts.org",
            "event_bus_addr": "http://127.0.0.1:17023",
            "segw": {{ "listen": "0.0.0.0:500" }},
            "acs": {{ "listen": "0.0.0.0:7003" }},
            "sip": {{ "listen": "0.0.0.0:5060", "realm": "ims.example" }},
            "packet": {{ "adapter": "external_pdsn" }},
            "fap_profiles": {{ "p": {{ "implementation": "airvana_airave2" }} }},
            "sites": {{
                "a": {{
                    "feid": "0005B90000AA", "profile": "p",
                    "cell": {{ "mcc": "310", "imsi_11_12": "00", "cell_id": 403 }},
                    "1x": {{ "sid": 1, "nid": 65535, "band_class": 1, "channel": 50 }},
                    "acs": {acs}, "sip": {sip}, "segw": {{}}
                }}
            }}
        }}"#
        )
    }

    #[test]
    fn rejects_sites_without_an_acs_username() {
        let json = site_credentials_config(r#"{ "password": "x" }"#, r#"{ "password": "x" }"#);
        let error = serde_json::from_str::<FemtoConfig>(&json)
            .expect_err("a site without acs.username must not parse");
        assert!(error.to_string().contains("username"), "got {error}");
    }

    #[test]
    fn rejects_sites_with_empty_credentials() {
        for (acs, sip, field) in [
            (
                r#"{ "username": "", "password": "x" }"#,
                r#"{ "password": "x" }"#,
                "acs.username",
            ),
            (
                r#"{ "username": "x", "password": "" }"#,
                r#"{ "password": "x" }"#,
                "acs.password",
            ),
            (
                r#"{ "username": "x", "password": "x" }"#,
                r#"{ "password": "" }"#,
                "sip.password",
            ),
        ] {
            let cfg: FemtoConfig =
                serde_json::from_str(&site_credentials_config(acs, sip)).expect("parse");
            let error = cfg.validate().expect_err("empty credential must fail");
            assert!(
                matches!(error, ConfigError::Validation(ref m) if m.contains(field)),
                "got {error:?}"
            );
        }
    }

    struct A11ConfigFixture(std::path::PathBuf);

    impl A11ConfigFixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "femto-a11-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            std::fs::write(
                path.join("femto.json"),
                include_str!("../../../config/femto.json"),
            )
            .unwrap();
            Self(path)
        }

        fn write(&self, name: &str, value: serde_json::Value) {
            std::fs::write(self.0.join(name), serde_json::to_vec(&value).unwrap()).unwrap();
        }

        fn load(&self) -> Result<FemtoConfig, ConfigError> {
            FemtoConfig::load(self.0.join("femto.json").to_str().unwrap())
        }
    }

    impl Drop for A11ConfigFixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn a11_provisioning_uses_pdsn_local_overrides_and_reloads_rotated_keys() {
        let fixture = A11ConfigFixture::new();
        fixture.write(
            "pdsn.json",
            serde_json::json!({"a11_security": {"spi":256, "shared_secret_hex":"00112233"}}),
        );
        fixture.write(
            "pdsn.local.json",
            serde_json::json!({"a11_security": {"spi":512, "shared_secret_hex":"aabbccdd"}}),
        );
        let config = fixture.load().unwrap();
        let a11 = config.a11_provisioning.unwrap();
        assert_eq!(
            (a11.0.spi, a11.0.shared_secret_hex.as_str()),
            (512, "aabbccdd")
        );
        assert!(!format!("{a11:?}").contains("aabbccdd"));
        fixture.write(
            "pdsn.local.json",
            serde_json::json!({"a11_security": {"shared_secret_hex":"11223344"}}),
        );
        let config = fixture.load().unwrap();
        assert_eq!(
            config.a11_provisioning.unwrap().0.shared_secret_hex,
            "11223344"
        );
    }

    #[test]
    fn a11_provisioning_selects_explicit_peer_from_configured_pdsn_path() {
        let fixture = A11ConfigFixture::new();
        fixture.write(
            "femto.local.json",
            serde_json::json!({"packet":{"pdsn_config":"other.json", "a11_spi":512}}),
        );
        fixture.write("other.json", serde_json::json!({"a11_security":{"spi":256,"shared_secret_hex":"00112233"},"a11_peers":[{"spi":512,"shared_secret_hex":"aabbccdd","feid":"0005B9000001"}]}));
        let a11 = fixture.load().unwrap().a11_provisioning.unwrap();
        assert_eq!(
            (a11.0.spi, a11.0.shared_secret_hex.as_str()),
            (512, "aabbccdd")
        );
        fixture.write(
            "femto.local.json",
            serde_json::json!({"packet":{"pdsn_config":"other.json", "a11_spi":1024}}),
        );
        assert!(
            fixture
                .load()
                .unwrap_err()
                .to_string()
                .contains("not configured")
        );
    }

    #[test]
    fn a11_provisioning_rejects_missing_and_invalid_credentials() {
        let fixture = A11ConfigFixture::new();
        assert!(fixture.load().is_err());
        for (spi, key) in [(255, "00112233"), (256, ""), (256, "not-hex")] {
            fixture.write(
                "pdsn.json",
                serde_json::json!({"a11_security":{"spi":spi,"shared_secret_hex":key}}),
            );
            assert!(fixture.load().is_err());
        }
    }
    #[test]
    fn a11_provisioning_rejects_duplicate_spis_and_allows_profiles_without_a11() {
        let fixture = A11ConfigFixture::new();
        fixture.write(
            "pdsn.json",
            serde_json::json!({
                "a11_security":{"spi":256,"shared_secret_hex":"00112233"},
                "a11_peers":[{"spi":256,"shared_secret_hex":"aabbccdd"}]
            }),
        );
        assert!(
            fixture
                .load()
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
        fixture.write("femto.local.json", serde_json::json!({"fap_profiles":null}));
        std::fs::remove_file(fixture.0.join("pdsn.json")).unwrap();
        assert!(fixture.load().unwrap().a11_provisioning.is_none());
    }
}
