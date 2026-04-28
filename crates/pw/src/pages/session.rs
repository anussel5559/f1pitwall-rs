use anyhow::Result;
use chrono::DateTime;
use ratatui::prelude::*;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::input::{LocationTask, TelemetryTask, handle_input};
use crate::{app, bootstrap, session_types::SessionType, ui};
use app::{AppState, SessionClock, ViewMode};
use f1core::{api, db, polling};

pub async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    session_key: i64,
    speed: f64,
    client: &Arc<api::OpenF1Client>,
    db: &Arc<Mutex<db::Db>>,
) -> Result<()> {
    let sk_str = session_key.to_string();
    let sessions = client.get_sessions(&[("session_key", &sk_str)]).await?;
    let s = sessions
        .first()
        .ok_or_else(|| anyhow::anyhow!("Session {} not found", session_key))?;

    {
        let db_lock = db.lock().unwrap();
        db_lock.upsert_session(s)?;
    }

    let date_start_str = s
        .date_start
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No date_start"))?;
    let date_start = date_start_str.parse::<DateTime<chrono::Utc>>()?;
    let gmt_offset = s.gmt_offset.clone();
    let session_type = SessionType::from_api_str(s.session_type.as_deref().unwrap_or("Race"))
        .unwrap_or(SessionType::Race);

    let is_live = s.date_end.is_none();
    let clock = SessionClock::new(date_start, speed, gmt_offset.as_deref(), is_live);
    let toasts: app::Toasts = Arc::new(Mutex::new(Vec::new()));
    let bootstrap_status = bootstrap::new_status();

    // Auto-resume from saved position if available
    if !clock.is_live
        && let Ok(Some(pos)) = db.lock().unwrap().get_replay_position(session_key)
        && let Ok(ts) = pos.parse::<DateTime<chrono::Utc>>()
    {
        let elapsed = ts - date_start;
        let mins = elapsed.num_minutes();
        let secs = elapsed.num_seconds() % 60;
        app::push_toast(
            &toasts,
            format!("Resuming at {}:{:02} into session", mins, secs),
            false,
        );
        clock.resume_from(ts);
    }

    let clock = Arc::new(clock);

    // Start polling
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let poll_db = db.clone();
    let poll_client = client.clone();
    let poll_toasts = toasts.clone();
    let poll_clock = clock.clone();
    let meeting_key = s.meeting_key;
    tokio::spawn(async move {
        polling::run_polling(
            session_key,
            meeting_key,
            session_type,
            poll_client,
            poll_db,
            poll_clock,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            poll_toasts,
            stop_rx,
        )
        .await;
    });

    // For replays, kick off the chunked per-driver car_data + location bootstrap
    // (idempotent via completeness check) so map/telemetry can serve from DB.
    // The bootstrap reads the drivers list from the DB on entry, so we wait for
    // `run_polling`'s session-type bootstrap (above) to populate it first —
    // otherwise the empty-drivers early return makes the whole thing a no-op
    // and the session never accumulates car_data / location.
    if !clock.is_live {
        let bootstrap_db = db.clone();
        let bootstrap_client = client.clone();
        let bootstrap_toasts = toasts.clone();
        let bootstrap_status = bootstrap_status.clone();
        tokio::spawn(async move {
            for _ in 0..60 {
                let has_drivers = bootstrap_db
                    .lock()
                    .unwrap()
                    .get_driver_numbers(session_key)
                    .map(|d| !d.is_empty())
                    .unwrap_or(false);
                if has_drivers {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            bootstrap::run(
                session_key,
                bootstrap_client,
                bootstrap_db,
                bootstrap_toasts,
                bootstrap_status,
            )
            .await;
        });
    }

    let authenticated = client.is_authenticated().await;
    let mut state = AppState::new(session_key, session_type, toasts, clock, authenticated);
    state.bootstrap_status = bootstrap_status;

    let result = run_event_loop(terminal, &mut state, db, client, session_key);

    let _ = stop_tx.send(true);

    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut AppState,
    db: &Arc<Mutex<db::Db>>,
    client: &Arc<api::OpenF1Client>,
    session_key: i64,
) -> Result<()> {
    let mut telem_task = TelemetryTask::new();
    let mut loc_task = LocationTask::new();

    loop {
        if handle_input(
            state,
            Duration::from_millis(0),
            &mut telem_task,
            client,
            db,
            session_key,
        )? {
            telem_task.stop();
            loc_task.stop();
            return Ok(());
        }

        state.refresh(db)?;

        // Update telemetry state from board data each frame
        if let ViewMode::Telemetry { driver_number } = state.view_mode
            && let Some(ref shared) = telem_task.shared
        {
            let new_lap_start = state.driver_lap_start(driver_number);
            let clock_now = state.clock.ceiling();
            let lap_boundaries: Vec<(i64, String)> = db
                .lock()
                .unwrap()
                .get_driver_lap_starts(session_key, driver_number, &clock_now)
                .unwrap_or_default()
                .into_iter()
                .map(|(lap, date_start, _, _, _, _)| (lap, date_start))
                .collect();

            let mut ts = shared.lock().unwrap();
            let mut needs_recompute = false;
            if ts.lap_start != new_lap_start {
                ts.lap_start = new_lap_start;
                needs_recompute = true;
            }
            if ts.lap_boundaries != lap_boundaries {
                ts.lap_boundaries = lap_boundaries;
                needs_recompute = true;
            }
            if needs_recompute {
                ts.recompute_charts();
            }
        }

        // Start/stop location polling based on track map state
        if state.view_mode == ViewMode::TrackMap && !state.selected_drivers.is_empty() {
            loc_task.update_drivers(state);
            loc_task.start(session_key, client, &state.clock, db, &state.toasts);
        } else {
            loc_task.stop();
        }

        // Read locations from DB when track map is active
        let locations =
            if state.view_mode == ViewMode::TrackMap && !state.selected_drivers.is_empty() {
                let clock_now = state.clock.ceiling();
                let drivers: Vec<i64> = state.selected_drivers.iter().copied().collect();
                db.lock()
                    .unwrap()
                    .get_latest_locations(session_key, &drivers, &clock_now)
                    .unwrap_or_default()
            } else {
                Vec::new()
            };

        terminal.draw(|f| ui::draw(f, &mut *state, telem_task.shared.as_ref(), &locations))?;

        if handle_input(
            state,
            Duration::from_millis(100),
            &mut telem_task,
            client,
            db,
            session_key,
        )? {
            telem_task.stop();
            loc_task.stop();
            return Ok(());
        }
    }
}
