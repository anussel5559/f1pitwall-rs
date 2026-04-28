use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::app::{AppState, SessionClock, ViewMode};
use f1core::{db, telemetry};

/// Manages the lifecycle of the telemetry chart-refresh task.
pub struct TelemetryTask {
    stop_tx: Option<tokio::sync::watch::Sender<bool>>,
    pub shared: Option<telemetry::SharedTelemetry>,
}

impl TelemetryTask {
    pub fn new() -> Self {
        Self {
            stop_tx: None,
            shared: None,
        }
    }

    /// Start refreshing telemetry for `driver_number`. Stops any existing task first.
    pub fn start(
        &mut self,
        session_key: i64,
        driver_number: i64,
        lap_start: Option<String>,
        clock: &Arc<SessionClock>,
        db: &Arc<Mutex<db::Db>>,
    ) {
        self.stop();

        let mut ts = telemetry::TelemetryState::new(driver_number);
        ts.lap_start = lap_start;
        let shared = Arc::new(std::sync::Mutex::new(ts));
        self.shared = Some(shared.clone());

        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        self.stop_tx = Some(stop_tx);

        let clock = clock.clone();
        let db = db.clone();
        tokio::spawn(async move {
            telemetry::run_telemetry_chart_refresh(session_key, clock, db, shared, stop_rx).await;
        });
    }

    /// Stop the current telemetry refresh task, if running.
    pub fn stop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(true);
        }
        self.shared = None;
    }
}

/// Switch telemetry to an adjacent driver (+1 = next, -1 = previous).
fn switch_telemetry_driver(
    state: &mut AppState,
    telem_task: &mut TelemetryTask,
    db: &Arc<Mutex<db::Db>>,
    session_key: i64,
    current_dn: i64,
    delta: isize,
) {
    if let Some(dn) = telemetry::cycle_driver(&state.session.rows, current_dn, delta) {
        let lap_start = state.driver_lap_start(dn);
        telem_task.start(session_key, dn, lap_start, &state.clock, db);
        state.view_mode = ViewMode::Telemetry { driver_number: dn };
    }
}

/// Handle seek input (left/right arrows), shared between Board and Telemetry views.
fn handle_seek(state: &AppState, shift: bool, direction: i64) {
    let secs = if shift { 60 } else { 10 };
    state
        .clock
        .seek(chrono::Duration::seconds(secs * direction));
}

pub fn handle_input(
    state: &mut AppState,
    timeout: Duration,
    telem_task: &mut TelemetryTask,
    db: &Arc<Mutex<db::Db>>,
    session_key: i64,
) -> Result<bool> {
    if event::poll(timeout)?
        && let Event::Key(key) = event::read()?
        && key.kind == KeyEventKind::Press
    {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        match &state.view_mode {
            ViewMode::Board => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
                KeyCode::Up | KeyCode::Char('k') => state.scroll_up(),
                KeyCode::Down | KeyCode::Char('j') => state.scroll_down(),
                KeyCode::Char('r') => state.toggle_race_control(),
                KeyCode::Char('t') => {
                    if let Some(dn) = state.selected_driver() {
                        let lap_start = state.driver_lap_start(dn);
                        telem_task.start(session_key, dn, lap_start, &state.clock, db);
                        state.view_mode = ViewMode::Telemetry { driver_number: dn };
                    }
                }
                KeyCode::Char('m') if state.authenticated || !state.clock.is_live => {
                    state.view_mode = ViewMode::TrackMap;
                }
                KeyCode::Char(' ') if state.authenticated || !state.clock.is_live => {
                    if let Some(dn) = state.selected_driver() {
                        state.toggle_selected_driver(dn);
                    }
                }
                KeyCode::Left => handle_seek(state, shift, -1),
                KeyCode::Right => handle_seek(state, shift, 1),
                KeyCode::Char('p') => {
                    state.clock.toggle_pause();
                }
                _ => {}
            },
            ViewMode::TrackMap => match key.code {
                KeyCode::Esc | KeyCode::Char('m') => {
                    state.view_mode = ViewMode::Board;
                }
                KeyCode::Char('q') => return Ok(true),
                KeyCode::Left => handle_seek(state, shift, -1),
                KeyCode::Right => handle_seek(state, shift, 1),
                KeyCode::Char('p') => {
                    state.clock.toggle_pause();
                }
                _ => {}
            },
            ViewMode::Telemetry { driver_number } => {
                let current_dn = *driver_number;
                match key.code {
                    KeyCode::Esc | KeyCode::Char('t') => {
                        telem_task.stop();
                        state.view_mode = ViewMode::Board;
                    }
                    KeyCode::Char('q') => return Ok(true),
                    KeyCode::Down | KeyCode::Char('j') => {
                        switch_telemetry_driver(
                            state,
                            telem_task,
                            db,
                            session_key,
                            current_dn,
                            1,
                        );
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        switch_telemetry_driver(
                            state,
                            telem_task,
                            db,
                            session_key,
                            current_dn,
                            -1,
                        );
                    }
                    KeyCode::Char('h') => {
                        if let Some(ref shared) = telem_task.shared {
                            let secs = if shift { 30.0 } else { 10.0 };
                            shared.lock().unwrap().scroll_back(secs);
                        }
                    }
                    KeyCode::Char('l') => {
                        if let Some(ref shared) = telem_task.shared {
                            let secs = if shift { 30.0 } else { 10.0 };
                            shared.lock().unwrap().scroll_forward(secs);
                        }
                    }
                    KeyCode::Char('0') => {
                        if let Some(ref shared) = telem_task.shared {
                            shared.lock().unwrap().scroll_to_live();
                        }
                    }
                    KeyCode::Left => handle_seek(state, shift, -1),
                    KeyCode::Right => handle_seek(state, shift, 1),
                    KeyCode::Char('p') => {
                        state.clock.toggle_pause();
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(false)
}
