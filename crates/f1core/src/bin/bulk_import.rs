//! Bulk-imports historical race/sprint session data from the OpenF1 API.
//!
//! Reuses the same bootstrap functions the replay system uses, so the imported
//! data is identical to what you'd get by opening each session in the app.
//!
//! Usage:
//!   cargo run --bin bulk-import -- --years 2024,2025
//!   cargo run --bin bulk-import -- --years 2024 --username $U --password $P

use anyhow::Result;
use chrono::Utc;
use f1core::api::OpenF1Client;
use f1core::auth::Credentials;
use f1core::clock::SessionClock;
use f1core::db::Db;
use f1core::session_types::SessionType;
use f1core::toast::Toasts;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(clap::Parser)]
#[command(name = "bulk-import", about = "Bulk-import historical F1 session data")]
struct Args {
    /// Comma-separated years to import, e.g. "2024,2025"
    #[arg(long)]
    years: String,

    /// Path to SQLite database file
    #[arg(long, default_value_os_t = f1core::db::default_db_path())]
    db: PathBuf,

    /// OpenF1 username (optional, faster rate limit when authenticated)
    #[arg(long, env = "OPENF1_USERNAME")]
    username: Option<String>,

    /// OpenF1 password (required if username is set)
    #[arg(long, env = "OPENF1_PASSWORD")]
    password: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = <Args as clap::Parser>::parse();

    let credentials = match (args.username, args.password) {
        (Some(u), Some(p)) => Some(Credentials::new(u, p)),
        (None, None) => None,
        _ => anyhow::bail!("Both --username and --password must be provided together"),
    };

    let db = Db::open(&args.db)?;
    let db = Arc::new(Mutex::new(db));
    let client = OpenF1Client::new(credentials).await?;

    let authenticated = client.is_authenticated().await;
    println!(
        "bulk-import: db={} auth={}",
        args.db.display(),
        if authenticated {
            "yes"
        } else {
            "no (public rate limit)"
        }
    );

    let years: Vec<&str> = args.years.split(',').map(|s| s.trim()).collect();

    // Collect all race/sprint sessions across requested years
    let mut sessions_to_import = Vec::new();

    for year in &years {
        println!("Fetching session list for {}...", year);
        let sessions = client.get_sessions(&[("year", year)]).await?;

        for session in sessions {
            let session_type_str = session.session_type.as_deref().unwrap_or("");
            let session_type = SessionType::from_api_str(session_type_str);

            // Only import races and sprints — those are what we train on
            let is_target = matches!(session_type, Some(SessionType::Race | SessionType::Sprint));
            if !is_target {
                continue;
            }

            // Skip if already imported
            let already_cached = db.lock().unwrap().has_drivers(session.session_key)?;
            if already_cached {
                continue;
            }

            sessions_to_import.push(session);
        }
    }

    if sessions_to_import.is_empty() {
        println!("Nothing to import — all sessions already cached.");
        return Ok(());
    }

    println!(
        "Importing {} sessions (Ctrl-C to stop, progress is saved)...\n",
        sessions_to_import.len()
    );

    // No-op toast sink — bootstrap functions log errors through this but we just print them
    let toasts: Toasts = Arc::new(Mutex::new(Vec::new()));

    for (i, session) in sessions_to_import.iter().enumerate() {
        let name = session.session_name.as_deref().unwrap_or("?");
        let circuit = session.circuit_short_name.as_deref().unwrap_or("?");
        let country = session.country_name.as_deref().unwrap_or("?");
        let stype = session.session_type.as_deref().unwrap_or("?");

        println!(
            "[{}/{}] {} {} — {} ({}) [sk={}]",
            i + 1,
            sessions_to_import.len(),
            country,
            circuit,
            name,
            stype,
            session.session_key,
        );

        // Store session metadata first
        db.lock().unwrap().upsert_session(session)?;

        // Create a dummy replay clock — bootstrap only uses it to decide `ceiling`
        // (None for replay = fetch everything), so the actual time doesn't matter.
        let clock = SessionClock::new(
            session
                .date_start
                .as_deref()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(Utc::now),
            1.0,
            session.gmt_offset.as_deref(),
            false, // is_live = false → ceiling will be None → fetches all data
        );

        let mut cursors: HashMap<&str, String> = HashMap::new();

        let result = f1core::session_types::race::bootstrap(
            session.session_key,
            session.meeting_key,
            &client,
            &db,
            &clock,
            &toasts,
            &mut cursors,
        )
        .await;

        // Drain toasts and print any errors
        {
            let mut t = toasts.lock().unwrap();
            for toast in t.drain(..) {
                if toast.is_error {
                    eprintln!("  warn: {}", toast.message);
                }
            }
        }

        if let Err(e) = result {
            eprintln!("  ERROR: {e}");
        }
    }

    println!("\nDone.");
    Ok(())
}
