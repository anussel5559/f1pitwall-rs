//! Pitwall Manager scoring.
//!
//! Pure function so QA can re-tune the constants without touching the WS layer.
//! Server is the only authority for points — clients render the result, never
//! recompute it. Numbers come from HANDOFF §2; tuned in QA.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Inputs needed to score a single resolved call.
#[derive(Debug, Clone, Copy)]
pub struct ScoreInputs {
    /// Lap the player called for.
    pub target_lap: i64,
    /// Lap the real team actually pitted on (or chequered flag if they didn't).
    pub real_lap: i64,
    /// Position change from start of the call window to one lap after the
    /// real pit (positive = gained places vs. ghost).
    pub position_delta: i32,
    /// Lap on which the player locked the call.
    pub locked_at_lap: i64,
    /// Live mode only: seconds between locking and the real pit-entry.
    /// `None` for replay (no anti-cheat penalty applies).
    pub seconds_before_pit: Option<f64>,
}

/// Tunable scoring constants. Lifted out of the function body so a future
/// `tweaks-panel` (mocks reference one) can swap them in at runtime.
#[derive(Debug, Clone, Copy)]
pub struct ScoreWeights {
    pub base: i32,
    pub lap_accuracy_max: i32,
    pub position_per_place: i32,
    pub late_penalty: i32,
    pub late_penalty_lap_threshold: i64,
    pub anti_cheat_penalty: i32,
    pub anti_cheat_seconds_threshold: f64,
    /// Points per lap of lead-time beyond `late_penalty_lap_threshold`.
    /// Rewards bold-but-correct calls per HANDOFF §8 Q1.
    pub early_conviction_per_lap: i32,
    /// Cap on `early_conviction_bonus` so the term stays a kicker, not a
    /// dominant signal — keeps lap accuracy + position the headline factors.
    pub early_conviction_max: i32,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self {
            base: 100,
            lap_accuracy_max: 50,
            position_per_place: 30,
            late_penalty: 20,
            late_penalty_lap_threshold: 2,
            anti_cheat_penalty: 100, // effectively zeroes the call
            anti_cheat_seconds_threshold: 30.0,
            early_conviction_per_lap: 1,
            early_conviction_max: 20,
        }
    }
}

/// Per-component score breakdown. Exported on the wire so clients can show
/// the user *why* they got the score they got, instead of just a magic
/// number. Penalty fields are non-negative — they're subtracted in `total()`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, TS)]
#[ts(export)]
pub struct ScoreBreakdown {
    pub base: i32,
    pub lap_accuracy: i32,
    pub position: i32,
    pub early_conviction: i32,
    pub late_penalty: i32,
    pub anti_cheat_penalty: i32,
}

impl ScoreBreakdown {
    pub fn total(&self) -> i32 {
        self.base + self.lap_accuracy + self.position + self.early_conviction
            - self.late_penalty
            - self.anti_cheat_penalty
    }
}

/// Compute the per-component breakdown. `score()` is a thin wrapper that
/// totals the result.
///
/// Formula:
/// ```text
/// base
///   + lap_accuracy_bonus       // (max - |lap_delta|) clamped to [0, max]
///   + position_delta_bonus     // ±per_place * position_delta
///   + early_conviction_bonus   // per_lap * max(0, lead - threshold), capped at max
///   − late_penalty             // if locked < threshold laps before pit
///   − anti_cheat_penalty       // live only, if locked < N s before pit entry
/// ```
pub fn score_breakdown(inputs: ScoreInputs, w: ScoreWeights) -> ScoreBreakdown {
    let lap_delta = (inputs.target_lap - inputs.real_lap).unsigned_abs() as i32;
    let lap_accuracy = (w.lap_accuracy_max - lap_delta).max(0);

    let position = w.position_per_place * inputs.position_delta;

    let lap_gap_to_pit = (inputs.real_lap - inputs.locked_at_lap).max(0);
    let late_penalty = if lap_gap_to_pit < w.late_penalty_lap_threshold {
        w.late_penalty
    } else {
        0
    };

    // Reward locking early. Anything before the late-penalty threshold earns
    // per_lap points up to the cap. Late or threshold-grazing locks get 0
    // here (and the late branch above handles the penalty).
    let early_lead = (lap_gap_to_pit - w.late_penalty_lap_threshold).max(0) as i32;
    let early_conviction = (early_lead * w.early_conviction_per_lap).min(w.early_conviction_max);

    let anti_cheat_penalty = match inputs.seconds_before_pit {
        Some(s) if s < w.anti_cheat_seconds_threshold => w.anti_cheat_penalty,
        _ => 0,
    };

    ScoreBreakdown {
        base: w.base,
        lap_accuracy,
        position,
        early_conviction,
        late_penalty,
        anti_cheat_penalty,
    }
}

/// Total points awarded for a resolved call.
pub fn score(inputs: ScoreInputs, w: ScoreWeights) -> i32 {
    score_breakdown(inputs, w).total()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perfect() -> ScoreInputs {
        // locked_at_lap = real_lap - threshold (2) so the early-conviction
        // bonus is exactly 0 — keeps the existing baseline assertions
        // valid while letting a dedicated test exercise the new term.
        ScoreInputs {
            target_lap: 25,
            real_lap: 25,
            position_delta: 0,
            locked_at_lap: 23,
            seconds_before_pit: Some(120.0),
        }
    }

    #[test]
    fn perfect_call_scores_base_plus_max_accuracy() {
        let pts = score(perfect(), ScoreWeights::default());
        assert_eq!(pts, 100 + 50);
    }

    #[test]
    fn lap_accuracy_decays_linearly_and_clamps_at_zero() {
        let mut inp = perfect();
        inp.target_lap = 30; // 5 laps off
        assert_eq!(score(inp, ScoreWeights::default()), 100 + 45);
        inp.target_lap = 100; // way off — accuracy clamps to 0
        assert_eq!(score(inp, ScoreWeights::default()), 100);
    }

    #[test]
    fn gaining_a_place_adds_position_bonus() {
        let mut inp = perfect();
        inp.position_delta = 1;
        assert_eq!(score(inp, ScoreWeights::default()), 100 + 50 + 30);
    }

    #[test]
    fn losing_places_subtracts() {
        let mut inp = perfect();
        inp.position_delta = -2;
        assert_eq!(score(inp, ScoreWeights::default()), 100 + 50 - 60);
    }

    #[test]
    fn late_lock_within_threshold_takes_penalty() {
        let mut inp = perfect();
        inp.locked_at_lap = 24; // 1 lap before pit, threshold is 2
        assert_eq!(score(inp, ScoreWeights::default()), 100 + 50 - 20);
    }

    #[test]
    fn anti_cheat_zeroes_calls_locked_under_30s_before_pit() {
        let mut inp = perfect();
        inp.locked_at_lap = 24; // also late
        inp.seconds_before_pit = Some(10.0);
        // base 100 + accuracy 50 - late 20 - anti_cheat 100 = 30
        assert_eq!(score(inp, ScoreWeights::default()), 30);
    }

    #[test]
    fn early_conviction_bonus_grows_with_lead_time_then_caps() {
        let mut inp = perfect();
        // 5 laps lead — 3 beyond threshold — bonus = 3 * 1 = 3.
        inp.locked_at_lap = 20;
        assert_eq!(score(inp, ScoreWeights::default()), 100 + 50 + 3);
        // 25 laps lead — 23 beyond — would be 23 but capped at 20.
        inp.locked_at_lap = 0;
        assert_eq!(score(inp, ScoreWeights::default()), 100 + 50 + 20);
    }

    #[test]
    fn early_conviction_does_not_cancel_the_late_penalty() {
        // A late lock (lead < threshold) earns 0 conviction bonus AND eats
        // the late penalty — they don't net out.
        let mut inp = perfect();
        inp.locked_at_lap = 24; // lead = 1 < threshold = 2
        assert_eq!(score(inp, ScoreWeights::default()), 100 + 50 - 20);
    }

    #[test]
    fn replay_skips_anti_cheat() {
        let mut inp = perfect();
        inp.seconds_before_pit = None;
        inp.locked_at_lap = 24;
        // No anti-cheat applied even with late penalty.
        assert_eq!(score(inp, ScoreWeights::default()), 100 + 50 - 20);
    }
}
