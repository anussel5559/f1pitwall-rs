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
        // OpenF1 stores `laps.date_start` (and similar columns) as naive ISO
        // with no timezone suffix, e.g. "2026-05-01T21:11:45.087000". Treat
        // these as UTC so callers like `visible_sectors` can gate progressive
        // reveal instead of falling back to "show everything".
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
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

#[cfg(test)]
mod parse_ts_tests {
    use super::*;

    #[test]
    fn rfc3339_with_offset() {
        let dt = parse_ts("2026-05-01T21:11:45.087000+00:00").unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-05-01T21:11:45.087+00:00");
    }

    #[test]
    fn z_suffix() {
        let dt = parse_ts("2026-05-01T21:11:45.087Z").unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-05-01T21:11:45.087+00:00");
    }

    /// Regression: OpenF1 stores `laps.date_start` as naive ISO with no
    /// timezone suffix. Treat as UTC instead of returning `None`.
    #[test]
    fn naive_iso_treated_as_utc() {
        let dt = parse_ts("2026-05-01T21:11:45.087000").unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-05-01T21:11:45.087+00:00");
    }
}
