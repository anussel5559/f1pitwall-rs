pub const CREATE_TABLES: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    session_key INTEGER PRIMARY KEY,
    meeting_key INTEGER,
    session_name TEXT,
    session_type TEXT,
    circuit_short_name TEXT,
    country_name TEXT,
    date_start TEXT,
    date_end TEXT,
    gmt_offset TEXT,
    replay_position TEXT
);

CREATE TABLE IF NOT EXISTS drivers (
    session_key INTEGER,
    driver_number INTEGER,
    broadcast_name TEXT,
    name_acronym TEXT,
    team_name TEXT,
    team_colour TEXT,
    PRIMARY KEY (session_key, driver_number)
);

CREATE TABLE IF NOT EXISTS laps (
    session_key INTEGER,
    driver_number INTEGER,
    lap_number INTEGER,
    lap_duration REAL,
    duration_sector_1 REAL,
    duration_sector_2 REAL,
    duration_sector_3 REAL,
    i1_speed REAL,
    i2_speed REAL,
    st_speed REAL,
    is_pit_out_lap INTEGER,
    date_start TEXT,
    PRIMARY KEY (session_key, driver_number, lap_number)
);

CREATE TABLE IF NOT EXISTS positions (
    session_key INTEGER,
    driver_number INTEGER,
    position INTEGER,
    date TEXT,
    PRIMARY KEY (session_key, driver_number, date)
);

CREATE TABLE IF NOT EXISTS intervals (
    session_key INTEGER,
    driver_number INTEGER,
    gap_to_leader TEXT,
    interval TEXT,
    date TEXT,
    PRIMARY KEY (session_key, driver_number, date)
);

CREATE TABLE IF NOT EXISTS stints (
    session_key INTEGER,
    driver_number INTEGER,
    stint_number INTEGER,
    compound TEXT,
    lap_start INTEGER,
    lap_end INTEGER,
    tyre_age_at_start INTEGER,
    PRIMARY KEY (session_key, driver_number, stint_number)
);

CREATE TABLE IF NOT EXISTS pit_stops (
    session_key INTEGER,
    driver_number INTEGER,
    date TEXT,
    lap_number INTEGER,
    stop_duration REAL,
    lane_duration REAL,
    PRIMARY KEY (session_key, driver_number, lap_number)
);

CREATE TABLE IF NOT EXISTS race_control (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_key INTEGER,
    date TEXT,
    category TEXT,
    flag TEXT,
    message TEXT,
    driver_number INTEGER,
    lap_number INTEGER,
    scope TEXT,
    sector INTEGER,
    UNIQUE(session_key, date, message)
);

CREATE TABLE IF NOT EXISTS weather (
    session_key INTEGER,
    date TEXT,
    air_temperature REAL,
    track_temperature REAL,
    humidity REAL,
    rainfall INTEGER,
    wind_speed REAL,
    wind_direction INTEGER,
    PRIMARY KEY (session_key, date)
);

CREATE TABLE IF NOT EXISTS starting_grid (
    session_key INTEGER,
    driver_number INTEGER,
    position INTEGER,
    PRIMARY KEY (session_key, driver_number)
);

CREATE TABLE IF NOT EXISTS car_data (
    session_key INTEGER NOT NULL,
    driver_number INTEGER NOT NULL,
    date TEXT NOT NULL,
    speed INTEGER,
    throttle INTEGER,
    brake INTEGER,
    n_gear INTEGER,
    rpm INTEGER,
    drs INTEGER,
    PRIMARY KEY (session_key, driver_number, date)
);

CREATE TABLE IF NOT EXISTS location (
    session_key INTEGER NOT NULL,
    driver_number INTEGER NOT NULL,
    date TEXT NOT NULL,
    x REAL,
    y REAL,
    z REAL,
    PRIMARY KEY (session_key, driver_number, date)
);

CREATE TABLE IF NOT EXISTS users (
    clerk_user_id TEXT PRIMARY KEY,
    subscription_tier TEXT NOT NULL DEFAULT 'free',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS compound_allocations (
    year INTEGER NOT NULL,
    circuit TEXT NOT NULL,
    hard TEXT NOT NULL,
    medium TEXT NOT NULL,
    soft TEXT NOT NULL,
    PRIMARY KEY (year, circuit)
);

CREATE TABLE IF NOT EXISTS pm_participant (
    session_key   INTEGER NOT NULL,
    user_id       TEXT    NOT NULL,
    handle        TEXT    NOT NULL,
    team          TEXT    NOT NULL,
    joined_at_lap INTEGER NOT NULL,
    mode          TEXT    NOT NULL CHECK (mode IN ('live','replay')),
    score         INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (session_key, user_id, mode)
);

CREATE TABLE IF NOT EXISTS pm_call (
    id              TEXT    PRIMARY KEY,
    session_key     INTEGER NOT NULL,
    user_id         TEXT    NOT NULL,
    mode            TEXT    NOT NULL,
    driver_number   INTEGER NOT NULL,
    target_lap      INTEGER NOT NULL,
    compound        TEXT    NOT NULL,
    locked_at_lap   INTEGER NOT NULL,
    state           TEXT    NOT NULL,
    real_lap        INTEGER,
    real_compound   TEXT,
    lap_delta       INTEGER,
    position_delta  INTEGER,
    time_delta_s    REAL,
    points_awarded  INTEGER
);

CREATE INDEX IF NOT EXISTS pm_call_session ON pm_call(session_key, mode, user_id);

";
