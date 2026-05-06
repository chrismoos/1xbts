/// Signal quality snapshot from a single access channel decode.
#[derive(Debug, Clone)]
pub struct RxMeasurement {
    pub snr_db: Option<f32>,
    pub signal_power_db: Option<f32>,
    pub raw_power_db: Option<f32>,
    pub demod_quality_pct: Option<f32>,
    pub timestamp_us: u64,
}

/// Key for the RX measurement store — identifies a mobile by ESN or full IMSI string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RxMeasurementKey {
    Esn(u32),
    Imsi(String),
}
