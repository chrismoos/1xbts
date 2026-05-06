/// Logical channel type for MAC service primitives.
///
/// Covers all channel types that appear in MAC-Data.Request /
/// MAC-Availability.Indication exchanges between the LAC and MAC sublayers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelType {
    FchDcch5ms,
    FchDcch20ms,
    FPdch,
    RPdch,
    FCcch,
    FBcch,
    RCcch,
    FPch,
    FSync,
    RAch,
    EnhancedAccess,
    /// Forward Traffic Channel (F-TCH / F-FCH).
    FTch,
}

#[derive(Debug, Clone, Copy)]
pub enum AccessMode {
    Basic,
    Reservation,
}

#[derive(Debug, Clone, Copy)]
pub enum SchedulingHint {}

#[derive(Debug, Clone, Copy)]
pub enum Reason {
    TimerExpired,
    LossOfChannel,
    InsufficientTransmissionRate,
}
