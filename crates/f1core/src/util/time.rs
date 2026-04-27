use chrono::FixedOffset;

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
