use serde::Serialize;
use std::collections::{HashMap, HashSet};
use ts_rs::TS;

use super::battle::Battle;
use super::degradation::TyreCliff;
use super::strategy::PitWindow;
use crate::db::RaceControlMsg;

// ── Constants ──────────────────────────────────────────────────────

/// Minimum laps between alerts for the same event key.
const COOLDOWN_LAPS: i64 = 5;
/// laps_to_contact threshold for "watch this" alerts.
const CONTACT_IMMINENT_LAPS: f64 = 3.0;
/// Position drop in a single tick that indicates a pit stop, not on-track action.
const PIT_MOVER_THRESHOLD: i64 = 3;
/// Suppress all alerts before this lap (lap 1 chaos is too noisy).
const MIN_ALERT_LAP: i64 = 3;

// ── Types ──────────────────────────────────────────────────────────

#[derive(Serialize, Clone, Debug, PartialEq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AlertType {
    Overtake,
    ContactImminent,
    SafetyCar,
    RainOnset,
    TyreCliff,
    PitWindowClosing,
}

#[derive(Serialize, Clone, Debug, PartialEq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AlertPriority {
    Hot,
    Warm,
}

#[derive(Serialize, Clone, Debug, PartialEq, TS)]
#[ts(export)]
pub struct Alert {
    pub alert_type: AlertType,
    pub priority: AlertPriority,
    pub headline: String,
    pub detail: String,
    pub driver_numbers: Vec<i64>,
    pub lap: i64,
    pub interestingness: i64,
}

// ── Inputs ─────────────────────────────────────────────────────────

/// Extracted from a `BoardSnapshot` by the caller so alert detection
/// doesn't depend on the pitwall board types.
pub struct AlertInputs {
    pub positions: HashMap<i64, i64>,
    pub acronyms: HashMap<i64, String>,
    pub is_pit_out: HashSet<i64>,
    pub in_pit: HashSet<i64>,
    pub is_in_lap: HashSet<i64>,
    pub tyres: HashMap<i64, (String, i64)>, // driver_number → (compound, age)
    pub best_lap: Option<(i64, f64)>,
    pub rainfall: bool,
    pub tyre_cliffs: Vec<TyreCliff>,
    pub pit_windows: Vec<PitWindow>,
}

// ── Helpers ───────────────────────────────────────────────────────

/// Whether the race is currently neutralized (SC, VSC, red flag).
/// Messages must be ordered most-recent-first (the DB query uses `ORDER BY date DESC`).
fn is_neutralized(race_control: &[RaceControlMsg]) -> bool {
    for msg in race_control {
        let m = msg.message.to_uppercase();
        if m.contains("GREEN LIGHT") || m.contains("TRACK CLEAR") || m.contains("OVERTAKE ENABLED")
        {
            return false;
        }
        if m.contains("RED FLAG") || m.contains("SAFETY CAR") || m.contains("VSC") {
            return true;
        }
    }
    false
}

// ── Tracker ────────────────────────────────────────────────────────

pub struct AlertTracker {
    prev_positions: HashMap<i64, i64>,
    prev_battle_pairs: HashSet<(i64, i64)>,
    prev_contact_imminent: HashSet<(i64, i64)>,
    prev_cliff_keys: HashSet<String>,
    prev_rc_count: usize,
    prev_rainfall: bool,
    cooldowns: HashMap<String, i64>,
    initialized: bool,
}

impl Default for AlertTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl AlertTracker {
    pub fn new() -> Self {
        Self {
            prev_positions: HashMap::new(),
            prev_battle_pairs: HashSet::new(),
            prev_contact_imminent: HashSet::new(),
            prev_cliff_keys: HashSet::new(),
            prev_rc_count: 0,
            prev_rainfall: false,
            cooldowns: HashMap::new(),
            initialized: false,
        }
    }

    /// Detect rising-edge events by comparing current state against previous tick.
    /// Returns alerts for this tick only (ephemeral).
    ///
    /// Only surfaces high-conviction "watch this" moments:
    /// - Overtakes in the top 10 that culminate a tracked battle (not pit-induced)
    /// - Contact imminent (about to overtake) — at most one per tick
    /// - Safety car / VSC / red flag deployments
    /// - Rain onset
    pub fn detect(
        &mut self,
        inputs: &AlertInputs,
        battles: &[Battle],
        race_control: &[RaceControlMsg],
        current_lap: i64,
    ) -> Vec<Alert> {
        if !self.initialized {
            self.store(inputs, battles, race_control);
            self.initialized = true;
            return Vec::new();
        }

        // Suppress position-based alerts during opening lap chaos or neutralization.
        // Safety car and rain still fire — those are always important.
        let suppress_racing = current_lap < MIN_ALERT_LAP || is_neutralized(race_control);

        // Identify "pit movers" — drivers who lost 3+ positions in one tick.
        // Position changes for these drivers are pit-induced, not on-track action.
        let pit_movers = self.find_pit_movers(inputs);

        let mut alerts = Vec::new();

        if !suppress_racing {
            self.detect_overtakes(inputs, battles, current_lap, &pit_movers, &mut alerts);
            self.detect_contact_imminent(inputs, battles, current_lap, &pit_movers, &mut alerts);
            self.detect_tyre_cliff(inputs, current_lap, &mut alerts);
            self.detect_pit_window_closing(inputs, current_lap, &mut alerts);
        }
        self.detect_safety_car(race_control, current_lap, &mut alerts);
        self.detect_rain(inputs, current_lap, &mut alerts);

        self.store(inputs, battles, race_control);
        alerts
    }

    /// Drivers who lost 3+ positions in one tick — likely pitting or major incident.
    fn find_pit_movers(&self, inputs: &AlertInputs) -> HashSet<i64> {
        let mut movers = HashSet::new();
        for (&dn, &pos_now) in &inputs.positions {
            if let Some(&pos_prev) = self.prev_positions.get(&dn)
                && pos_now - pos_prev >= PIT_MOVER_THRESHOLD
            {
                movers.insert(dn);
            }
        }
        movers
    }

    fn store(&mut self, inputs: &AlertInputs, battles: &[Battle], race_control: &[RaceControlMsg]) {
        self.prev_positions = inputs.positions.clone();
        self.prev_battle_pairs = battles.iter().map(|b| (b.attacker, b.defender)).collect();
        self.prev_contact_imminent = battles
            .iter()
            .filter(|b| matches!(b.laps_to_contact, Some(l) if l < CONTACT_IMMINENT_LAPS))
            .map(|b| (b.attacker, b.defender))
            .collect();
        self.prev_cliff_keys = inputs
            .tyre_cliffs
            .iter()
            .map(|c| format!("cliff-{}-{}", c.driver_number, c.stint_number))
            .collect();
        self.prev_rc_count = race_control.len();
        self.prev_rainfall = inputs.rainfall;
    }

    fn on_cooldown(&self, key: &str, current_lap: i64) -> bool {
        self.cooldowns
            .get(key)
            .map(|&last| current_lap - last < COOLDOWN_LAPS)
            .unwrap_or(false)
    }

    fn record_cooldown(&mut self, key: String, current_lap: i64) {
        self.cooldowns.insert(key, current_lap);
    }

    fn acr(&self, inputs: &AlertInputs, dn: i64) -> String {
        inputs
            .acronyms
            .get(&dn)
            .cloned()
            .unwrap_or_else(|| format!("#{}", dn))
    }

    /// Build a tyre context string comparing two drivers, e.g.
    /// "Mediums (8 laps) vs Hards (22 laps)"
    fn tyre_context(inputs: &AlertInputs, dn_a: i64, dn_b: i64) -> Option<String> {
        let (comp_a, age_a) = inputs.tyres.get(&dn_a)?;
        let (comp_b, age_b) = inputs.tyres.get(&dn_b)?;
        if comp_a == comp_b && (age_a - age_b).abs() < 5 {
            return None; // Same compound, similar age — not interesting
        }
        Some(format!(
            "{} ({} laps) vs {} ({} laps)",
            super::compound_label(comp_a),
            age_a,
            super::compound_label(comp_b),
            age_b,
        ))
    }

    fn position_label(pos: i64) -> &'static str {
        match pos {
            1 => "the lead",
            2 => "P2",
            3 => "the final podium spot",
            p if p <= 10 => "a points position",
            _ => "",
        }
    }

    /// Returns true if a contact-imminent alert for this battle should be suppressed.
    fn should_suppress_contact(
        &self,
        inputs: &AlertInputs,
        b: &Battle,
        current_lap: i64,
        pit_movers: &HashSet<i64>,
    ) -> bool {
        // Already alerted for this pair last tick
        if self
            .prev_contact_imminent
            .contains(&(b.attacker, b.defender))
        {
            return true;
        }
        // Defender is pitting — dropping, not defending
        if pit_movers.contains(&b.defender)
            || inputs.in_pit.contains(&b.defender)
            || inputs.is_in_lap.contains(&b.defender)
            || inputs.is_pit_out.contains(&b.defender)
        {
            return true;
        }
        // Attacker just lost positions — got passed, not attacking
        if let Some(&prev_pos) = self.prev_positions.get(&b.attacker) {
            let cur_pos = inputs.positions.get(&b.attacker).copied().unwrap_or(0);
            if cur_pos > prev_pos {
                return true;
            }
        }
        // Cooldown: this pair or the reverse (same battle, either direction)
        let key = format!("contact-{}-{}", b.attacker, b.defender);
        let reverse_key = format!("contact-{}-{}", b.defender, b.attacker);
        if self.on_cooldown(&key, current_lap) || self.on_cooldown(&reverse_key, current_lap) {
            return true;
        }
        // Recent overtake between this pair
        let ot_key = format!(
            "overtake-{}-{}",
            b.attacker.min(b.defender),
            b.attacker.max(b.defender),
        );
        if self.on_cooldown(&ot_key, current_lap) {
            return true;
        }
        false
    }

    // ── Detection methods ──────────────────────────────────────────

    fn detect_overtakes(
        &mut self,
        inputs: &AlertInputs,
        battles: &[Battle],
        current_lap: i64,
        pit_movers: &HashSet<i64>,
        alerts: &mut Vec<Alert>,
    ) {
        let battle_map: HashMap<(i64, i64), &Battle> = battles
            .iter()
            .map(|b| ((b.attacker, b.defender), b))
            .collect();

        // Collect all position swaps, grouped by the overtaker.
        let mut gains_by_driver: HashMap<i64, Vec<(i64, i64)>> = HashMap::new(); // overtaker -> [(overtaken, new_pos)]

        for (&dn_a, &pos_a_now) in &inputs.positions {
            let Some(&pos_a_prev) = self.prev_positions.get(&dn_a) else {
                continue;
            };
            if pos_a_now >= pos_a_prev {
                continue;
            }
            // Skip if the "overtaker" is just a pit mover gaining back positions
            // or the "overtaken" lost positions to a pit stop.
            if pit_movers.contains(&dn_a) {
                continue;
            }
            for (&dn_b, &pos_b_prev) in &self.prev_positions {
                if dn_b == dn_a {
                    continue;
                }
                let Some(&pos_b_now) = inputs.positions.get(&dn_b) else {
                    continue;
                };
                if pos_b_prev == pos_a_now && pos_b_now == pos_a_prev {
                    // The overtaken driver is pitting — not an on-track pass.
                    if pit_movers.contains(&dn_b)
                        || inputs.in_pit.contains(&dn_b)
                        || inputs.is_in_lap.contains(&dn_b)
                        || inputs.is_pit_out.contains(&dn_b)
                    {
                        continue;
                    }
                    gains_by_driver
                        .entry(dn_a)
                        .or_default()
                        .push((dn_b, pos_a_now));
                }
            }
        }

        // Emit one consolidated alert per overtaker.
        for (overtaker_dn, mut gains) in gains_by_driver {
            // Sort by position (best position first)
            gains.sort_by_key(|&(_, pos)| pos);
            let best_pos = gains[0].1;

            let key = if gains.len() == 1 {
                format!(
                    "overtake-{}-{}",
                    overtaker_dn.min(gains[0].0),
                    overtaker_dn.max(gains[0].0)
                )
            } else {
                format!("overtake-multi-{}", overtaker_dn)
            };

            if self.on_cooldown(&key, current_lap) {
                continue;
            }

            // P1–P3: always visible. P4–P10: only if tracked battle.
            // P11+ or untracked P4–P10: emit as silent (for resolving contact_imminent).
            let dominated_battle = gains.iter().any(|&(dn_b, _)| {
                self.prev_battle_pairs.contains(&(overtaker_dn, dn_b))
                    || self.prev_battle_pairs.contains(&(dn_b, overtaker_dn))
                    || battle_map.contains_key(&(overtaker_dn, dn_b))
                    || battle_map.contains_key(&(dn_b, overtaker_dn))
            });

            let (priority, interestingness) = if best_pos == 1 {
                (AlertPriority::Hot, 95)
            } else if best_pos <= 3 {
                (AlertPriority::Hot, 80)
            } else if best_pos <= 10 && dominated_battle {
                (AlertPriority::Warm, 60)
            } else {
                // Outside top 10 or no tracked battle — low interestingness
                // so the frontend sensitivity filter handles visibility.
                (AlertPriority::Warm, 30)
            };

            let overtaker = self.acr(inputs, overtaker_dn);

            let (headline, detail) = if gains.len() == 1 {
                let overtaken = self.acr(inputs, gains[0].0);
                let pos_label = Self::position_label(best_pos);
                let hl = if best_pos == 1 {
                    format!("{} takes the lead from {}", overtaker, overtaken)
                } else {
                    format!("{} overtakes {} for {}", overtaker, overtaken, pos_label)
                };
                let mut det_parts: Vec<String> = Vec::new();
                if let Some(b) = battle_map
                    .get(&(overtaker_dn, gains[0].0))
                    .or_else(|| battle_map.get(&(gains[0].0, overtaker_dn)))
                {
                    let reasons: Vec<&str> = b
                        .reasons
                        .iter()
                        .filter(|r| r.as_str() != "Gap closing" && r.as_str() != "Contact imminent")
                        .map(|r| r.as_str())
                        .collect();
                    if !reasons.is_empty() {
                        det_parts.push(reasons.join(", "));
                    }
                }
                if let Some(tyre) = Self::tyre_context(inputs, overtaker_dn, gains[0].0) {
                    det_parts.push(tyre);
                }
                (hl, det_parts.join(" — "))
            } else {
                let names: Vec<String> =
                    gains.iter().map(|&(dn, _)| self.acr(inputs, dn)).collect();
                let pos_label = Self::position_label(best_pos);
                let hl = format!(
                    "{} gains {} positions to {}",
                    overtaker,
                    gains.len(),
                    pos_label,
                );
                let det = format!("Passes {}", names.join(", "));
                (hl, det)
            };

            let mut driver_numbers = vec![overtaker_dn];
            driver_numbers.extend(gains.iter().map(|&(dn, _)| dn));

            alerts.push(Alert {
                alert_type: AlertType::Overtake,
                priority,
                headline,
                detail,
                driver_numbers,
                lap: current_lap,
                interestingness,
            });

            self.record_cooldown(key, current_lap);
            // Suppress contact-imminent for all involved pairs post-overtake
            for &(dn_b, _) in &gains {
                self.record_cooldown(format!("contact-{}-{}", overtaker_dn, dn_b), current_lap);
                self.record_cooldown(format!("contact-{}-{}", dn_b, overtaker_dn), current_lap);
            }
        }
    }

    fn detect_contact_imminent(
        &mut self,
        inputs: &AlertInputs,
        battles: &[Battle],
        current_lap: i64,
        pit_movers: &HashSet<i64>,
        alerts: &mut Vec<Alert>,
    ) {
        // Collect candidates, then emit only the single most interesting one
        // to avoid "train" spam (A closing on B, A closing on C, B closing on C).
        let mut best: Option<Alert> = None;
        let mut best_key: Option<String> = None;

        for b in battles {
            let ltc = match b.laps_to_contact {
                Some(l) if l < CONTACT_IMMINENT_LAPS => l,
                _ => continue,
            };
            if self.should_suppress_contact(inputs, b, current_lap, pit_movers) {
                continue;
            }

            let atk = self.acr(inputs, b.attacker);
            let def = self.acr(inputs, b.defender);
            let def_pos = inputs.positions.get(&b.defender).copied().unwrap_or(0);
            let pos_label = Self::position_label(def_pos);

            let ltc_label = if ltc < 1.0 {
                "< 1 lap".to_string()
            } else {
                format!("{:.1} laps", ltc)
            };

            let headline = if pos_label.is_empty() {
                format!("{} closing on {} — {}", atk, def, ltc_label)
            } else {
                format!(
                    "{} closing on {} for {} — {}",
                    atk, def, pos_label, ltc_label
                )
            };

            let mut detail_parts: Vec<String> = Vec::new();
            detail_parts.push(format!(
                "Gap {:.2}s, closing {:.2}s/lap",
                b.gap, b.closing_rate
            ));
            if let Some(tyre) = Self::tyre_context(inputs, b.attacker, b.defender) {
                detail_parts.push(tyre);
            }
            let context_reasons: Vec<&str> = b
                .reasons
                .iter()
                .filter(|r| {
                    let r = r.as_str();
                    r != "Gap closing" && r != "Contact imminent"
                })
                .map(|r| r.as_str())
                .collect();
            if !context_reasons.is_empty() {
                detail_parts.push(context_reasons.join(", "));
            }

            let score = b.interestingness.max(70);
            let candidate = Alert {
                alert_type: AlertType::ContactImminent,
                priority: AlertPriority::Hot,
                headline,
                detail: detail_parts.join(" — "),
                driver_numbers: vec![b.attacker, b.defender],
                lap: current_lap,
                interestingness: score,
            };

            if best
                .as_ref()
                .is_none_or(|prev| score > prev.interestingness)
            {
                best = Some(candidate);
                best_key = Some(format!("contact-{}-{}", b.attacker, b.defender));
            }
        }

        if let Some(alert) = best {
            if let Some(key) = best_key {
                self.record_cooldown(key, current_lap);
            }
            alerts.push(alert);
        }
    }

    fn detect_safety_car(
        &mut self,
        race_control: &[RaceControlMsg],
        current_lap: i64,
        alerts: &mut Vec<Alert>,
    ) {
        if race_control.len() <= self.prev_rc_count {
            return;
        }
        let new_count = race_control.len() - self.prev_rc_count;
        for msg in race_control.iter().take(new_count) {
            let m = msg.message.to_uppercase();

            let (headline, priority, interestingness) = if m.contains("RED FLAG") {
                ("Red flag".to_string(), AlertPriority::Hot, 95)
            } else if m.contains("VIRTUAL SAFETY CAR") || m.contains("VSC") {
                (
                    "Virtual Safety Car deployed".to_string(),
                    AlertPriority::Warm,
                    70,
                )
            } else if m.contains("SAFETY CAR") && !m.contains("IN THIS LAP") {
                ("Safety Car deployed".to_string(), AlertPriority::Hot, 80)
            } else {
                continue;
            };

            let key = format!("sc-{}", race_control.len());
            if self.on_cooldown(&key, current_lap) {
                continue;
            }

            alerts.push(Alert {
                alert_type: AlertType::SafetyCar,
                priority,
                headline,
                detail: msg.message.clone(),
                driver_numbers: vec![],
                lap: current_lap,
                interestingness,
            });
            self.record_cooldown(key, current_lap);
        }
    }

    fn detect_rain(&mut self, inputs: &AlertInputs, current_lap: i64, alerts: &mut Vec<Alert>) {
        if inputs.rainfall && !self.prev_rainfall {
            let key = "rain".to_string();
            if self.on_cooldown(&key, current_lap) {
                return;
            }
            alerts.push(Alert {
                alert_type: AlertType::RainOnset,
                priority: AlertPriority::Hot,
                headline: "Rain detected at the circuit".to_string(),
                detail: "Strategy calls incoming".to_string(),
                driver_numbers: vec![],
                lap: current_lap,
                interestingness: 85,
            });
            self.record_cooldown(key, current_lap);
        }
    }

    fn detect_tyre_cliff(
        &mut self,
        inputs: &AlertInputs,
        current_lap: i64,
        alerts: &mut Vec<Alert>,
    ) {
        for cliff in &inputs.tyre_cliffs {
            let key = format!("cliff-{}-{}", cliff.driver_number, cliff.stint_number);

            // Rising edge: only alert when a cliff is newly detected
            if self.prev_cliff_keys.contains(&key) {
                continue;
            }
            if self.on_cooldown(&key, current_lap) {
                continue;
            }

            let priority = if cliff.severity >= 1.0 {
                AlertPriority::Hot
            } else {
                AlertPriority::Warm
            };

            let interestingness = (65.0 + cliff.severity * 20.0).min(85.0) as i64;

            alerts.push(Alert {
                alert_type: AlertType::TyreCliff,
                priority,
                headline: cliff.headline.clone(),
                detail: format!(
                    "+{:.1}s degradation on {} at {} laps",
                    cliff.severity,
                    cliff.compound.to_lowercase(),
                    cliff.tyre_age,
                ),
                driver_numbers: vec![cliff.driver_number],
                lap: current_lap,
                interestingness,
            });

            self.record_cooldown(key, current_lap);
        }
    }

    fn detect_pit_window_closing(
        &mut self,
        inputs: &AlertInputs,
        current_lap: i64,
        alerts: &mut Vec<Alert>,
    ) {
        // Aggregate by compound — one alert per compound group, not per driver.
        // Only include high-confidence predictions.
        let mut compound_groups: HashMap<String, Vec<&PitWindow>> = HashMap::new();
        for pw in &inputs.pit_windows {
            if pw.estimated_laps_remaining > 3 {
                continue;
            }
            if !matches!(pw.confidence, super::strategy::Confidence::High) {
                continue;
            }
            compound_groups
                .entry(pw.compound.clone())
                .or_default()
                .push(pw);
        }

        for (compound, pws) in &compound_groups {
            let key = format!("pitwindow-{}", compound.to_lowercase());
            if self.on_cooldown(&key, current_lap) {
                continue;
            }

            let compound_label = match compound.to_uppercase().as_str() {
                "SOFT" => "softs",
                "MEDIUM" => "mediums",
                "HARD" => "hards",
                "INTERMEDIATE" => "inters",
                "WET" => "wets",
                _ => compound.as_str(),
            };

            let driver_names: Vec<String> = pws
                .iter()
                .map(|pw| self.acr(inputs, pw.driver_number))
                .collect();
            let driver_numbers: Vec<i64> = pws.iter().map(|pw| pw.driver_number).collect();

            let age_min = pws.iter().map(|pw| pw.tyre_age).min().unwrap_or(0);
            let age_max = pws.iter().map(|pw| pw.tyre_age).max().unwrap_or(0);
            let age_str = if age_min == age_max {
                format!("{}", age_min)
            } else {
                format!("{}\u{2013}{}", age_min, age_max)
            };

            let headline = if pws.len() == 1 {
                format!(
                    "{} pit window opening \u{2014} {} at {} laps",
                    driver_names[0], compound_label, age_str
                )
            } else {
                format!(
                    "Pit window opening: {} ({} at {} laps)",
                    driver_names.join(", "),
                    compound_label,
                    age_str
                )
            };

            alerts.push(Alert {
                alert_type: AlertType::PitWindowClosing,
                priority: AlertPriority::Warm,
                headline,
                detail: String::new(),
                driver_numbers,
                lap: current_lap,
                interestingness: 55,
            });

            self.record_cooldown(key, current_lap);
        }
    }
}
