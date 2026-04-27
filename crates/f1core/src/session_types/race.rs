use anyhow::Result;
use std::sync::{Arc, Mutex};

use super::Endpoint;
use crate::api::OpenF1Client;
use crate::clock::SessionClock;
use crate::db::Db;
use crate::polling;
use crate::toast::{Toasts, push_toast};

pub type Cursors = std::collections::HashMap<&'static str, String>;

pub async fn bootstrap(
    session_key: i64,
    meeting_key: i64,
    client: &OpenF1Client,
    db: &Arc<Mutex<Db>>,
    clock: &SessionClock,
    toasts: &Toasts,
    cursors: &mut Cursors,
) -> Result<()> {
    // 1. Starting grid -> seed positions so drivers appear in grid order
    match client.get_starting_grid(meeting_key).await {
        Ok(grid) if !grid.is_empty() => {
            let db = db.lock().unwrap();
            db.begin()?;
            for entry in &grid {
                db.upsert_position_if_missing(session_key, entry.driver_number, entry.position)
                    .ok();
                db.upsert_starting_grid(session_key, entry.driver_number, entry.position)
                    .ok();
            }
            db.commit()?;
        }
        Ok(_) => {}
        Err(e) => push_toast(toasts, format!("Starting grid: {e}"), true),
    }

    // 2. Drivers (needed for board query to return anything) — skip if cached
    let has_drivers = { db.lock().unwrap().has_drivers(session_key)? };
    if !has_drivers {
        let drivers = client.get_drivers(session_key).await?;
        let db = db.lock().unwrap();
        db.begin()?;
        for d in &drivers {
            db.upsert_driver(d)?;
        }
        db.commit()?;
    }

    // 3. Fetch all data endpoints.
    // For replays, fetch everything (no ceiling) so the full session is cached locally
    // and seeking is instant. For live, fetch up to current time.
    let ceiling = if clock.is_live {
        Some(clock.ceiling())
    } else {
        None
    };
    let endpoints = [
        Endpoint::Position,
        Endpoint::Intervals,
        Endpoint::Laps,
        Endpoint::Stints,
        Endpoint::PitStops,
        Endpoint::RaceControl,
        Endpoint::Weather,
    ];

    for ep in endpoints {
        if let Err(e) = polling::fetch_endpoint(
            session_key,
            ep,
            None,
            ceiling.as_deref(),
            client,
            db,
            cursors,
        )
        .await
        {
            push_toast(toasts, format!("Init {}: {e}", ep.name()), true);
        }
    }

    Ok(())
}
