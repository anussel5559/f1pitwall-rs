//! Build ML feature vectors from live race data.
//!
//! Constructs the 24-element feature vector that matches the Python training
//! pipeline's `FEATURE_COLS` ordering. Features come from degradation analysis,
//! display rows, weather, and DB lookups.

use std::collections::HashMap;

use super::degradation::DegradationAnalysis;
use super::ml::MlFeatures;
use crate::db::{CompoundAllocation, WeatherInfo};

/// Circuit name aliases matching the Python training pipeline.
/// Maps OpenF1 `circuit_short_name` to `compound_allocations.circuit`.
fn circuit_alias(name: &str) -> &str {
    match name {
        "Melbourne" => "Albert Park",
        "Monte Carlo" => "Monaco",
        "Sakhir" => "Bahrain",
        "Singapore" => "Marina Bay",
        "Yas Marina Circuit" => "Yas Island",
        "Madring" => "Madrid",
        _ => name,
    }
}

/// Resolve the aliased circuit name for compound allocation lookup.
pub fn resolve_circuit(circuit_short_name: &str) -> &str {
    circuit_alias(circuit_short_name)
}

fn compound_encode(compound: &str) -> f32 {
    match compound.to_uppercase().as_str() {
        "SOFT" => 1.0,
        "MEDIUM" => 2.0,
        "HARD" => 3.0,
        "INTERMEDIATE" => 4.0,
        "WET" => 5.0,
        _ => 0.0,
    }
}

fn physical_compound_ordinal(alloc: &Option<CompoundAllocation>, compound: &str) -> f32 {
    let alloc = match alloc {
        Some(a) => a,
        None => return 0.0,
    };
    let phys = match compound.to_uppercase().as_str() {
        "HARD" => &alloc.hard,
        "MEDIUM" => &alloc.medium,
        "SOFT" => &alloc.soft,
        _ => return 0.0,
    };
    match phys.as_str() {
        "C1" => 1.0,
        "C2" => 2.0,
        "C3" => 3.0,
        "C4" => 4.0,
        "C5" => 5.0,
        _ => 0.0,
    }
}

fn default_compound_life(compound: &str) -> i64 {
    match compound.to_uppercase().as_str() {
        "SOFT" => 18,
        "MEDIUM" => 26,
        "HARD" => 35,
        _ => 35,
    }
}

/// Information about a driver's display state, extracted from board rows.
pub struct DriverDisplayInfo {
    pub driver_number: i64,
    pub position: i64,
    pub gap_to_leader: f64,
    pub grid_position: i64,
}

/// Build ML feature vectors for all drivers on current stints.
///
/// Only produces features for drivers with sufficient clean data (Medium/High confidence).
/// Skips wet/intermediate compounds (model wasn't trained on them).
pub fn build_ml_features(
    degradation: &DegradationAnalysis,
    drivers: &[DriverDisplayInfo],
    weather: &Option<WeatherInfo>,
    alloc: &Option<CompoundAllocation>,
    total_laps: Option<i64>,
    current_lap: i64,
    sc_count_before: i64,
) -> Vec<MlFeatures> {
    let driver_map: HashMap<i64, &DriverDisplayInfo> =
        drivers.iter().map(|d| (d.driver_number, d)).collect();

    // Compute field evidence: completed stint lengths per compound (same as strategy.rs logic)
    let race_end_cutoff = total_laps.map(|tl| tl - 2).unwrap_or(i64::MAX);
    let mut field_stints: HashMap<String, Vec<i64>> = HashMap::new();
    for stint in &degradation.stints {
        if stint.is_current {
            continue;
        }
        let stint_end_lap = stint.lap_start + stint.lap_count - 1;
        if stint_end_lap >= race_end_cutoff {
            continue;
        }
        let tyre_life = stint.tyre_age_end - stint.tyre_age_start + 1;
        if tyre_life < 5 {
            continue;
        }
        let default_life = default_compound_life(&stint.compound);
        if tyre_life < default_life / 2 {
            continue;
        }
        field_stints
            .entry(stint.compound.clone())
            .or_default()
            .push(tyre_life);
    }

    // Compute field average pace per compound (from completed stints)
    let mut compound_paces: HashMap<String, Vec<f64>> = HashMap::new();
    for stint in &degradation.stints {
        if stint.is_current || stint.avg_pace <= 0.0 {
            continue;
        }
        compound_paces
            .entry(stint.compound.clone())
            .or_default()
            .push(stint.avg_pace);
    }

    let mut features = Vec::new();

    for stint in &degradation.stints {
        if !stint.is_current {
            continue;
        }

        // Skip wet compounds (model not trained on them)
        let compound_upper = stint.compound.to_uppercase();
        if compound_upper == "INTERMEDIATE" || compound_upper == "WET" || stint.compound.is_empty()
        {
            continue;
        }

        let clean_count = stint.lap_deltas.iter().filter(|d| d.is_some()).count();
        if clean_count < 4 {
            continue;
        }

        let display = driver_map.get(&stint.driver_number);

        let total_race_laps = total_laps.unwrap_or(60) as f32;
        let grid_pos = display.map(|d| d.grid_position).unwrap_or(20) as f32;
        let position = display.map(|d| d.position).unwrap_or(10) as f32;
        let gap = display.map(|d| d.gap_to_leader).unwrap_or(0.0) as f32;

        let (air_temp, track_temp, humidity, is_rain) = match weather {
            Some(w) => (
                w.air_temp.unwrap_or(25.0) as f32,
                w.track_temp.unwrap_or(40.0) as f32,
                w.humidity.unwrap_or(50.0) as f32,
                if w.rainfall { 1.0f32 } else { 0.0 },
            ),
            None => (25.0, 40.0, 50.0, 0.0),
        };

        let fc_deg = stint.fuel_corrected_deg_rate.unwrap_or(stint.deg_rate) as f32;
        let raw_deg = stint.deg_rate as f32;

        // Field evidence for this compound
        let field_lengths = field_stints.get(&stint.compound);
        let field_avg_life = field_lengths
            .map(|v| v.iter().sum::<i64>() as f64 / v.len() as f64)
            .unwrap_or(0.0) as f32;
        let field_count = field_lengths.map(|v| v.len()).unwrap_or(0) as f32;

        // Pace delta to field
        let field_pace = compound_paces
            .get(&stint.compound)
            .filter(|v| !v.is_empty())
            .map(|v| v.iter().sum::<f64>() / v.len() as f64);
        let pace_delta = field_pace
            .map(|fp| (stint.avg_pace - fp) as f32)
            .unwrap_or(0.0);

        // Feature vector — MUST match Python FEATURE_COLS order exactly
        let values: [f32; 24] = [
            compound_encode(&stint.compound), // 0: compound_encoded
            physical_compound_ordinal(alloc, &stint.compound), // 1: physical_compound
            stint.tyre_age_start as f32,      // 2: tyre_age_at_start
            stint.stint_number as f32,        // 3: stint_number_feat
            total_race_laps,                  // 4: total_race_laps
            grid_pos,                         // 5: grid_position
            track_temp,                       // 6: track_temp
            air_temp,                         // 7: air_temp
            humidity,                         // 8: humidity
            is_rain,                          // 9: is_rain
            sc_count_before as f32,           // 10: sc_count_before
            stint.tyre_age_end as f32,        // 11: current_tyre_age
            fc_deg,                           // 12: fuel_corrected_deg_rate
            raw_deg,                          // 13: raw_deg_rate
            stint.avg_pace as f32,            // 14: avg_pace
            pace_delta,                       // 15: pace_delta_to_field
            stint.recent_3lap_avg as f32,     // 16: recent_3lap_avg
            stint.recent_3lap_delta as f32,   // 17: recent_3lap_delta
            stint.slope_acceleration as f32,  // 18: slope_acceleration
            stint.max_lap_delta as f32,       // 19: max_lap_delta
            field_avg_life,                   // 20: field_avg_stint_length
            field_count,                      // 21: field_evidence_count
            position,                         // 22: position
            gap,                              // 23: gap_to_leader
        ];

        features.push(MlFeatures {
            driver_number: stint.driver_number,
            compound: stint.compound.clone(),
            tyre_age: stint.tyre_age_end,
            current_lap,
            values,
        });
    }

    features
}
