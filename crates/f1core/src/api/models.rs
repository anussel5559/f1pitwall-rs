#![allow(dead_code)]
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Session {
    pub session_key: i64,
    pub meeting_key: i64,
    pub session_name: Option<String>,
    pub session_type: Option<String>,
    pub circuit_short_name: Option<String>,
    pub country_name: Option<String>,
    pub date_start: Option<String>,
    pub date_end: Option<String>,
    pub gmt_offset: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Driver {
    pub session_key: i64,
    pub driver_number: i64,
    pub broadcast_name: Option<String>,
    pub name_acronym: Option<String>,
    pub team_name: Option<String>,
    pub team_colour: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Lap {
    pub session_key: Option<i64>,
    pub driver_number: i64,
    pub lap_number: i64,
    pub lap_duration: Option<f64>,
    pub duration_sector_1: Option<f64>,
    pub duration_sector_2: Option<f64>,
    pub duration_sector_3: Option<f64>,
    pub i1_speed: Option<f64>,
    pub i2_speed: Option<f64>,
    pub st_speed: Option<f64>,
    pub is_pit_out_lap: Option<bool>,
    pub date_start: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Position {
    pub session_key: Option<i64>,
    pub driver_number: i64,
    pub position: i64,
    pub date: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Interval {
    pub session_key: Option<i64>,
    pub driver_number: i64,
    pub gap_to_leader: Option<serde_json::Value>,
    pub interval: Option<serde_json::Value>,
    pub date: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Stint {
    pub session_key: Option<i64>,
    pub driver_number: i64,
    pub stint_number: i64,
    pub compound: Option<String>,
    pub lap_start: Option<i64>,
    pub lap_end: Option<i64>,
    pub tyre_age_at_start: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PitStop {
    pub session_key: Option<i64>,
    pub driver_number: i64,
    pub date: Option<String>,
    pub lap_number: Option<i64>,
    pub stop_duration: Option<f64>,
    pub lane_duration: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RaceControl {
    pub session_key: Option<i64>,
    pub date: Option<String>,
    pub category: Option<String>,
    pub flag: Option<String>,
    pub message: Option<String>,
    pub driver_number: Option<i64>,
    pub lap_number: Option<i64>,
    pub scope: Option<String>,
    pub sector: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Weather {
    pub session_key: Option<i64>,
    pub date: Option<String>,
    pub air_temperature: Option<f64>,
    pub track_temperature: Option<f64>,
    pub humidity: Option<f64>,
    pub rainfall: Option<i64>,
    pub wind_speed: Option<f64>,
    pub wind_direction: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StartingGrid {
    pub position: i64,
    pub driver_number: i64,
    pub meeting_key: i64,
    pub session_key: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CarData {
    pub date: Option<String>,
    pub session_key: Option<i64>,
    pub driver_number: i64,
    pub speed: Option<i64>,
    pub throttle: Option<i64>,
    pub brake: Option<i64>,
    pub n_gear: Option<i64>,
    pub rpm: Option<i64>,
    pub drs: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Location {
    pub session_key: Option<i64>,
    pub driver_number: i64,
    pub date: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub z: Option<f64>,
}
