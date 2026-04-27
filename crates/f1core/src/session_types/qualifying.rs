use anyhow::Result;
use std::sync::{Arc, Mutex};

use super::Endpoint;
use crate::api::OpenF1Client;
use crate::db::Db;
use crate::polling;
use crate::toast::{Toasts, push_toast};

pub type Cursors = std::collections::HashMap<&'static str, String>;

pub async fn bootstrap(
    session_key: i64,
    client: &OpenF1Client,
    db: &Arc<Mutex<Db>>,
    toasts: &Toasts,
    cursors: &mut Cursors,
) -> Result<()> {
    // 1. Drivers (needed for board query to return anything) — skip if cached
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

    // 2. Fetch all data endpoints (no PitStops or StartingGrid for qualifying).
    // Always fetch everything (no ceiling) so replay has full data.
    let endpoints = [
        Endpoint::Laps,
        Endpoint::Stints,
        Endpoint::RaceControl,
        Endpoint::Weather,
    ];

    for ep in endpoints {
        if let Err(e) =
            polling::fetch_endpoint(session_key, ep, None, None, client, db, cursors).await
        {
            push_toast(toasts, format!("Init {}: {e}", ep.name()), true);
        }
    }

    Ok(())
}
