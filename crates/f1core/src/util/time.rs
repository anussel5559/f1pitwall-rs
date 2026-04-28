use chrono::{DateTime, FixedOffset, Utc};

pub fn fmt_ts(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
}

pub fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f+00:00")
                .map(|ndt| ndt.and_utc())
                .ok()
        })
}

/// Parse "HH:MM:SS" gmt_offset string (from OpenF1 API) into a chrono FixedOffset.
pub fn parse_gmt_offset(s: &str) -> Option<FixedOffset> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() >= 2 {
        let hours: i32 = parts[0].parse().ok()?;
        let mins: i32 = parts[1].parse().ok()?;
        let sign = if hours < 0 { -1 } else { 1 };
        let total_secs = hours * 3600 + sign * mins * 60;
        FixedOffset::east_opt(total_secs)
    } else {
        None
    }
}
