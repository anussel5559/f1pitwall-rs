//! Replay-session bootstrap that pre-loads car_data + location into SQLite.
//!
//! Each chunk is a single all-drivers request (no `driver_number` filter), which
//! is dramatically more efficient than fetching per driver: a 2-hour race goes
//! from ~176 requests to ~16. OpenF1 caps the time range for these all-drivers
//! requests with a 422 "too much data" once you go beyond ~15 minutes, so chunks
//! stay at 15 minutes — that bound also keeps each response under the 10s
//! reqwest timeout (15min ≈ 12MB ≈ 6-7s typical).
//!
//! Both the lower (`date>=`) and upper (`date<=`) bound are always supplied;
//! requests without a lower bound let OpenF1 walk back through pre-session
//! samples and have been observed to time out or 422.
//!
//! Lives in `pw` rather than `f1core` so the all-drivers strategy doesn't leak
//! into other consumers of the library.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use f1core::api::OpenF1Client;
use f1core::util::time::{fmt_ts, parse_ts};
use f1core::db::Db;
use f1core::toast::{Toasts, push_toast};

/// Shared progress state surfaced to the UI as a loading spinner. `None` means
/// no bootstrap is running (or it's already done); `Some(_)` is rendered as a
/// `Loading N/M` overlay with a spinner.
pub type Status = Arc<Mutex<Option<Progress>>>;

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub completed: usize,
    pub total: usize,
}

pub fn new_status() -> Status {
    Arc::new(Mutex::new(None))
}

/// 15-minute chunks: largest window OpenF1 will return for all-driver requests
/// without 422'ing, and stays comfortably under the 10s reqwest timeout.
const CHUNK_SECS: i64 = 900;

/// Per-chunk retry budget. The dominant failure mode is transient network /
/// API hiccups ("error decoding response body", occasional 5xx) — a couple of
/// quick retries with backoff turns those into eventually-consistent loads
/// rather than permanent gaps.
const CHUNK_RETRIES: usize = 3;

/// Pre-load car_data + location for every driver in a replay session, scoped to
/// `(date_start, date_end)`. Idempotent across reopens via the upsert-on-conflict
/// in `upsert_car_data` / `upsert_location`.
pub async fn run(
    session_key: i64,
    client: Arc<OpenF1Client>,
    db: Arc<Mutex<Db>>,
    toasts: Toasts,
    status: Status,
) {
    let (drivers, bounds) = {
        let db = db.lock().unwrap();
        let drivers = match db.get_driver_numbers(session_key) {
            Ok(d) => d,
            Err(e) => {
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
        return;
    };
    let (Some(start_ts), Some(end_ts)) = (parse_ts(&date_start), parse_ts(&date_end)) else {
        return;
    };

    // If every driver is already complete on both endpoints, there's nothing to do.
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

    let chunks_list = chunks(start_ts, end_ts);
    let total = chunks_list.len();
    *status.lock().unwrap() = Some(Progress {
        completed: 0,
        total,
    });

    for (idx, (from, to)) in chunks_list.iter().enumerate() {
        // Fetch both endpoints for this window in parallel — they share the
        // OpenF1 client's rate limiter, so this just lets two requests be
        // in-flight at once instead of strictly serializing.
        tokio::join!(
            fetch_car_data_chunk(session_key, from, to, &client, &db, &toasts),
            fetch_location_chunk(session_key, from, to, &client, &db, &toasts),
        );
        *status.lock().unwrap() = Some(Progress {
            completed: idx + 1,
            total,
        });
    }

    *status.lock().unwrap() = None;
}

async fn fetch_car_data_chunk(
    session_key: i64,
    from: &str,
    to: &str,
    client: &OpenF1Client,
    db: &Arc<Mutex<Db>>,
    toasts: &Toasts,
) {
    let Some(rows) = fetch_with_retry(CHUNK_RETRIES, || {
        client.get_car_data_all_drivers(session_key, Some(from), Some(to))
    })
    .await
    else {
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
    let Some(rows) = fetch_with_retry(CHUNK_RETRIES, || {
        client.get_location(session_key, &[], Some(from), None, Some(to))
    })
    .await
    else {
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
            push_toast(toasts, format!("location upsert: {e}"), true);
        }
        let _ = db.commit();
    }
}

/// Yield `(from, to)` RFC3339 pairs walking `start..end` in `CHUNK_SECS` steps.
/// First chunk's lower bound is exactly `date_start` so we never issue a request
/// that omits the lower bound.
fn chunks(start: DateTime<Utc>, end: DateTime<Utc>) -> Vec<(String, String)> {
    let chunk = chrono::Duration::seconds(CHUNK_SECS);
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
/// Backs off 1s/2s/4s/... between tries so a brief API hiccup doesn't
/// permanently leave a hole in a chunk's data.
async fn fetch_with_retry<T, F, Fut>(attempts: usize, mut op: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    for attempt in 0..attempts {
        match op().await {
            Ok(v) => return Some(v),
            Err(_) if attempt + 1 < attempts => {
                let backoff = std::time::Duration::from_secs(1u64 << attempt);
                tokio::time::sleep(backoff).await;
            }
            Err(_) => return None,
        }
    }
    None
}
