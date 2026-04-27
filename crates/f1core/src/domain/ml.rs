//! ML-based pit window prediction using ONNX quantile regression models.
//!
//! Three XGBoost models (q25/q50/q75) predict tyre stint remaining laps
//! with probability ranges. Models are embedded at compile time when the
//! `ml` feature is enabled.

#[cfg(feature = "ml")]
use ndarray::Array2;
#[cfg(feature = "ml")]
use ort::session::Session;
#[cfg(feature = "ml")]
use std::sync::Mutex;

#[cfg(feature = "ml")]
use super::strategy::Confidence;
use super::strategy::MlPitPrediction;

/// Number of input features (must match Python training pipeline).
pub const NUM_FEATURES: usize = 24;

/// Raw ML feature vector for a single driver's current stint.
pub struct MlFeatures {
    pub driver_number: i64,
    pub compound: String,
    pub tyre_age: i64,
    pub current_lap: i64,
    pub values: [f32; NUM_FEATURES],
}

/// Loads and runs ONNX quantile regression models for pit window prediction.
/// Uses interior mutability because `ort::Session::run` requires `&mut self`.
pub struct PitPredictor {
    #[cfg(feature = "ml")]
    q25: Mutex<Session>,
    #[cfg(feature = "ml")]
    q50: Mutex<Session>,
    #[cfg(feature = "ml")]
    q75: Mutex<Session>,
}

#[cfg(feature = "ml")]
static Q25_BYTES: &[u8] = include_bytes!("../../../../data/models/pit_window_q25.onnx");
#[cfg(feature = "ml")]
static Q50_BYTES: &[u8] = include_bytes!("../../../../data/models/pit_window_q50.onnx");
#[cfg(feature = "ml")]
static Q75_BYTES: &[u8] = include_bytes!("../../../../data/models/pit_window_q75.onnx");

impl PitPredictor {
    /// Load ONNX models from embedded bytes. Returns None if ML feature is disabled
    /// or if models fail to load.
    pub fn load() -> Option<Self> {
        #[cfg(feature = "ml")]
        {
            let q25 = Session::builder()
                .ok()?
                .commit_from_memory(Q25_BYTES)
                .ok()?;
            let q50 = Session::builder()
                .ok()?
                .commit_from_memory(Q50_BYTES)
                .ok()?;
            let q75 = Session::builder()
                .ok()?
                .commit_from_memory(Q75_BYTES)
                .ok()?;
            Some(Self {
                q25: Mutex::new(q25),
                q50: Mutex::new(q50),
                q75: Mutex::new(q75),
            })
        }
        #[cfg(not(feature = "ml"))]
        {
            None
        }
    }

    /// Run inference for all provided feature vectors.
    /// Returns ML pit predictions sorted by urgency (fewest laps remaining first).
    pub fn predict(&self, features: &[MlFeatures]) -> Vec<MlPitPrediction> {
        #[cfg(feature = "ml")]
        {
            self.predict_impl(features)
        }
        #[cfg(not(feature = "ml"))]
        {
            let _ = features;
            Vec::new()
        }
    }

    #[cfg(feature = "ml")]
    fn predict_impl(&self, features: &[MlFeatures]) -> Vec<MlPitPrediction> {
        if features.is_empty() {
            return Vec::new();
        }

        let n = features.len();
        let mut input_data = Array2::<f32>::zeros((n, NUM_FEATURES));
        for (i, f) in features.iter().enumerate() {
            for (j, &v) in f.values.iter().enumerate() {
                input_data[[i, j]] = v;
            }
        }

        let run_model = |session: &Mutex<Session>| -> Option<Vec<f32>> {
            let tensor = ort::value::Tensor::from_array(input_data.clone()).ok()?;
            let mut session = session.lock().ok()?;
            let outputs = session.run(ort::inputs!["features" => tensor]).ok()?;
            let output = outputs.values().next()?;
            let (_, data) = output.try_extract_tensor::<f32>().ok()?;
            Some(data.to_vec())
        };

        let q25_preds = match run_model(&self.q25) {
            Some(v) => v,
            None => return Vec::new(),
        };
        let q50_preds = match run_model(&self.q50) {
            Some(v) => v,
            None => return Vec::new(),
        };
        let q75_preds = match run_model(&self.q75) {
            Some(v) => v,
            None => return Vec::new(),
        };

        let mut predictions: Vec<MlPitPrediction> = features
            .iter()
            .enumerate()
            .filter_map(|(i, f)| {
                let q25 = q25_preds.get(i)?;
                let q50 = q50_preds.get(i)?;
                let q75 = q75_preds.get(i)?;

                let remaining = q50.round().max(0.0) as i64;
                let early = q25.round().max(0.0) as i64;
                let late = q75.round().max(0.0) as i64;

                let window_open = f.current_lap + early;
                let window_close = f.current_lap + late;

                let clean_count = f.values[11] as i64; // current_tyre_age as proxy
                let confidence = if clean_count >= 12 {
                    Confidence::High
                } else if clean_count >= 8 {
                    Confidence::Medium
                } else {
                    Confidence::Low
                };

                Some(MlPitPrediction {
                    driver_number: f.driver_number,
                    compound: f.compound.clone(),
                    tyre_age: f.tyre_age,
                    estimated_laps_remaining: remaining,
                    window_open_lap: window_open,
                    window_close_lap: window_close,
                    confidence,
                })
            })
            .collect();

        predictions.sort_by_key(|p| p.estimated_laps_remaining);
        predictions
    }
}
