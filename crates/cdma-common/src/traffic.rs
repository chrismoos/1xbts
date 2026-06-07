/// Parameters for creating a reverse traffic channel receiver.
///
/// Stored in the shared pool by the BSC; the RX thread reads these to
/// build the actual pipeline processors.
#[derive(Debug, Clone)]
pub struct TrafficRxRequest {
    /// Walsh code assigned to this traffic channel.
    pub walsh_code: u8,
    /// Mobile ESN (used for LC mask).
    pub esn: u32,
    /// Assigned reverse Radio Configuration for this traffic channel.
    pub assigned_rev_rc: u8,
    /// Number of preamble PCGs for pilot acquisition (NUM_PREAMBLE).
    /// None = use default (4 PCGs). Only used for RC3.
    pub preamble_num_pcgs: Option<usize>,
    /// Per C.S0002-E §2.1.3.12.7: when true and rate is 1500 bps (RC3),
    /// the mobile only transmits R-FCH on PCGs {2,3,6,7,10,11,14,15}.
    pub rev_fch_gating_mode: bool,
}

/// Initial linear amplitude gain for an RC1 forward traffic channel.
pub const RC1_TRAFFIC_INITIAL_GAIN_LINEAR: f32 = 0.838;

/// Initial linear amplitude gain for an RC3 forward traffic channel.
pub const RC3_TRAFFIC_INITIAL_GAIN_LINEAR: f32 = 0.5;
