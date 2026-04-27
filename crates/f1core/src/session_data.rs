use crate::db::{BoardRow, QualifyingBoardRow, RaceControlMsg, WeatherInfo};
use crate::session_types::SessionType;
use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use std::collections::HashMap;

pub type DriverBestSectors = HashMap<i64, (Option<f64>, Option<f64>, Option<f64>)>;

#[derive(Debug, Clone, PartialEq)]
pub enum ViewMode {
    Board,
    Telemetry { driver_number: i64 },
    TrackMap,
}

/// A board row enriched with computed display values.
/// `board` contains the raw data from the database;
/// `display_*` fields are derived by refresh (progressive reveal + fallback).
pub struct DisplayRow {
    pub board: BoardRow,
    pub display_s1: Option<f64>,
    pub display_s2: Option<f64>,
    pub display_s3: Option<f64>,
    pub display_last_lap: Option<f64>,
    /// Which lap's sectors are currently displayed.
    pub display_lap: Option<i64>,
}

/// A qualifying board row enriched with computed display values.
pub struct QualifyingDisplayRow {
    pub board: QualifyingBoardRow,
    pub display_s1: Option<f64>,
    pub display_s2: Option<f64>,
    pub display_s3: Option<f64>,
    pub display_last_lap: Option<f64>,
    pub display_lap: Option<i64>,
}

/// Wrapper enum for session-type-specific board rows.
pub enum BoardRows {
    Race(Vec<DisplayRow>),
    Qualifying(Vec<QualifyingDisplayRow>),
}

impl BoardRows {
    pub fn len(&self) -> usize {
        match self {
            Self::Race(rows) => rows.len(),
            Self::Qualifying(rows) => rows.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get_driver_number(&self, idx: usize) -> Option<i64> {
        match self {
            Self::Race(rows) => rows.get(idx).map(|r| r.board.driver_number),
            Self::Qualifying(rows) => rows.get(idx).map(|r| r.board.driver_number),
        }
    }

    pub fn find_driver_lap_start(&self, driver_number: i64) -> Option<String> {
        match self {
            Self::Race(rows) => rows
                .iter()
                .find(|r| r.board.driver_number == driver_number)
                .and_then(|r| r.board.lap_date_start.clone()),
            Self::Qualifying(rows) => rows
                .iter()
                .find(|r| r.board.driver_number == driver_number)
                .and_then(|r| r.board.lap_date_start.clone()),
        }
    }

    pub fn find_driver_info(&self, driver_number: i64) -> Option<(String, String, String)> {
        match self {
            Self::Race(rows) => rows
                .iter()
                .find(|r| r.board.driver_number == driver_number)
                .map(|r| {
                    (
                        r.board.acronym.clone(),
                        r.board.team.clone(),
                        r.board.team_colour.clone(),
                    )
                }),
            Self::Qualifying(rows) => rows
                .iter()
                .find(|r| r.board.driver_number == driver_number)
                .map(|r| {
                    (
                        r.board.acronym.clone(),
                        r.board.team.clone(),
                        r.board.team_colour.clone(),
                    )
                }),
        }
    }

    /// Collect driver numbers in order, for telemetry driver cycling.
    pub fn driver_numbers(&self) -> Vec<i64> {
        match self {
            Self::Race(rows) => rows.iter().map(|r| r.board.driver_number).collect(),
            Self::Qualifying(rows) => rows.iter().map(|r| r.board.driver_number).collect(),
        }
    }
}

/// Session and timing data — the "model" side of the app.
pub struct SessionData {
    pub session_key: i64,
    pub circuit: String,
    pub session_name: String,
    pub session_type: String,
    pub session_type_enum: SessionType,
    pub country: String,
    pub current_lap: i64,
    pub total_laps: Option<i64>,
    pub rows: BoardRows,
    pub best_s1: Option<f64>,
    pub best_lap_time: Option<f64>,
    pub best_s2: Option<f64>,
    pub best_s3: Option<f64>,
    pub driver_best_sectors: DriverBestSectors,
    pub race_control: Vec<RaceControlMsg>,
    pub weather: Option<WeatherInfo>,
    /// UTC time the formation lap is scheduled to start (parsed from race control).
    pub formation_lap_at: Option<DateTime<Utc>>,
    pub session_started: bool,
}

/// Parse "FORMATION LAP WILL START AT HH:MM" -> UTC DateTime.
/// Uses the session date and the local timezone offset to resolve the full timestamp.
pub fn parse_formation_lap_time(
    message: &str,
    local_offset: FixedOffset,
    session_start: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let prefix = "FORMATION LAP WILL START AT ";
    let time_str = message.strip_prefix(prefix)?;
    let parts: Vec<&str> = time_str.trim().split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let hour: u32 = parts[0].parse().ok()?;
    let min: u32 = parts[1].parse().ok()?;

    // Use the session start date (in local time) to build the full datetime.
    let session_local = session_start.with_timezone(&local_offset);
    let session_date = session_local.date_naive();
    let local_time = chrono::NaiveTime::from_hms_opt(hour, min, 0)?;
    let local_dt = session_date.and_time(local_time);
    let local_fixed = local_offset.from_local_datetime(&local_dt).single()?;
    Some(local_fixed.with_timezone(&Utc))
}
