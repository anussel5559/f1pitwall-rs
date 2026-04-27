use chrono::{DateTime, Duration, Utc};

pub const CHUNK_SECS: i64 = 120;
pub const BUFFER_AHEAD_SECS: i64 = 600;

pub fn fmt_ts(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
}

pub fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f+00:00")
                .map(|ndt| ndt.and_utc())
                .ok()
        })
}

/// Tracks how far ahead we've pre-fetched data for a given stream.
/// Shared between location and telemetry pollers.
#[derive(Default)]
pub struct FetchFrontier {
    fetched_up_to: Option<DateTime<Utc>>,
    last_seek_gen: u64,
}

impl FetchFrontier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check for seek and reset frontier if needed. Returns true if a seek occurred.
    pub fn check_seek(&mut self, seek_gen: u64) -> bool {
        if seek_gen != self.last_seek_gen {
            self.last_seek_gen = seek_gen;
            self.fetched_up_to = None;
            true
        } else {
            false
        }
    }

    /// Reset the frontier (e.g. on driver change).
    pub fn reset(&mut self) {
        self.fetched_up_to = None;
    }

    /// Returns `Some((from, to))` if a new chunk should be fetched, `None` if fully buffered.
    /// `backfill_secs` controls how far back the initial fetch reaches (e.g. 5 for location,
    /// 300 for telemetry scrollback).
    ///
    /// If the frontier has fallen behind `now` (e.g. the UI was closed and reopened),
    /// it resets and starts fresh from the current position.
    pub fn next_chunk(
        &mut self,
        now: DateTime<Utc>,
        backfill_secs: i64,
    ) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        // Reset stale frontier — no point filling a gap the user skipped past.
        // Only reset if the frontier has fallen more than a chunk behind `now`,
        // not when it's just slightly behind (normal operation after sync fallback).
        if let Some(frontier) = self.fetched_up_to
            && frontier + Duration::seconds(CHUNK_SECS) < now
        {
            self.fetched_up_to = None;
        }

        let refetch_threshold = now + Duration::seconds(BUFFER_AHEAD_SECS * 9 / 10);
        let needs_fetch = match self.fetched_up_to {
            None => true,
            Some(frontier) => frontier < refetch_threshold,
        };

        if !needs_fetch {
            return None;
        }

        let fetch_from = self
            .fetched_up_to
            .unwrap_or(now - Duration::seconds(backfill_secs));
        let fetch_to = fetch_from + Duration::seconds(CHUNK_SECS);

        Some((fetch_from, fetch_to))
    }

    /// Advance the frontier after a successful fetch. Never regresses —
    /// protects against race between background and synchronous fetches.
    pub fn advance(&mut self, to: DateTime<Utc>) {
        self.fetched_up_to = Some(match self.fetched_up_to {
            Some(current) if current > to => current,
            _ => to,
        });
    }

    /// Whether any data has been fetched yet.
    pub fn is_primed(&self) -> bool {
        self.fetched_up_to.is_some()
    }
}
