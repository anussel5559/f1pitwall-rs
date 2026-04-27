use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::api::OpenF1Client;
use crate::buffer::{FetchFrontier, fmt_ts};
use crate::clock::SessionClock;
use crate::db::Db;
use crate::toast::{Toasts, push_toast};

/// Background task that pre-fetches location data in 2-minute chunks into SQLite.
/// The render loop reads the latest positions from DB directly.
///
/// On replays, `bootstrap_session_data` already loads every driver's location
/// for the full session, so this task short-circuits to a stop-only wait.
pub async fn run_location_polling(
    session_key: i64,
    client: Arc<OpenF1Client>,
    clock: Arc<SessionClock>,
    db: Arc<Mutex<Db>>,
    drivers: Arc<Mutex<Vec<i64>>>,
    toasts: Toasts,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    if !clock.is_live {
        let _ = stop.changed().await;
        return;
    }

    let mut frontier = FetchFrontier::new();
    let mut last_drivers: Vec<i64> = Vec::new();

    loop {
        if *stop.borrow() {
            break;
        }

        let cycle_start = std::time::Instant::now();
        let now = clock.now();
        let driver_list = drivers.lock().unwrap().clone();

        frontier.check_seek(clock.seek_generation.load(Ordering::Relaxed));

        if driver_list != last_drivers {
            last_drivers = driver_list.clone();
            frontier.reset();
        }

        if !driver_list.is_empty()
            && let Some((from, to)) = frontier.next_chunk(now, 5)
        {
            let result = client
                .get_location(
                    session_key,
                    &driver_list,
                    Some(&fmt_ts(from)),
                    None,
                    Some(&fmt_ts(to)),
                )
                .await;

            match result {
                Ok(data) => {
                    if !data.is_empty() {
                        let db = db.lock().unwrap();
                        let _ = db.upsert_location(session_key, &data);
                    }
                    frontier.advance(to);
                }
                Err(e) => {
                    push_toast(&toasts, format!("location: {e}"), true);
                }
            }
        }

        let remaining = Duration::from_secs(3).saturating_sub(cycle_start.elapsed());
        tokio::select! {
            _ = tokio::time::sleep(remaining) => {},
            _ = stop.changed() => break,
        }
    }
}
