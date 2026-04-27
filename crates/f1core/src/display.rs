use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::db::{BoardRow, Db, QualifyingBoardRow};
use crate::domain::sector;
use crate::session_data::DriverBestSectors;

/// Computed display values for a single board row.
/// Shared between TUI and web — each app maps these into its own display types.
pub struct ComputedDisplay {
    pub s1: Option<f64>,
    pub s2: Option<f64>,
    pub s3: Option<f64>,
    /// True when the sector value came from the prev-lap fallback (stale),
    /// not from a timed sector on the current in-progress lap.
    pub s1_stale: bool,
    pub s2_stale: bool,
    pub s3_stale: bool,
    pub last_lap: Option<f64>,
    pub lap: Option<i64>,
}

/// Compute display values for a race board row:
/// progressive sector reveal with previous-lap fallback.
pub fn race_display(virtual_now: DateTime<Utc>, b: &BoardRow) -> ComputedDisplay {
    let (vis_s1, vis_s2, vis_s3) = sector::visible_sectors(
        virtual_now,
        b.lap_date_start.as_deref(),
        b.sector_1,
        b.sector_2,
        b.sector_3,
    );
    ComputedDisplay {
        s1: vis_s1.or(b.prev_sector_1),
        s2: vis_s2.or(b.prev_sector_2),
        s3: vis_s3.or(b.prev_sector_3),
        s1_stale: vis_s1.is_none() && b.prev_sector_1.is_some(),
        s2_stale: vis_s2.is_none() && b.prev_sector_2.is_some(),
        s3_stale: vis_s3.is_none() && b.prev_sector_3.is_some(),
        last_lap: if vis_s3.is_some() {
            b.last_lap
        } else {
            b.prev_last_lap
        },
        lap: if vis_s1.is_some() {
            b.lap_number
        } else {
            b.prev_lap_number
        },
    }
}

/// Compute display values for a qualifying board row:
/// throwaway laps (pit out/in) show previous hot-lap data,
/// hot laps get progressive reveal with previous-lap fallback.
pub fn qualifying_display(virtual_now: DateTime<Utc>, b: &QualifyingBoardRow) -> ComputedDisplay {
    let is_throwaway = b.is_pit_out_lap || b.is_in_lap;
    if is_throwaway {
        return ComputedDisplay {
            s1: b.prev_sector_1,
            s2: b.prev_sector_2,
            s3: b.prev_sector_3,
            s1_stale: b.prev_sector_1.is_some(),
            s2_stale: b.prev_sector_2.is_some(),
            s3_stale: b.prev_sector_3.is_some(),
            last_lap: b.prev_last_lap,
            lap: b.prev_lap_number,
        };
    }

    let (vis_s1, vis_s2, vis_s3) = sector::visible_sectors(
        virtual_now,
        b.lap_date_start.as_deref(),
        b.sector_1,
        b.sector_2,
        b.sector_3,
    );
    // Fall back to prev sectors only when the prev hot lap was the
    // *immediately* preceding lap (back-to-back flyers). If a throwaway lap
    // (out/in) sits between them, the prev sectors are stale and should not
    // leak into a fresh flying lap.
    let prev_is_adjacent = match (b.lap_number, b.prev_lap_number) {
        (Some(cur), Some(prev)) => cur == prev + 1,
        _ => false,
    };
    let prev_s1 = if prev_is_adjacent {
        b.prev_sector_1
    } else {
        None
    };
    let prev_s2 = if prev_is_adjacent {
        b.prev_sector_2
    } else {
        None
    };
    let prev_s3 = if prev_is_adjacent {
        b.prev_sector_3
    } else {
        None
    };
    ComputedDisplay {
        s1: vis_s1.or(prev_s1),
        s2: vis_s2.or(prev_s2),
        s3: vis_s3.or(prev_s3),
        s1_stale: vis_s1.is_none() && prev_s1.is_some(),
        s2_stale: vis_s2.is_none() && prev_s2.is_some(),
        s3_stale: vis_s3.is_none() && prev_s3.is_some(),
        last_lap: if vis_s3.is_some() {
            b.last_lap
        } else {
            b.prev_last_lap
        },
        // Always report the raw current lap, so the frontend can tell whether
        // the driver is actually on track — even before s1 of their first hot
        // lap has been timed (when prev_lap_number would be NULL).
        lap: b.lap_number,
    }
}

/// Build qualifying board rows with segment elimination, positioning, and gap computation.
/// Returns rows in display order (active first, then eliminated) along with the
/// active segment number: `None` before Q1 has started, `Some(1|2|3)` otherwise.
pub fn build_qualifying_rows(
    db: &Db,
    session_key: i64,
    clock_now: &str,
) -> Result<(Vec<QualifyingBoardRow>, Option<u8>)> {
    let segment_starts = db.get_qualifying_segment_starts(session_key, clock_now)?;
    let current_segment = segment_starts.len();
    let segment_start = if segment_starts.len() > 1 {
        Some(segment_starts.last().unwrap().as_str())
    } else {
        None // Q1 or no segments yet — no min date filter
    };

    // Determine eliminated drivers from previous segments.
    // Eliminations per segment = (grid_size - 10) / 2, so Q3 always has 10.
    // 2025 and earlier: 20 drivers → 5 per segment. 2026+: 22 drivers → 6.
    let mut eliminated: HashMap<i64, (String, Option<f64>)> = HashMap::new();
    if current_segment >= 2 {
        let q1_results =
            db.get_segment_results(session_key, &segment_starts[0], &segment_starts[1])?;
        let elim_count = q1_results.len().saturating_sub(10) / 2;
        let q1_cutoff = q1_results.len().saturating_sub(elim_count);
        for (dn, best) in q1_results.iter().skip(q1_cutoff) {
            eliminated.insert(*dn, ("Q1".to_string(), *best));
        }
    }
    if current_segment >= 3 {
        let q2_results =
            db.get_segment_results(session_key, &segment_starts[1], &segment_starts[2])?;
        let q2_active: Vec<_> = q2_results
            .iter()
            .filter(|(dn, _)| !eliminated.contains_key(dn))
            .collect();
        // Q2 always advances the top 10 regardless of grid size.
        let q2_cutoff = q2_active.len().min(10);
        for (dn, best) in q2_active.iter().skip(q2_cutoff) {
            eliminated.insert(*dn, ("Q2".to_string(), *best));
        }
    }

    let mut raw_rows = db.get_qualifying_board_rows(session_key, clock_now, segment_start)?;

    // Partition: active vs eliminated.
    let mut active_rows = Vec::new();
    let mut elim_rows = Vec::new();
    for mut row in raw_rows.drain(..) {
        if let Some((tag, seg_best)) = eliminated.get(&row.driver_number) {
            row.knocked_out = tag.clone();
            row.best_lap = *seg_best;
            elim_rows.push(row);
        } else {
            active_rows.push(row);
        }
    }

    // Sort eliminated: Q1 first, then Q2, within each group by best_lap.
    elim_rows.sort_by(|a, b| {
        a.knocked_out
            .cmp(&b.knocked_out)
            .then_with(|| match (a.best_lap, b.best_lap) {
                (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
    });

    // Assign positions and compute gap to leader for active rows.
    let leader_best = active_rows.first().and_then(|r| r.best_lap);
    for (i, row) in active_rows.iter_mut().enumerate() {
        row.position = (i + 1) as i64;
        row.gap = match (row.best_lap, leader_best) {
            (Some(bl), Some(lb)) if (bl - lb).abs() < 0.001 => String::new(),
            (Some(bl), Some(lb)) => format!("+{:.3}", bl - lb),
            _ => String::new(),
        };
    }
    let elim_start = active_rows.len() + 1;
    for (i, row) in elim_rows.iter_mut().enumerate() {
        row.position = (elim_start + i) as i64;
    }

    active_rows.extend(elim_rows);
    let segment = match current_segment {
        0 => None,
        n => Some(n.min(3) as u8),
    };
    Ok((active_rows, segment))
}

/// Compute overall and per-driver best sectors.
///
/// `driver_laps` is an iterator of `(driver_number, display_lap)` — typically
/// one entry per visible board row.
pub fn compute_best_sectors(
    db: &Db,
    session_key: i64,
    driver_laps: &[(i64, Option<i64>)],
    since_date: Option<&str>,
    clock_now: &str,
) -> Result<BestSectorsResult> {
    let max_display_lap = driver_laps.iter().filter_map(|(_, dl)| *dl).max();
    let (s1, s2, s3) = db.get_best_sectors(session_key, max_display_lap, since_date, clock_now)?;

    let mut per_driver = HashMap::new();
    for &(dn, dl) in driver_laps {
        let best = db.get_driver_best_sectors(session_key, dn, dl, since_date, clock_now)?;
        per_driver.insert(dn, best);
    }

    Ok(BestSectorsResult {
        overall: (s1, s2, s3),
        per_driver,
    })
}

pub struct BestSectorsResult {
    pub overall: (Option<f64>, Option<f64>, Option<f64>),
    pub per_driver: DriverBestSectors,
}
