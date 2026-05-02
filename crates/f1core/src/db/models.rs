use ts_rs::TS;

#[derive(Debug, Clone, serde::Serialize, TS)]
#[ts(export)]
pub struct SessionEntry {
    pub session_key: i64,
    pub meeting_key: i64,
    pub session_name: String,
    pub session_type: String,
    pub circuit: String,
    pub country: String,
    pub date_start: String,
    pub date_end: Option<String>,
    #[allow(dead_code)]
    pub replay_position: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, TS)]
#[ts(export)]
pub struct SessionInfo {
    pub circuit: String,
    pub session_name: String,
    pub country: String,
    pub session_type: String,
    #[allow(dead_code)]
    pub gmt_offset: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BoardRow {
    pub position: i64,
    pub driver_number: i64,
    pub acronym: String,
    pub team: String,
    pub team_colour: String,
    pub gap: String,
    pub interval: String,
    pub last_lap: Option<f64>,
    pub sector_1: Option<f64>,
    pub sector_2: Option<f64>,
    pub sector_3: Option<f64>,
    #[allow(dead_code)]
    pub lap_number: Option<i64>,
    pub lap_date_start: Option<String>,
    pub prev_sector_1: Option<f64>,
    pub prev_sector_2: Option<f64>,
    pub prev_sector_3: Option<f64>,
    pub prev_last_lap: Option<f64>,
    pub prev_lap_number: Option<i64>,
    pub compound: String,
    pub tyre_age: Option<i64>,
    pub prev_compound: String,
    pub prev_tyre_age: Option<i64>,
    pub pit_count: i64,
    pub grid_position: Option<i64>,
    pub is_pit_out_lap: bool,
    #[allow(dead_code)]
    pub stint_lap_end: Option<i64>,
    pub is_in_lap: bool,
    pub stopped: bool,
    pub in_pit: bool,
    pub pit_exit_confirmed: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct QualifyingBoardRow {
    pub position: i64,
    pub driver_number: i64,
    pub acronym: String,
    pub team: String,
    pub team_colour: String,
    pub best_lap: Option<f64>,
    pub pb_sector_1: Option<f64>,
    pub pb_sector_2: Option<f64>,
    pub pb_sector_3: Option<f64>,
    pub gap: String,
    pub last_lap: Option<f64>,
    pub sector_1: Option<f64>,
    pub sector_2: Option<f64>,
    pub sector_3: Option<f64>,
    pub lap_number: Option<i64>,
    pub lap_date_start: Option<String>,
    pub prev_sector_1: Option<f64>,
    pub prev_sector_2: Option<f64>,
    pub prev_sector_3: Option<f64>,
    pub prev_last_lap: Option<f64>,
    pub prev_lap_number: Option<i64>,
    pub compound: String,
    pub tyre_age: Option<i64>,
    pub lap_count: i64,
    pub is_pit_out_lap: bool,
    pub is_in_lap: bool,
    pub in_pit: bool,
    /// Non-empty ("Q1", "Q2") if the driver was eliminated in a previous segment.
    pub knocked_out: String,
}

#[derive(Debug, Clone, serde::Serialize, TS)]
#[ts(export)]
pub struct CarDataRow {
    pub date: String,
    pub speed: Option<i64>,
    pub throttle: Option<i64>,
    pub brake: Option<i64>,
    pub n_gear: Option<i64>,
    pub rpm: Option<i64>,
    pub drs: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize, TS)]
#[ts(export)]
pub struct DriverLocation {
    pub driver_number: i64,
    pub x: f64,
    pub y: f64,
    pub date: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CompoundAllocation {
    pub year: i64,
    pub circuit: String,
    pub hard: String,
    pub medium: String,
    pub soft: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, TS)]
#[ts(export)]
pub struct RaceControlMsg {
    pub date: String,
    pub flag: String,
    pub message: String,
    pub lap_number: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize, TS)]
#[ts(export)]
pub struct PodiumEntry {
    pub position: i64,
    pub driver_number: i64,
    pub name_acronym: String,
    pub broadcast_name: String,
    pub team_name: String,
    pub team_colour: String,
}

#[derive(Debug, Clone, serde::Serialize, TS)]
#[ts(export)]
pub struct FastestLap {
    pub driver_number: i64,
    pub name_acronym: String,
    pub team_colour: String,
    pub lap_time_s: f64,
}

#[derive(Debug, Clone, serde::Serialize, TS)]
#[ts(export)]
pub struct RaceResults {
    pub podium: Vec<PodiumEntry>,
    pub fastest_lap: Option<FastestLap>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, TS)]
#[ts(export)]
pub struct WeatherInfo {
    pub air_temp: Option<f64>,
    pub track_temp: Option<f64>,
    pub humidity: Option<f64>,
    pub rainfall: bool,
    pub wind_speed: Option<f64>,
    #[allow(dead_code)]
    pub wind_direction: Option<i64>,
}
