use serde::Serialize;
use std::collections::HashMap;
use ts_rs::TS;

use super::degradation::{CompoundBenchmark, DegradationAnalysis, StintSummary};

// ── Constants ──────────────────────────────────────────────────────

/// Minimum clean laps for long-run practice stint analysis.
const LONG_RUN_MIN_LAPS: i64 = 5;
/// Default delta threshold (seconds above baseline) at which a stint is "expired".
const DELTA_THRESHOLD: f64 = 2.0;
/// Margin laps before projected expiry for the pit window to open.
const WINDOW_MARGIN_BEFORE: i64 = 3;
/// Margin laps after projected expiry (driver might push a little).
const WINDOW_MARGIN_AFTER: i64 = 2;
/// Default compound lifetime caps when no other data is available.
const DEFAULT_LIFE_SOFT: i64 = 18;
const DEFAULT_LIFE_MEDIUM: i64 = 26;
const DEFAULT_LIFE_HARD: i64 = 35;
const DEFAULT_LIFE_OTHER: i64 = 35;

// ── Input types ────────────────────────────────────────────────────

/// State of a single driver at a point in time.
/// Parameterized: can come from live degradation data OR a user-provided scenario.
#[derive(Clone)]
pub struct DriverState {
    pub driver_number: i64,
    pub position: i64,
    pub gap_to_leader: f64,
    pub compound: String,
    pub tyre_age: i64,
    pub deg_rate: f64,
    pub fuel_corrected_deg_rate: Option<f64>,
    pub avg_pace: f64,
    pub clean_lap_count: usize,
}

/// Compound baselines: practice long-run data + live race field benchmarks.
pub struct CompoundBaselines {
    pub practice: Vec<PracticeBaseline>,
    pub field_benchmarks: Vec<CompoundBenchmark>,
}

/// A scenario for stint projection.
/// In automated mode, `pit_on_lap` is None (engine determines expiry).
/// In interactive mode, user provides pit_on_lap + switch_to_compound.
#[derive(Clone)]
pub struct StintScenario {
    pub driver_number: i64,
    pub pit_on_lap: Option<i64>,
    pub switch_to_compound: Option<String>,
}

// ── Output types ───────────────────────────────────────────────────

#[derive(Serialize, Clone, PartialEq, TS)]
#[ts(export)]
pub struct PitWindow {
    pub driver_number: i64,
    pub compound: String,
    pub tyre_age: i64,
    pub estimated_laps_remaining: i64,
    pub window_open_lap: i64,
    pub window_close_lap: i64,
    pub confidence: Confidence,
    pub reason: String,
}

#[derive(Serialize, Clone, PartialEq, TS)]
#[ts(export)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

/// Projected pace curve for a stint. Used by both automated predictions and
/// the future interactive scenario mode.
#[derive(Serialize, Clone, PartialEq)]
pub struct ProjectedStint {
    pub compound: String,
    pub start_lap: i64,
    pub projected_pace: Vec<(i64, f64)>,
    pub projected_deg_rate: f64,
}

#[derive(Serialize, Clone, PartialEq, TS)]
#[ts(export)]
pub struct PracticeBaseline {
    pub compound: String,
    pub expected_deg_rate: f64,
    pub expected_pace: f64,
    pub sample_stints: i64,
    pub sample_laps: i64,
}

#[derive(Serialize, Clone, PartialEq, TS)]
#[ts(export)]
pub struct PracticeAnalysis {
    pub baselines: Vec<PracticeBaseline>,
    pub sessions_loaded: Vec<String>,
}

#[derive(Serialize, Clone, PartialEq, TS)]
#[ts(export)]
pub struct StrategyAnalysis {
    pub pit_windows: Vec<PitWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub practice: Option<PracticeAnalysis>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ml_predictions: Option<Vec<MlPitPrediction>>,
}

/// ML model pit window prediction with quantile-based confidence range.
#[derive(Serialize, Clone, PartialEq, TS)]
#[ts(export)]
pub struct MlPitPrediction {
    pub driver_number: i64,
    pub compound: String,
    pub tyre_age: i64,
    pub estimated_laps_remaining: i64,
    pub window_open_lap: i64,
    pub window_close_lap: i64,
    pub confidence: Confidence,
}

// ── Practice baseline extraction ───────────────────────────────────

/// Extract compound baselines from practice session degradation data.
/// Filters to long-run stints (>= LONG_RUN_MIN_LAPS) and averages per compound.
pub fn extract_practice_baselines(
    practice_stints: &[StintSummary],
    session_names: Vec<String>,
) -> PracticeAnalysis {
    let mut compound_data: HashMap<String, Vec<(f64, f64, i64)>> = HashMap::new();

    for stint in practice_stints {
        if stint.lap_count < LONG_RUN_MIN_LAPS {
            continue;
        }
        // Only use stints with computed degradation (deg_rate != 0 means >= MIN_CLEAN_LAPS)
        if stint.deg_rate == 0.0 {
            continue;
        }
        compound_data
            .entry(stint.compound.clone())
            .or_default()
            .push((stint.deg_rate, stint.avg_pace, stint.lap_count));
    }

    let mut baselines: Vec<PracticeBaseline> = compound_data
        .into_iter()
        .map(|(compound, data)| {
            let n = data.len() as f64;
            let avg_deg = data.iter().map(|(d, _, _)| d).sum::<f64>() / n;
            let avg_pace = data.iter().map(|(_, p, _)| p).sum::<f64>() / n;
            let total_laps: i64 = data.iter().map(|(_, _, l)| l).sum();
            PracticeBaseline {
                compound,
                expected_deg_rate: avg_deg,
                expected_pace: avg_pace,
                sample_stints: data.len() as i64,
                sample_laps: total_laps,
            }
        })
        .collect();

    // Sort: SOFT → MEDIUM → HARD → INTERMEDIATE → WET
    baselines.sort_by_key(|b| compound_sort_key(&b.compound));

    PracticeAnalysis {
        baselines,
        sessions_loaded: session_names,
    }
}

// ── Stint projection (core reusable engine) ────────────────────────

/// Project a single driver's stint forward.
///
/// This is the core building block for both automated pit window predictions
/// and future interactive "what if" scenarios.
pub fn project_stint(
    driver: &DriverState,
    _scenario: &StintScenario,
    baselines: &CompoundBaselines,
    total_laps: Option<i64>,
    current_lap: i64,
) -> ProjectedStint {
    // Use fuel-corrected deg rate when available, fall back to raw
    let deg_rate = driver.fuel_corrected_deg_rate.unwrap_or(driver.deg_rate);

    // If driver has no meaningful deg data, try practice/field baselines
    let effective_deg_rate = if deg_rate.abs() < f64::EPSILON {
        baseline_deg_rate(&driver.compound, baselines).unwrap_or(0.0)
    } else {
        deg_rate
    };

    // Project forward from current age
    let horizon = total_laps.map(|tl| tl - current_lap + 10).unwrap_or(30);
    let mut projected_pace = Vec::new();
    for laps_forward in 0..horizon {
        let future_lap = current_lap + laps_forward;
        let future_age = driver.tyre_age + laps_forward;
        let projected_time = driver.avg_pace + effective_deg_rate * future_age as f64;
        projected_pace.push((future_lap, projected_time));
    }

    ProjectedStint {
        compound: driver.compound.clone(),
        start_lap: current_lap,
        projected_pace,
        projected_deg_rate: effective_deg_rate,
    }
}

// ── Pit window analysis (automated mode) ───────────────────────────

/// Analyze pit windows for all drivers on current stints.
///
/// Returns one `PitWindow` per driver currently on track. Sorted by urgency
/// (fewest estimated laps remaining first).
pub fn analyze_pit_windows(
    degradation: &DegradationAnalysis,
    current_lap: i64,
    total_laps: Option<i64>,
    baselines: &CompoundBaselines,
) -> Vec<PitWindow> {
    let mut windows = Vec::new();

    // Build field evidence: average completed stint length per compound.
    // Once drivers have pitted, their actual stint lengths are the best predictor
    // for remaining drivers on the same compound at this circuit.
    let field_stint_lengths = completed_stint_lengths(&degradation.stints, total_laps);

    for stint in &degradation.stints {
        if !stint.is_current {
            continue;
        }

        let clean_lap_count = stint.lap_deltas.iter().filter(|d| d.is_some()).count();

        let confidence = match clean_lap_count {
            n if n >= 8 => Confidence::High,
            n if n >= 4 => Confidence::Medium,
            _ => Confidence::Low,
        };

        // Skip low-confidence predictions — not enough data for meaningful analysis
        if matches!(confidence, Confidence::Low) {
            continue;
        }

        let driver_state = DriverState {
            driver_number: stint.driver_number,
            position: 0, // not needed for pit window calc
            gap_to_leader: 0.0,
            compound: stint.compound.clone(),
            tyre_age: stint.tyre_age_end,
            deg_rate: stint.deg_rate,
            fuel_corrected_deg_rate: stint.fuel_corrected_deg_rate,
            avg_pace: stint.avg_pace,
            clean_lap_count,
        };

        let projection = project_stint(
            &driver_state,
            &StintScenario {
                driver_number: stint.driver_number,
                pit_on_lap: None,
                switch_to_compound: None,
            },
            baselines,
            total_laps,
            current_lap,
        );

        let (expiry_age, reason) = compute_expiry(
            &driver_state,
            &projection,
            baselines,
            &degradation.cliffs,
            &field_stint_lengths,
            current_lap,
        );

        let laps_remaining = (expiry_age - driver_state.tyre_age).max(0);
        let expiry_lap = current_lap + laps_remaining;

        // Clamp window within remaining race laps
        let race_end = total_laps.map(|tl| tl + 1).unwrap_or(i64::MAX);
        let window_open = (expiry_lap - WINDOW_MARGIN_BEFORE)
            .max(current_lap + 1)
            .min(race_end);
        let window_close = (expiry_lap + WINDOW_MARGIN_AFTER).min(race_end);

        windows.push(PitWindow {
            driver_number: stint.driver_number,
            compound: stint.compound.clone(),
            tyre_age: driver_state.tyre_age,
            estimated_laps_remaining: laps_remaining,
            window_open_lap: window_open,
            window_close_lap: window_close,
            confidence,
            reason,
        });
    }

    // Sort by urgency: fewest laps remaining first
    windows.sort_by_key(|w| w.estimated_laps_remaining);
    windows
}

// ── Helpers ────────────────────────────────────────────────────────

/// Determine the tyre age at which this stint is projected to expire.
/// Uses multiple bounds and picks the most conservative (earliest).
fn compute_expiry(
    driver: &DriverState,
    projection: &ProjectedStint,
    baselines: &CompoundBaselines,
    cliffs: &[super::degradation::TyreCliff],
    field_stint_lengths: &HashMap<String, (i64, usize)>,
    current_lap: i64,
) -> (i64, String) {
    let deg_rate = projection.projected_deg_rate;
    let compound_life = default_compound_life(&driver.compound);
    let mut expiry_age = compound_life;
    let mut reason = format!(
        "Default {}-lap {} life cap",
        compound_life,
        compound_label(&driver.compound)
    );

    // Bound 1: field completed stint lengths for this compound.
    // This is a strong signal: actual pit stop decisions at this circuit today.
    // Requires 3+ completed stints so a few early undercuts don't dominate.
    if let Some(&(avg_length, count)) = field_stint_lengths.get(&driver.compound)
        && count >= 3
        && avg_length > 0
        && avg_length < expiry_age
    {
        expiry_age = avg_length;
        reason = format!(
            "Field avg {} stint: {} laps ({} completed)",
            compound_label(&driver.compound),
            avg_length,
            count
        );
    }

    // Bound 2: delta threshold — when does projected delta exceed DELTA_THRESHOLD?
    if deg_rate > f64::EPSILON {
        // projected_delta_at_age = deg_rate * age
        // Solve: deg_rate * age = DELTA_THRESHOLD
        let threshold_age = (DELTA_THRESHOLD / deg_rate).floor() as i64;
        if threshold_age > 0 && threshold_age < expiry_age {
            expiry_age = threshold_age;
            reason = format!(
                "Projected +{:.1}s delta at age {} (deg {:.3}s/lap)",
                DELTA_THRESHOLD, threshold_age, deg_rate
            );
        }
    }

    // Bound 3: field compound benchmark — average cliff age for this compound.
    // Requires 2+ observations so a single driver's cliff doesn't set
    // expiry for the entire field (different cars degrade differently).
    let field_cliff_ages: Vec<i64> = cliffs
        .iter()
        .filter(|c| c.compound == driver.compound && c.driver_number != driver.driver_number)
        .map(|c| c.tyre_age)
        .collect();
    if field_cliff_ages.len() >= 3 {
        let avg_cliff_age = field_cliff_ages.iter().sum::<i64>() / field_cliff_ages.len() as i64;
        if avg_cliff_age < expiry_age {
            expiry_age = avg_cliff_age;
            reason = format!(
                "Field avg cliff at age {} for {}",
                avg_cliff_age,
                compound_label(&driver.compound)
            );
        }
    }

    // Bound 4: practice baseline expected life (if available)
    // Use practice deg rate to estimate when delta hits threshold
    if let Some(practice_rate) = practice_deg_rate(&driver.compound, baselines)
        && practice_rate > f64::EPSILON
    {
        let practice_life = (DELTA_THRESHOLD / practice_rate).floor() as i64;
        if practice_life > 0 && practice_life < expiry_age {
            expiry_age = practice_life;
            reason = format!(
                "Practice baseline: {} life ~{} laps (deg {:.3}s/lap)",
                compound_label(&driver.compound),
                practice_life,
                practice_rate
            );
        }
    }

    // If the driver already has a cliff detected, they're past expiry
    let has_active_cliff = cliffs.iter().any(|c| {
        c.driver_number == driver.driver_number
            && c.detected_at_lap >= current_lap.saturating_sub(3)
    });
    if has_active_cliff {
        expiry_age = driver.tyre_age;
        reason = format!(
            "Active cliff detected — {} at {} laps old",
            compound_label(&driver.compound),
            driver.tyre_age
        );
    }

    (expiry_age, reason)
}

/// Look up practice deg rate for a compound from baselines.
fn practice_deg_rate(compound: &str, baselines: &CompoundBaselines) -> Option<f64> {
    baselines
        .practice
        .iter()
        .find(|b| b.compound.eq_ignore_ascii_case(compound))
        .map(|b| b.expected_deg_rate)
}

/// Look up any baseline deg rate for a compound (practice first, then field).
fn baseline_deg_rate(compound: &str, baselines: &CompoundBaselines) -> Option<f64> {
    practice_deg_rate(compound, baselines).or_else(|| {
        baselines
            .field_benchmarks
            .iter()
            .find(|b| b.compound.eq_ignore_ascii_case(compound))
            .map(|b| b.avg_deg_rate)
    })
}

fn compound_label(compound: &str) -> &str {
    match compound.to_uppercase().as_str() {
        "SOFT" => "softs",
        "MEDIUM" => "mediums",
        "HARD" => "hards",
        "INTERMEDIATE" => "inters",
        "WET" => "wets",
        _ => compound,
    }
}

/// Compute average completed stint length per compound from this race's data.
/// Only includes stints that ended with a pit stop (not final stints that ran to the flag).
/// Filters out short strategic stops (undercuts) that don't represent tyre life.
/// Returns map of compound -> (avg_stint_length_in_tyre_age, count).
fn completed_stint_lengths(
    stints: &[super::degradation::StintSummary],
    total_laps: Option<i64>,
) -> HashMap<String, (i64, usize)> {
    let race_end_cutoff = total_laps.map(|tl| tl - 2).unwrap_or(i64::MAX);

    let mut compound_lengths: HashMap<String, Vec<i64>> = HashMap::new();
    for stint in stints {
        // Skip current stints (not yet completed)
        if stint.is_current {
            continue;
        }
        // Skip final stints that ran to the flag (not a strategic pit decision)
        let stint_end_lap = stint.lap_start + stint.lap_count - 1;
        if stint_end_lap >= race_end_cutoff {
            continue;
        }
        let tyre_life = stint.tyre_age_end - stint.tyre_age_start + 1;
        // Skip very short stints (likely SC/incident related, not representative)
        if tyre_life < 5 {
            continue;
        }
        // Skip stints shorter than 50% of default compound life — these are
        // strategic stops (undercuts/overcuts), not degradation-driven, and
        // would falsely pull everyone's expiry forward.
        let default_life = default_compound_life(&stint.compound);
        if tyre_life < default_life / 2 {
            continue;
        }
        if !stint.compound.is_empty() {
            compound_lengths
                .entry(stint.compound.clone())
                .or_default()
                .push(tyre_life);
        }
    }

    compound_lengths
        .into_iter()
        .map(|(compound, lengths)| {
            let count = lengths.len();
            let avg = lengths.iter().sum::<i64>() / count as i64;
            (compound, (avg, count))
        })
        .collect()
}

fn default_compound_life(compound: &str) -> i64 {
    match compound.to_uppercase().as_str() {
        "SOFT" => DEFAULT_LIFE_SOFT,
        "MEDIUM" => DEFAULT_LIFE_MEDIUM,
        "HARD" => DEFAULT_LIFE_HARD,
        _ => DEFAULT_LIFE_OTHER,
    }
}

fn compound_sort_key(compound: &str) -> u8 {
    match compound.to_uppercase().as_str() {
        "SOFT" => 0,
        "MEDIUM" => 1,
        "HARD" => 2,
        "INTERMEDIATE" => 3,
        "WET" => 4,
        _ => 5,
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::degradation::{DegradationAnalysis, StintSummary, TyreCliff};

    fn make_stint(
        driver: i64,
        compound: &str,
        tyre_age_end: i64,
        deg_rate: f64,
        avg_pace: f64,
        clean_laps: usize,
    ) -> StintSummary {
        StintSummary {
            driver_number: driver,
            stint_number: 1,
            compound: compound.to_string(),
            lap_start: 1,
            lap_count: tyre_age_end,
            tyre_age_start: 0,
            tyre_age_end,
            avg_pace,
            deg_rate,
            fuel_corrected_deg_rate: None,
            lap_deltas: (0..clean_laps).map(|_| Some(0.0)).collect(),
            is_current: true,
            recent_3lap_avg: avg_pace,
            recent_3lap_delta: 0.0,
            slope_acceleration: 0.0,
            max_lap_delta: 0.0,
        }
    }

    fn empty_baselines() -> CompoundBaselines {
        CompoundBaselines {
            practice: Vec::new(),
            field_benchmarks: Vec::new(),
        }
    }

    #[test]
    fn test_pit_window_basic() {
        let degradation = DegradationAnalysis {
            stints: vec![make_stint(1, "SOFT", 10, 0.1, 90.0, 10)],
            cliffs: Vec::new(),
            compound_benchmarks: Vec::new(),
        };

        let windows = analyze_pit_windows(&degradation, 15, Some(60), &empty_baselines());

        assert_eq!(windows.len(), 1);
        let w = &windows[0];
        assert_eq!(w.driver_number, 1);
        assert_eq!(w.compound, "SOFT");
        // deg_rate=0.1, DELTA_THRESHOLD=2.0 → threshold expiry at age 20
        // but SOFT cap = 18, so expiry_age = 18
        // tyre_age=10, so laps_remaining = 8
        assert_eq!(w.estimated_laps_remaining, 8);
        assert!(matches!(w.confidence, Confidence::High));
    }

    #[test]
    fn test_pit_window_with_cliff() {
        let degradation = DegradationAnalysis {
            stints: vec![make_stint(1, "SOFT", 18, 0.08, 90.0, 15)],
            cliffs: vec![TyreCliff {
                driver_number: 1,
                stint_number: 1,
                compound: "SOFT".to_string(),
                detected_at_lap: 20,
                tyre_age: 18,
                severity: 0.8,
                headline: "test cliff".to_string(),
            }],
            compound_benchmarks: Vec::new(),
        };

        let windows = analyze_pit_windows(&degradation, 20, Some(60), &empty_baselines());

        assert_eq!(windows.len(), 1);
        let w = &windows[0];
        // Active cliff detected at current lap → laps remaining = 0
        assert_eq!(w.estimated_laps_remaining, 0);
        assert!(w.reason.contains("Active cliff"));
    }

    #[test]
    fn test_pit_window_with_practice_baselines() {
        let baselines = CompoundBaselines {
            practice: vec![PracticeBaseline {
                compound: "MEDIUM".to_string(),
                expected_deg_rate: 0.15,
                expected_pace: 91.0,
                sample_stints: 5,
                sample_laps: 40,
            }],
            field_benchmarks: Vec::new(),
        };

        let degradation = DegradationAnalysis {
            stints: vec![make_stint(1, "MEDIUM", 5, 0.08, 91.0, 5)],
            cliffs: Vec::new(),
            compound_benchmarks: Vec::new(),
        };

        let windows = analyze_pit_windows(&degradation, 10, Some(60), &baselines);

        assert_eq!(windows.len(), 1);
        let w = &windows[0];
        // Practice says 0.15 s/lap → life = 2.0/0.15 ≈ 13 laps
        // Driver's own rate says 2.0/0.08 = 25 laps
        // Practice is more conservative, should be used
        assert!(w.reason.contains("Practice baseline"));
        assert_eq!(w.estimated_laps_remaining, 13 - 5); // expiry age 13, current age 5
    }

    #[test]
    fn test_low_confidence_filtered_out() {
        let degradation = DegradationAnalysis {
            stints: vec![make_stint(1, "HARD", 3, 0.0, 91.0, 2)],
            cliffs: Vec::new(),
            compound_benchmarks: Vec::new(),
        };

        let windows = analyze_pit_windows(&degradation, 5, Some(60), &empty_baselines());

        // Low confidence (< 4 clean laps) should be filtered out entirely
        assert!(windows.is_empty());
    }

    #[test]
    fn test_pit_window_sorted_by_urgency() {
        let degradation = DegradationAnalysis {
            stints: vec![
                make_stint(1, "SOFT", 15, 0.12, 90.0, 10), // expires sooner
                make_stint(44, "HARD", 5, 0.04, 91.0, 10), // expires later
            ],
            cliffs: Vec::new(),
            compound_benchmarks: Vec::new(),
        };

        let windows = analyze_pit_windows(&degradation, 20, Some(60), &empty_baselines());

        assert_eq!(windows.len(), 2);
        // Driver 1 (softs, high deg) should be first (fewer laps remaining)
        assert_eq!(windows[0].driver_number, 1);
        assert!(windows[0].estimated_laps_remaining < windows[1].estimated_laps_remaining);
    }

    #[test]
    fn test_practice_baselines_extraction() {
        let stints = vec![
            // Long run on softs
            make_stint(1, "SOFT", 8, 0.12, 89.5, 8),
            // Long run on mediums
            make_stint(2, "MEDIUM", 10, 0.07, 90.0, 10),
            // Short run (should be filtered out)
            make_stint(3, "SOFT", 3, 0.15, 89.0, 3),
            // Another long run on softs
            make_stint(4, "SOFT", 7, 0.10, 89.8, 7),
        ];

        let result = extract_practice_baselines(&stints, vec!["Practice 1".into()]);

        assert_eq!(result.baselines.len(), 2); // SOFT and MEDIUM
        let soft = result
            .baselines
            .iter()
            .find(|b| b.compound == "SOFT")
            .unwrap();
        assert_eq!(soft.sample_stints, 2);
        assert!((soft.expected_deg_rate - 0.11).abs() < 0.01); // avg of 0.12 and 0.10
    }

    #[test]
    fn test_skips_completed_stints() {
        let mut stint = make_stint(1, "SOFT", 10, 0.1, 90.0, 10);
        stint.is_current = false;

        let degradation = DegradationAnalysis {
            stints: vec![stint],
            cliffs: Vec::new(),
            compound_benchmarks: Vec::new(),
        };

        let windows = analyze_pit_windows(&degradation, 15, Some(60), &empty_baselines());
        assert!(windows.is_empty());
    }

    #[test]
    fn test_field_completed_stint_bound() {
        // Three completed medium stints (avg 18 laps) + one current medium stint
        let mut completed1 = make_stint(11, "MEDIUM", 17, 0.05, 91.0, 17);
        completed1.is_current = false;
        completed1.lap_start = 1;
        completed1.tyre_age_start = 0;
        completed1.tyre_age_end = 16;

        let mut completed2 = make_stint(44, "MEDIUM", 19, 0.06, 91.0, 19);
        completed2.is_current = false;
        completed2.lap_start = 1;
        completed2.tyre_age_start = 0;
        completed2.tyre_age_end = 18;

        let mut completed3 = make_stint(77, "MEDIUM", 18, 0.055, 91.0, 18);
        completed3.is_current = false;
        completed3.lap_start = 1;
        completed3.tyre_age_start = 0;
        completed3.tyre_age_end = 17;

        // Current driver on mediums, near-zero deg rate (would default to 26-lap cap)
        let current = make_stint(1, "MEDIUM", 10, 0.001, 91.0, 10);

        let degradation = DegradationAnalysis {
            stints: vec![completed1, completed2, completed3, current],
            cliffs: Vec::new(),
            compound_benchmarks: Vec::new(),
        };

        let windows = analyze_pit_windows(&degradation, 15, Some(53), &empty_baselines());

        assert_eq!(windows.len(), 1);
        let w = &windows[0];
        // Field avg medium stint: (17 + 19 + 18) / 3 = 18 laps
        // Without field evidence, would default to 26 (MEDIUM cap)
        // With field evidence: expiry_age = 18, tyre_age = 10, remaining = 8
        assert!(w.reason.contains("Field avg"));
        assert_eq!(w.estimated_laps_remaining, 18 - 10);
    }

    #[test]
    fn test_early_undercuts_dont_poison_field() {
        // Two early undercuts on mediums (10-11 laps — below 50% of default 26)
        let mut undercut1 = make_stint(11, "MEDIUM", 10, 0.05, 91.0, 10);
        undercut1.is_current = false;
        undercut1.lap_start = 1;
        undercut1.tyre_age_start = 0;
        undercut1.tyre_age_end = 9;

        let mut undercut2 = make_stint(44, "MEDIUM", 11, 0.06, 91.0, 11);
        undercut2.is_current = false;
        undercut2.lap_start = 1;
        undercut2.tyre_age_start = 0;
        undercut2.tyre_age_end = 10;

        // Current driver on mediums with low deg
        let current = make_stint(1, "MEDIUM", 12, 0.03, 91.0, 12);

        let degradation = DegradationAnalysis {
            stints: vec![undercut1, undercut2, current],
            cliffs: Vec::new(),
            compound_benchmarks: Vec::new(),
        };

        let windows = analyze_pit_windows(&degradation, 15, Some(53), &empty_baselines());

        assert_eq!(windows.len(), 1);
        let w = &windows[0];
        // The two undercuts (10-11 laps) should be filtered out because
        // they're < 50% of the default 26-lap medium life.
        // Driver should NOT be told to pit based on undercut evidence.
        assert!(!w.reason.contains("Field avg"));
    }
}
