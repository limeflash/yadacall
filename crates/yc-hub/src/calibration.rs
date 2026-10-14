//! Persisted measured-performance factors used to improve future predictions.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result, catalog::ModelKind};

/// Catalog cost revision used by shared kind/backend calibration factors.
///
/// Bump this whenever model cost fields in `catalog/models.json` change. Direct measurements remain
/// useful across revisions, but shared factors from older cost estimates do not.
pub const COST_EPOCH: u32 = 1;

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
    /// Measurement path. Missing values in old calibration files mean the primary throughput
    /// (decode for LLMs, normal throughput for other model kinds).
    #[serde(default)]
    pub metric: CalibrationMetric,
    /// Runtime backend name, such as `metal`, `cuda`, or `cpu`.
    pub backend: String,
    /// Estimate before calibration, in the same unit as `measured`.
    pub predicted: f64,
    /// Measured throughput, in realtime x, passages/s, or tokens/s.
    pub measured: f64,
    /// Unix time when the benchmark completed.
    pub measured_at_unix: u64,
    /// Catalog cost revision used to produce `predicted`.
    #[serde(default)]
    pub cost_epoch: u32,
}

/// Throughput path represented by one calibration sample.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CalibrationMetric {
    /// Main model throughput: decode tokens/s for LLMs and the existing metric for other kinds.
    #[default]
    Primary,
    /// LLM prompt-processing tokens/s.
    Prefill,
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
    pub fn record(&mut self, mut measurement: Measurement) {
        measurement.cost_epoch = COST_EPOCH;
        self.measurements.push(measurement);
        if self.measurements.len() > 100 {
            let extra = self.measurements.len() - 100;
            self.measurements.drain(..extra);
        }
    }

    /// Return a bounded median measured/predicted factor for a model category/backend.
    pub fn factor(&self, kind: ModelKind, backend: &str) -> Option<f64> {
        self.factor_for(kind, backend, CalibrationMetric::Primary)
    }

    /// Return the bounded median prefill calibration factor for an LLM backend.
    pub fn prefill_factor(&self, kind: ModelKind, backend: &str) -> Option<f64> {
        self.factor_for(kind, backend, CalibrationMetric::Prefill)
    }

    /// Return the median of up to the three most recent valid measurements for a model.
    pub fn measured(
        &self,
        model_id: &str,
        kind: ModelKind,
        backend: &str,
        metric: CalibrationMetric,
    ) -> Option<f64> {
        let mut recent = self
            .measurements
            .iter()
            .filter(|item| {
                item.model_id == model_id
                    && item.kind == kind
                    && item.backend == backend
                    && item.metric == metric
                    && item.measured.is_finite()
                    && item.measured > 0.0
            })
            .collect::<Vec<_>>();
        recent.sort_by_key(|item| item.measured_at_unix);
        let mut values = recent
            .into_iter()
            .rev()
            .take(3)
            .map(|item| item.measured)
            .collect::<Vec<_>>();
        if values.is_empty() {
            return None;
        }
        values.sort_by(f64::total_cmp);
        let middle = values.len() / 2;
        Some(if values.len() % 2 == 0 {
            (values[middle - 1] + values[middle]) / 2.0
        } else {
            values[middle]
        })
    }

    fn factor_for(&self, kind: ModelKind, backend: &str, metric: CalibrationMetric) -> Option<f64> {
        let mut factors = self
            .measurements
            .iter()
            .filter(|item| {
                item.kind == kind
                    && item.backend == backend
                    && item.metric == metric
                    && item.cost_epoch == COST_EPOCH
            })
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
        Some(median.clamp(0.25, 4.0))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_calibration_measurements_default_to_primary_throughput() {
        let data = r#"{
            "measurements": [{
                "modelId": "gemma",
                "kind": "llm",
                "backend": "metal",
                "predicted": 40.0,
                "measured": 50.0,
                "measuredAtUnix": 1
            }]
        }"#;
        let calibration: Calibration = serde_json::from_str(data).unwrap();
        assert_eq!(calibration.factor(ModelKind::Llm, "metal"), None);
        assert_eq!(calibration.prefill_factor(ModelKind::Llm, "metal"), None);
        assert_eq!(
            calibration.measured("gemma", ModelKind::Llm, "metal", CalibrationMetric::Primary),
            Some(50.0)
        );
    }

    #[test]
    fn prefill_and_primary_factors_are_independent() {
        let mut calibration = Calibration::default();
        for (metric, predicted, measured) in [
            (CalibrationMetric::Primary, 10.0, 20.0),
            (CalibrationMetric::Prefill, 100.0, 150.0),
        ] {
            calibration.record(Measurement {
                model_id: "gemma".to_owned(),
                kind: ModelKind::Llm,
                metric,
                backend: "metal".to_owned(),
                predicted,
                measured,
                measured_at_unix: 1,
                cost_epoch: COST_EPOCH,
            });
        }
        assert_eq!(calibration.factor(ModelKind::Llm, "metal"), Some(2.0));
        assert_eq!(
            calibration.prefill_factor(ModelKind::Llm, "metal"),
            Some(1.5)
        );
    }

    #[test]
    fn recording_stamps_current_epoch_and_factor_is_clamped() {
        let mut calibration = Calibration::default();
        calibration.record(Measurement {
            model_id: "gemma".to_owned(),
            kind: ModelKind::Llm,
            metric: CalibrationMetric::Primary,
            backend: "metal".to_owned(),
            predicted: 1.0,
            measured: 100.0,
            measured_at_unix: 1,
            cost_epoch: 0,
        });
        assert_eq!(calibration.measurements[0].cost_epoch, COST_EPOCH);
        assert_eq!(calibration.factor(ModelKind::Llm, "metal"), Some(4.0));
    }

    #[test]
    fn direct_measurement_uses_median_of_three_most_recent_samples() {
        let mut calibration = Calibration::default();
        for (measured_at_unix, measured) in [(1, 100.0), (2, 32.0), (3, 34.0), (4, 29.0)] {
            calibration.measurements.push(Measurement {
                model_id: "gigaam".to_owned(),
                kind: ModelKind::Asr,
                metric: CalibrationMetric::Primary,
                backend: "coreml".to_owned(),
                predicted: 1.0,
                measured,
                measured_at_unix,
                cost_epoch: 0,
            });
        }
        assert_eq!(
            calibration.measured(
                "gigaam",
                ModelKind::Asr,
                "coreml",
                CalibrationMetric::Primary
            ),
            Some(32.0)
        );
    }
}
