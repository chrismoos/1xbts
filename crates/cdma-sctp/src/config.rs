use std::time::Duration;

use serde::{Deserialize, Serialize};

/// RFC 6951 registered UDP encapsulation port for SCTP.
pub const SCTP_UDP_ENCAPSULATION_PORT: u16 = 9899;

const DEFAULT_OUTBOUND_STREAMS: u16 = 16;
const DEFAULT_MAX_INBOUND_STREAMS: u16 = 16;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SctpBackend {
    /// Kernel SCTP when available, otherwise usrsctp over UDP.
    #[default]
    Auto,
    /// Linux only.
    Kernel,
    Usrsctp,
}

/// RFC 6951. usrsctp shares `local_port` across the process. The kernel
/// ignores it and uses the `net.sctp.udp_port` sysctl.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UdpEncapsulation {
    pub local_port: u16,
    pub remote_port: u16,
}

impl Default for UdpEncapsulation {
    fn default() -> Self {
        Self {
            local_port: SCTP_UDP_ENCAPSULATION_PORT,
            remote_port: SCTP_UDP_ENCAPSULATION_PORT,
        }
    }
}

/// Omitted timer and retransmission values keep the stack's own defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SctpConfig {
    pub backend: SctpBackend,
    /// Unset means raw IP protocol 132, which needs raw socket privileges with
    /// usrsctp.
    pub udp_encapsulation: Option<UdpEncapsulation>,
    /// Every process on a host needs its own `local_port`.
    pub fallback_udp_encapsulation: UdpEncapsulation,
    /// The peer may grant fewer.
    pub outbound_streams: u16,
    pub max_inbound_streams: u16,
    pub heartbeat_interval_ms: Option<u32>,
    pub path_max_retransmissions: Option<u16>,
    pub rto_initial_ms: Option<u32>,
    pub rto_min_ms: Option<u32>,
    pub rto_max_ms: Option<u32>,
}

impl Default for SctpConfig {
    fn default() -> Self {
        Self {
            backend: SctpBackend::Auto,
            udp_encapsulation: None,
            fallback_udp_encapsulation: UdpEncapsulation::default(),
            outbound_streams: DEFAULT_OUTBOUND_STREAMS,
            max_inbound_streams: DEFAULT_MAX_INBOUND_STREAMS,
            heartbeat_interval_ms: None,
            path_max_retransmissions: None,
            rto_initial_ms: None,
            rto_min_ms: None,
            rto_max_ms: None,
        }
    }
}

impl SctpConfig {
    pub(crate) fn has_rto_settings(&self) -> bool {
        self.rto_initial_ms.is_some() || self.rto_min_ms.is_some() || self.rto_max_ms.is_some()
    }

    pub(crate) fn has_path_settings(&self) -> bool {
        self.heartbeat_interval_ms.is_some() || self.path_max_retransmissions.is_some()
    }

    pub fn heartbeat_interval(&self) -> Option<Duration> {
        self.heartbeat_interval_ms
            .map(|ms| Duration::from_millis(u64::from(ms)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_json_object_uses_defaults() {
        let config: SctpConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config, SctpConfig::default());
    }

    #[test]
    fn udp_encapsulation_defaults_to_rfc6951_port() {
        let config: SctpConfig =
            serde_json::from_str(r#"{"backend":"usrsctp","udp_encapsulation":{}}"#).unwrap();
        assert_eq!(config.backend, SctpBackend::Usrsctp);
        assert_eq!(
            config.udp_encapsulation,
            Some(UdpEncapsulation {
                local_port: SCTP_UDP_ENCAPSULATION_PORT,
                remote_port: SCTP_UDP_ENCAPSULATION_PORT,
            })
        );
    }
}
