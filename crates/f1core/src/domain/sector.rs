use chrono::{DateTime, Duration as ChronoDuration, Utc};

/// Tolerance for comparing floating-point sector times.
const FLOAT_TOLERANCE: f64 = 0.001;

/// Classification of a sector time relative to session and personal bests.
/// Serialized to the frontend so styling decisions happen once in Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SectorStatus {
    /// Matches the overall session-best sector time (purple).
    SessionBest,
    /// Matches the driver's personal-best sector time (green).
    PersonalBest,
    /// Valid time but neither session nor personal best (yellow).
    Normal,
    /// No sector time available.
    None,
}

/// Classify a sector value against session and personal bests.
pub fn classify_sector(
    value: Option<f64>,
    session_best: Option<f64>,
    personal_best: Option<f64>,
) -> SectorStatus {
    let Some(v) = value else {
        return SectorStatus::None;
    };
    if session_best.is_some_and(|sb| (v - sb).abs() < FLOAT_TOLERANCE) {
        SectorStatus::SessionBest
    } else if personal_best.is_some_and(|pb| (v - pb).abs() < FLOAT_TOLERANCE) {
        SectorStatus::PersonalBest
    } else {
        SectorStatus::Normal
    }
}

/// Given the virtual clock time and a lap's date_start + sector durations,
/// return which sectors should be visible.
/// S1 visible after date_start + s1, S2 after + s1 + s2, S3 after + s1 + s2 + s3.
pub fn visible_sectors(
    now: DateTime<Utc>,
    lap_date_start: Option<&str>,
    s1: Option<f64>,
    s2: Option<f64>,
    s3: Option<f64>,
) -> (Option<f64>, Option<f64>, Option<f64>) {
    let start = match lap_date_start.and_then(|d| d.parse::<DateTime<Utc>>().ok()) {
        Some(t) => t,
        None => return (s1, s2, s3), // no date info, show all
    };

    let s1_dur = match s1 {
        Some(d) if d > 0.0 => d,
        _ => return (None, None, None),
    };
    let s1_complete = start + ChronoDuration::milliseconds((s1_dur * 1000.0) as i64);
    if now < s1_complete {
        return (None, None, None);
    }

    let s2_dur = match s2 {
        Some(d) if d > 0.0 => d,
        _ => return (s1, None, None),
    };
    let s2_complete = s1_complete + ChronoDuration::milliseconds((s2_dur * 1000.0) as i64);
    if now < s2_complete {
        return (s1, None, None);
    }

    let s3_dur = match s3 {
        Some(d) if d > 0.0 => d,
        _ => return (s1, s2, None),
    };
    let s3_complete = s2_complete + ChronoDuration::milliseconds((s3_dur * 1000.0) as i64);
    if now < s3_complete {
        return (s1, s2, None);
    }

    (s1, s2, s3)
}
