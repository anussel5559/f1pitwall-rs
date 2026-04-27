use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::api::OpenF1Client;
use crate::clock::SessionClock;
use crate::session_data::BoardRows;
use crate::toast::{Toasts, push_toast};

/// A single car_data sample, compact in-memory representation.
#[derive(Debug, Clone)]
pub struct CarDataPoint {
    pub date: String,
    /// Pre-parsed timestamp, avoids re-parsing on every recompute_charts call.
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub speed: i64,
    pub throttle: i64,
    pub brake: i64,
    pub gear: i64,
}

/// Shared telemetry state, read by UI and written by the fetch task.
pub struct TelemetryState {
    /// Driver currently being viewed.
    pub driver_number: i64,
    /// Raw data points, sorted by date ascending.
    pub data: Vec<CarDataPoint>,
    /// Lap start timestamp used as x=0 reference for charts.
    pub lap_start: Option<String>,
    /// (lap_number, date_start) for each lap boundary.
    pub lap_boundaries: Vec<(i64, String)>,

    // Pre-computed chart data (seconds_from_lap_start, value).
    pub speed_points: Vec<(f64, f64)>,
    pub throttle_points: Vec<(f64, f64)>,
    pub brake_points: Vec<(f64, f64)>,
    pub gear_points: Vec<(f64, f64)>,
    /// (x_coordinate, lap_number) for each lap boundary.
    pub lap_boundary_xs: Vec<(f64, i64)>,
    /// X-axis bounds: (min_seconds, max_seconds).
    pub x_bounds: (f64, f64),
    /// When Some, the chart is pinned to this absolute right-edge x value (not following live).
    /// None = following live edge.
    pub pinned_edge: Option<f64>,
    /// Cached offset from live edge for UI display (seconds behind live). 0 = live.
    pub scroll_offset: f64,
    /// Stable x-axis origin, set once from the first data point and preserved across trims.
    ref_time: Option<chrono::DateTime<chrono::Utc>>,
}

pub type SharedTelemetry = Arc<Mutex<TelemetryState>>;

impl TelemetryState {
    pub fn new(driver_number: i64) -> Self {
        Self {
            driver_number,
            data: Vec::new(),
            lap_start: None,
            lap_boundaries: Vec::new(),
            speed_points: Vec::new(),
            throttle_points: Vec::new(),
            brake_points: Vec::new(),
            gear_points: Vec::new(),
            lap_boundary_xs: Vec::new(),
            x_bounds: (0.0, 90.0),
            pinned_edge: None,
            scroll_offset: 0.0,
            ref_time: None,
        }
    }

    /// Clear all data (e.g. on driver change or seek).
    pub fn clear(&mut self) {
        self.data.clear();
        self.lap_start = None;
        self.lap_boundaries.clear();
        self.speed_points.clear();
        self.throttle_points.clear();
        self.brake_points.clear();
        self.gear_points.clear();
        self.lap_boundary_xs.clear();
        self.x_bounds = (0.0, 90.0);
        self.pinned_edge = None;
        self.scroll_offset = 0.0;
        self.ref_time = None;
    }

    /// Recompute chart point vectors from raw data using a rolling window.
    pub fn recompute_charts(&mut self) {
        // Use a stable ref_time: set once from the first data point, then preserved
        // across trims so x-coordinates (and pinned_edge) never shift.
        let ref_time = if let Some(t) = self.ref_time {
            t
        } else if let Some(first) = self.data.first() {
            self.ref_time = Some(first.timestamp);
            first.timestamp
        } else {
            return;
        };

        self.speed_points.clear();
        self.throttle_points.clear();
        self.brake_points.clear();
        self.gear_points.clear();
        self.lap_boundary_xs.clear();

        for pt in &self.data {
            let x = (pt.timestamp - ref_time).num_milliseconds() as f64 / 1000.0;
            self.speed_points.push((x, pt.speed as f64));
            self.throttle_points.push((x, pt.throttle as f64));
            self.brake_points.push((x, pt.brake as f64));
            // Scale gear (0-8) to 0-100 range for overlay
            self.gear_points.push((x, pt.gear as f64 * 12.5));
        }

        // Compute lap boundary x-coordinates with lap numbers
        for (lap_num, boundary) in &self.lap_boundaries {
            if let Ok(t) = boundary.parse::<chrono::DateTime<chrono::Utc>>() {
                let x = (t - ref_time).num_milliseconds() as f64 / 1000.0;
                self.lap_boundary_xs.push((x, *lap_num));
            }
        }

        self.update_bounds();
    }

    /// Recalculate x_bounds. When pinned, the view stays fixed; otherwise follows live edge.
    pub fn update_bounds(&mut self) {
        let max_x = self.speed_points.last().map(|(x, _)| *x).unwrap_or(0.0);
        const DISPLAY_WINDOW: f64 = 90.0;

        let right_edge = if let Some(pinned) = self.pinned_edge {
            // Clamp: don't pin beyond available data on either side
            pinned.clamp(DISPLAY_WINDOW.min(max_x), max_x)
        } else {
            max_x
        };

        let x_min = (right_edge - DISPLAY_WINDOW).max(0.0);
        self.x_bounds = (x_min, right_edge.max(x_min + 10.0));
        self.scroll_offset = (max_x - right_edge).max(0.0);

        // If pinned edge caught up to live, unpin
        if self.pinned_edge.is_some() && self.scroll_offset < 0.5 {
            self.pinned_edge = None;
            self.scroll_offset = 0.0;
        }
    }

    pub fn scroll_back(&mut self, secs: f64) {
        let max_x = self.speed_points.last().map(|(x, _)| *x).unwrap_or(0.0);
        let current_right = self.pinned_edge.unwrap_or(max_x);
        self.pinned_edge = Some(current_right - secs);
        self.update_bounds();
    }

    pub fn scroll_forward(&mut self, secs: f64) {
        if let Some(pinned) = self.pinned_edge {
            self.pinned_edge = Some(pinned + secs);
        }
        self.update_bounds();
    }

    pub fn scroll_to_live(&mut self) {
        self.pinned_edge = None;
        self.scroll_offset = 0.0;
        self.update_bounds();
    }
}

/// Navigate to the adjacent driver in the telemetry view.
/// `delta`: +1 for next, -1 for previous.
/// Returns the new driver_number if successful.
pub fn cycle_driver(rows: &BoardRows, current_driver: i64, delta: isize) -> Option<i64> {
    let driver_numbers = rows.driver_numbers();
    if driver_numbers.is_empty() {
        return None;
    }
    let idx = driver_numbers.iter().position(|&dn| dn == current_driver)?;
    let new_idx = (idx as isize + delta).rem_euclid(driver_numbers.len() as isize) as usize;
    Some(driver_numbers[new_idx])
}

/// Background task that pre-fetches car_data in 2-minute chunks into SQLite,
/// then reads the display window back into TelemetryState for chart rendering.
pub async fn run_telemetry_polling(
    session_key: i64,
    client: Arc<OpenF1Client>,
    clock: Arc<SessionClock>,
    db: Arc<Mutex<crate::db::Db>>,
    state: SharedTelemetry,
    toasts: Toasts,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    use crate::buffer::{FetchFrontier, fmt_ts};

    let mut frontier = FetchFrontier::new();
    // On replays, `bootstrap_session_data` pre-loads car_data into SQLite for
    // every driver — skip per-driver chunked fetches and just read DB faster.
    let skip_api = !clock.is_live;
    let cycle_duration = if skip_api {
        std::time::Duration::from_millis(250)
    } else {
        std::time::Duration::from_secs(3)
    };

    loop {
        if *stop.borrow() {
            break;
        }

        let cycle_start = std::time::Instant::now();
        let now = clock.now();

        // Reset on seek
        let seek_gen = clock.seek_generation.load(Ordering::Relaxed);
        let did_seek = frontier.check_seek(seek_gen);
        if did_seek {
            state.lock().unwrap().clear();
        }

        let driver_number = state.lock().unwrap().driver_number;

        // Fetch next chunk into SQLite if needed (300s backfill on first fetch)
        if !skip_api && let Some((from, to)) = frontier.next_chunk(now, 300) {
            let result = client
                .get_car_data(
                    session_key,
                    driver_number,
                    Some(&fmt_ts(from)),
                    None,
                    Some(&fmt_ts(to)),
                )
                .await;

            match result {
                Ok(data) => {
                    if !data.is_empty() {
                        let db = db.lock().unwrap();
                        let _ = db.upsert_car_data(session_key, &data);
                    }
                    frontier.advance(to);
                }
                Err(e) => {
                    push_toast(&toasts, format!("car_data: {e}"), true);
                }
            }
        }

        // Read display window from SQLite into TelemetryState
        {
            let window_start = fmt_ts(now - chrono::Duration::seconds(360));
            let ceiling = fmt_ts(now);
            let rows = db
                .lock()
                .unwrap()
                .get_car_data(
                    session_key,
                    driver_number,
                    Some(&window_start),
                    None,
                    Some(&ceiling),
                )
                .unwrap_or_default();

            let mut st = state.lock().unwrap();
            if st.driver_number == driver_number {
                let prev_len = st.data.len();
                st.data.clear();
                for row in &rows {
                    if let Ok(timestamp) = row.date.parse::<chrono::DateTime<chrono::Utc>>() {
                        st.data.push(CarDataPoint {
                            date: row.date.clone(),
                            timestamp,
                            speed: row.speed.unwrap_or(0),
                            throttle: row.throttle.unwrap_or(0),
                            brake: row.brake.unwrap_or(0),
                            gear: row.n_gear.unwrap_or(0),
                        });
                    }
                }
                if st.data.len() != prev_len {
                    st.recompute_charts();
                }
            }
        }

        let remaining = cycle_duration.saturating_sub(cycle_start.elapsed());
        tokio::select! {
            _ = tokio::time::sleep(remaining) => {},
            _ = stop.changed() => break,
        }
    }
}
