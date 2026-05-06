use chrono::{DateTime, Duration, Utc};

pub type CdmaSystemTime = DateTime<Utc>;

const CDMA_EPOCH_UNIX_SECONDS: i64 = 315_964_800; // 1980-01-06T00:00:00Z
const NANOS_PER_SECOND: i128 = 1_000_000_000;
const NANOS_PER_20MS: i128 = 20_000_000;

pub fn cdma_epoch() -> CdmaSystemTime {
    DateTime::from_timestamp(CDMA_EPOCH_UNIX_SECONDS, 0)
        .expect("CDMA epoch timestamp must be representable")
}

pub fn system_time_now() -> CdmaSystemTime {
    Utc::now()
}

pub fn chips_since_epoch(system_time: CdmaSystemTime, chip_rate_hz: u64) -> u64 {
    let epoch = cdma_epoch();
    let delta = system_time.signed_duration_since(epoch);
    let nanos = delta.num_nanoseconds().unwrap_or(0).max(0) as i128;
    ((nanos * chip_rate_hz as i128) / NANOS_PER_SECOND) as u64
}

pub fn system_time_from_chips(chips: u64, chip_rate_hz: u64) -> CdmaSystemTime {
    let epoch = cdma_epoch();
    let seconds = (chips / chip_rate_hz) as i64;
    let rem_chips = chips % chip_rate_hz;
    let nanos = ((rem_chips as i128) * NANOS_PER_SECOND / chip_rate_hz as i128) as i64;
    epoch + Duration::seconds(seconds) + Duration::nanoseconds(nanos)
}

pub fn system_time_20ms_frames(system_time: CdmaSystemTime) -> u64 {
    let epoch = cdma_epoch();
    let delta = system_time.signed_duration_since(epoch);
    let nanos = delta.num_nanoseconds().unwrap_or(0).max(0) as i128;
    (nanos / NANOS_PER_20MS) as u64
}
