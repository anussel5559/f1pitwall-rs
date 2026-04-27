use std::sync::{Arc, Mutex};
use std::time::Instant;

/// A toast notification displayed temporarily in the UI.
pub struct Toast {
    pub message: String,
    pub is_error: bool,
    pub created: Instant,
}

/// Shared error/info sink that the poller writes to and the UI reads from.
pub type Toasts = Arc<Mutex<Vec<Toast>>>;

pub fn push_toast(toasts: &Toasts, message: String, is_error: bool) {
    let mut t = toasts.lock().unwrap();
    t.push(Toast {
        message,
        is_error,
        created: Instant::now(),
    });
    if t.len() > 5 {
        t.remove(0);
    }
}
