use anyhow::Result;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::api::OpenF1Client;
use crate::clock::SessionClock;
use crate::db::Db;
use crate::session_types::{Endpoint, SessionType};
use crate::toast::{Toasts, push_toast};

/// How many drivers to fetch car_data + location for in parallel during replay bootstrap.
const TELEMETRY_BOOTSTRAP_CONCURRENCY: usize = 4;

#[allow(clippy::too_many_arguments)]
pub async fn run_polling(
    session_key: i64,
    meeting_key: i64,
    session_type: SessionType,
    client: Arc<OpenF1Client>,
    db: Arc<Mutex<Db>>,
    clock: Arc<SessionClock>,
    persist_high_rate: Arc<AtomicBool>,
    toasts: Toasts,
    stop: tokio::sync::watch::Receiver<bool>,
) {
    let mut cursors: std::collections::HashMap<&str, String> = std::collections::HashMap::new();

    // Bootstrap: fetch drivers + starting grid + one round of every endpoint
    // so the UI has data immediately instead of waiting for the round-robin.
    let bootstrap_result = match session_type {
        SessionType::Race | SessionType::Sprint => {
            crate::session_types::race::bootstrap(
                session_key,
                meeting_key,
                &client,
                &db,
                &clock,
                &toasts,
                &mut cursors,
            )
            .await
        }
        SessionType::Qualifying | SessionType::SprintQualifying => {
            crate::session_types::qualifying::bootstrap(
                session_key,
                &client,
                &db,
                &toasts,
                &mut cursors,
            )
            .await
        }
        SessionType::Practice => {
            crate::session_types::practice::bootstrap(
                session_key,
                &client,
                &db,
                &toasts,
                &mut cursors,
            )
            .await
        }
    };
    if let Err(e) = bootstrap_result {
        push_toast(&toasts, format!("Bootstrap: {e}"), true);
    }

    if clock.is_live {
        crate::mqtt::run_mqtt_streaming(session_key, client, db, persist_high_rate, toasts, stop)
            .await;
    } else {
        push_toast(
            &toasts,
            format!("Replaying from {}", clock.now().format("%H:%M:%S UTC")),
            false,
        );
        run_replay_idle(session_key, db, clock, stop).await;
    }
}

/// Pre-load car_data + location for every driver in a replay session, scoped to
/// the session window (`date_start`..`date_end`) so we don't pull pre-race
/// formation/grid samples we'd never display. Idempotent per driver — drivers
/// whose row counts already meet the completeness threshold are skipped.
pub async fn bootstrap_session_data(
    session_key: i64,
    client: Arc<OpenF1Client>,
    db: Arc<Mutex<Db>>,
    toasts: Toasts,
) {
    let (drivers, bounds) = {
        let db = db.lock().unwrap();
        let drivers = match db.get_driver_numbers(session_key) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!(session_key, error = %e, "bootstrap_session_data: enumerate drivers failed");
                push_toast(&toasts, format!("Session-data bootstrap: {e}"), true);
                return;
            }
        };
        let bounds = db
            .get_session_entry(session_key)
            .ok()
            .flatten()
            .map(|s| (s.date_start, s.date_end));
        (drivers, bounds)
    };
    if drivers.is_empty() {
        return;
    }

    let Some((date_start, Some(date_end))) = bounds else {
        tracing::warn!(
            session_key,
            "bootstrap_session_data: missing session date bounds, skipping"
        );
        return;
    };
    let date_start = Arc::new(date_start);
    let date_end = Arc::new(date_end);

    let mut set: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
    let mut iter = drivers.into_iter();

    for _ in 0..TELEMETRY_BOOTSTRAP_CONCURRENCY {
        if let Some(d) = iter.next() {
            spawn_driver_bootstrap(
                &mut set,
                session_key,
                d,
                client.clone(),
                db.clone(),
                date_start.clone(),
                date_end.clone(),
                toasts.clone(),
            );
        }
    }

    while set.join_next().await.is_some() {
        if let Some(d) = iter.next() {
            spawn_driver_bootstrap(
                &mut set,
                session_key,
                d,
                client.clone(),
                db.clone(),
                date_start.clone(),
                date_end.clone(),
                toasts.clone(),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_driver_bootstrap(
    set: &mut tokio::task::JoinSet<()>,
    session_key: i64,
    driver: i64,
    client: Arc<OpenF1Client>,
    db: Arc<Mutex<Db>>,
    date_start: Arc<String>,
    date_end: Arc<String>,
    toasts: Toasts,
) {
    set.spawn(async move {
        fetch_driver_car_data(
            session_key,
            driver,
            &client,
            &db,
            &date_start,
            &date_end,
            &toasts,
        )
        .await;
        fetch_driver_location(
            session_key,
            driver,
            &client,
            &db,
            &date_start,
            &date_end,
            &toasts,
        )
        .await;
    });
}

async fn fetch_driver_car_data(
    session_key: i64,
    driver: i64,
    client: &OpenF1Client,
    db: &Arc<Mutex<Db>>,
    date_start: &str,
    date_end: &str,
    toasts: &Toasts,
) {
    let already_complete = db
        .lock()
        .unwrap()
        .car_data_complete(session_key, driver)
        .unwrap_or(false);
    if already_complete {
        return;
    }

    let rows = match client
        .get_car_data(session_key, driver, Some(date_start), None, Some(date_end))
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(session_key, driver, error = %e, "bootstrap car_data fetch failed");
            push_toast(toasts, format!("car_data #{driver}: {e}"), true);
            return;
        }
    };
    if rows.is_empty() {
        return;
    }

    let db = db.lock().unwrap();
    if db.begin().is_ok() {
        if let Err(e) = db.upsert_car_data(session_key, &rows) {
            tracing::error!(session_key, driver, error = %e, "bootstrap car_data upsert failed");
            push_toast(toasts, format!("car_data #{driver} upsert: {e}"), true);
        }
        let _ = db.commit();
    }
}

async fn fetch_driver_location(
    session_key: i64,
    driver: i64,
    client: &OpenF1Client,
    db: &Arc<Mutex<Db>>,
    date_start: &str,
    date_end: &str,
    toasts: &Toasts,
) {
    let already_complete = db
        .lock()
        .unwrap()
        .location_complete(session_key, driver)
        .unwrap_or(false);
    if already_complete {
        return;
    }

    let rows = match client
        .get_location(
            session_key,
            &[driver],
            Some(date_start),
            None,
            Some(date_end),
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(session_key, driver, error = %e, "bootstrap location fetch failed");
            push_toast(toasts, format!("location #{driver}: {e}"), true);
            return;
        }
    };
    if rows.is_empty() {
        return;
    }

    let db = db.lock().unwrap();
    if db.begin().is_ok() {
        if let Err(e) = db.upsert_location(session_key, &rows) {
            tracing::error!(session_key, driver, error = %e, "bootstrap location upsert failed");
            push_toast(toasts, format!("location #{driver} upsert: {e}"), true);
        }
        let _ = db.commit();
    }
}

/// Lightweight loop for replay sessions.
///
/// After bootstrap pre-loads all session data, no further API calls are needed.
/// This loop just periodically saves the replay position so the user can resume later.
async fn run_replay_idle(
    session_key: i64,
    db: Arc<Mutex<Db>>,
    clock: Arc<SessionClock>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let mut save_counter: u32 = 0;

    loop {
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {},
            _ = stop.changed() => break,
        }

        save_counter += 1;
        // Save replay position every ~5 cycles (~15s)
        if save_counter.is_multiple_of(5) {
            let ts = clock.now().to_rfc3339();
            let _ = db.lock().unwrap().save_replay_position(session_key, &ts);
        }
    }

    // Save final position on shutdown
    let ts = clock.now().to_rfc3339();
    let _ = db.lock().unwrap().save_replay_position(session_key, &ts);
}

fn max_date<'a>(dates: impl Iterator<Item = Option<&'a String>>) -> Option<String> {
    dates.flatten().max().cloned()
}

/// Persist a batch of API results: optionally update cursor, then upsert all items in a transaction.
fn persist_batch<'a, T>(
    data: &[T],
    db: &Arc<Mutex<Db>>,
    cursors: &mut std::collections::HashMap<&'a str, String>,
    cursor_key: Option<&'a str>,
    date_extractor: impl Fn(&T) -> Option<&String>,
    upsert: impl Fn(&Db, &T) -> Result<()>,
) -> Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    if let Some(key) = cursor_key
        && let Some(ts) = max_date(data.iter().map(&date_extractor))
    {
        cursors.insert(key, ts);
    }
    let db = db.lock().unwrap();
    db.begin()?;
    for item in data {
        upsert(&db, item)?;
    }
    db.commit()?;
    Ok(())
}

pub async fn fetch_endpoint(
    session_key: i64,
    endpoint: Endpoint,
    cursor: Option<&str>,
    ceiling: Option<&str>,
    client: &OpenF1Client,
    db: &Arc<Mutex<Db>>,
    cursors: &mut std::collections::HashMap<&str, String>,
) -> Result<()> {
    match endpoint {
        Endpoint::Drivers => {
            let data = client.get_drivers(session_key).await?;
            persist_batch(
                &data,
                db,
                cursors,
                None,
                |_| None,
                |db, d| db.upsert_driver(d),
            )?;
        }
        Endpoint::Laps => {
            let data = client.get_laps(session_key, cursor, ceiling).await?;
            persist_batch(
                &data,
                db,
                cursors,
                Some("laps"),
                |l| l.date_start.as_ref(),
                |db, l| db.upsert_lap(session_key, l),
            )?;
        }
        Endpoint::Position => {
            let data = client.get_positions(session_key, cursor, ceiling).await?;
            persist_batch(
                &data,
                db,
                cursors,
                Some("position"),
                |p| p.date.as_ref(),
                |db, p| db.upsert_position(session_key, p),
            )?;
        }
        Endpoint::Intervals => {
            let data = client.get_intervals(session_key, cursor, ceiling).await?;
            persist_batch(
                &data,
                db,
                cursors,
                Some("intervals"),
                |i| i.date.as_ref(),
                |db, i| db.upsert_interval(session_key, i),
            )?;
        }
        Endpoint::Stints => {
            let data = client.get_stints(session_key).await?;
            persist_batch(
                &data,
                db,
                cursors,
                None,
                |_| None,
                |db, s| db.upsert_stint(session_key, s),
            )?;
        }
        Endpoint::PitStops => {
            let data = client.get_pit_stops(session_key).await?;
            persist_batch(
                &data,
                db,
                cursors,
                None,
                |_| None,
                |db, p| db.upsert_pit_stop(session_key, p),
            )?;
        }
        Endpoint::RaceControl => {
            let data = client
                .get_race_control(session_key, cursor, ceiling)
                .await?;
            persist_batch(
                &data,
                db,
                cursors,
                Some("race_control"),
                |rc| rc.date.as_ref(),
                |db, rc| db.upsert_race_control(session_key, rc),
            )?;
        }
        Endpoint::Weather => {
            let data = client.get_weather(session_key, ceiling).await?;
            persist_batch(
                &data,
                db,
                cursors,
                None,
                |_| None,
                |db, w| db.upsert_weather(session_key, w),
            )?;
        }
    }

    Ok(())
}
