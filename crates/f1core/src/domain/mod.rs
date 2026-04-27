pub mod alert;
pub mod battle;
pub mod degradation;
pub mod ml;
pub mod ml_features;
pub mod pm_score;
pub mod position;
pub mod rules;
pub mod sector;
pub mod strategy;
pub mod track;

// ── Shared helpers ────────────────────────────────────────────────

/// Simple linear regression slope over (x, y) pairs.
pub fn linear_slope(pairs: &[(f64, f64)]) -> f64 {
    let n = pairs.len() as f64;
    if n < 2.0 {
        return 0.0;
    }
    let mut sx: f64 = 0.0;
    let mut sy: f64 = 0.0;
    let mut sxy: f64 = 0.0;
    let mut sxx: f64 = 0.0;
    for &(x, y) in pairs {
        sx += x;
        sy += y;
        sxy += x * y;
        sxx += x * x;
    }
    let denom = n * sxx - sx * sx;
    if denom.abs() < f64::EPSILON {
        return 0.0;
    }
    (n * sxy - sx * sy) / denom
}

/// Tyre compound display label (title case, e.g. "Softs", "Mediums").
pub fn compound_label(compound: &str) -> &str {
    match compound.to_uppercase().as_str() {
        "SOFT" => "Softs",
        "MEDIUM" => "Mediums",
        "HARD" => "Hards",
        "INTERMEDIATE" => "Inters",
        "WET" => "Wets",
        _ => compound,
    }
}

/// Tyre compound hardness rank (higher = harder = longer lasting).
pub fn compound_rank(compound: &str) -> i32 {
    match compound.to_uppercase().as_str() {
        "SOFT" => 1,
        "MEDIUM" => 2,
        "HARD" => 3,
        "INTERMEDIATE" => 2,
        "WET" => 2,
        _ => 0,
    }
}
