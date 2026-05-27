use anyhow::Result;
use chrono::{DateTime, Utc};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::api::OpenF1Client;
use crate::clock::SessionClock;
use crate::db::Db;
use crate::session_types::{Endpoint, SessionType};
use crate::toast::{Toasts, push_toast};
use crate::util::time::{fmt_ts, parse_ts};

/// Replay-bootstrap chunk size. The largest window OpenF1 will return for
/// all-driver `car_data` / `location` requests before 422'ing on payload size,
/// and small enough that each response stays comfortably under the 10 s
/// `reqwest` timeout (15 min of `car_data` ≈ 12 MB ≈ 6–7 s typical).
const BOOTSTRAP_CHUNK_SECS: i64 = 900;

/// Per-chunk retry budget. Dominant failure mode is transient network / API
/// hiccups (`error decoding response body`, occasional 5xx); a couple of
/// quick retries with exponential backoff turns those into eventually-
/// consistent loads rather than permanent gaps.
const BOOTSTRAP_CHUNK_RETRIES: usize = 3;

/// Progress signal for the replay bootstrap. Emitted via the callback passed
/// to [`bootstrap_session_data_with_progress`] after each chunk completes,
/// suitable for driving a `Loading N/M` overlay.
#[derive(Debug, Clone, Copy)]
pub struct BootstrapProgress {
    pub completed: usize,
    pub total: usize,
}

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

/// Pre-load `car_data` + `location` for every driver in a replay session,
/// scoped to the session window (`date_start..date_end`) so we don't pull
/// pre-race formation/grid samples we'd never display.
///
/// Implementation note: each request covers a fixed 15-minute window for *all*
/// drivers (no `driver_number` filter), rather than per-driver full-race
/// requests. For a 2 h race that's ~16 total requests instead of ~88, and each
/// response stays well under `reqwest`'s 10 s timeout — per-driver full-race
/// `car_data` fetches (~3.9 MB) reliably time out under any concurrency on
/// modestly-bandwidth-constrained connections.
///
/// Idempotent across reopens: a quick check up front skips the entire fetch
/// loop when every driver already meets the per-table completeness threshold
/// (`car_data_complete` / `location_complete`). Mid-loop reopens are safe too,
/// since `upsert_car_data` / `upsert_location` use `INSERT OR IGNORE` on a PK
/// that includes `date`.
pub async fn bootstrap_session_data(
    session_key: i64,
    client: Arc<OpenF1Client>,
    db: Arc<Mutex<Db>>,
    toasts: Toasts,
) {
    bootstrap_session_data_with_progress(session_key, client, db, toasts, |_| {}).await
}

/// [`bootstrap_session_data`] with a progress callback fired after each chunk
/// completes. Used by TUI replay sessions to drive the `Loading N/M` overlay;
/// web callers that don't surface bootstrap progress should call the plain
/// [`bootstrap_session_data`] above.
pub async fn bootstrap_session_data_with_progress<F>(
    session_key: i64,
    client: Arc<OpenF1Client>,
    db: Arc<Mutex<Db>>,
    toasts: Toasts,
    mut on_progress: F,
) where
    F: FnMut(BootstrapProgress) + Send,
{
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
    let (Some(start_ts), Some(end_ts)) = (parse_ts(&date_start), parse_ts(&date_end)) else {
        tracing::warn!(
            session_key,
            "bootstrap_session_data: unparseable session date bounds, skipping"
        );
        return;
    };

    // Fast path: every driver already complete on both endpoints. Avoids
    // walking the chunk list (and the per-chunk no-op upserts) on reopen.
    let all_complete = {
        let db = db.lock().unwrap();
        drivers.iter().all(|d| {
            db.car_data_complete(session_key, *d).unwrap_or(false)
                && db.location_complete(session_key, *d).unwrap_or(false)
        })
    };
    if all_complete {
        return;
    }

    let chunks_list = bootstrap_chunks(start_ts, end_ts);
    let total = chunks_list.len();
    on_progress(BootstrapProgress {
        completed: 0,
        total,
    });

    for (idx, (from, to)) in chunks_list.iter().enumerate() {
        // car_data + location for this window are independent — let them be
        // in-flight together. The OpenF1 client's rate limiter still serialises
        // at the wire level, so this just hides one request's round-trip
        // behind the other rather than uncapping concurrency.
        tokio::join!(
            fetch_car_data_chunk(session_key, from, to, &client, &db, &toasts),
            fetch_location_chunk(session_key, from, to, &client, &db, &toasts),
        );
        on_progress(BootstrapProgress {
            completed: idx + 1,
            total,
        });
    }
}

async fn fetch_car_data_chunk(
    session_key: i64,
    from: &str,
    to: &str,
    client: &OpenF1Client,
    db: &Arc<Mutex<Db>>,
    toasts: &Toasts,
) {
    let Some(rows) = fetch_with_retry(BOOTSTRAP_CHUNK_RETRIES, || {
        client.get_car_data_all_drivers(session_key, Some(from), Some(to))
    })
    .await
    else {
        tracing::error!(
            session_key,
            from,
            "bootstrap car_data chunk failed after retries"
        );
        push_toast(
            toasts,
            format!("car_data {from}: chunk failed after retries"),
            true,
        );
        return;
    };
    if rows.is_empty() {
        return;
    }
    let db = db.lock().unwrap();
    if db.begin().is_ok() {
        if let Err(e) = db.upsert_car_data(session_key, &rows) {
            tracing::error!(session_key, error = %e, "bootstrap car_data upsert failed");
            push_toast(toasts, format!("car_data upsert: {e}"), true);
        }
        let _ = db.commit();
    }
}

async fn fetch_location_chunk(
    session_key: i64,
    from: &str,
    to: &str,
    client: &OpenF1Client,
    db: &Arc<Mutex<Db>>,
    toasts: &Toasts,
) {
    // Empty driver slice → no `driver_number` filter → all-drivers response.
    let Some(rows) = fetch_with_retry(BOOTSTRAP_CHUNK_RETRIES, || {
        client.get_location(session_key, &[], Some(from), None, Some(to))
    })
    .await
    else {
        tracing::error!(
            session_key,
            from,
            "bootstrap location chunk failed after retries"
        );
        push_toast(
            toasts,
            format!("location {from}: chunk failed after retries"),
            true,
        );
        return;
    };
    if rows.is_empty() {
        return;
    }
    let db = db.lock().unwrap();
    if db.begin().is_ok() {
        if let Err(e) = db.upsert_location(session_key, &rows) {
            tracing::error!(session_key, error = %e, "bootstrap location upsert failed");
            push_toast(toasts, format!("location upsert: {e}"), true);
        }
        let _ = db.commit();
    }
}

/// Yield `(from, to)` RFC3339 pairs walking `start..end` in `BOOTSTRAP_CHUNK_SECS`
/// steps. The first chunk's lower bound is exactly `date_start` so we never
/// issue an open-ended request (OpenF1 has been observed to time out or 422
/// when the lower bound is omitted).
fn bootstrap_chunks(start: DateTime<Utc>, end: DateTime<Utc>) -> Vec<(String, String)> {
    let chunk = chrono::Duration::seconds(BOOTSTRAP_CHUNK_SECS);
    let mut out = Vec::new();
    let mut cursor = start;
    while cursor < end {
        let next = (cursor + chunk).min(end);
        out.push((fmt_ts(cursor), fmt_ts(next)));
        cursor = next;
    }
    out
}

/// Run `op` up to `attempts` times, returning `Some(value)` on first success.
/// Backs off 1 s / 2 s / 4 s / … between tries so a brief API hiccup doesn't
/// permanently leave a hole in a chunk's data.
async fn fetch_with_retry<T, F, Fut>(attempts: usize, mut op: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    for attempt in 0..attempts {
        match op().await {
            Ok(v) => return Some(v),
            Err(e) if attempt + 1 < attempts => {
                let backoff = std::time::Duration::from_secs(1u64 << attempt);
                tracing::warn!(error = %e, attempt = attempt + 1, ?backoff, "bootstrap chunk retry");
                tokio::time::sleep(backoff).await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "bootstrap chunk gave up after retries");
                return None;
            }
        }
    }
    None
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

#[cfg(test)]
mod bootstrap_chunks_tests {
    use super::*;
    use chrono::TimeZone;

    fn dt(secs_from_epoch: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs_from_epoch, 0).unwrap()
    }

    #[test]
    fn splits_full_window_into_15min_chunks() {
        // 2-hour race → 8 fifteen-minute chunks.
        let start = dt(0);
        let end = dt(2 * 60 * 60);
        let out = bootstrap_chunks(start, end);
        assert_eq!(out.len(), 8);
        // First chunk starts at start, last chunk ends at end.
        assert_eq!(out[0].0, fmt_ts(start));
        assert_eq!(out[7].1, fmt_ts(end));
    }

    #[test]
    fn final_chunk_is_clamped_to_end() {
        // 20-minute window: one full 15-min chunk then a 5-min tail.
        let start = dt(0);
        let end = dt(20 * 60);
        let out = bootstrap_chunks(start, end);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], (fmt_ts(dt(0)), fmt_ts(dt(15 * 60))));
        assert_eq!(out[1], (fmt_ts(dt(15 * 60)), fmt_ts(dt(20 * 60))));
    }

    #[test]
    fn sub_chunk_window_yields_single_chunk() {
        let start = dt(0);
        let end = dt(5 * 60);
        let out = bootstrap_chunks(start, end);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], (fmt_ts(start), fmt_ts(end)));
    }

    #[test]
    fn empty_window_yields_no_chunks() {
        // Defensive: bootstrap should be a no-op when the session has
        // collapsed bounds. The walk's `cursor < end` guard handles it.
        let t = dt(0);
        assert!(bootstrap_chunks(t, t).is_empty());
    }
}
