use anyhow::Result;
use chrono::DateTime;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::prelude::*;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{session_types::SessionType, ui};
use f1core::{api, db};
use ui::picker::PickerState;

pub enum PickerAction {
    Quit,
    Select {
        session_key: i64,
    },
    /// User logged in — caller should reinitialize the client with these credentials.
    Login(f1core::auth::Credentials),
    /// User logged out — caller should reinitialize the client without auth.
    Logout,
}

pub async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    client: &Arc<api::OpenF1Client>,
    db: &Arc<Mutex<db::Db>>,
    authenticated: bool,
) -> Result<PickerAction> {
    let mut picker = PickerState::new();
    picker.authenticated = authenticated;
    picker.paused = db
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_paused_sessions()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| SessionType::from_api_str(&e.session_type).is_some_and(|t| t.is_supported()))
        .collect();
    picker.update_total();

    // Start initial fetch and update check in background
    let mut pending_fetch = Some(spawn_fetch(picker.browse_year, client, db));
    let (update_tx, mut update_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = update_tx.send(crate::update::check_for_update().await);
    });

    loop {
        // Check if background fetch completed
        check_fetch(&mut pending_fetch, &mut picker);

        // Check if background update check completed
        if let Ok(result) = update_rx.try_recv() {
            picker.update_available = result;
        }

        terminal.draw(|f| ui::picker::render_picker(f, &picker))?;

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(PickerAction::Quit),
                KeyCode::Up | KeyCode::Char('k') => picker.move_up(),
                KeyCode::Down | KeyCode::Char('j') => picker.move_down(),
                KeyCode::Enter => {
                    if let Some(entry) = picker.selected_entry() {
                        if entry.date_end.is_none() && !picker.authenticated {
                            picker.error =
                                Some("Login required for live sessions (press 'a')".to_string());
                        } else {
                            return Ok(PickerAction::Select {
                                session_key: entry.session_key,
                            });
                        }
                    }
                }
                KeyCode::Char('h') | KeyCode::Left => {
                    picker.browse_year -= 1;
                    picker.loading = true;
                    picker.sessions.clear();
                    picker.update_total();
                    pending_fetch = Some(spawn_fetch(picker.browse_year, client, db));
                }
                KeyCode::Char('l') | KeyCode::Right => {
                    picker.browse_year += 1;
                    picker.loading = true;
                    picker.sessions.clear();
                    picker.update_total();
                    pending_fetch = Some(spawn_fetch(picker.browse_year, client, db));
                }
                KeyCode::Char('a') => match super::login::run(terminal).await? {
                    super::login::LoginAction::Authenticated(creds) => {
                        return Ok(PickerAction::Login(creds));
                    }
                    super::login::LoginAction::Cancel => {}
                },
                KeyCode::Char('d') if authenticated => {
                    f1core::auth::keychain::clear();
                    return Ok(PickerAction::Logout);
                }
                _ => {}
            }
        }
    }
}

type FetchResult = (Vec<db::SessionEntry>, Option<String>);
type FetchRx = tokio::sync::oneshot::Receiver<FetchResult>;

fn check_fetch(pending: &mut Option<FetchRx>, picker: &mut PickerState) {
    let Some(rx) = pending.as_mut() else { return };
    match rx.try_recv() {
        Ok((entries, None)) => {
            picker.sessions = entries;
            picker.error = None;
            picker.loading = false;
            picker.update_total();
            if picker.total_items > 0 {
                picker.selected = picker.selected.min(picker.total_items - 1);
            } else {
                picker.selected = 0;
            }
            *pending = None;
        }
        Ok((_, Some(err))) => {
            picker.sessions.clear();
            picker.error = Some(err);
            picker.loading = false;
            picker.update_total();
            picker.selected = 0;
            *pending = None;
        }
        Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
            picker.error = Some("Failed to fetch sessions".to_string());
            picker.loading = false;
            picker.update_total();
            *pending = None;
        }
        Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
    }
}

fn filter_future_sessions(entries: Vec<db::SessionEntry>, year: i32) -> Vec<db::SessionEntry> {
    let now = chrono::Utc::now();
    let current_year: i32 = now.format("%Y").to_string().parse().unwrap_or(0);
    entries
        .into_iter()
        .filter(|e| SessionType::from_api_str(&e.session_type).is_some_and(|t| t.is_supported()))
        .filter(|e| {
            if year != current_year {
                return true;
            }
            e.date_start
                .parse::<DateTime<chrono::Utc>>()
                .map(|dt| dt <= now)
                .unwrap_or(true)
        })
        .collect()
}

fn spawn_fetch(year: i32, client: &Arc<api::OpenF1Client>, db: &Arc<Mutex<db::Db>>) -> FetchRx {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let client = client.clone();
    let db = db.clone();
    tokio::spawn(async move {
        // Try DB first
        let cached = {
            let db_lock = db.lock().unwrap_or_else(|e| e.into_inner());
            db_lock.get_sessions_by_year(year).unwrap_or_default()
        };

        let current_year: i32 = chrono::Utc::now()
            .format("%Y")
            .to_string()
            .parse()
            .unwrap_or(0);
        if !cached.is_empty() && year != current_year {
            let _ = tx.send((filter_future_sessions(cached, year), None));
            return;
        }

        // DB empty for this year — fetch from API
        let year_str = year.to_string();
        let result = match client.get_sessions(&[("year", &year_str)]).await {
            Ok(sessions) => {
                let db_lock = db.lock().unwrap_or_else(|e| e.into_inner());
                for s in &sessions {
                    let _ = db_lock.upsert_session(s);
                }
                let entries = db_lock.get_sessions_by_year(year).unwrap_or_default();
                drop(db_lock);
                (filter_future_sessions(entries, year), None)
            }
            Err(e) => (Vec::new(), Some(format!("{e}"))),
        };
        let _ = tx.send(result);
    });
    rx
}
