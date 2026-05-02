use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use super::Db;
use super::models::*;
use crate::domain::degradation::StintLapData;
use crate::session_types::SessionType;

/// One row from `get_driver_lap_starts`:
/// `(lap_number, date_start, s1, s2, s3, lap_duration)`.
pub type LapSummary = (
    i64,
    String,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
);

fn session_entry_from_row(row: &rusqlite::Row) -> rusqlite::Result<SessionEntry> {
    Ok(SessionEntry {
        session_key: row.get(0)?,
        meeting_key: row.get(1)?,
        session_name: row.get(2)?,
        session_type: row.get(3)?,
        circuit: row.get(4)?,
        country: row.get(5)?,
        date_start: row.get(6)?,
        date_end: row.get(7)?,
        replay_position: row.get(8)?,
    })
}

fn driver_location_from_row(row: &rusqlite::Row) -> rusqlite::Result<DriverLocation> {
    Ok(DriverLocation {
        driver_number: row.get(0)?,
        x: row.get(1)?,
        y: row.get(2)?,
        date: row.get(3)?,
    })
}

impl Db {
    // -- Replay state --

    pub fn save_replay_position(&self, session_key: i64, virtual_ts: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET replay_position=?1 WHERE session_key=?2",
            params![virtual_ts, session_key],
        )?;
        Ok(())
    }

    pub fn get_replay_position(&self, session_key: i64) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT replay_position FROM sessions WHERE session_key=?1")?;
        let result: Option<Option<String>> =
            stmt.query_row(params![session_key], |row| row.get(0)).ok();
        Ok(result.flatten())
    }

    // -- Session picker queries --

    pub fn get_paused_sessions(&self) -> Result<Vec<SessionEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT session_key, meeting_key, COALESCE(session_name,''), COALESCE(session_type,''),
                    COALESCE(circuit_short_name,''), COALESCE(country_name,''),
                    COALESCE(date_start,''), date_end, replay_position
             FROM sessions WHERE replay_position IS NOT NULL
             ORDER BY date_start DESC",
        )?;
        let rows = stmt
            .query_map([], session_entry_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_sessions_by_year(&self, year: i32) -> Result<Vec<SessionEntry>> {
        let start = format!("{}-01-01", year);
        let end = format!("{}-01-01", year + 1);
        let mut stmt = self.conn.prepare(
            "SELECT session_key, meeting_key, COALESCE(session_name,''), COALESCE(session_type,''),
                    COALESCE(circuit_short_name,''), COALESCE(country_name,''),
                    COALESCE(date_start,''), date_end, replay_position
             FROM sessions
             WHERE date_start >= ?1 AND date_start < ?2
             ORDER BY date_start DESC",
        )?;
        let rows = stmt
            .query_map(params![start, end], session_entry_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // -- Practice / strategy queries --

    pub fn get_session_entry(&self, session_key: i64) -> Result<Option<SessionEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT session_key, meeting_key, COALESCE(session_name,''), COALESCE(session_type,''),
                    COALESCE(circuit_short_name,''), COALESCE(country_name,''),
                    COALESCE(date_start,''), date_end, replay_position
             FROM sessions WHERE session_key=?1",
        )?;
        let result = stmt
            .query_row(params![session_key], session_entry_from_row)
            .ok();
        Ok(result)
    }

    pub fn get_sessions_by_meeting_and_type(
        &self,
        meeting_key: i64,
        session_type: &str,
    ) -> Result<Vec<SessionEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT session_key, meeting_key, COALESCE(session_name,''), COALESCE(session_type,''),
                    COALESCE(circuit_short_name,''), COALESCE(country_name,''),
                    COALESCE(date_start,''), date_end, replay_position
             FROM sessions
             WHERE meeting_key=?1 AND session_type=?2
             ORDER BY date_start ASC",
        )?;
        let rows = stmt
            .query_map(params![meeting_key, session_type], session_entry_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // -- Compound allocation queries --

    pub fn get_compound_allocation(
        &self,
        year: i64,
        circuit: &str,
    ) -> Result<Option<CompoundAllocation>> {
        let mut stmt = self.conn.prepare(
            "SELECT year, circuit, hard, medium, soft
             FROM compound_allocations WHERE year=?1 AND circuit=?2",
        )?;
        let result = stmt
            .query_row(params![year, circuit], |row| {
                Ok(CompoundAllocation {
                    year: row.get(0)?,
                    circuit: row.get(1)?,
                    hard: row.get(2)?,
                    medium: row.get(3)?,
                    soft: row.get(4)?,
                })
            })
            .ok();
        Ok(result)
    }

    /// Count SC/VSC deployments before a given lap in this session.
    pub fn get_sc_count_before(&self, session_key: i64, lap: i64, clock_now: &str) -> Result<i64> {
        let mut stmt = self.conn.prepare(
            "SELECT COUNT(*) FROM race_control
             WHERE session_key=?1 AND category='SafetyCar'
               AND message LIKE '%DEPLOYED%'
               AND lap_number < ?2
               AND (date IS NULL OR date <= ?3)",
        )?;
        let count: i64 = stmt.query_row(params![session_key, lap, clock_now], |row| row.get(0))?;
        Ok(count)
    }

    // -- Queries for UI --

    /// Top-3 final positions + fastest lap of the field for a completed race/sprint.
    pub fn get_race_results(&self, session_key: i64) -> Result<RaceResults> {
        let mut stmt = self.conn.prepare(
            "WITH ranked_positions AS (
                SELECT driver_number, position,
                       ROW_NUMBER() OVER (PARTITION BY driver_number ORDER BY date DESC) as rn
                FROM positions WHERE session_key=?1
             )
             SELECT rp.position,
                    d.driver_number,
                    COALESCE(d.name_acronym, ''),
                    COALESCE(d.broadcast_name, ''),
                    COALESCE(d.team_name, ''),
                    COALESCE(d.team_colour, 'FFFFFF')
             FROM ranked_positions rp
             JOIN drivers d ON d.session_key=?1 AND d.driver_number=rp.driver_number
             WHERE rp.rn=1 AND rp.position BETWEEN 1 AND 3
             ORDER BY rp.position ASC",
        )?;
        let podium = stmt
            .query_map(params![session_key], |row| {
                Ok(PodiumEntry {
                    position: row.get(0)?,
                    driver_number: row.get(1)?,
                    name_acronym: row.get(2)?,
                    broadcast_name: row.get(3)?,
                    team_name: row.get(4)?,
                    team_colour: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut stmt = self.conn.prepare(
            "SELECT l.driver_number,
                    l.lap_duration,
                    COALESCE(d.name_acronym, ''),
                    COALESCE(d.team_colour, 'FFFFFF')
             FROM laps l
             JOIN drivers d ON d.session_key=l.session_key AND d.driver_number=l.driver_number
             WHERE l.session_key=?1
               AND l.is_pit_out_lap=0
               AND l.lap_duration IS NOT NULL AND l.lap_duration > 0
             ORDER BY l.lap_duration ASC
             LIMIT 1",
        )?;
        let fastest_lap = stmt
            .query_row(params![session_key], |row| {
                Ok(FastestLap {
                    driver_number: row.get(0)?,
                    lap_time_s: row.get(1)?,
                    name_acronym: row.get(2)?,
                    team_colour: row.get(3)?,
                })
            })
            .ok();

        Ok(RaceResults {
            podium,
            fastest_lap,
        })
    }

    pub fn has_drivers(&self, session_key: i64) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare("SELECT COUNT(*) FROM drivers WHERE session_key=?1")?;
        let count: i64 = stmt.query_row(params![session_key], |row| row.get(0))?;
        Ok(count > 0)
    }

    pub fn get_driver_numbers(&self, session_key: i64) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT driver_number FROM drivers WHERE session_key=?1")?;
        let rows = stmt.query_map(params![session_key], |row| row.get(0))?;
        rows.collect::<rusqlite::Result<Vec<i64>>>()
            .map_err(Into::into)
    }

    pub fn car_data_complete(&self, session_key: i64, driver_number: i64) -> Result<bool> {
        self.driver_table_complete("car_data", session_key, driver_number)
    }

    pub fn location_complete(&self, session_key: i64, driver_number: i64) -> Result<bool> {
        self.driver_table_complete("location", session_key, driver_number)
    }

    /// Compare row count for `(session_key, driver_number)` in `table` against the expected
    /// count derived from the session duration, treating ~70% of a 3 Hz stream as "complete".
    /// Falls back to "any rows = complete" when the session has no date_start/date_end so
    /// we never spuriously re-fetch sessions we don't have bounds for.
    fn driver_table_complete(
        &self,
        table: &str,
        session_key: i64,
        driver_number: i64,
    ) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE session_key=?1 AND driver_number=?2"),
            params![session_key, driver_number],
            |r| r.get(0),
        )?;
        if count == 0 {
            return Ok(false);
        }

        let bounds: Option<(Option<String>, Option<String>)> = self
            .conn
            .query_row(
                "SELECT date_start, date_end FROM sessions WHERE session_key=?1",
                params![session_key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;

        let Some((Some(start), Some(end))) = bounds else {
            return Ok(true);
        };

        let (Ok(start_ts), Ok(end_ts)) = (
            chrono::DateTime::parse_from_rfc3339(&start),
            chrono::DateTime::parse_from_rfc3339(&end),
        ) else {
            return Ok(true);
        };

        let duration_secs = (end_ts - start_ts).num_seconds().max(1);
        // OpenF1 car_data + location publish around 3-4 Hz; require 70% of a 3 Hz baseline
        // so drivers who joined late or retired early still register as complete.
        let expected = (duration_secs * 3) * 70 / 100;
        Ok(count >= expected)
    }

    pub fn get_session(&self, session_key: i64) -> Result<Option<SessionInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT COALESCE(circuit_short_name,''), COALESCE(session_name,''), COALESCE(country_name,''), COALESCE(session_type,''), gmt_offset
             FROM sessions WHERE session_key=?1"
        )?;
        let result = stmt
            .query_row(params![session_key], |row| {
                Ok(SessionInfo {
                    circuit: row.get(0)?,
                    session_name: row.get(1)?,
                    country: row.get(2)?,
                    session_type: row.get(3)?,
                    gmt_offset: row.get(4)?,
                })
            })
            .ok();
        Ok(result)
    }

    pub fn get_board_rows(
        &self,
        _session_type: SessionType,
        session_key: i64,
        clock_now: &str,
    ) -> Result<Vec<BoardRow>> {
        // Qualifying sessions use get_qualifying_board_rows directly.
        // All other session types use the race board query.
        self.get_race_board_rows(session_key, clock_now)
    }

    pub fn get_qualifying_board_rows(
        &self,
        session_key: i64,
        clock_now: &str,
        segment_start: Option<&str>,
    ) -> Result<Vec<QualifyingBoardRow>> {
        // ?3 is the segment start filter — if NULL, no lower bound on date_start.
        let mut stmt = self.conn.prepare(
            "WITH ranked_laps AS (
                SELECT *, ROW_NUMBER() OVER (PARTITION BY driver_number ORDER BY lap_number DESC) as rn
                FROM laps WHERE session_key=?1
                  AND date_start IS NOT NULL AND date_start <= ?2
                  AND (?3 IS NULL OR date_start >= ?3)
             ),
             driver_max_lap AS (
                SELECT driver_number, COALESCE(MAX(lap_number), 0) as max_lap
                FROM laps WHERE session_key=?1
                  AND date_start IS NOT NULL AND date_start <= ?2
                  AND (?3 IS NULL OR date_start >= ?3)
                GROUP BY driver_number
             ),
             ranked_stints AS (
                SELECT s.*, ROW_NUMBER() OVER (PARTITION BY s.driver_number ORDER BY s.stint_number DESC) as rn
                FROM stints s
                LEFT JOIN driver_max_lap dml ON dml.driver_number=s.driver_number
                WHERE s.session_key=?1 AND s.lap_start <= COALESCE(dml.max_lap, 1)
             ),
             -- Every stint's first lap is, by definition, an out lap. Use
             -- this set everywhere we need to exclude out laps, instead of
             -- trusting OpenF1's per-lap is_pit_out_lap flag (which is often
             -- missing on fresh rows). Covers all stints, not just the most
             -- recent one — so a stale out lap in a prior stint also gets
             -- filtered from prev-lap fallbacks and best-lap aggregates.
             out_lap_numbers AS (
                SELECT driver_number, lap_start as lap_number
                FROM stints
                WHERE session_key=?1 AND lap_start IS NOT NULL
             ),
             -- A stint's lap_end is only a real in-lap once we have positive
             -- evidence the stint actually closed: either a successor stint
             -- exists, or a pit_stops row at that lap. Without this gate,
             -- OpenF1's stints endpoint reports lap_end = the latest lap of
             -- an open (live) stint, so every fresh lap of a live driver
             -- looks like an in-lap.
             closed_in_laps AS (
                SELECT s.driver_number, s.lap_end as lap_number
                FROM stints s
                WHERE s.session_key=?1 AND s.lap_end IS NOT NULL
                  AND (
                    EXISTS (
                        SELECT 1 FROM stints s2
                        WHERE s2.session_key=s.session_key
                          AND s2.driver_number=s.driver_number
                          AND s2.stint_number > s.stint_number
                    )
                    OR EXISTS (
                        SELECT 1 FROM pit_stops ps
                        WHERE ps.session_key=s.session_key
                          AND ps.driver_number=s.driver_number
                          AND ps.lap_number=s.lap_end
                          AND ps.date IS NOT NULL
                          AND datetime(ps.date) <= datetime(?2)
                    )
                  )
             ),
             completed_laps AS (
                SELECT l.*
                FROM laps l
                LEFT JOIN closed_in_laps cil ON cil.driver_number=l.driver_number
                  AND cil.lap_number=l.lap_number
                LEFT JOIN out_lap_numbers oln ON oln.driver_number=l.driver_number
                  AND oln.lap_number=l.lap_number
                WHERE l.session_key=?1
                  AND l.date_start IS NOT NULL
                  AND (?3 IS NULL OR l.date_start >= ?3)
                  AND l.is_pit_out_lap=0 AND l.lap_duration IS NOT NULL AND l.lap_duration > 0
                  AND datetime(l.date_start, '+' || CAST(CAST(l.lap_duration AS INTEGER) + 1 AS TEXT) || ' seconds') <= datetime(?2)
                  AND cil.driver_number IS NULL
                  AND oln.driver_number IS NULL
             ),
             best_laps AS (
                SELECT driver_number, MIN(lap_duration) as best_lap
                FROM completed_laps
                GROUP BY driver_number
             ),
             pb_lap AS (
                SELECT cl.driver_number,
                       cl.duration_sector_1 as pb_sector_1,
                       cl.duration_sector_2 as pb_sector_2,
                       cl.duration_sector_3 as pb_sector_3,
                       ROW_NUMBER() OVER (PARTITION BY cl.driver_number ORDER BY cl.lap_duration ASC) as rn
                FROM completed_laps cl
             ),
             timed_lap_count AS (
                SELECT driver_number, COUNT(*) as cnt
                FROM completed_laps
                GROUP BY driver_number
             ),
             prev_non_outlap AS (
                SELECT l2.driver_number,
                       l2.duration_sector_1, l2.duration_sector_2, l2.duration_sector_3,
                       l2.lap_duration, l2.lap_number,
                       ROW_NUMBER() OVER (PARTITION BY l2.driver_number ORDER BY l2.lap_number DESC) as rn
                FROM laps l2
                JOIN ranked_laps rl ON rl.driver_number=l2.driver_number AND rl.rn=1
                LEFT JOIN closed_in_laps cil2 ON cil2.driver_number=l2.driver_number
                  AND cil2.lap_number=l2.lap_number
                LEFT JOIN out_lap_numbers oln2 ON oln2.driver_number=l2.driver_number
                  AND oln2.lap_number=l2.lap_number
                WHERE l2.session_key=?1
                  AND l2.date_start IS NOT NULL AND l2.date_start <= ?2
                  AND (?3 IS NULL OR l2.date_start >= ?3)
                  AND l2.is_pit_out_lap=0
                  AND cil2.driver_number IS NULL
                  AND oln2.driver_number IS NULL
                  AND l2.lap_number < rl.lap_number
             )
             SELECT
                d.driver_number,
                COALESCE(d.name_acronym, '') as acronym,
                COALESCE(d.team_name, '') as team,
                COALESCE(d.team_colour, 'FFFFFF') as team_colour,
                bl.best_lap,
                pb.pb_sector_1,
                pb.pb_sector_2,
                pb.pb_sector_3,
                l.lap_duration,
                l.duration_sector_1,
                l.duration_sector_2,
                l.duration_sector_3,
                l.lap_number,
                l.date_start as lap_date_start,
                pnol.duration_sector_1 as prev_sector_1,
                pnol.duration_sector_2 as prev_sector_2,
                pnol.duration_sector_3 as prev_sector_3,
                pnol.lap_duration as prev_last_lap,
                pnol.lap_number as prev_lap_number,
                COALESCE(st.compound, '') as compound,
                COALESCE(st.tyre_age_at_start, 0) + MAX(COALESCE(l.lap_number, 0) - COALESCE(st.lap_start, 0), 0) as tyre_age,
                COALESCE(tlc.cnt, 0) as lap_count,
                -- Out-lap = first lap of any stint. Trust the stint boundary
                -- over the API flag (which is sometimes missing on freshly-
                -- polled lap rows early in a session). Joining on
                -- out_lap_numbers checks against ALL stints, not just the
                -- most-recent one, matching the set used by prev_non_outlap.
                CASE WHEN (l.is_pit_out_lap = 1)
                       OR oln_cur.driver_number IS NOT NULL
                     THEN 1 ELSE 0 END as is_pit_out_lap,
                -- Only flag is_in_lap when we have positive evidence the
                -- stint actually closed (see closed_in_laps CTE above).
                CASE WHEN cil.driver_number IS NOT NULL
                     THEN 1 ELSE 0 END as is_in_lap,
                -- in_pit reuses the closed_in_laps gate: the driver is in
                -- the pit lane once a confirmed in-lap has finished. Avoids
                -- aggregating over pit_stops with MAX(...), which fires
                -- forever when older traversals have NULL lane_duration
                -- (common for qualifying pit-lane visits in OpenF1 data).
                CASE WHEN cil.driver_number IS NOT NULL
                       AND l.lap_duration IS NOT NULL
                       AND datetime(l.date_start, '+' || CAST(CAST(l.lap_duration AS INTEGER) + 1 AS TEXT) || ' seconds') <= datetime(?2)
                     THEN 1 ELSE 0 END as in_pit
             FROM drivers d
             LEFT JOIN ranked_laps l ON l.driver_number=d.driver_number AND l.rn=1
             LEFT JOIN prev_non_outlap pnol ON pnol.driver_number=d.driver_number AND pnol.rn=1
             LEFT JOIN ranked_stints st ON st.driver_number=d.driver_number AND st.rn=1
             LEFT JOIN best_laps bl ON bl.driver_number=d.driver_number
             LEFT JOIN pb_lap pb ON pb.driver_number=d.driver_number AND pb.rn=1
             LEFT JOIN timed_lap_count tlc ON tlc.driver_number=d.driver_number
             LEFT JOIN closed_in_laps cil ON cil.driver_number=d.driver_number
                AND cil.lap_number=l.lap_number
             LEFT JOIN out_lap_numbers oln_cur ON oln_cur.driver_number=d.driver_number
                AND oln_cur.lap_number=l.lap_number
             WHERE d.session_key=?1
             ORDER BY bl.best_lap ASC NULLS LAST"
        )?;

        let rows = stmt
            .query_map(params![session_key, clock_now, segment_start], |row| {
                Ok(QualifyingBoardRow {
                    position: 0, // assigned after sort
                    driver_number: row.get(0)?,
                    acronym: row.get(1)?,
                    team: row.get(2)?,
                    team_colour: row.get(3)?,
                    best_lap: row.get(4)?,
                    pb_sector_1: row.get(5)?,
                    pb_sector_2: row.get(6)?,
                    pb_sector_3: row.get(7)?,
                    gap: String::new(), // computed in Rust from best_lap diffs
                    last_lap: row.get(8)?,
                    sector_1: row.get(9)?,
                    sector_2: row.get(10)?,
                    sector_3: row.get(11)?,
                    lap_number: row.get(12)?,
                    lap_date_start: row.get(13)?,
                    prev_sector_1: row.get(14)?,
                    prev_sector_2: row.get(15)?,
                    prev_sector_3: row.get(16)?,
                    prev_last_lap: row.get(17)?,
                    prev_lap_number: row.get(18)?,
                    compound: row.get(19)?,
                    tyre_age: row.get(20)?,
                    lap_count: row.get(21)?,
                    is_pit_out_lap: row.get::<_, i64>(22).unwrap_or(0) == 1,
                    is_in_lap: row.get::<_, i64>(23).unwrap_or(0) == 1,
                    in_pit: row.get::<_, i64>(24).unwrap_or(0) == 1,
                    knocked_out: String::new(), // assigned in app logic
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    fn get_race_board_rows(&self, session_key: i64, clock_now: &str) -> Result<Vec<BoardRow>> {
        let mut stmt = self.conn.prepare(
            "WITH ranked_positions AS (
                SELECT *, ROW_NUMBER() OVER (PARTITION BY driver_number ORDER BY date DESC) as rn
                FROM positions WHERE session_key=?1 AND (date IS NULL OR date <= ?2)
             ),
             ranked_intervals AS (
                SELECT *, ROW_NUMBER() OVER (PARTITION BY driver_number ORDER BY date DESC) as rn
                FROM intervals WHERE session_key=?1 AND (date IS NULL OR date <= ?2)
             ),
             ranked_laps AS (
                SELECT *, ROW_NUMBER() OVER (PARTITION BY driver_number ORDER BY lap_number DESC) as rn
                FROM laps WHERE session_key=?1 AND date_start IS NOT NULL AND date_start <= ?2
             ),
             driver_max_lap AS (
                SELECT driver_number, COALESCE(MAX(lap_number), 1) as max_lap
                FROM laps WHERE session_key=?1 AND date_start IS NOT NULL AND date_start <= ?2
                GROUP BY driver_number
             ),
             ranked_stints AS (
                SELECT s.*, ROW_NUMBER() OVER (PARTITION BY s.driver_number ORDER BY s.stint_number DESC) as rn
                FROM stints s
                LEFT JOIN driver_max_lap dml ON dml.driver_number=s.driver_number
                WHERE s.session_key=?1 AND s.lap_start <= COALESCE(dml.max_lap, 1)
             ),
             -- See get_qualifying_board_rows for the open-stint rationale:
             -- only treat a stint's lap_end as a true in-lap once we have
             -- positive evidence the stint closed (successor stint exists,
             -- or pit_stops row at that lap).
             closed_in_laps AS (
                SELECT s.driver_number, s.lap_end as lap_number
                FROM stints s
                WHERE s.session_key=?1 AND s.lap_end IS NOT NULL
                  AND (
                    EXISTS (
                        SELECT 1 FROM stints s2
                        WHERE s2.session_key=s.session_key
                          AND s2.driver_number=s.driver_number
                          AND s2.stint_number > s.stint_number
                    )
                    OR EXISTS (
                        SELECT 1 FROM pit_stops ps
                        WHERE ps.session_key=s.session_key
                          AND ps.driver_number=s.driver_number
                          AND ps.lap_number=s.lap_end
                          AND ps.date IS NOT NULL
                          AND datetime(ps.date) <= datetime(?2)
                    )
                  )
             ),
             pit_counts AS (
                SELECT ps.driver_number, l2.lap_number, COUNT(*) as cnt
                FROM pit_stops ps
                JOIN ranked_laps l2 ON l2.driver_number=ps.driver_number AND l2.rn=1
                WHERE ps.session_key=?1 AND l2.lap_number IS NOT NULL
                  AND ps.lap_number < l2.lap_number
                GROUP BY ps.driver_number
             ),
             stopped_drivers AS (
                SELECT driver_number
                FROM race_control
                WHERE session_key=?1 AND category='CarEvent' AND message LIKE '%STOPPED%'
                  AND (date IS NULL OR date <= ?2)
                GROUP BY driver_number
             ),
             pit_status AS (
                SELECT ps.driver_number,
                    MAX(CASE
                        WHEN ps.date IS NOT NULL
                         AND datetime(ps.date) <= datetime(?2)
                         AND (ps.lane_duration IS NULL
                              OR datetime(ps.date, '+' || CAST(CAST(ps.lane_duration + 1 AS INTEGER) AS TEXT) || ' seconds') >= datetime(?2))
                        THEN 1 ELSE 0
                    END) as in_pit
                FROM pit_stops ps
                WHERE ps.session_key=?1
                GROUP BY ps.driver_number
             ),
             latest_pit AS (
                SELECT ps.driver_number,
                    CASE
                        WHEN ps.lane_duration IS NOT NULL
                             AND datetime(ps.date, '+' || CAST(CAST(ps.lane_duration AS INTEGER) AS TEXT) || ' seconds') < datetime(?2)
                        THEN 1 ELSE 0
                    END as exit_confirmed
                FROM pit_stops ps
                WHERE ps.session_key=?1 AND ps.date IS NOT NULL
                  AND datetime(ps.date) <= datetime(?2)
                  AND ps.date = (
                    SELECT MAX(ps2.date) FROM pit_stops ps2
                    WHERE ps2.session_key=ps.session_key AND ps2.driver_number=ps.driver_number
                      AND ps2.date IS NOT NULL AND datetime(ps2.date) <= datetime(?2)
                  )
             )
             SELECT
                COALESCE(p.position, 0) as fallback_pos,
                d.driver_number,
                COALESCE(d.name_acronym, '') as acronym,
                COALESCE(d.team_name, '') as team,
                COALESCE(d.team_colour, 'FFFFFF') as team_colour,
                COALESCE(i.gap_to_leader, '') as gap,
                COALESCE(i.interval, '') as interval_gap,
                l.lap_duration,
                l.duration_sector_1,
                l.duration_sector_2,
                l.duration_sector_3,
                l.lap_number,
                l.date_start as lap_date_start,
                pl.duration_sector_1 as prev_sector_1,
                pl.duration_sector_2 as prev_sector_2,
                pl.duration_sector_3 as prev_sector_3,
                pl.lap_duration as prev_last_lap,
                pl.lap_number as prev_lap_number,
                COALESCE(st.compound, '') as compound,
                COALESCE(st.tyre_age_at_start, 0) + MAX(COALESCE(l.lap_number, 0) - COALESCE(st.lap_start, 0), 0) as tyre_age,
                COALESCE(pst.compound, '') as prev_compound,
                COALESCE(pst.tyre_age_at_start, 0) + MAX(COALESCE(pst.lap_end, pst.lap_start, 0) - COALESCE(pst.lap_start, 0), 0) as prev_tyre_age,
                COALESCE(pc.cnt, 0) as pit_count,
                sg.position as grid_position,
                CASE WHEN l.is_pit_out_lap = 1 THEN 1 ELSE 0 END as is_pit_out_lap,
                st.lap_end as stint_lap_end,
                CASE WHEN cil.driver_number IS NOT NULL THEN 1 ELSE 0 END as is_in_lap,
                CASE WHEN sd.driver_number IS NOT NULL THEN 1 ELSE 0 END as stopped,
                COALESCE(pst2.in_pit, 0) as in_pit,
                COALESCE(lp.exit_confirmed, 0) as pit_exit_confirmed
             FROM drivers d
             LEFT JOIN ranked_positions p ON p.driver_number=d.driver_number AND p.rn=1
             LEFT JOIN ranked_intervals i ON i.driver_number=d.driver_number AND i.rn=1
             LEFT JOIN ranked_laps l ON l.driver_number=d.driver_number AND l.rn=1
             LEFT JOIN ranked_laps pl ON pl.driver_number=d.driver_number AND pl.rn=2
             LEFT JOIN ranked_stints st ON st.driver_number=d.driver_number AND st.rn=1
             LEFT JOIN ranked_stints pst ON pst.driver_number=d.driver_number AND pst.rn=2
             LEFT JOIN starting_grid sg ON sg.session_key=d.session_key AND sg.driver_number=d.driver_number
             LEFT JOIN pit_counts pc ON pc.driver_number=d.driver_number
             LEFT JOIN stopped_drivers sd ON sd.driver_number=d.driver_number
             LEFT JOIN pit_status pst2 ON pst2.driver_number=d.driver_number
             LEFT JOIN latest_pit lp ON lp.driver_number=d.driver_number
             LEFT JOIN closed_in_laps cil ON cil.driver_number=d.driver_number
                AND cil.lap_number=l.lap_number
             WHERE d.session_key=?1
             ORDER BY COALESCE(p.position, 99)"
        )?;

        let rows = stmt
            .query_map(params![session_key, clock_now], |row| {
                Ok(BoardRow {
                    position: row.get(0)?, // fallback from positions table
                    driver_number: row.get(1)?,
                    acronym: row.get(2)?,
                    team: row.get(3)?,
                    team_colour: row.get(4)?,
                    gap: row.get(5)?,
                    interval: row.get(6)?,
                    last_lap: row.get(7)?,
                    sector_1: row.get(8)?,
                    sector_2: row.get(9)?,
                    sector_3: row.get(10)?,
                    lap_number: row.get(11)?,
                    lap_date_start: row.get(12)?,
                    prev_sector_1: row.get(13)?,
                    prev_sector_2: row.get(14)?,
                    prev_sector_3: row.get(15)?,
                    prev_last_lap: row.get(16)?,
                    prev_lap_number: row.get(17)?,
                    compound: row.get(18)?,
                    tyre_age: row.get(19)?,
                    prev_compound: row.get(20)?,
                    prev_tyre_age: row.get(21)?,
                    pit_count: row.get(22)?,
                    grid_position: row.get(23)?,
                    is_pit_out_lap: row.get::<_, i64>(24).unwrap_or(0) == 1,
                    stint_lap_end: row.get(25)?,
                    is_in_lap: row.get::<_, i64>(26).unwrap_or(0) == 1,
                    stopped: row.get::<_, i64>(27).unwrap_or(0) > 0,
                    in_pit: row.get::<_, i64>(28).unwrap_or(0) > 0,
                    pit_exit_confirmed: row.get::<_, i64>(29).unwrap_or(0) > 0,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    /// Fetch the last N interval entries per driver for convergence analysis.
    /// Returns driver_number → Vec<f64> of parsed intervals in chronological order.
    pub fn get_interval_history(
        &self,
        session_key: i64,
        clock_now: &str,
        max_entries: i64,
    ) -> Result<std::collections::HashMap<i64, Vec<f64>>> {
        let mut stmt = self.conn.prepare(
            "WITH ranked AS (
                SELECT driver_number, interval,
                       ROW_NUMBER() OVER (PARTITION BY driver_number ORDER BY date DESC) as rn
                FROM intervals
                WHERE session_key = ?1
                  AND date IS NOT NULL AND date <= ?2
                  AND interval IS NOT NULL AND interval != ''
             )
             SELECT driver_number, interval
             FROM ranked WHERE rn <= ?3
             ORDER BY driver_number, rn DESC",
        )?;
        let rows = stmt
            .query_map(params![session_key, clock_now, max_entries], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut result: std::collections::HashMap<i64, Vec<f64>> = std::collections::HashMap::new();
        for (driver_number, interval_str) in rows {
            if let Some(val) = crate::domain::battle::parse_interval(&interval_str)
                && val > 0.0
            {
                result.entry(driver_number).or_default().push(val);
            }
        }
        Ok(result)
    }

    /// Compute overall best s1/s2/s3 across the field.
    ///
    /// Mirrors the filtering of `completed_laps` used by the board queries, so
    /// "purple" sectors shown on the frontend are always owned by a driver's
    /// `pb_sector_*` — i.e. the two code paths can't disagree in replay mode.
    pub fn get_best_sectors(
        &self,
        session_key: i64,
        up_to_lap: Option<i64>,
        since_date: Option<&str>,
        clock_now: &str,
    ) -> Result<(Option<f64>, Option<f64>, Option<f64>)> {
        self.best_sectors_inner(session_key, None, up_to_lap, since_date, clock_now)
    }

    pub fn get_driver_best_sectors(
        &self,
        session_key: i64,
        driver_number: i64,
        up_to_lap: Option<i64>,
        since_date: Option<&str>,
        clock_now: &str,
    ) -> Result<(Option<f64>, Option<f64>, Option<f64>)> {
        self.best_sectors_inner(
            session_key,
            Some(driver_number),
            up_to_lap,
            since_date,
            clock_now,
        )
    }

    /// Overall fastest completed-lap duration across the field.
    /// Same filtering as `get_best_sectors` so purple-lap attribution is consistent.
    pub fn get_best_lap(
        &self,
        session_key: i64,
        up_to_lap: Option<i64>,
        since_date: Option<&str>,
        clock_now: &str,
    ) -> Result<Option<f64>> {
        self.best_lap_inner(session_key, None, up_to_lap, since_date, clock_now)
    }

    pub fn get_driver_best_lap(
        &self,
        session_key: i64,
        driver_number: i64,
        up_to_lap: Option<i64>,
        since_date: Option<&str>,
        clock_now: &str,
    ) -> Result<Option<f64>> {
        self.best_lap_inner(
            session_key,
            Some(driver_number),
            up_to_lap,
            since_date,
            clock_now,
        )
    }

    fn best_lap_inner(
        &self,
        session_key: i64,
        driver_number: Option<i64>,
        up_to_lap: Option<i64>,
        since_date: Option<&str>,
        clock_now: &str,
    ) -> Result<Option<f64>> {
        let lap = up_to_lap.unwrap_or(i64::MAX);
        let since = since_date.unwrap_or("");
        let dn = driver_number.unwrap_or(-1);
        let mut stmt = self.conn.prepare(
            "SELECT MIN(l.lap_duration)
             FROM laps l
             LEFT JOIN stints s ON s.session_key=l.session_key
               AND s.driver_number=l.driver_number
               AND l.lap_number BETWEEN s.lap_start AND s.lap_end
             WHERE l.session_key=?1
               AND (?5 = -1 OR l.driver_number = ?5)
               AND l.date_start IS NOT NULL
               AND l.is_pit_out_lap=0
               AND l.lap_duration IS NOT NULL AND l.lap_duration > 0
               AND l.lap_number <= ?2
               AND (?3='' OR l.date_start >= ?3)
               AND datetime(l.date_start, '+' || CAST(CAST(l.lap_duration AS INTEGER) + 1 AS TEXT) || ' seconds') <= datetime(?4)
               AND (s.lap_end IS NULL OR l.lap_number != s.lap_end)",
        )?;
        let result = stmt.query_row(params![session_key, lap, since, clock_now, dn], |row| {
            row.get::<_, Option<f64>>(0)
        })?;
        Ok(result)
    }

    fn best_sectors_inner(
        &self,
        session_key: i64,
        driver_number: Option<i64>,
        up_to_lap: Option<i64>,
        since_date: Option<&str>,
        clock_now: &str,
    ) -> Result<(Option<f64>, Option<f64>, Option<f64>)> {
        let lap = up_to_lap.unwrap_or(i64::MAX);
        let since = since_date.unwrap_or("");
        let dn = driver_number.unwrap_or(-1);
        let mut stmt = self.conn.prepare(
            "SELECT MIN(l.duration_sector_1), MIN(l.duration_sector_2), MIN(l.duration_sector_3)
             FROM laps l
             LEFT JOIN stints s ON s.session_key=l.session_key
               AND s.driver_number=l.driver_number
               AND l.lap_number BETWEEN s.lap_start AND s.lap_end
             WHERE l.session_key=?1
               AND (?5 = -1 OR l.driver_number = ?5)
               AND l.date_start IS NOT NULL
               AND l.is_pit_out_lap=0
               AND l.lap_duration IS NOT NULL AND l.lap_duration > 0
               AND l.lap_number <= ?2
               AND (?3='' OR l.date_start >= ?3)
               AND datetime(l.date_start, '+' || CAST(CAST(l.lap_duration AS INTEGER) + 1 AS TEXT) || ' seconds') <= datetime(?4)
               AND (s.lap_end IS NULL OR l.lap_number != s.lap_end)",
        )?;
        let result = stmt.query_row(params![session_key, lap, since, clock_now, dn], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        Ok(result)
    }

    /// Returns (driver_number, best_lap) for all drivers who set a lap during
    /// a qualifying segment, ordered by best_lap ASC (NULLS LAST).
    pub fn get_segment_results(
        &self,
        session_key: i64,
        segment_start: &str,
        segment_end: &str,
    ) -> Result<Vec<(i64, Option<f64>)>> {
        let mut stmt = self.conn.prepare(
            "WITH segment_completed AS (
                SELECT l.*
                FROM laps l
                LEFT JOIN stints s ON s.session_key=l.session_key
                  AND s.driver_number=l.driver_number
                  AND l.lap_number BETWEEN s.lap_start AND s.lap_end
                WHERE l.session_key=?1
                  AND l.date_start IS NOT NULL
                  AND l.date_start >= ?2
                  AND l.date_start <= ?3
                  AND l.is_pit_out_lap=0 AND l.lap_duration IS NOT NULL AND l.lap_duration > 0
                  AND (s.lap_end IS NULL OR l.lap_number != s.lap_end)
             )
             SELECT d.driver_number, MIN(sc.lap_duration) as best_lap
             FROM drivers d
             LEFT JOIN segment_completed sc ON sc.driver_number=d.driver_number
             WHERE d.session_key=?1
             GROUP BY d.driver_number
             ORDER BY best_lap ASC NULLS LAST",
        )?;
        let rows = stmt
            .query_map(params![session_key, segment_start, segment_end], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Returns the timestamps of GREEN LIGHT messages in chronological order.
    /// Each one marks the start of a qualifying segment (Q1, Q2, Q3).
    pub fn get_qualifying_segment_starts(
        &self,
        session_key: i64,
        clock_now: &str,
    ) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT date FROM race_control
             WHERE session_key=?1 AND date IS NOT NULL AND date <= ?2
               AND message LIKE '%GREEN LIGHT%'
             ORDER BY date ASC",
        )?;
        let rows = stmt
            .query_map(params![session_key, clock_now], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        Ok(rows)
    }

    pub fn get_race_control_messages(
        &self,
        session_key: i64,
        limit: usize,
        clock_now: &str,
    ) -> Result<Vec<RaceControlMsg>> {
        let mut stmt = self.conn.prepare(
            "SELECT COALESCE(date,''), COALESCE(flag,''), COALESCE(message,''), lap_number
             FROM race_control WHERE session_key=?1
               AND (date IS NULL OR date <= ?3)
             ORDER BY date DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![session_key, limit as i64, clock_now], |row| {
                Ok(RaceControlMsg {
                    date: row.get(0)?,
                    flag: row.get(1)?,
                    message: row.get(2)?,
                    lap_number: row.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_latest_weather(
        &self,
        session_key: i64,
        clock_now: &str,
    ) -> Result<Option<WeatherInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT air_temperature, track_temperature, humidity, rainfall, wind_speed, wind_direction
             FROM weather WHERE session_key=?1
               AND (date IS NULL OR date <= ?2)
             ORDER BY date DESC LIMIT 1"
        )?;
        let result = stmt
            .query_row(params![session_key, clock_now], |row| {
                Ok(WeatherInfo {
                    air_temp: row.get(0)?,
                    track_temp: row.get(1)?,
                    humidity: row.get(2)?,
                    rainfall: row.get::<_, Option<i64>>(3)?.unwrap_or(0) != 0,
                    wind_speed: row.get(4)?,
                    wind_direction: row.get(5)?,
                })
            })
            .ok();
        Ok(result)
    }

    /// Per-lap summary for a driver: (lap_number, date_start, s1, s2, s3, lap_duration).
    /// Used both to mark lap boundaries on telemetry charts and to surface
    /// the most-recently-completed lap's sector breakdown.
    pub fn get_driver_lap_starts(
        &self,
        session_key: i64,
        driver_number: i64,
        clock_now: &str,
    ) -> Result<Vec<LapSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT lap_number, date_start,
                    duration_sector_1, duration_sector_2, duration_sector_3,
                    lap_duration
             FROM laps
             WHERE session_key=?1 AND driver_number=?2
               AND date_start IS NOT NULL AND date_start <= ?3
             ORDER BY lap_number",
        )?;
        let rows = stmt
            .query_map(params![session_key, driver_number, clock_now], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<f64>>(2)?,
                    row.get::<_, Option<f64>>(3)?,
                    row.get::<_, Option<f64>>(4)?,
                    row.get::<_, Option<f64>>(5)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Query car telemetry data for a driver within a time range.
    /// Supports both initial fetch (date_gte) and incremental (date_gt) patterns.
    pub fn get_car_data(
        &self,
        session_key: i64,
        driver_number: i64,
        date_gte: Option<&str>,
        date_gt: Option<&str>,
        date_lte: Option<&str>,
    ) -> Result<Vec<CarDataRow>> {
        let mut sql = String::from(
            "SELECT date, speed, throttle, brake, n_gear, rpm, drs FROM car_data
             WHERE session_key=?1 AND driver_number=?2",
        );
        let mut param_idx = 3;
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(session_key), Box::new(driver_number)];

        if let Some(v) = date_gte {
            sql.push_str(&format!(" AND date >= ?{}", param_idx));
            params_vec.push(Box::new(v.to_string()));
            param_idx += 1;
        }
        if let Some(v) = date_gt {
            sql.push_str(&format!(" AND date > ?{}", param_idx));
            params_vec.push(Box::new(v.to_string()));
            param_idx += 1;
        }
        if let Some(v) = date_lte {
            sql.push_str(&format!(" AND date <= ?{}", param_idx));
            params_vec.push(Box::new(v.to_string()));
        }
        sql.push_str(" ORDER BY date");

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(CarDataRow {
                    date: row.get(0)?,
                    speed: row.get(1)?,
                    throttle: row.get(2)?,
                    brake: row.get(3)?,
                    n_gear: row.get(4)?,
                    rpm: row.get(5)?,
                    drs: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Get the most recent location for each of the requested drivers.
    pub fn get_latest_locations(
        &self,
        session_key: i64,
        driver_numbers: &[i64],
        clock_now: &str,
    ) -> Result<Vec<DriverLocation>> {
        if driver_numbers.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: String = driver_numbers
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "WITH ranked AS (
                SELECT driver_number, x, y, date,
                       ROW_NUMBER() OVER (PARTITION BY driver_number ORDER BY date DESC) as rn
                FROM location
                WHERE session_key = ?1 AND date <= ?2
                  AND driver_number IN ({})
             )
             SELECT driver_number, x, y, date FROM ranked WHERE rn = 1",
            placeholders
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(session_key), Box::new(clock_now.to_string())];
        for dn in driver_numbers {
            params_vec.push(Box::new(*dn));
        }
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(params_refs.as_slice(), driver_location_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Return all location rows in a time range for the given drivers (ordered by date).
    pub fn get_locations_since(
        &self,
        session_key: i64,
        driver_numbers: &[i64],
        date_gt: &str,
        date_lte: &str,
    ) -> Result<Vec<DriverLocation>> {
        if driver_numbers.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: String = driver_numbers
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT driver_number, x, y, date FROM location
             WHERE session_key = ?1 AND date > ?2 AND date <= ?3
               AND driver_number IN ({})
             ORDER BY date",
            placeholders
        );
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
            Box::new(session_key),
            Box::new(date_gt.to_string()),
            Box::new(date_lte.to_string()),
        ];
        for dn in driver_numbers {
            params_vec.push(Box::new(*dn));
        }
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_refs.as_slice(), driver_location_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_user_tier(&self, clerk_user_id: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT subscription_tier FROM users WHERE clerk_user_id = ?1")?;
        let result: Option<String> = stmt
            .query_row(params![clerk_user_id], |row| row.get(0))
            .ok();
        Ok(result)
    }

    pub fn get_max_lap(&self, session_key: i64, clock_now: &str) -> Result<i64> {
        let mut stmt = self.conn.prepare(
            "SELECT COALESCE(MAX(lap_number), 0) FROM laps WHERE session_key=?1
             AND date_start IS NOT NULL AND date_start <= ?2",
        )?;
        let result = stmt.query_row(params![session_key, clock_now], |row| row.get(0))?;
        Ok(result)
    }

    /// Fetch all lap data joined with stint info for degradation analysis.
    /// Marks laps under SC/VSC/red flag as neutralized, and pit-in laps as pit-out.
    pub fn get_stint_lap_data(
        &self,
        session_key: i64,
        clock_now: &str,
    ) -> Result<Vec<StintLapData>> {
        let mut stmt = self.conn.prepare(
            "SELECT
                l.driver_number,
                s.stint_number,
                COALESCE(s.compound, '') as compound,
                COALESCE(s.tyre_age_at_start, 0) as tyre_age_at_start,
                l.lap_number,
                l.lap_duration,
                CASE
                    WHEN l.is_pit_out_lap = 1 THEN 1
                    WHEN s.lap_end IS NOT NULL AND l.lap_number = s.lap_end THEN 1
                    ELSE 0
                END as is_pit_out_lap,
                0 as is_neutralized
             FROM laps l
             JOIN stints s ON s.session_key = l.session_key
                AND s.driver_number = l.driver_number
                AND l.lap_number >= s.lap_start
                AND l.lap_number <= COALESCE(s.lap_end, 999)
             WHERE l.session_key = ?1
                AND l.date_start IS NOT NULL
                AND l.date_start <= ?2
                AND l.lap_duration IS NOT NULL
                AND l.lap_duration > 0
             ORDER BY l.driver_number, s.stint_number, l.lap_number",
        )?;

        let rows = stmt
            .query_map(params![session_key, clock_now], |row| {
                Ok(StintLapData {
                    driver_number: row.get(0)?,
                    stint_number: row.get(1)?,
                    compound: row.get(2)?,
                    tyre_age_at_start: row.get(3)?,
                    lap_number: row.get(4)?,
                    lap_duration: row.get(5)?,
                    is_pit_out_lap: row.get::<_, i64>(6).unwrap_or(0) == 1,
                    is_neutralized: row.get::<_, i64>(7).unwrap_or(0) == 1,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    // ── Pitwall Manager ────────────────────────────────────────────
    //
    // Returns plain tuples; pm.rs maps them into wire types. Keeps this
    // crate ignorant of the pitwall transport layer.

    /// (user_id, handle, team, joined_at_lap, score)
    #[allow(clippy::type_complexity)]
    pub fn load_pm_participants(
        &self,
        session_key: i64,
        mode: &str,
    ) -> Result<Vec<(String, String, String, i64, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT user_id, handle, team, joined_at_lap, score
             FROM pm_participant
             WHERE session_key = ?1 AND mode = ?2",
        )?;
        let rows = stmt
            .query_map(params![session_key, mode], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Active (non-cancelled) calls for a room. Resolved calls are returned too
    /// so reconnecting clients see their resolution history.
    /// `(id, user_id, driver_number, target_lap, compound, locked_at_lap,
    ///   state, real_lap, real_compound, lap_delta, position_delta,
    ///   time_delta_s, points_awarded)`
    #[allow(clippy::type_complexity)]
    pub fn load_pm_calls(
        &self,
        session_key: i64,
        mode: &str,
    ) -> Result<
        Vec<(
            String,
            String,
            i64,
            i64,
            String,
            i64,
            String,
            Option<i64>,
            Option<String>,
            Option<i64>,
            Option<i64>,
            Option<f64>,
            Option<i64>,
        )>,
    > {
        let mut stmt = self.conn.prepare(
            "SELECT id, user_id, driver_number, target_lap, compound, locked_at_lap,
                    state, real_lap, real_compound, lap_delta, position_delta,
                    time_delta_s, points_awarded
             FROM pm_call
             WHERE session_key = ?1 AND mode = ?2 AND state != 'cancelled'",
        )?;
        let rows = stmt
            .query_map(params![session_key, mode], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use crate::db::Db;
    use rusqlite::params;

    const SK: i64 = 9999;

    fn setup_driver(db: &Db, dn: i64) {
        db.conn.execute(
            "INSERT INTO drivers (session_key, driver_number, name_acronym, team_name, team_colour)              VALUES (?1, ?2, ?3, 'Team', 'FFFFFF')",
            params![SK, dn, format!("D{}", dn)],
        ).unwrap();
    }

    fn insert_lap(db: &Db, dn: i64, lap: i64, date_start: &str, duration: f64, is_pit_out: bool) {
        db.conn.execute(
            "INSERT INTO laps (session_key, driver_number, lap_number, lap_duration,              duration_sector_1, duration_sector_2, duration_sector_3, is_pit_out_lap, date_start)              VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?5, ?6, ?7)",
            params![SK, dn, lap, duration, duration / 3.0, is_pit_out as i64, date_start],
        ).unwrap();
    }

    fn insert_stint(db: &Db, dn: i64, stint_no: i64, lap_start: i64, lap_end: i64) {
        db.conn.execute(
            "INSERT INTO stints (session_key, driver_number, stint_number, compound,              lap_start, lap_end, tyre_age_at_start)              VALUES (?1, ?2, ?3, 'SOFT', ?4, ?5, 0)",
            params![SK, dn, stint_no, lap_start, lap_end],
        ).unwrap();
    }

    fn insert_pit_stop(db: &Db, dn: i64, lap: i64, date: &str, lane_duration: f64) {
        db.conn.execute(
            "INSERT INTO pit_stops (session_key, driver_number, date, lap_number,              stop_duration, lane_duration)              VALUES (?1, ?2, ?3, ?4, 3.0, ?5)",
            params![SK, dn, date, lap, lane_duration],
        ).unwrap();
    }

    /// Open stint in a live session: lap_end advances to the latest completed
    /// lap, but no successor stint nor pit_stops row exists yet. The lap must
    /// NOT be flagged as is_in_lap.
    #[test]
    fn qualifying_open_stint_does_not_flag_in_lap() {
        let db = Db::open_in_memory().unwrap();
        setup_driver(&db, 16);
        insert_lap(&db, 16, 1, "2026-05-01T10:00:00", 90.0, true);
        insert_lap(&db, 16, 2, "2026-05-01T10:01:30", 75.0, false);
        // Open stint: lap_end = 2 (latest completed lap), no successor, no pit stop.
        insert_stint(&db, 16, 1, 1, 2);

        let rows = db
            .get_qualifying_board_rows(SK, "2026-05-01T10:03:00", None)
            .unwrap();
        let r = rows.iter().find(|r| r.driver_number == 16).unwrap();
        assert_eq!(r.lap_number, Some(2));
        assert!(!r.is_in_lap, "open-stint lap should not flag is_in_lap");
        assert!(!r.in_pit, "open-stint lap should not flag in_pit");
    }

    /// A closed stint (successor exists) correctly flags the lap_end as in-lap.
    #[test]
    fn qualifying_closed_stint_via_successor_flags_in_lap() {
        let db = Db::open_in_memory().unwrap();
        setup_driver(&db, 16);
        insert_lap(&db, 16, 1, "2026-05-01T10:00:00", 90.0, true);
        insert_lap(&db, 16, 2, "2026-05-01T10:01:30", 75.0, false);
        insert_lap(&db, 16, 3, "2026-05-01T10:02:45", 100.0, false); // in-lap
        insert_lap(&db, 16, 4, "2026-05-01T10:04:25", 95.0, true); // out-lap of stint 2
        insert_stint(&db, 16, 1, 1, 3);
        insert_stint(&db, 16, 2, 4, 4); // successor exists → stint 1 is closed

        let rows = db
            .get_qualifying_board_rows(SK, "2026-05-01T10:06:00", None)
            .unwrap();
        let r = rows.iter().find(|r| r.driver_number == 16).unwrap();
        // Latest lap = 4 (out-lap of stint 2), not an in-lap.
        assert_eq!(r.lap_number, Some(4));
        assert!(!r.is_in_lap);
        assert!(r.is_pit_out_lap);
    }

    /// A pit_stops row at lap_end also closes the stint (pit entered, no
    /// new stint reported yet).
    #[test]
    fn qualifying_closed_stint_via_pit_stop_flags_in_lap() {
        let db = Db::open_in_memory().unwrap();
        setup_driver(&db, 16);
        insert_lap(&db, 16, 1, "2026-05-01T10:00:00", 90.0, true);
        insert_lap(&db, 16, 2, "2026-05-01T10:01:30", 75.0, false);
        insert_lap(&db, 16, 3, "2026-05-01T10:02:45", 100.0, false); // in-lap
        insert_stint(&db, 16, 1, 1, 3);
        // Pit stop at lap 3 (the in-lap), well within clock_now's window.
        insert_pit_stop(&db, 16, 3, "2026-05-01T10:04:25", 25.0);

        let rows = db
            .get_qualifying_board_rows(SK, "2026-05-01T10:04:30", None)
            .unwrap();
        let r = rows.iter().find(|r| r.driver_number == 16).unwrap();
        assert_eq!(r.lap_number, Some(3));
        assert!(r.is_in_lap, "pit-stop at lap_end should close the stint");
    }

    /// In-pit window: while the driver is still in the pit lane (pit_stops
    /// date..date+lane_duration covers clock_now), in_pit must be true even
    /// for an open stint.
    #[test]
    fn qualifying_in_pit_follows_pit_stops_timing() {
        let db = Db::open_in_memory().unwrap();
        setup_driver(&db, 16);
        insert_lap(&db, 16, 1, "2026-05-01T10:00:00", 90.0, true);
        insert_lap(&db, 16, 2, "2026-05-01T10:01:30", 75.0, false);
        insert_lap(&db, 16, 3, "2026-05-01T10:02:45", 100.0, false);
        insert_stint(&db, 16, 1, 1, 3);
        insert_pit_stop(&db, 16, 3, "2026-05-01T10:04:25", 25.0);

        // Clock is mid-pit-lane (date + 10s, well before date + 25s).
        let rows = db
            .get_qualifying_board_rows(SK, "2026-05-01T10:04:35", None)
            .unwrap();
        let r = rows.iter().find(|r| r.driver_number == 16).unwrap();
        assert!(r.in_pit, "driver should still be in pit lane");
    }

    /// Regression: qualifying pit-lane traversals often arrive with
    /// lane_duration=NULL in OpenF1 data. Once the driver is back out on a
    /// new stint (out-lap of stint 2 in progress), in_pit must clear — not
    /// stay stuck because an older pit_stops row had a NULL lane_duration.
    #[test]
    fn qualifying_in_pit_clears_after_out_lap_with_null_lane_duration() {
        let db = Db::open_in_memory().unwrap();
        setup_driver(&db, 16);
        insert_lap(&db, 16, 1, "2026-05-01T10:00:00", 90.0, true);
        insert_lap(&db, 16, 2, "2026-05-01T10:01:30", 75.0, false);
        insert_lap(&db, 16, 3, "2026-05-01T10:02:45", 100.0, false); // in-lap
        insert_lap(&db, 16, 4, "2026-05-01T10:04:25", 95.0, true); // out-lap of stint 2
        insert_stint(&db, 16, 1, 1, 3);
        insert_stint(&db, 16, 2, 4, 4); // successor stint → stint 1 closed

        // Pit-lane traversal at lap 3 with NULL lane_duration.
        db.conn
            .execute(
                "INSERT INTO pit_stops (session_key, driver_number, date, lap_number,                  stop_duration, lane_duration) VALUES (?1, ?2, ?3, ?4, 3.0, NULL)",
                params![SK, 16, "2026-05-01T10:04:25", 3],
            )
            .unwrap();

        // Clock is mid-out-lap (lap 4 in progress); driver is on track.
        let rows = db
            .get_qualifying_board_rows(SK, "2026-05-01T10:05:00", None)
            .unwrap();
        let r = rows.iter().find(|r| r.driver_number == 16).unwrap();
        assert_eq!(r.lap_number, Some(4));
        assert!(
            !r.in_pit,
            "driver on out-lap must not be flagged in_pit, even with NULL lane_duration on prior traversal"
        );
        assert!(r.is_pit_out_lap);
    }

    /// Race query mirrors the qualifying gate via the new is_in_lap field
    /// on BoardRow.
    #[test]
    fn race_open_stint_does_not_flag_in_lap() {
        let db = Db::open_in_memory().unwrap();
        setup_driver(&db, 16);
        insert_lap(&db, 16, 1, "2026-05-01T14:00:00", 90.0, false);
        insert_lap(&db, 16, 2, "2026-05-01T14:01:30", 88.0, false);
        // Open stint: lap_end advances live, no successor, no pit stop.
        insert_stint(&db, 16, 1, 1, 2);

        let rows = db.get_race_board_rows(SK, "2026-05-01T14:03:00").unwrap();
        let r = rows.iter().find(|r| r.driver_number == 16).unwrap();
        assert_eq!(r.lap_number, Some(2));
        assert!(
            !r.is_in_lap,
            "open-stint lap should not flag is_in_lap (race)"
        );
    }

    #[test]
    fn race_closed_stint_flags_in_lap() {
        let db = Db::open_in_memory().unwrap();
        setup_driver(&db, 16);
        insert_lap(&db, 16, 1, "2026-05-01T14:00:00", 90.0, false);
        insert_lap(&db, 16, 2, "2026-05-01T14:01:30", 88.0, false);
        insert_stint(&db, 16, 1, 1, 2);
        insert_pit_stop(&db, 16, 2, "2026-05-01T14:02:58", 25.0);

        let rows = db.get_race_board_rows(SK, "2026-05-01T14:03:05").unwrap();
        let r = rows.iter().find(|r| r.driver_number == 16).unwrap();
        assert_eq!(r.lap_number, Some(2));
        assert!(
            r.is_in_lap,
            "pit-stop at lap_end should mark race lap as in-lap"
        );
    }
}
