//! TUI wrapper around `f1core::polling::bootstrap_session_data_with_progress`.
//!
//! The chunked all-drivers fetch logic itself lives in `f1core` so the web
//! backend benefits from it too. This module owns the `Status` arc the TUI
//! reads each frame to render the `Loading N/M` overlay, plus a thin `run`
//! that maps `BootstrapProgress` callbacks onto the arc.

use std::sync::{Arc, Mutex};

use f1core::api::OpenF1Client;
use f1core::db::Db;
use f1core::polling::bootstrap_session_data_with_progress;
use f1core::toast::Toasts;

pub use f1core::polling::BootstrapProgress as Progress;

/// Shared progress state surfaced to the UI as a loading spinner. `None`
/// means no bootstrap is running (or it's already done); `Some(_)` is
/// rendered as a `Loading N/M` overlay.
pub type Status = Arc<Mutex<Option<Progress>>>;

pub fn new_status() -> Status {
    Arc::new(Mutex::new(None))
}

/// Pre-load `car_data` + `location` for a replay session, updating `status`
/// after every chunk so the UI can render a progress overlay.
pub async fn run(
    session_key: i64,
    client: Arc<OpenF1Client>,
    db: Arc<Mutex<Db>>,
    toasts: Toasts,
    status: Status,
) {
    let status_for_cb = status.clone();
    bootstrap_session_data_with_progress(
        session_key,
        client,
        db,
        toasts,
        move |p: Progress| {
            *status_for_cb.lock().unwrap() = Some(p);
        },
    )
    .await;
    *status.lock().unwrap() = None;
}
