pub use f1core::clock::SessionClock;
pub use f1core::session_data::{
    BoardRows, DisplayRow, QualifyingDisplayRow, SessionData, ViewMode, parse_formation_lap_time,
};
pub use f1core::toast::{Toasts, push_toast};

use anyhow::Result;
use f1core::db::{BoardRow, Db};
use f1core::display;
use f1core::domain::{position, track};
use f1core::session_types::SessionType;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Full application state: session data + UI state + shared resources.
pub struct AppState {
    pub session: SessionData,

    // -- UI state --
    pub selected_index: usize,
    pub scroll_offset: usize,
    /// Number of visible rows in the board (set during render).
    pub visible_rows: usize,
    pub show_race_control: bool,
    pub view_mode: ViewMode,
    /// Drivers selected for track map display (toggled via Space in Board view).
    pub selected_drivers: HashSet<i64>,
    /// Tracks when each driver's displayed sector value last changed, for flash highlights.
    /// Key: (driver_number, sector 0/1/2), Value: wall-clock instant of change.
    pub sector_flash: HashMap<(i64, u8), Instant>,
    /// Previous frame's displayed sector values per driver, used to detect changes.
    prev_display: HashMap<i64, [Option<f64>; 3]>,

    // -- Shared resources --
    pub toasts: Toasts,
    pub clock: Arc<SessionClock>,
    pub authenticated: bool,
}

impl AppState {
    pub fn new(
        session_key: i64,
        session_type: SessionType,
        toasts: Toasts,
        clock: Arc<SessionClock>,
        authenticated: bool,
    ) -> Self {
        Self {
            session: SessionData {
                session_key,
                circuit: String::new(),
                session_name: String::new(),
                session_type: String::new(),
                session_type_enum: session_type,
                country: String::new(),
                current_lap: 0,
                total_laps: None,
                rows: BoardRows::Race(Vec::new()),
                best_s1: None,
                best_lap_time: None,
                best_s2: None,
                best_s3: None,
                driver_best_sectors: HashMap::new(),
                race_control: Vec::new(),
                weather: None,
                formation_lap_at: None,
                session_started: false,
            },
            selected_index: 0,
            scroll_offset: 0,
            visible_rows: 20,
            show_race_control: true,
            view_mode: ViewMode::Board,
            selected_drivers: HashSet::new(),
            sector_flash: HashMap::new(),
            prev_display: HashMap::new(),
            toasts,
            clock,
            authenticated,
        }
    }

    /// Get active toasts (less than 15 seconds old), pruning expired ones.
    pub fn active_toasts(&self) -> Vec<(String, bool)> {
        let mut t = self.toasts.lock().unwrap();
        t.retain(|toast| toast.created.elapsed().as_secs() < 15);
        t.iter()
            .map(|toast| (toast.message.clone(), toast.is_error))
            .collect()
    }

    pub fn refresh(&mut self, db: &Arc<Mutex<Db>>) -> Result<()> {
        let db = db.lock().unwrap();
        self.load_session_info(&db)?;
        match self.session.session_type_enum {
            SessionType::Qualifying | SessionType::SprintQualifying | SessionType::Practice => {
                self.load_qualifying_board_and_context(&db)?;
                self.compute_qualifying_best_sectors(&db)?;
            }
            _ => {
                let raw_rows = self.load_board_and_context(&db)?;
                self.detect_formation_lap();
                self.session.rows = self.build_display_rows(raw_rows);
                let clock_now = self.clock.ceiling();
                self.refresh_best_sectors(&db, None, &clock_now)?;
            }
        }
        self.sector_flash.retain(|_, t| t.elapsed().as_secs() < 5);
        Ok(())
    }

    fn load_session_info(&mut self, db: &Db) -> Result<()> {
        let s = &mut self.session;
        if let Some(info) = db.get_session(s.session_key)? {
            if s.total_laps.is_none() {
                s.total_laps = track::get_track_outline(&info.circuit).and_then(|o| o.race_laps);
            }
            s.circuit = info.circuit;
            s.session_name = info.session_name;
            s.session_type = info.session_type;
            s.country = info.country;
        }
        Ok(())
    }

    fn load_board_and_context(&mut self, db: &Db) -> Result<Vec<BoardRow>> {
        let clock_now = self.clock.ceiling();
        let s = &mut self.session;
        s.current_lap = db.get_max_lap(s.session_key, &clock_now)?;
        let mut raw_rows = db.get_board_rows(s.session_type_enum, s.session_key, &clock_now)?;
        position::sort_and_assign_positions(&mut raw_rows);
        s.race_control = db.get_race_control_messages(s.session_key, 20, &clock_now)?;
        s.weather = db.get_latest_weather(s.session_key, &clock_now)?;
        Ok(raw_rows)
    }

    fn detect_formation_lap(&mut self) {
        if self.session.formation_lap_at.is_none()
            && let Some(local_offset) = self.clock.local_offset
        {
            for msg in &self.session.race_control {
                if let Some(t) =
                    parse_formation_lap_time(&msg.message, local_offset, self.clock.session_start)
                {
                    self.session.formation_lap_at = Some(t);
                    break;
                }
            }
        }
        if !self.session.session_started {
            self.session.session_started = self
                .session
                .race_control
                .iter()
                .any(|m| m.message.contains("SESSION STARTED"));
        }
    }

    /// Build DisplayRows from raw board data: progressive sector reveal,
    /// fallback to previous lap, and flash highlight change detection.
    fn build_display_rows(&mut self, raw_rows: Vec<BoardRow>) -> BoardRows {
        let virtual_now = self.clock.now();
        let now_wall = Instant::now();
        let mut display_rows = Vec::with_capacity(raw_rows.len());

        for board in raw_rows {
            let cd = display::race_display(virtual_now, &board);
            self.update_flash(board.driver_number, cd.s1, cd.s2, cd.s3, now_wall);

            display_rows.push(DisplayRow {
                board,
                display_s1: cd.s1,
                display_s2: cd.s2,
                display_s3: cd.s3,
                display_last_lap: cd.last_lap,
                display_lap: cd.lap,
            });
        }
        BoardRows::Race(display_rows)
    }

    fn load_qualifying_board_and_context(&mut self, db: &Db) -> Result<()> {
        let clock_now = self.clock.ceiling();
        let session_key = self.session.session_key;
        self.session.current_lap = db.get_max_lap(session_key, &clock_now)?;

        let (all_rows, _segment) = display::build_qualifying_rows(db, session_key, &clock_now)?;

        self.session.race_control = db.get_race_control_messages(session_key, 20, &clock_now)?;
        self.session.weather = db.get_latest_weather(session_key, &clock_now)?;
        self.session.rows = self.build_qualifying_display_rows(all_rows);
        Ok(())
    }

    fn build_qualifying_display_rows(
        &mut self,
        raw_rows: Vec<f1core::db::QualifyingBoardRow>,
    ) -> BoardRows {
        let virtual_now = self.clock.now();
        let now_wall = Instant::now();
        let mut display_rows = Vec::with_capacity(raw_rows.len());

        for board in raw_rows {
            let cd = display::qualifying_display(virtual_now, &board);
            self.update_flash(board.driver_number, cd.s1, cd.s2, cd.s3, now_wall);

            display_rows.push(QualifyingDisplayRow {
                board,
                display_s1: cd.s1,
                display_s2: cd.s2,
                display_s3: cd.s3,
                display_last_lap: cd.last_lap,
                display_lap: cd.lap,
            });
        }
        BoardRows::Qualifying(display_rows)
    }

    fn compute_qualifying_best_sectors(&mut self, db: &Db) -> Result<()> {
        // Scope purple sectors to the current qualifying segment.
        let clock_now = self.clock.ceiling();
        let segment_starts =
            db.get_qualifying_segment_starts(self.session.session_key, &clock_now)?;
        let segment_start = segment_starts.last().cloned();
        self.refresh_best_sectors(db, segment_start.as_deref(), &clock_now)?;
        if let BoardRows::Qualifying(ref rows) = self.session.rows {
            self.session.best_lap_time = rows
                .iter()
                .filter_map(|r| r.board.best_lap)
                .reduce(f64::min);
        }
        Ok(())
    }

    fn refresh_best_sectors(
        &mut self,
        db: &Db,
        since_date: Option<&str>,
        clock_now: &str,
    ) -> Result<()> {
        let driver_laps: Vec<(i64, Option<i64>)> = match &self.session.rows {
            BoardRows::Race(rows) => rows
                .iter()
                .map(|r| (r.board.driver_number, r.display_lap))
                .collect(),
            BoardRows::Qualifying(rows) => rows
                .iter()
                .map(|r| (r.board.driver_number, r.display_lap))
                .collect(),
        };
        let result = display::compute_best_sectors(
            db,
            self.session.session_key,
            &driver_laps,
            since_date,
            clock_now,
        )?;
        let (s1, s2, s3) = result.overall;
        self.session.best_s1 = s1;
        self.session.best_s2 = s2;
        self.session.best_s3 = s3;
        self.session.driver_best_sectors = result.per_driver;
        Ok(())
    }

    /// Track sector flash highlights: detect when a display value changes.
    fn update_flash(
        &mut self,
        driver_number: i64,
        s1: Option<f64>,
        s2: Option<f64>,
        s3: Option<f64>,
        now_wall: Instant,
    ) {
        let prev = self.prev_display.get(&driver_number);
        if prev.map(|p| p[0]) != Some(s1) {
            self.sector_flash.insert((driver_number, 0), now_wall);
        }
        if prev.map(|p| p[1]) != Some(s2) {
            self.sector_flash.insert((driver_number, 1), now_wall);
        }
        if prev.map(|p| p[2]) != Some(s3) {
            self.sector_flash.insert((driver_number, 2), now_wall);
        }
        self.prev_display.insert(driver_number, [s1, s2, s3]);
    }

    pub fn scroll_up(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(1);
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        }
    }

    pub fn scroll_down(&mut self) {
        if self.selected_index < self.session.rows.len().saturating_sub(1) {
            self.selected_index += 1;
            if self.selected_index >= self.scroll_offset + self.visible_rows {
                self.scroll_offset = self.selected_index - self.visible_rows + 1;
            }
        }
    }

    pub fn toggle_race_control(&mut self) {
        self.show_race_control = !self.show_race_control;
    }

    /// Toggle a driver in/out of the track map selection set.
    pub fn toggle_selected_driver(&mut self, driver_number: i64) {
        if !self.selected_drivers.remove(&driver_number) {
            self.selected_drivers.insert(driver_number);
        }
    }

    /// Get the driver number at the current selected index.
    pub fn selected_driver(&self) -> Option<i64> {
        self.session.rows.get_driver_number(self.selected_index)
    }

    /// Get the lap_date_start for a given driver from the current board rows.
    pub fn driver_lap_start(&self, driver_number: i64) -> Option<String> {
        self.session.rows.find_driver_lap_start(driver_number)
    }

    /// Get driver info (acronym, team, team_colour) for a given driver number.
    pub fn driver_info(&self, driver_number: i64) -> Option<(String, String, String)> {
        self.session.rows.find_driver_info(driver_number)
    }

    pub fn lap_display(&self) -> String {
        let s = &self.session;
        match s.session_type_enum {
            SessionType::Qualifying | SessionType::SprintQualifying | SessionType::Practice => {
                // No lap counter for qualifying/practice — session name is enough context
                String::new()
            }
            _ => {
                if s.current_lap == 0 {
                    if s.session_started {
                        return "Formation Lap".to_string();
                    }
                    if let Some(fl_time) = s.formation_lap_at
                        && self.clock.now() >= fl_time
                    {
                        return "Formation Lap".to_string();
                    }
                    return "Not Started".to_string();
                }
                match s.total_laps {
                    Some(total) => format!("Lap {}/{}", s.current_lap, total),
                    None => format!("Lap {}", s.current_lap),
                }
            }
        }
    }
}
