use serde::Serialize;
use std::cmp::Reverse;
use std::collections::HashMap;
use ts_rs::TS;

// ── Constants ──────────────────────────────────────────────────────

/// EMA smoothing factor for gap values. Lower = smoother.
/// At 500 ms ticks, 0.12 gives a ~4 s time constant.
const GAP_EMA_ALPHA: f64 = 0.12;
/// If raw gap differs from smoothed by more than this, reset the EMA
/// (handles overtakes / position changes where "car ahead" changes).
const GAP_EMA_RESET: f64 = 2.0;
/// EMA smoothing for the interestingness score itself.
/// Catches closing-rate jumps that gap smoothing can't reach.
const SCORE_EMA_ALPHA: f64 = 0.3;
/// A battle must outscore an incumbent by this many points to displace it.
const HYSTERESIS_POINTS: i64 = 8;

/// Minimum laps of history needed to compute a closing rate.
const MIN_HISTORY_LAPS: usize = 3;
/// Maximum laps of history to use for rate computation.
const MAX_HISTORY_LAPS: usize = 6;
/// Ignore battles where projected catch is more than this many laps away.
const MAX_LAPS_TO_CONTACT: f64 = 30.0;
/// Maximum gap (seconds) to even consider as a battle.
const MAX_BATTLE_GAP: f64 = 5.0;
/// DRS detection threshold in seconds.
const DRS_THRESHOLD: f64 = 1.0;

// ── Input types ────────────────────────────────────────────────────

/// Lightweight input for battle computation.
/// Constructed from RaceRow in board.rs — avoids coupling to the full row struct.
pub struct DriverSnapshot {
    pub driver_number: i64,
    pub position: i64,
    /// Parsed interval to car ahead in seconds, or None if unparseable.
    pub interval: Option<f64>,
    pub compound: String,
    pub tyre_age: i64,
    pub stopped: bool,
    pub in_pit: bool,
    pub is_pit_out_lap: bool,
}

// ── Output types ───────────────────────────────────────────────────

#[derive(Serialize, Clone, PartialEq, TS)]
#[ts(export)]
pub struct PressureFactors {
    pub proximity_behind: f64,
    pub convergence_behind: f64,
    pub tyre_threat: f64,
}

#[derive(Serialize, Clone, PartialEq, TS)]
#[ts(export)]
pub struct PressureInfo {
    pub driver_number: i64,
    pub score: i64,
    pub factors: PressureFactors,
}

#[derive(Serialize, Clone, PartialEq, TS)]
#[ts(export)]
pub struct Battle {
    pub attacker: i64,
    pub defender: i64,
    pub gap: f64,
    pub closing_rate: f64,
    pub laps_to_contact: Option<f64>,
    pub interestingness: i64,
    pub reasons: Vec<String>,
    pub defender_pressure: Option<PressureInfo>,
    /// Attacker's interval-to-defender history, oldest → newest. Sourced
    /// from `interval_history` (fetched at 12 laps in `board.rs`); empty
    /// until the attacker has at least one completed-lap interval recorded.
    pub history: Vec<f64>,
}

// ── Stabilisation state ───────────────────────────────────────────

/// Persistent state kept across ticks to smooth battle display.
///
/// Three layers of stabilisation:
/// 1. **Gap EMA** – damps the 500 ms noise on the live interval that feeds
///    proximity / urgency scoring.
/// 2. **Score EMA** – smooths the interestingness score itself, catching
///    closing-rate jumps and other per-lap discontinuities.
/// 3. **Ordering hysteresis** – prevents battles with similar smoothed scores
///    from swapping positions every tick.
pub struct BattleState {
    smoothed_gaps: HashMap<i64, f64>,
    smoothed_scores: HashMap<i64, f64>, // attacker → smoothed interestingness
    prev_ranking: Vec<i64>,             // attacker driver numbers in display order
}

impl Default for BattleState {
    fn default() -> Self {
        Self::new()
    }
}

impl BattleState {
    pub fn new() -> Self {
        Self {
            smoothed_gaps: HashMap::new(),
            smoothed_scores: HashMap::new(),
            prev_ranking: Vec::new(),
        }
    }

    /// Apply EMA smoothing to a driver's interval gap.
    /// Resets on large jumps (e.g., after an overtake changes the car ahead).
    pub fn smooth_gap(&mut self, driver_number: i64, raw_gap: Option<f64>) -> Option<f64> {
        let raw = match raw_gap {
            Some(g) if g > 0.0 => g,
            _ => {
                self.smoothed_gaps.remove(&driver_number);
                return raw_gap;
            }
        };

        let smoothed = match self.smoothed_gaps.get(&driver_number) {
            Some(&prev) if (raw - prev).abs() < GAP_EMA_RESET => {
                GAP_EMA_ALPHA * raw + (1.0 - GAP_EMA_ALPHA) * prev
            }
            _ => raw, // no previous value or large jump → reset
        };

        self.smoothed_gaps.insert(driver_number, smoothed);
        Some(smoothed)
    }

    /// Smooth a battle's interestingness score with EMA, then return the
    /// rounded integer to write back into the `Battle`.
    fn smooth_score(&mut self, attacker: i64, raw_score: i64) -> i64 {
        let raw = raw_score as f64;
        let smoothed = match self.smoothed_scores.get(&attacker) {
            Some(&prev) => SCORE_EMA_ALPHA * raw + (1.0 - SCORE_EMA_ALPHA) * prev,
            None => raw,
        };
        self.smoothed_scores.insert(attacker, smoothed);
        smoothed.round() as i64
    }

    /// Re-rank `raw_battles` with score smoothing, ordering hysteresis, and
    /// truncation to `top_n`.
    ///
    /// * Each battle's interestingness is EMA-smoothed before ordering.
    /// * Battles that were visible last tick keep their position unless a
    ///   challenger outscores them by [`HYSTERESIS_POINTS`].
    /// * New battles must also clear the hysteresis gap to enter the list.
    pub fn stabilize(&mut self, raw_battles: Vec<Battle>, top_n: usize) -> Vec<Battle> {
        // Apply score smoothing to every candidate battle.
        let battles: Vec<Battle> = raw_battles
            .into_iter()
            .map(|mut b| {
                b.interestingness = self.smooth_score(b.attacker, b.interestingness);
                b
            })
            .collect();

        if self.prev_ranking.is_empty() {
            let mut result = battles;
            result.truncate(top_n);
            self.prev_ranking = result.iter().map(|b| b.attacker).collect();
            return result;
        }

        let battle_map: HashMap<i64, &Battle> = battles.iter().map(|b| (b.attacker, b)).collect();

        // Start with previous order, keeping only battles still detected.
        let mut result: Vec<Battle> = self
            .prev_ranking
            .iter()
            .filter_map(|&atk| battle_map.get(&atk).map(|b| (*b).clone()))
            .collect();

        // Insert newcomers where their score warrants (with hysteresis).
        for battle in &battles {
            if self.prev_ranking.contains(&battle.attacker) {
                continue;
            }
            let pos = result.iter().position(|existing| {
                battle.interestingness > existing.interestingness + HYSTERESIS_POINTS
            });
            match pos {
                Some(i) => result.insert(i, battle.clone()),
                None => result.push(battle.clone()),
            }
        }

        // Bubble pass: swap adjacent battles only on significant score gap.
        let mut swapped = true;
        while swapped {
            swapped = false;
            for i in 0..result.len().saturating_sub(1) {
                if result[i + 1].interestingness > result[i].interestingness + HYSTERESIS_POINTS {
                    result.swap(i, i + 1);
                    swapped = true;
                }
            }
        }

        result.truncate(top_n);
        self.prev_ranking = result.iter().map(|b| b.attacker).collect();
        // Prune stale score entries for battles that are no longer detected.
        let active: std::collections::HashSet<i64> = battles.iter().map(|b| b.attacker).collect();
        self.smoothed_scores.retain(|k, _| active.contains(k));
        result
    }
}

// ── Internal types ─────────────────────────────────────────────────

struct ConvergenceInfo {
    attacker: i64,
    defender: i64,
    gap: f64,
    closing_rate: f64,
    laps_to_contact: Option<f64>,
}

// ── Helpers ────────────────────────────────────────────────────────

/// Parse an interval string like "+1.234" or "1.234" to a float.
pub fn parse_interval(s: &str) -> Option<f64> {
    let trimmed = s.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("lap")
        || trimmed.eq_ignore_ascii_case("laps")
    {
        return None;
    }
    let cleaned = trimmed.replace('+', "");
    cleaned.parse::<f64>().ok()
}

use super::{compound_rank, linear_slope};

// ── Convergence ────────────────────────────────────────────────────

/// Compute convergence info for adjacent position pairs.
/// Returns only pairs where the attacker is closing or within DRS range.
fn compute_convergence(
    drivers: &[DriverSnapshot],
    history: &HashMap<i64, Vec<f64>>,
) -> Vec<ConvergenceInfo> {
    let mut sorted: Vec<&DriverSnapshot> =
        drivers.iter().filter(|d| !d.stopped && !d.in_pit).collect();
    sorted.sort_by_key(|d| d.position);

    let mut results = Vec::new();

    for i in 1..sorted.len() {
        let attacker = sorted[i];
        let defender = sorted[i - 1];

        let gap = match attacker.interval {
            Some(g) if g > 0.0 && g <= MAX_BATTLE_GAP => g,
            _ => continue,
        };

        let attacker_history = history.get(&attacker.driver_number);
        let has_enough_history = attacker_history
            .map(|h| h.len() >= MIN_HISTORY_LAPS)
            .unwrap_or(false);

        if !has_enough_history {
            // Not enough history — still report if within DRS range
            if gap < DRS_THRESHOLD {
                results.push(ConvergenceInfo {
                    attacker: attacker.driver_number,
                    defender: defender.driver_number,
                    gap,
                    closing_rate: 0.0,
                    laps_to_contact: None,
                });
            }
            continue;
        }

        let hist = attacker_history.unwrap();
        let start = if hist.len() > MAX_HISTORY_LAPS {
            hist.len() - MAX_HISTORY_LAPS
        } else {
            0
        };
        let recent = &hist[start..];

        let pairs: Vec<(f64, f64)> = recent
            .iter()
            .enumerate()
            .map(|(i, &v)| (i as f64, v))
            .collect();
        let slope = linear_slope(&pairs);
        let closing_rate = -slope; // positive = closing

        if closing_rate <= 0.01 {
            // Not converging — include only if within DRS range
            if gap < DRS_THRESHOLD {
                results.push(ConvergenceInfo {
                    attacker: attacker.driver_number,
                    defender: defender.driver_number,
                    gap,
                    closing_rate: 0.0,
                    laps_to_contact: None,
                });
            }
            continue;
        }

        let laps_to_contact = gap / closing_rate;
        if laps_to_contact > MAX_LAPS_TO_CONTACT {
            continue;
        }

        results.push(ConvergenceInfo {
            attacker: attacker.driver_number,
            defender: defender.driver_number,
            gap,
            closing_rate,
            laps_to_contact: Some(laps_to_contact),
        });
    }

    results
}

// ── Pressure Index ─────────────────────────────────────────────────

/// Compute a pressure score (0–100) for each driver.
fn compute_pressure(
    drivers: &[DriverSnapshot],
    convergence: &[ConvergenceInfo],
) -> Vec<PressureInfo> {
    let mut sorted: Vec<&DriverSnapshot> = drivers.iter().filter(|d| !d.stopped).collect();
    sorted.sort_by_key(|d| d.position);

    // Build a lookup: defender driver_number → most threatening convergence
    let mut threat_map: HashMap<i64, &ConvergenceInfo> = HashMap::new();
    for c in convergence {
        let entry = threat_map.get(&c.defender);
        if entry.is_none() || c.closing_rate > entry.unwrap().closing_rate {
            threat_map.insert(c.defender, c);
        }
    }

    let mut results = Vec::new();

    for driver in &sorted {
        // Find the car directly behind
        let car_behind = sorted.iter().find(|r| r.position == driver.position + 1);
        let car_behind = match car_behind {
            Some(cb) => cb,
            None => {
                results.push(PressureInfo {
                    driver_number: driver.driver_number,
                    score: 0,
                    factors: PressureFactors {
                        proximity_behind: 0.0,
                        convergence_behind: 0.0,
                        tyre_threat: 0.0,
                    },
                });
                continue;
            }
        };

        let gap_behind = car_behind.interval.unwrap_or(-1.0);

        // Factor 1: Proximity (0–40 points)
        let proximity_behind = if gap_behind > 0.0 {
            if gap_behind <= DRS_THRESHOLD {
                40.0 - 10.0 * gap_behind
            } else {
                (30.0 * (1.0 - (gap_behind - DRS_THRESHOLD) / 4.0)).max(0.0)
            }
        } else {
            0.0
        };

        // Factor 2: Convergence rate (0–35 points)
        let convergence_behind = threat_map
            .get(&driver.driver_number)
            .filter(|t| t.closing_rate > 0.0)
            .map(|t| (t.closing_rate / 0.5 * 35.0).min(35.0))
            .unwrap_or(0.0);

        // Factor 3: Tyre threat (0–25 points)
        let attacker_age = car_behind.tyre_age;
        let defender_age = driver.tyre_age;
        let age_delta = defender_age - attacker_age;
        let mut tyre_threat = 0.0;
        if age_delta > 0 {
            tyre_threat += (age_delta as f64).min(15.0);
        }
        if compound_rank(&car_behind.compound) > compound_rank(&driver.compound) {
            tyre_threat += 10.0;
        }
        tyre_threat = tyre_threat.min(25.0);

        let score = (proximity_behind + convergence_behind + tyre_threat)
            .round()
            .min(100.0) as i64;

        results.push(PressureInfo {
            driver_number: driver.driver_number,
            score,
            factors: PressureFactors {
                proximity_behind,
                convergence_behind,
                tyre_threat,
            },
        });
    }

    results.sort_by_key(|r| Reverse(r.score));
    results
}

// ── Race Director (Interestingness) ────────────────────────────────

/// Top-level entry point: compute convergence, pressure, and ranked battles.
/// Returns (battles, all_pressure).
pub fn analyze_battles(
    drivers: &[DriverSnapshot],
    history: &HashMap<i64, Vec<f64>>,
    top_n: usize,
) -> (Vec<Battle>, Vec<PressureInfo>) {
    let convergence = compute_convergence(drivers, history);
    let pressure = compute_pressure(drivers, &convergence);

    // Build pressure lookup for inlining into battles
    let pressure_map: HashMap<i64, &PressureInfo> =
        pressure.iter().map(|p| (p.driver_number, p)).collect();

    // Build driver snapshot lookup for compound comparison
    let driver_map: HashMap<i64, &DriverSnapshot> =
        drivers.iter().map(|d| (d.driver_number, d)).collect();

    let mut battles: Vec<Battle> = Vec::new();

    for conv in &convergence {
        let mut score: f64 = 0.0;
        let mut reasons: Vec<String> = Vec::new();

        // Convergence rate: primary signal (0–35)
        let conv_score = (conv.closing_rate / 0.5 * 35.0).min(35.0);
        score += conv_score;
        if conv.closing_rate >= 0.3 {
            reasons.push("Closing fast".into());
        } else if conv.closing_rate > 0.0 {
            reasons.push("Gap shrinking".into());
        }

        // Proximity: already close = exciting (0–25)
        if conv.gap < DRS_THRESHOLD {
            score += 25.0;
            reasons.push("Overtake available".into());
        } else if conv.gap < 2.0 {
            score += 25.0 * (1.0 - (conv.gap - DRS_THRESHOLD) / (2.0 - DRS_THRESHOLD));
        }

        // Position significance (0–20)
        let def_pos = driver_map
            .get(&conv.defender)
            .map(|d| d.position)
            .unwrap_or(99);
        if def_pos == 1 {
            score += 20.0;
            reasons.push("Fight for the lead".into());
        } else if def_pos <= 3 {
            score += 15.0;
            reasons.push("Podium battle".into());
        } else if def_pos <= 10 {
            score += 5.0;
            reasons.push("Points battle".into());
        }

        // Tyre strategy divergence (0–10)
        let atk_compound = driver_map.get(&conv.attacker).map(|d| d.compound.as_str());
        let def_compound = driver_map.get(&conv.defender).map(|d| d.compound.as_str());
        if atk_compound != def_compound {
            score += 10.0;
            reasons.push("Different strategies".into());
        }

        // Time urgency (0–10)
        if let Some(ltc) = conv.laps_to_contact
            && ltc < 5.0
        {
            score += 10.0 * (1.0 - ltc / 5.0);
            if ltc < 2.0 {
                reasons.push("Contact imminent".into());
            }
        }

        // Tyre age advantage reason
        let atk_age = driver_map
            .get(&conv.attacker)
            .map(|d| d.tyre_age)
            .unwrap_or(0);
        let def_age = driver_map
            .get(&conv.defender)
            .map(|d| d.tyre_age)
            .unwrap_or(0);
        if def_age - atk_age >= 8 {
            reasons.push("Tyre advantage".into());
        }

        let interestingness = score.round().min(100.0) as i64;
        let defender_pressure = pressure_map.get(&conv.defender).cloned().cloned();

        let history = history.get(&conv.attacker).cloned().unwrap_or_default();

        battles.push(Battle {
            attacker: conv.attacker,
            defender: conv.defender,
            gap: conv.gap,
            closing_rate: conv.closing_rate,
            laps_to_contact: conv.laps_to_contact,
            interestingness,
            reasons,
            defender_pressure,
            history,
        });
    }

    battles.sort_by_key(|b| Reverse(b.interestingness));
    battles.truncate(top_n);

    (battles, pressure)
}
