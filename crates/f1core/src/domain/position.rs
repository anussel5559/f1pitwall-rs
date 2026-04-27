use crate::db::BoardRow;

/// Parse gap_to_leader into a sortable value.
/// - "" or no gap → use fallback position (grid/previous)
/// - "0" or "0.000" → leader (sort first)
/// - "1.234" → seconds behind (sort by value)
/// - "1 LAP" / "2 LAPS" → lapped cars (sort after all on-lead-lap drivers)
///
pub fn gap_sort_key(gap: &str, fallback_position: i64) -> (i64, f64) {
    let trimmed = gap.trim().trim_start_matches('+');

    // No gap data — not actively classified. Sort below all drivers with a gap.
    if trimmed.is_empty() {
        return (3, fallback_position as f64);
    }

    // Check for lapped: "1 LAP", "2 LAPS", etc.
    if let Some(rest) = trimmed
        .strip_suffix(" LAP")
        .or_else(|| trimmed.strip_suffix(" LAPS"))
        && let Ok(n) = rest.trim().parse::<i64>()
    {
        return (1, n as f64);
    }

    if let Ok(secs) = trimmed.parse::<f64>() {
        return (0, secs);
    }

    // Anything else (e.g. "RETIRED") — sort to the bottom
    (2, fallback_position as f64)
}

/// Sort rows by position from the timing system and ensure sequential numbering.
/// The positions table (from F1's official timing) is the authoritative source
/// for race order. Gap-based re-sorting is only used as a fallback when no
/// position data is available — gap/interval data from the API can be
/// temporarily inconsistent across drivers, causing visible position jumps.
pub fn sort_and_assign_positions(rows: &mut [BoardRow]) {
    let has_positions = rows.iter().any(|r| r.position > 0);

    if has_positions {
        // Positions already set from the DB — just ensure sorted order.
        rows.sort_by_key(|r| r.position);
    } else {
        // No position data yet — fall back to gap-based sorting.
        let has_gaps = rows.iter().any(|r| !r.gap.is_empty());
        if has_gaps {
            rows.sort_by(|a, b| {
                let ka = gap_sort_key(&a.gap, a.position);
                let kb = gap_sort_key(&b.gap, b.position);
                ka.0.cmp(&kb.0)
                    .then(ka.1.partial_cmp(&kb.1).unwrap_or(std::cmp::Ordering::Equal))
            });
            for (i, row) in rows.iter_mut().enumerate() {
                row.position = (i + 1) as i64;
            }
        }
    }
}
