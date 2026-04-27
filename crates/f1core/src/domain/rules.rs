/// Interval threshold in seconds for DRS activation range.
pub const DRS_RANGE_SECS: f64 = 1.0;

/// Tyre age in laps at which visual degradation indicator kicks in.
pub const TYRE_CRITICAL_AGE: i64 = 25;

/// Duration in seconds for sector flash highlight after a value changes.
pub const SECTOR_FLASH_SECS: u64 = 2;

pub fn is_drs_range(interval_secs: f64) -> bool {
    interval_secs > 0.0 && interval_secs < DRS_RANGE_SECS
}

pub fn is_tyre_degraded(age: i64) -> bool {
    age >= TYRE_CRITICAL_AGE
}
