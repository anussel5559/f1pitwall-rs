//! Track outline + turn marker data.
//!
//! Coordinates are in the MultiViewer/OpenF1 reference frame (meters, circuit-local,
//! math-frame with Y up). Frontend rendering applies `rotation` + a Y flip to
//! match broadcast orientation and SVG coordinates.
//!
//! Per-circuit files live in `data/tracks/{key}.json` and are embedded at
//! compile time via `include_dir`. Each file contains `{ rotation, points, turns }`
//! where every `turns[i]` has explicit (x, y) in the same frame as `points[]`
//! plus an outward-normal `angle` for label placement.

use std::collections::HashMap;
use std::sync::LazyLock;

use include_dir::{Dir, include_dir};

/// Pre-computed bounding box for normalizing raw coordinates into 0.0..1.0 space.
/// Compute once via `TrackOutline::bounding_box()`, then reuse for all points.
#[derive(Debug, Clone)]
pub struct BoundingBox {
    min_x: f64,
    min_y: f64,
    scale: f64,
    ox: f64,
    oy: f64,
    pub aspect_ratio: f64,
}

impl BoundingBox {
    /// Map a raw (x, y) position into normalized 0.0..1.0 space.
    pub fn normalize_point(&self, x: f64, y: f64) -> (f64, f64) {
        (
            (x - self.min_x + self.ox) / self.scale,
            (y - self.min_y + self.oy) / self.scale,
        )
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TurnMarker {
    pub number: u8,
    pub x: f64,
    pub y: f64,
    pub angle: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub letter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Best single-lap qualifying time ever set at a circuit (pole-lap record).
/// Not an official FIA statistic — curated from public records.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QualifyingRecord {
    pub time_s: f64,
    pub driver: String,
    pub team: String,
    pub year: i64,
}

/// Official FIA race lap record (fastest green-flag lap set during a race).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RaceLapRecord {
    pub time_s: f64,
    pub driver: String,
    pub team: String,
    pub year: i64,
}

/// Winner of the most recent Grand Prix at this circuit.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PreviousWinner {
    pub year: i64,
    pub driver: String,
    pub team: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TrackOutline {
    #[serde(default)]
    pub rotation: f64,
    pub points: Vec<(f64, f64)>,
    #[serde(default)]
    pub turns: Vec<TurnMarker>,
    /// Scheduled race distance in laps. Sourced from FIA-published figures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub race_laps: Option<i64>,
    /// Scheduled sprint distance in laps. Only populated for circuits on the
    /// current sprint calendar; `None` everywhere else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprint_laps: Option<i64>,
    /// Circuit length in kilometres. Sourced from FIA-published figures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_km: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qualifying_record: Option<QualifyingRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub race_lap_record: Option<RaceLapRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_winner: Option<PreviousWinner>,
}

impl TrackOutline {
    /// Compute the bounding box for normalization. Returns None for empty outlines.
    pub fn bounding_box(&self) -> Option<BoundingBox> {
        if self.points.is_empty() {
            return None;
        }
        let (mut min_x, mut max_x) = (f64::MAX, f64::MIN);
        let (mut min_y, mut max_y) = (f64::MAX, f64::MIN);
        for &(x, y) in &self.points {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        let w = (max_x - min_x).max(1.0);
        let h = (max_y - min_y).max(1.0);
        let scale = w.max(h);
        Some(BoundingBox {
            min_x,
            min_y,
            scale,
            ox: (scale - w) / 2.0,
            oy: (scale - h) / 2.0,
            aspect_ratio: w / h,
        })
    }

    /// Normalize all points into a 0.0..1.0 bounding box, preserving aspect ratio.
    /// Returns (normalized_points, aspect_ratio) where aspect_ratio = width/height.
    pub fn normalize(&self) -> (Vec<(f64, f64)>, f64) {
        let Some(bb) = self.bounding_box() else {
            return (Vec::new(), 1.0);
        };
        let pts = self
            .points
            .iter()
            .map(|&(x, y)| bb.normalize_point(x, y))
            .collect();
        (pts, bb.aspect_ratio)
    }
}

// ---------------------------------------------------------------------------
// Data loading — `data/tracks/` is embedded at compile time.
// ---------------------------------------------------------------------------

static TRACKS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../data/tracks");

#[derive(serde::Deserialize)]
struct TrackFile {
    #[serde(default)]
    rotation: f64,
    points: Vec<[f64; 2]>,
    #[serde(default)]
    turns: Vec<TurnMarker>,
    #[serde(default)]
    race_laps: Option<i64>,
    #[serde(default)]
    sprint_laps: Option<i64>,
    #[serde(default)]
    length_km: Option<f64>,
    #[serde(default)]
    qualifying_record: Option<QualifyingRecord>,
    #[serde(default)]
    race_lap_record: Option<RaceLapRecord>,
    #[serde(default)]
    previous_winner: Option<PreviousWinner>,
}

static TRACK_DATA: LazyLock<HashMap<String, TrackOutline>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    for file in TRACKS_DIR.files() {
        let key = file
            .path()
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("track file has utf8 stem")
            .to_string();
        let raw = file.contents_utf8().expect("track file is utf8");
        let parsed: TrackFile = serde_json::from_str(raw)
            .unwrap_or_else(|e| panic!("data/tracks/{}.json invalid: {e}", key));
        let outline = TrackOutline {
            rotation: parsed.rotation,
            points: parsed.points.into_iter().map(|[x, y]| (x, y)).collect(),
            turns: parsed.turns,
            race_laps: parsed.race_laps,
            sprint_laps: parsed.sprint_laps,
            length_km: parsed.length_km,
            qualifying_record: parsed.qualifying_record,
            race_lap_record: parsed.race_lap_record,
            previous_winner: parsed.previous_winner,
        };
        map.insert(key, outline);
    }
    map
});

/// Resolve a circuit display name (from OpenF1) to the canonical key used in
/// the track files.
fn resolve_circuit(name: &str) -> Option<&'static str> {
    match name.to_lowercase().as_str() {
        "bahrain" | "sakhir" => Some("bahrain"),
        "jeddah" => Some("jeddah"),
        "albert park" | "melbourne" => Some("albert_park"),
        "suzuka" => Some("suzuka"),
        "shanghai" => Some("shanghai"),
        "miami" => Some("miami"),
        "imola" => Some("imola"),
        "monaco" | "monte carlo" => Some("monaco"),
        "montreal" | "gilles villeneuve" => Some("montreal"),
        "barcelona" | "catalunya" => Some("barcelona"),
        "spielberg" | "red bull ring" => Some("spielberg"),
        "silverstone" => Some("silverstone"),
        "hungaroring" | "budapest" => Some("hungaroring"),
        "spa" | "spa-francorchamps" => Some("spa"),
        "zandvoort" => Some("zandvoort"),
        "monza" => Some("monza"),
        "madrid" | "madring" => Some("madring"),
        "baku" => Some("baku"),
        "marina bay" | "singapore" => Some("marina_bay"),
        "austin" | "cota" => Some("austin"),
        "mexico" | "mexico city" | "hermanos rodriguez" => Some("mexico"),
        "interlagos" | "sao paulo" | "s\u{e3}o paulo" => Some("interlagos"),
        "las vegas" => Some("las_vegas"),
        "lusail" | "qatar" => Some("lusail"),
        "yas marina" | "yas marina circuit" | "abu dhabi" | "yas island" => Some("yas_marina"),
        _ => None,
    }
}

/// Look up a static track outline by circuit short name.
/// Returns None for unknown circuits.
pub fn get_track_outline(circuit: &str) -> Option<TrackOutline> {
    let key = resolve_circuit(circuit)?;
    TRACK_DATA.get(key).cloned()
}
