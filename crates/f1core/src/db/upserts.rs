use anyhow::Result;
use rusqlite::params;

use super::Db;
use crate::api::models::*;

impl Db {
    pub fn upsert_session(&self, s: &Session) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sessions (session_key, meeting_key, session_name, session_type, circuit_short_name, country_name, date_start, date_end, gmt_offset)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(session_key) DO UPDATE SET
               meeting_key=excluded.meeting_key, session_name=excluded.session_name,
               session_type=excluded.session_type, circuit_short_name=excluded.circuit_short_name,
               country_name=excluded.country_name, date_start=excluded.date_start, date_end=excluded.date_end,
               gmt_offset=excluded.gmt_offset",
            params![s.session_key, s.meeting_key, s.session_name, s.session_type, s.circuit_short_name, s.country_name, s.date_start, s.date_end, s.gmt_offset],
        )?;
        Ok(())
    }

    pub fn upsert_driver(&self, d: &Driver) -> Result<()> {
        self.conn.execute(
            "INSERT INTO drivers (session_key, driver_number, broadcast_name, name_acronym, team_name, team_colour)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(session_key, driver_number) DO UPDATE SET
               broadcast_name=excluded.broadcast_name, name_acronym=excluded.name_acronym,
               team_name=excluded.team_name, team_colour=excluded.team_colour",
            params![d.session_key, d.driver_number, d.broadcast_name, d.name_acronym, d.team_name, d.team_colour],
        )?;
        Ok(())
    }

    pub fn upsert_lap(&self, session_key: i64, l: &Lap) -> Result<()> {
        self.conn.execute(
            "INSERT INTO laps (session_key, driver_number, lap_number, lap_duration, duration_sector_1, duration_sector_2, duration_sector_3, i1_speed, i2_speed, st_speed, is_pit_out_lap, date_start)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
             ON CONFLICT(session_key, driver_number, lap_number) DO UPDATE SET
               lap_duration=excluded.lap_duration, duration_sector_1=excluded.duration_sector_1,
               duration_sector_2=excluded.duration_sector_2, duration_sector_3=excluded.duration_sector_3,
               i1_speed=excluded.i1_speed, i2_speed=excluded.i2_speed, st_speed=excluded.st_speed,
               is_pit_out_lap=excluded.is_pit_out_lap, date_start=excluded.date_start",
            params![
                session_key, l.driver_number, l.lap_number, l.lap_duration,
                l.duration_sector_1, l.duration_sector_2, l.duration_sector_3,
                l.i1_speed, l.i2_speed, l.st_speed,
                l.is_pit_out_lap.map(|b| b as i64),
                l.date_start,
            ],
        )?;
        Ok(())
    }

    pub fn upsert_position(&self, session_key: i64, p: &Position) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO positions (session_key, driver_number, position, date)
             VALUES (?1,?2,?3,?4)",
            params![session_key, p.driver_number, p.position, p.date],
        )?;
        Ok(())
    }

    /// Insert a position only if we don't have one yet (for starting grid seeding).
    pub fn upsert_position_if_missing(
        &self,
        session_key: i64,
        driver_number: i64,
        position: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO positions (session_key, driver_number, position, date)
             VALUES (?1, ?2, ?3, NULL)",
            params![session_key, driver_number, position],
        )?;
        Ok(())
    }

    pub fn upsert_starting_grid(
        &self,
        session_key: i64,
        driver_number: i64,
        position: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO starting_grid (session_key, driver_number, position)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(session_key, driver_number) DO UPDATE SET position=excluded.position",
            params![session_key, driver_number, position],
        )?;
        Ok(())
    }

    pub fn upsert_interval(&self, session_key: i64, i: &Interval) -> Result<()> {
        let gap = i.gap_to_leader.as_ref().map(value_to_string);
        let int = i.interval.as_ref().map(value_to_string);
        self.conn.execute(
            "INSERT OR REPLACE INTO intervals (session_key, driver_number, gap_to_leader, interval, date)
             VALUES (?1,?2,?3,?4,?5)",
            params![session_key, i.driver_number, gap, int, i.date],
        )?;
        Ok(())
    }

    pub fn upsert_stint(&self, session_key: i64, s: &Stint) -> Result<()> {
        self.conn.execute(
            "INSERT INTO stints (session_key, driver_number, stint_number, compound, lap_start, lap_end, tyre_age_at_start)
             VALUES (?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(session_key, driver_number, stint_number) DO UPDATE SET
               compound=excluded.compound, lap_start=excluded.lap_start,
               lap_end=excluded.lap_end, tyre_age_at_start=excluded.tyre_age_at_start",
            params![session_key, s.driver_number, s.stint_number, s.compound, s.lap_start, s.lap_end, s.tyre_age_at_start],
        )?;
        Ok(())
    }

    pub fn upsert_pit_stop(&self, session_key: i64, p: &PitStop) -> Result<()> {
        self.conn.execute(
            "INSERT INTO pit_stops (session_key, driver_number, date, lap_number, stop_duration, lane_duration)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(session_key, driver_number, lap_number) DO UPDATE SET
               date=excluded.date, stop_duration=excluded.stop_duration, lane_duration=excluded.lane_duration",
            params![session_key, p.driver_number, p.date, p.lap_number, p.stop_duration, p.lane_duration],
        )?;
        Ok(())
    }

    pub fn upsert_race_control(&self, session_key: i64, rc: &RaceControl) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO race_control (session_key, date, category, flag, message, driver_number, lap_number, scope, sector)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![session_key, rc.date, rc.category, rc.flag, rc.message, rc.driver_number, rc.lap_number, rc.scope, rc.sector],
        )?;
        Ok(())
    }

    pub fn upsert_weather(&self, session_key: i64, w: &Weather) -> Result<()> {
        self.conn.execute(
            "INSERT INTO weather (session_key, date, air_temperature, track_temperature, humidity, rainfall, wind_speed, wind_direction)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(session_key, date) DO UPDATE SET
               air_temperature=excluded.air_temperature, track_temperature=excluded.track_temperature,
               humidity=excluded.humidity, rainfall=excluded.rainfall,
               wind_speed=excluded.wind_speed, wind_direction=excluded.wind_direction",
            params![session_key, w.date, w.air_temperature, w.track_temperature, w.humidity, w.rainfall, w.wind_speed, w.wind_direction],
        )?;
        Ok(())
    }

    pub fn upsert_location(
        &self,
        session_key: i64,
        data: &[crate::api::models::Location],
    ) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "INSERT OR IGNORE INTO location (session_key, driver_number, date, x, y, z)
             VALUES (?1,?2,?3,?4,?5,?6)",
        )?;
        for d in data {
            if let Some(ref date) = d.date {
                stmt.execute(params![session_key, d.driver_number, date, d.x, d.y, d.z,])?;
            }
        }
        Ok(())
    }

    pub fn upsert_car_data(
        &self,
        session_key: i64,
        data: &[crate::api::models::CarData],
    ) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "INSERT OR IGNORE INTO car_data (session_key, driver_number, date, speed, throttle, brake, n_gear, rpm, drs)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        )?;
        for d in data {
            if let Some(ref date) = d.date {
                stmt.execute(params![
                    session_key,
                    d.driver_number,
                    date,
                    d.speed,
                    d.throttle,
                    d.brake,
                    d.n_gear,
                    d.rpm,
                    d.drs,
                ])?;
            }
        }
        Ok(())
    }

    pub fn upsert_user(&self, clerk_user_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO users (clerk_user_id)
             VALUES (?1)
             ON CONFLICT(clerk_user_id) DO UPDATE SET
               last_seen_at = datetime('now')",
            params![clerk_user_id],
        )?;
        Ok(())
    }

    // ── Pitwall Manager ─────────────────────────────────────────────
    //
    // Helpers take primitives (no pitwall types crossing the crate boundary)
    // so the persistence layer stays usable from any caller.

    pub fn upsert_pm_participant(
        &self,
        session_key: i64,
        user_id: &str,
        handle: &str,
        team: &str,
        joined_at_lap: i64,
        mode: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO pm_participant (session_key, user_id, handle, team, joined_at_lap, mode, score)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)
             ON CONFLICT(session_key, user_id, mode) DO UPDATE SET
               handle = excluded.handle,
               team   = excluded.team",
            params![session_key, user_id, handle, team, joined_at_lap, mode],
        )?;
        Ok(())
    }

    pub fn update_pm_score(
        &self,
        session_key: i64,
        user_id: &str,
        mode: &str,
        score: i64,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE pm_participant SET score = ?1
             WHERE session_key = ?2 AND user_id = ?3 AND mode = ?4",
            params![score, session_key, user_id, mode],
        )?;
        Ok(())
    }

    pub fn delete_pm_participant(&self, session_key: i64, user_id: &str, mode: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM pm_participant WHERE session_key = ?1 AND user_id = ?2 AND mode = ?3",
            params![session_key, user_id, mode],
        )?;
        Ok(())
    }

    /// Hard-delete every pm_call row owned by this user in the given mode.
    /// Used by clean-slate leave semantics so an old call history doesn't
    /// linger after the player leaves the room.
    pub fn delete_user_pm_calls(&self, session_key: i64, user_id: &str, mode: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM pm_call WHERE session_key = ?1 AND user_id = ?2 AND mode = ?3",
            params![session_key, user_id, mode],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_pm_call(
        &self,
        id: &str,
        session_key: i64,
        user_id: &str,
        mode: &str,
        driver_number: i64,
        target_lap: i64,
        compound: &str,
        locked_at_lap: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO pm_call (id, session_key, user_id, mode, driver_number, target_lap, compound, locked_at_lap, state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'locked')",
            params![id, session_key, user_id, mode, driver_number, target_lap, compound, locked_at_lap],
        )?;
        Ok(())
    }

    pub fn adjust_pm_call(
        &self,
        id: &str,
        target_lap: Option<i64>,
        compound: Option<&str>,
    ) -> Result<()> {
        // Two-step rather than dynamic SQL so the parameter count stays predictable.
        if let Some(lap) = target_lap {
            self.conn.execute(
                "UPDATE pm_call SET target_lap = ?1 WHERE id = ?2 AND state = 'locked'",
                params![lap, id],
            )?;
        }
        if let Some(c) = compound {
            self.conn.execute(
                "UPDATE pm_call SET compound = ?1 WHERE id = ?2 AND state = 'locked'",
                params![c, id],
            )?;
        }
        Ok(())
    }

    pub fn cancel_pm_call(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE pm_call SET state = 'cancelled' WHERE id = ?1 AND state = 'locked'",
            params![id],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn resolve_pm_call(
        &self,
        id: &str,
        real_lap: i64,
        real_compound: &str,
        lap_delta: i64,
        position_delta: i64,
        time_delta_s: f64,
        points_awarded: i64,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE pm_call SET state = 'resolved',
                real_lap = ?1, real_compound = ?2, lap_delta = ?3,
                position_delta = ?4, time_delta_s = ?5, points_awarded = ?6
             WHERE id = ?7 AND state = 'locked'",
            params![
                real_lap,
                real_compound,
                lap_delta,
                position_delta,
                time_delta_s,
                points_awarded,
                id
            ],
        )?;
        Ok(())
    }
}

fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                format!("{:.3}", f)
            } else {
                n.to_string()
            }
        }
        _ => v.to_string(),
    }
}
