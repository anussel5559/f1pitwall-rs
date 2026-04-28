use chrono::{DateTime, Duration as ChronoDuration, FixedOffset, Utc};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

/// Maps wall-clock time to session time.
/// For live sessions, virtual time == real UTC.
/// For replays, starts at session_start and advances 1:1 (or faster with speed).
pub struct SessionClock {
    pub session_start: DateTime<Utc>,
    wall_start: Mutex<Instant>,
    /// Pre-existing elapsed time from a resumed replay.
    start_offset: Mutex<ChronoDuration>,
    pub is_live: bool,
    pub speed: f64,
    pub local_offset: Option<FixedOffset>,
    /// Incremented on seek; poller resets cursors when it notices a change.
    pub seek_generation: AtomicU64,
    paused: AtomicBool,
}

impl SessionClock {
    pub fn new(
        session_start: DateTime<Utc>,
        speed: f64,
        gmt_offset: Option<&str>,
        is_live: bool,
    ) -> Self {
        let local_offset = gmt_offset.and_then(crate::util::time::parse_gmt_offset);
        Self {
            session_start,
            wall_start: Mutex::new(Instant::now()),
            start_offset: Mutex::new(ChronoDuration::zero()),
            is_live,
            speed,
            local_offset,
            seek_generation: AtomicU64::new(0),
            paused: AtomicBool::new(false),
        }
    }

    /// Resume a replay from a previously saved virtual timestamp.
    pub fn resume_from(&self, virtual_ts: DateTime<Utc>) {
        let offset = virtual_ts - self.session_start;
        if offset > ChronoDuration::zero() {
            *self.start_offset.lock().unwrap() = offset;
        }
    }

    /// Current virtual session time.
    pub fn now(&self) -> DateTime<Utc> {
        if self.is_live {
            Utc::now()
        } else {
            let start_offset = *self.start_offset.lock().unwrap();
            if self.paused.load(Ordering::Relaxed) {
                return self.session_start + start_offset;
            }
            let wall_start = *self.wall_start.lock().unwrap();
            let elapsed_ms = wall_start.elapsed().as_millis() as f64 * self.speed;
            self.session_start + start_offset + ChronoDuration::milliseconds(elapsed_ms as i64)
        }
    }

    /// Current virtual time as ISO string for API/DB filtering.
    /// Uses fixed 6-digit fractional seconds so string comparison with
    /// API timestamps (which always include fractional seconds) works correctly.
    pub fn ceiling(&self) -> String {
        crate::util::time::fmt_ts(self.now())
    }

    /// Formatted elapsed session time for display, e.g. "0:42:15"
    pub fn elapsed_display(&self) -> String {
        let elapsed = self.now() - self.session_start;
        let total_secs = elapsed.num_seconds().max(0);
        let hours = total_secs / 3600;
        let mins = (total_secs % 3600) / 60;
        let secs = total_secs % 60;
        if hours > 0 {
            format!("{}:{:02}:{:02}", hours, mins, secs)
        } else {
            format!("{}:{:02}", mins, secs)
        }
    }

    /// Current local time at the circuit, e.g. "14:42:15"
    pub fn local_time_display(&self) -> Option<String> {
        let offset = self.local_offset?;
        let local: DateTime<FixedOffset> = self.now().with_timezone(&offset);
        Some(local.format("%H:%M:%S").to_string())
    }

    /// Skip forward or backward by the given duration (replay only).
    pub fn seek(&self, delta: ChronoDuration) {
        if self.is_live {
            return;
        }
        let mut wall_start = self.wall_start.lock().unwrap();
        let mut start_offset = self.start_offset.lock().unwrap();
        // Freeze current elapsed into start_offset and reset wall clock.
        // If paused, elapsed contribution is zero by definition.
        let elapsed_ms = if self.paused.load(Ordering::Relaxed) {
            0.0
        } else {
            wall_start.elapsed().as_millis() as f64 * self.speed
        };
        *start_offset = *start_offset + ChronoDuration::milliseconds(elapsed_ms as i64) + delta;
        if *start_offset < ChronoDuration::zero() {
            *start_offset = ChronoDuration::zero();
        }
        *wall_start = Instant::now();
        self.seek_generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Toggle pause state (replay only). Returns the new paused state.
    pub fn toggle_pause(&self) -> bool {
        if self.is_live {
            return false;
        }
        let mut wall_start = self.wall_start.lock().unwrap();
        let mut start_offset = self.start_offset.lock().unwrap();
        let was_paused = self.paused.load(Ordering::Relaxed);
        if was_paused {
            // Unpausing: restart the wall clock from now.
            *wall_start = Instant::now();
            self.paused.store(false, Ordering::Relaxed);
            false
        } else {
            // Pausing: freeze current elapsed into start_offset.
            let elapsed_ms = wall_start.elapsed().as_millis() as f64 * self.speed;
            *start_offset += ChronoDuration::milliseconds(elapsed_ms as i64);
            *wall_start = Instant::now();
            self.paused.store(true, Ordering::Relaxed);
            true
        }
    }

    /// Label for the clock display
    pub fn label(&self) -> &'static str {
        if self.is_live {
            "LIVE"
        } else if self.is_paused() {
            "PAUSED"
        } else {
            "REPLAY"
        }
    }
}
