//! Persisted measured-performance factors used to improve future predictions.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result, catalog::ModelKind};

/// Calibration records stored in the app data directory as JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Calibration {
    /// Individual measurements retained for auditability and simple scaling.
    #[serde(default)]
    pub measurements: Vec<Measurement>,
}

/// A measured/predicted pair for one model and execution backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Measurement {
    /// Catalog model id used for the measurement.
    pub model_id: String,
    /// Model category being calibrated.
    pub kind: ModelKind,
    /// Runtime backend name, such as `metal`, `cuda`, or `cpu`.
    pub backend: String,
    /// Estimate before calibration, in the same unit as `measured`.
    pub predicted: f64,
    /// Measured throughput, in realtime x, passages/s, or tokens/s.
    pub measured: f64,
    /// Unix time when the benchmark completed.
    pub measured_at_unix: u64,
}

impl Calibration {
    /// Load calibration JSON, returning an empty store when no file exists yet.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(data) => serde_json::from_slice(&data)
                .map_err(|error| Error::Storage(format!("read calibration: {error}"))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    /// Append a measurement and keep the most recent 100 records.
    pub fn record(&mut self, measurement: Measurement) {
        self.measurements.push(measurement);
        if self.measurements.len() > 100 {
            let extra = self.measurements.len() - 100;
            self.measurements.drain(..extra);
        }
    }

    /// Return a bounded median measured/predicted factor for a model category/backend.
    pub fn factor(&self, kind: ModelKind, backend: &str) -> Option<f64> {
        let mut factors = self
            .measurements
            .iter()
            .filter(|item| item.kind == kind && item.backend == backend)
            .filter(|item| item.predicted > 0.0 && item.measured > 0.0)
            .map(|item| item.measured / item.predicted)
            .filter(|factor| factor.is_finite())
            .collect::<Vec<_>>();
        if factors.is_empty() {
            return None;
        }
        factors.sort_by(f64::total_cmp);
        let middle = factors.len() / 2;
        let median = if factors.len() % 2 == 0 {
            (factors[middle - 1] + factors[middle]) / 2.0
        } else {
            factors[middle]
        };
        Some(median.clamp(0.1, 10.0))
    }

    /// Save calibration JSON atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| Error::Invalid("calibration path needs a parent directory".into()))?;
        std::fs::create_dir_all(parent)?;
        let temp = path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(self)
            .map_err(|error| Error::Storage(format!("serialize calibration: {error}")))?;
        std::fs::write(&temp, data)?;
        std::fs::rename(temp, path)?;
        Ok(())
    }
}
