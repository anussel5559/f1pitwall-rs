use serde::Serialize;
use std::collections::HashMap;
use ts_rs::TS;

// ── Constants ──────────────────────────────────────────────────────

/// Minimum clean laps in a stint to compute a meaningful degradation rate.
const MIN_CLEAN_LAPS: usize = 4;
/// Rolling window size for cliff detection.
const CLIFF_WINDOW: usize = 4;
/// Lap time jump (seconds) in rolling avg that signals a tyre cliff.
/// 0.8s is a meaningful performance drop — not traffic or a single dirty lap.
const CLIFF_THRESHOLD: f64 = 0.8;
/// Lap times more than this many seconds above stint median are outliers.
const OUTLIER_THRESHOLD: f64 = 5.0;
/// Approximate fuel mass effect: car gets ~0.06s/lap faster as fuel burns off.
const FUEL_EFFECT_PER_LAP: f64 = 0.06;

// ── Input types ────────────────────────────────────────────────────

/// Per-driver, per-lap data fetched from the DB.
/// Constructed by the caller (queries.rs) — keeps domain logic decoupled.
pub struct StintLapData {
    pub driver_number: i64,
    pub stint_number: i64,
    pub compound: String,
    pub tyre_age_at_start: i64,
    pub lap_number: i64,
    pub lap_duration: f64,
    pub is_pit_out_lap: bool,
    /// True if this lap was under SC/VSC/red flag conditions.
    pub is_neutralized: bool,
}

// ── Output types ───────────────────────────────────────────────────

#[derive(Serialize, Clone, PartialEq, TS)]
#[ts(export)]
pub struct StintSummary {
    pub driver_number: i64,
    pub stint_number: i64,
    pub compound: String,
    pub lap_start: i64,
    pub lap_count: i64,
    pub tyre_age_start: i64,
    pub tyre_age_end: i64,
    /// Average lap time on clean laps.
    pub avg_pace: f64,
    /// Linear degradation rate in seconds/lap (positive = getting slower).
    pub deg_rate: f64,
    /// Fuel-corrected degradation rate. Isolates tyre-only degradation by removing
    /// the ~0.06s/lap fuel mass improvement. Only available when total_laps is known.
    pub fuel_corrected_deg_rate: Option<f64>,
    /// Per-lap deltas from stint baseline (best clean lap). None for filtered laps.
    /// Index 0 = first lap of stint.
    pub lap_deltas: Vec<Option<f64>>,
    /// True if this is the driver's currently active stint.
    pub is_current: bool,
    /// Average of last 3 clean lap times.
    pub recent_3lap_avg: f64,
    /// recent_3lap_avg minus avg_pace (positive = slowing down recently).
    pub recent_3lap_delta: f64,
    /// Slope acceleration: second-half slope minus first-half slope.
    /// Positive means degradation is accelerating.
    pub slope_acceleration: f64,
    /// Maximum absolute deviation from avg_pace among clean laps.
    pub max_lap_delta: f64,
}

#[derive(Serialize, Clone, PartialEq, TS)]
#[ts(export)]
pub struct TyreCliff {
    pub driver_number: i64,
    pub stint_number: i64,
    pub compound: String,
    pub detected_at_lap: i64,
    pub tyre_age: i64,
    /// How sharp the cliff is (rolling avg jump in seconds).
    pub severity: f64,
    /// Opinionated summary, e.g. "NOR softs falling off — +0.7s over 3 laps at 18 laps old"
    pub headline: String,
}

#[derive(Serialize, Clone, PartialEq, TS)]
#[ts(export)]
pub struct CompoundBenchmark {
    pub compound: String,
    pub sample_count: i64,
    /// Average degradation rate across all stints on this compound.
    pub avg_deg_rate: f64,
}

#[derive(Serialize, Clone, PartialEq, TS)]
#[ts(export)]
pub struct DegradationAnalysis {
    /// All stints for all drivers, ordered by driver then stint number.
    pub stints: Vec<StintSummary>,
    /// Currently active tyre cliffs (the "watch this" data).
    pub cliffs: Vec<TyreCliff>,
    /// Per-compound average deg rates for context.
    pub compound_benchmarks: Vec<CompoundBenchmark>,
}

// ── Helpers ────────────────────────────────────────────────────────

use super::{compound_label, linear_slope};

/// Remove outlier laps using Q1-based filtering.
/// Uses the 25th-percentile lap time as reference instead of the median so that
/// safety-car periods (which can outnumber racing laps) don't pull the reference
/// into the slow cluster and cause all racing laps to be discarded.
/// Returns (clean laps, baseline) where baseline is the minimum clean lap time.
fn remove_outliers(laps: &[(i64, f64)]) -> (Vec<(i64, f64)>, f64) {
    if laps.is_empty() {
        return (Vec::new(), 0.0);
    }
    let mut times: Vec<f64> = laps.iter().map(|(_, t)| *t).collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Q1 (25th percentile) sits in the racing-pace cluster even when SC laps
    // outnumber racing laps (e.g. 5 racing + 10 SC → Q1 index 3 → racing).
    let q1 = times[times.len() / 4];
    let clean: Vec<(i64, f64)> = laps
        .iter()
        .filter(|(_, t)| (*t - q1).abs() < OUTLIER_THRESHOLD)
        .copied()
        .collect();
    let baseline = clean.iter().map(|(_, t)| *t).reduce(f64::min).unwrap_or(q1);
    (clean, baseline)
}

/// Median of lap times in a window. Robust against a single noisy lap.
fn window_median(laps: &[(i64, f64)]) -> f64 {
    let mut times: Vec<f64> = laps.iter().map(|(_, t)| *t).collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = times.len() / 2;
    if times.len().is_multiple_of(2) {
        (times[mid - 1] + times[mid]) / 2.0
    } else {
        times[mid]
    }
}

/// Detect a tyre cliff in the most recent laps of a stint.
/// Compares the rolling median of the last CLIFF_WINDOW laps against the prior window.
fn detect_cliff(
    clean_laps: &[(i64, f64)],
    driver_number: i64,
    stint_number: i64,
    compound: &str,
    tyre_age_at_start: i64,
    stint_lap_start: i64,
    acronyms: &HashMap<i64, String>,
) -> Option<TyreCliff> {
    let n = clean_laps.len();
    if n < CLIFF_WINDOW * 2 {
        return None;
    }

    // Use median (not mean) for rolling windows so a single noisy lap
    // (traffic, brief yellow) can't trigger a false cliff.
    let recent_median = window_median(&clean_laps[n - CLIFF_WINDOW..]);
    let prior_median = window_median(&clean_laps[n - 2 * CLIFF_WINDOW..n - CLIFF_WINDOW]);

    let jump = recent_median - prior_median;
    if jump < CLIFF_THRESHOLD {
        return None;
    }

    let detected_at_lap = clean_laps[n - CLIFF_WINDOW].0;
    let tyre_age = tyre_age_at_start + (detected_at_lap - stint_lap_start);
    let acr = acronyms
        .get(&driver_number)
        .cloned()
        .unwrap_or_else(|| format!("#{}", driver_number));

    Some(TyreCliff {
        driver_number,
        stint_number,
        compound: compound.to_string(),
        detected_at_lap,
        tyre_age,
        severity: jump,
        headline: format!(
            "{} {} falling off \u{2014} +{:.1}s over {} laps at {} laps old",
            acr,
            compound_label(compound),
            jump,
            CLIFF_WINDOW,
            tyre_age
        ),
    })
}

// ── Core analysis ──────────────────────────────────────────────────

/// Analyze degradation across all stints in the session.
/// Pure function — no DB or mutable state.
///
/// When `total_laps` is provided (race/sprint), fuel correction is applied to
/// isolate tyre-only degradation from the fuel mass effect.
pub fn analyze_degradation(
    lap_data: &[StintLapData],
    acronyms: &HashMap<i64, String>,
    total_laps: Option<i64>,
) -> DegradationAnalysis {
    // Group by (driver_number, stint_number)
    let mut stint_groups: HashMap<(i64, i64), Vec<&StintLapData>> = HashMap::new();
    for lap in lap_data {
        stint_groups
            .entry((lap.driver_number, lap.stint_number))
            .or_default()
            .push(lap);
    }

    // Find the max stint number per driver to determine which stint is current
    let mut max_stint: HashMap<i64, i64> = HashMap::new();
    for &(driver_number, stint_number) in stint_groups.keys() {
        let entry = max_stint.entry(driver_number).or_insert(0);
        if stint_number > *entry {
            *entry = stint_number;
        }
    }

    let mut stints = Vec::new();
    let mut cliffs = Vec::new();
    let mut compound_degs: HashMap<String, Vec<f64>> = HashMap::new();

    for (&(driver_number, stint_number), laps) in &stint_groups {
        let mut laps = laps.clone();
        laps.sort_by_key(|l| l.lap_number);

        let compound = laps[0].compound.clone();
        let tyre_age_at_start = laps[0].tyre_age_at_start;
        let lap_start = laps[0].lap_number;
        let lap_count = laps.len() as i64;
        let is_current = max_stint.get(&driver_number) == Some(&stint_number);

        // Filter to clean laps for analysis (exclude pit out/in, SC, invalid)
        let candidate_times: Vec<(i64, f64)> = laps
            .iter()
            .filter(|l| !l.is_pit_out_lap && !l.is_neutralized && l.lap_duration > 0.0)
            .map(|l| (l.lap_number, l.lap_duration))
            .collect();

        // Remove outliers using median
        let (clean_laps, baseline) = remove_outliers(&candidate_times);

        let (deg_rate, fuel_corrected_deg_rate, avg_pace) = if clean_laps.len() >= MIN_CLEAN_LAPS {
            let pairs: Vec<(f64, f64)> = clean_laps.iter().map(|&(x, y)| (x as f64, y)).collect();
            let rate = linear_slope(&pairs);
            let avg = clean_laps.iter().map(|(_, t)| t).sum::<f64>() / clean_laps.len() as f64;

            // Fuel correction: remove fuel mass effect to isolate tyre degradation.
            // As fuel burns, the car gets lighter by ~FUEL_EFFECT_PER_LAP per lap,
            // making it faster. This masks tyre degradation. Subtract the fuel penalty
            // (proportional to remaining fuel) so the corrected slope reflects only tyres.
            let fc_rate = total_laps.map(|tl| {
                let fuel_corrected: Vec<(f64, f64)> = clean_laps
                    .iter()
                    .map(|&(lap, time)| {
                        let laps_remaining = (tl - lap).max(0) as f64;
                        (lap as f64, time - FUEL_EFFECT_PER_LAP * laps_remaining)
                    })
                    .collect();
                linear_slope(&fuel_corrected)
            });

            (rate, fc_rate, avg)
        } else {
            let avg = if clean_laps.is_empty() {
                0.0
            } else {
                clean_laps.iter().map(|(_, t)| t).sum::<f64>() / clean_laps.len() as f64
            };
            (0.0, None, avg)
        };

        // Build lap_deltas relative to baseline (best clean lap).
        // Only include laps that survived outlier removal.
        let clean_lap_set: std::collections::HashSet<i64> =
            clean_laps.iter().map(|(ln, _)| *ln).collect();

        let lap_deltas: Vec<Option<f64>> = laps
            .iter()
            .map(|l| {
                if clean_lap_set.contains(&l.lap_number) {
                    Some(l.lap_duration - baseline)
                } else {
                    None
                }
            })
            .collect();

        // Compute ML-relevant derived features from clean laps
        let recent_3lap_avg = if clean_laps.len() >= 3 {
            let last3 = &clean_laps[clean_laps.len() - 3..];
            last3.iter().map(|(_, t)| t).sum::<f64>() / 3.0
        } else if !clean_laps.is_empty() {
            clean_laps.iter().map(|(_, t)| t).sum::<f64>() / clean_laps.len() as f64
        } else {
            0.0
        };
        let recent_3lap_delta = recent_3lap_avg - avg_pace;

        let slope_acceleration = if clean_laps.len() >= 4 {
            let half = clean_laps.len() / 2;
            let first_half: Vec<(f64, f64)> = clean_laps[..half]
                .iter()
                .map(|&(x, y)| (x as f64, y))
                .collect();
            let second_half: Vec<(f64, f64)> = clean_laps[half..]
                .iter()
                .map(|&(x, y)| (x as f64, y))
                .collect();
            linear_slope(&second_half) - linear_slope(&first_half)
        } else {
            0.0
        };

        let max_lap_delta = if !clean_laps.is_empty() {
            clean_laps
                .iter()
                .map(|(_, t)| (t - avg_pace).abs())
                .reduce(f64::max)
                .unwrap_or(0.0)
        } else {
            0.0
        };

        // Cliff detection — only on current stints with enough data
        if is_current
            && clean_laps.len() >= CLIFF_WINDOW * 2
            && let Some(cliff) = detect_cliff(
                &clean_laps,
                driver_number,
                stint_number,
                &compound,
                tyre_age_at_start,
                lap_start,
                acronyms,
            )
        {
            cliffs.push(cliff);
        }

        // Contribute to compound benchmarks (any stint with enough clean data)
        // Prefer fuel-corrected rate for accuracy when available
        if clean_laps.len() >= MIN_CLEAN_LAPS {
            let benchmark_rate = fuel_corrected_deg_rate.unwrap_or(deg_rate);
            compound_degs
                .entry(compound.clone())
                .or_default()
                .push(benchmark_rate);
        }

        stints.push(StintSummary {
            driver_number,
            stint_number,
            compound,
            lap_start,
            lap_count,
            tyre_age_start: tyre_age_at_start,
            tyre_age_end: tyre_age_at_start + lap_count - 1,
            avg_pace,
            deg_rate,
            fuel_corrected_deg_rate,
            lap_deltas,
            is_current,
            recent_3lap_avg,
            recent_3lap_delta,
            slope_acceleration,
            max_lap_delta,
        });
    }

    // Sort stints by driver, then stint number
    stints.sort_by_key(|s| (s.driver_number, s.stint_number));

    // Build compound benchmarks
    let mut compound_benchmarks: Vec<CompoundBenchmark> = compound_degs
        .into_iter()
        .map(|(compound, rates)| {
            let avg = rates.iter().sum::<f64>() / rates.len() as f64;
            CompoundBenchmark {
                compound,
                sample_count: rates.len() as i64,
                avg_deg_rate: avg,
            }
        })
        .collect();
    compound_benchmarks.sort_by(|a, b| {
        a.compound
            .partial_cmp(&b.compound)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Sort cliffs by severity (worst first)
    cliffs.sort_by(|a, b| {
        b.severity
            .partial_cmp(&a.severity)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    DegradationAnalysis {
        stints,
        cliffs,
        compound_benchmarks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_lap(
        driver: i64,
        stint: i64,
        compound: &str,
        age_start: i64,
        lap: i64,
        duration: f64,
    ) -> StintLapData {
        StintLapData {
            driver_number: driver,
            stint_number: stint,
            compound: compound.to_string(),
            tyre_age_at_start: age_start,
            lap_number: lap,
            lap_duration: duration,
            is_pit_out_lap: false,
            is_neutralized: false,
        }
    }

    #[test]
    fn test_basic_degradation_rate() {
        // Simulate a stint with clear degradation: ~0.1s/lap
        let laps: Vec<StintLapData> = (1..=10)
            .map(|i| make_lap(1, 1, "SOFT", 0, i, 90.0 + (i as f64) * 0.1))
            .collect();
        let acronyms = HashMap::from([(1, "VER".to_string())]);
        let result = analyze_degradation(&laps, &acronyms, None);

        assert_eq!(result.stints.len(), 1);
        let stint = &result.stints[0];
        assert_eq!(stint.driver_number, 1);
        assert_eq!(stint.compound, "SOFT");
        assert_eq!(stint.lap_count, 10);
        assert!(
            stint.deg_rate > 0.08 && stint.deg_rate < 0.12,
            "deg_rate={}",
            stint.deg_rate
        );
    }

    #[test]
    fn test_cliff_detection() {
        let mut laps: Vec<StintLapData> = Vec::new();
        let acronyms = HashMap::from([(1, "NOR".to_string())]);

        // First 10 laps: consistent ~90s with mild deg
        for i in 1..=10 {
            laps.push(make_lap(1, 1, "SOFT", 0, i, 90.0 + (i as f64) * 0.05));
        }
        // Laps 11-14: sudden cliff (+1.0s jump)
        for i in 11..=14 {
            laps.push(make_lap(1, 1, "SOFT", 0, i, 91.5 + (i as f64) * 0.05));
        }

        let result = analyze_degradation(&laps, &acronyms, None);
        assert!(!result.cliffs.is_empty(), "Should detect a cliff");
        let cliff = &result.cliffs[0];
        assert_eq!(cliff.driver_number, 1);
        assert!(cliff.severity >= 0.8);
        assert!(cliff.headline.contains("NOR"));
        assert!(cliff.headline.contains("Softs"));
    }

    #[test]
    fn test_pit_out_laps_filtered() {
        let mut laps: Vec<StintLapData> = Vec::new();
        // First lap is pit out — should be filtered
        laps.push(StintLapData {
            is_pit_out_lap: true,
            ..make_lap(1, 1, "MEDIUM", 0, 1, 120.0)
        });
        for i in 2..=8 {
            laps.push(make_lap(1, 1, "MEDIUM", 0, i, 91.0 + (i as f64) * 0.05));
        }

        let acronyms = HashMap::from([(1, "HAM".to_string())]);
        let result = analyze_degradation(&laps, &acronyms, None);
        let stint = &result.stints[0];
        // First delta should be None (pit out)
        assert!(stint.lap_deltas[0].is_none());
        // avg_pace should not be skewed by the 120s pit out lap
        assert!(stint.avg_pace < 95.0);
    }

    #[test]
    fn test_multiple_drivers_and_stints() {
        let mut laps = Vec::new();
        // Driver 1, stint 1 (softs)
        for i in 1..=6 {
            laps.push(make_lap(1, 1, "SOFT", 0, i, 90.0 + (i as f64) * 0.15));
        }
        // Driver 1, stint 2 (hards)
        for i in 7..=14 {
            laps.push(make_lap(1, 2, "HARD", 0, i, 91.0 + ((i - 7) as f64) * 0.05));
        }
        // Driver 2, stint 1 (mediums)
        for i in 1..=10 {
            laps.push(make_lap(2, 1, "MEDIUM", 0, i, 90.5 + (i as f64) * 0.08));
        }

        let acronyms = HashMap::from([(1, "VER".to_string()), (2, "HAM".to_string())]);
        let result = analyze_degradation(&laps, &acronyms, None);

        assert_eq!(result.stints.len(), 3);
        // Compound benchmarks should exist
        assert!(!result.compound_benchmarks.is_empty());
    }

    #[test]
    fn test_no_cliff_on_completed_stint() {
        let mut laps = Vec::new();
        let acronyms = HashMap::from([(1, "VER".to_string())]);

        // Stint 1: cliff-like pattern but completed (not current)
        for i in 1..=8 {
            laps.push(make_lap(1, 1, "SOFT", 0, i, 90.0));
        }
        for i in 9..=12 {
            laps.push(make_lap(1, 1, "SOFT", 0, i, 91.5));
        }
        // Stint 2: current stint, no cliff
        for i in 13..=20 {
            laps.push(make_lap(1, 2, "HARD", 0, i, 91.0));
        }

        let result = analyze_degradation(&laps, &acronyms, None);
        // Should not detect cliff on stint 1 (it's not current)
        assert!(result.cliffs.is_empty());
    }
}
