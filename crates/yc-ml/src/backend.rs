//! ONNX Runtime session construction and backend selection.

use std::{
    fmt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use ort::session::Session;
use yc_core::{Error, Result};

/// Requested execution provider. [`Backend::Auto`] picks the measured-fastest provider per platform
/// (CPU on macOS, DirectML-or-CPU on Windows, CUDA-or-CPU on Linux with the `cuda` feature).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Probe available accelerators, then fall back to CPU.
    Auto,
    /// Use the ONNX Runtime CPU provider.
    Cpu,
    /// Use Apple CoreML, trying both supported compute-unit policies.
    CoreMl,
    /// Use Microsoft DirectML.
    DirectMl,
    /// Use NVIDIA CUDA (requires the `cuda` feature).
    Cuda,
}

impl Backend {
    /// Returns the stable lower-case name used by the CLI and model metadata.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cpu => "cpu",
            Self::CoreMl => "coreml",
            Self::DirectMl => "directml",
            Self::Cuda => "cuda",
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Backend {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "cpu" => Ok(Self::Cpu),
            "coreml" => Ok(Self::CoreMl),
            "directml" => Ok(Self::DirectMl),
            "cuda" => Ok(Self::Cuda),
            _ => Err(format!("unknown backend `{value}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoreMlUnits {
    All,
    CpuAndNeuralEngine,
}

struct Candidate {
    backend: Backend,
    coreml_units: Option<CoreMlUnits>,
    session: Session,
}

/// A model session set that times available providers on its first representative input.
///
/// The first invocation performs one warm-up and one timed inference per candidate. Later calls
/// use only the fastest successful session. This lets Auto keep CPU for dynamic models when its
/// measured latency beats CoreML, while retaining transparent CPU fallback on provider errors.
pub(crate) struct AdaptiveSession {
    candidates: Vec<Candidate>,
    selected: bool,
    requested: Backend,
    backend: Backend,
    coreml_units: Option<CoreMlUnits>,
    last_calibration: Option<Duration>,
}

impl AdaptiveSession {
    pub(crate) fn from_file(
        path: &Path,
        requested: Backend,
        model_cache_dir: &Path,
    ) -> Result<Self> {
        Self::load(path, None, requested, model_cache_dir)
    }

    pub(crate) fn from_memory(
        bytes: &[u8],
        requested: Backend,
        model_cache_dir: &Path,
    ) -> Result<Self> {
        Self::load(
            Path::new("embedded-model.onnx"),
            Some(bytes),
            requested,
            model_cache_dir,
        )
    }

    fn load(
        path: &Path,
        bytes: Option<&[u8]>,
        requested: Backend,
        model_cache_dir: &Path,
    ) -> Result<Self> {
        let providers = provider_plan(requested);
        let mut candidates = Vec::new();
        let mut errors = Vec::new();
        for (backend, units) in providers {
            match create_candidate(path, bytes, backend, units, model_cache_dir) {
                Ok(candidate) => candidates.push(candidate),
                Err(error) => errors.push(format!("{backend}: {error}")),
            }
        }
        if candidates.is_empty() {
            return Err(Error::Model {
                model: path.display().to_string(),
                message: if errors.is_empty() {
                    format!("requested backend `{requested}` is unavailable on this platform")
                } else {
                    errors.join("; ")
                },
            });
        }
        let backend = candidates[0].backend;
        let coreml_units = candidates[0].coreml_units;
        Ok(Self {
            candidates,
            selected: false,
            requested,
            backend,
            coreml_units,
            last_calibration: None,
        })
    }

    pub(crate) fn run_mapped<T>(
        &mut self,
        mut infer: impl FnMut(&mut Session) -> Result<T>,
    ) -> Result<T> {
        if self.selected {
            let candidate = self.candidates.first_mut().ok_or_else(|| Error::Model {
                model: "ONNX session".to_owned(),
                message: "selected provider session is missing".to_owned(),
            })?;
            return infer(&mut candidate.session);
        }

        let mut winner: Option<(usize, Duration, T)> = None;
        let mut failed = Vec::new();
        for (index, candidate) in self.candidates.iter_mut().enumerate() {
            if let Err(error) = infer(&mut candidate.session) {
                failed.push((index, error.to_string()));
                tracing::debug!(backend = %candidate.backend, error = %error, "backend warm-up failed");
                continue;
            }
            let started = Instant::now();
            match infer(&mut candidate.session) {
                Ok(output) => {
                    let elapsed = started.elapsed();
                    if winner.as_ref().is_none_or(|(_, best, _)| elapsed < *best) {
                        winner = Some((index, elapsed, output));
                    }
                }
                Err(error) => {
                    failed.push((index, error.to_string()));
                    tracing::debug!(backend = %candidate.backend, error = %error, "backend calibration failed");
                }
            }
        }

        let Some((winner_index, elapsed, output)) = winner else {
            let message = failed
                .into_iter()
                .map(|(index, error)| format!("{}: {error}", self.candidates[index].backend))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Error::Model {
                model: "ONNX session".to_owned(),
                message: format!(
                    "all loaded execution providers failed during inference ({message})"
                ),
            });
        };

        let chosen_backend = self.candidates[winner_index].backend;
        let chosen_units = self.candidates[winner_index].coreml_units;
        self.candidates = vec![self.candidates.swap_remove(winner_index)];
        self.backend = chosen_backend;
        self.coreml_units = chosen_units;
        self.selected = true;
        self.last_calibration = Some(elapsed);
        Ok(output)
    }

    pub(crate) const fn backend(&self) -> Backend {
        if self.selected || self.candidates.len() == 1 {
            self.backend
        } else {
            self.requested
        }
    }

    pub(crate) fn input_names(&self) -> Vec<String> {
        self.candidates
            .first()
            .map(|candidate| {
                candidate
                    .session
                    .inputs()
                    .iter()
                    .map(|input| input.name().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn output_names(&self) -> Vec<String> {
        self.candidates
            .first()
            .map(|candidate| {
                candidate
                    .session
                    .outputs()
                    .iter()
                    .map(|output| output.name().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[allow(dead_code)]
    pub(crate) const fn calibration_latency(&self) -> Option<Duration> {
        self.last_calibration
    }
}

fn provider_plan(requested: Backend) -> Vec<(Backend, Option<CoreMlUnits>)> {
    match requested {
        Backend::Cpu => vec![(Backend::Cpu, None)],
        Backend::Auto => auto_plan(),
        Backend::CoreMl => coreml_plan(),
        Backend::DirectMl => directml_plan(),
        Backend::Cuda => cuda_plan(),
    }
}

fn auto_plan() -> Vec<(Backend, Option<CoreMlUnits>)> {
    // Measured on M1 Pro (24 s Russian clip, int8 models): CPU runs GigaAM v3 at 26× and
    // Parakeet v3 at 22× realtime, CoreML at ~5× after a ~9 s first-load compile. The encoders'
    // dynamic shapes defeat CoreML, so Auto stays on CPU and avoids holding two sessions in memory.
    // CoreML remains available as an explicit choice.
    #[cfg(target_os = "macos")]
    {
        vec![(Backend::Cpu, None)]
    }
    #[cfg(target_os = "windows")]
    {
        return vec![(Backend::DirectMl, None), (Backend::Cpu, None)];
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows"), feature = "cuda"))]
    {
        return vec![(Backend::Cuda, None), (Backend::Cpu, None)];
    }
    #[cfg(all(
        not(target_os = "macos"),
        not(target_os = "windows"),
        not(feature = "cuda")
    ))]
    {
        vec![(Backend::Cpu, None)]
    }
}

fn coreml_plan() -> Vec<(Backend, Option<CoreMlUnits>)> {
    #[cfg(target_os = "macos")]
    {
        vec![
            (Backend::CoreMl, Some(CoreMlUnits::All)),
            (Backend::CoreMl, Some(CoreMlUnits::CpuAndNeuralEngine)),
        ]
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

fn directml_plan() -> Vec<(Backend, Option<CoreMlUnits>)> {
    #[cfg(target_os = "windows")]
    {
        vec![(Backend::DirectMl, None)]
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

fn cuda_plan() -> Vec<(Backend, Option<CoreMlUnits>)> {
    #[cfg(feature = "cuda")]
    {
        vec![(Backend::Cuda, None)]
    }
    #[cfg(not(feature = "cuda"))]
    {
        Vec::new()
    }
}

fn create_candidate(
    path: &Path,
    bytes: Option<&[u8]>,
    backend: Backend,
    units: Option<CoreMlUnits>,
    model_cache_dir: &Path,
) -> Result<Candidate> {
    let mut builder = Session::builder().map_err(|error| model_error(path, error))?;
    builder = builder
        .with_intra_threads(performance_core_count())
        .map_err(|error| model_error(path, error))?;
    builder = builder
        .with_inter_threads(1)
        .map_err(|error| model_error(path, error))?;

    match backend {
        Backend::Cpu | Backend::Auto => {}
        Backend::CoreMl => {
            #[cfg(target_os = "macos")]
            {
                use ort::ep::{CoreML, coreml::ComputeUnits};
                let units = match units.unwrap_or(CoreMlUnits::All) {
                    CoreMlUnits::All => ComputeUnits::All,
                    CoreMlUnits::CpuAndNeuralEngine => ComputeUnits::CPUAndNeuralEngine,
                };
                let cache = model_cache_dir.join("coreml").join(match units {
                    ComputeUnits::All => "all",
                    ComputeUnits::CPUAndNeuralEngine => "cpu-and-ane",
                    _ => "other",
                });
                std::fs::create_dir_all(&cache)?;
                let provider = CoreML::default()
                    .with_compute_units(units)
                    .with_model_cache_dir(cache.to_string_lossy())
                    .build();
                builder = builder
                    .with_execution_providers([provider.error_on_failure()])
                    .map_err(|error| model_error(path, error))?;
            }
            #[cfg(not(target_os = "macos"))]
            return Err(Error::Invalid(
                "CoreML is available only on macOS".to_owned(),
            ));
        }
        Backend::DirectMl => {
            #[cfg(target_os = "windows")]
            {
                let provider = ort::ep::DirectML::default().build();
                builder = builder
                    .with_execution_providers([provider.error_on_failure()])
                    .map_err(|error| model_error(path, error))?;
            }
            #[cfg(not(target_os = "windows"))]
            return Err(Error::Invalid(
                "DirectML is available only on Windows".to_owned(),
            ));
        }
        Backend::Cuda => {
            #[cfg(feature = "cuda")]
            {
                let provider = ort::ep::CUDA::default().build();
                builder = builder
                    .with_execution_providers([provider.error_on_failure()])
                    .map_err(|error| model_error(path, error))?;
            }
            #[cfg(not(feature = "cuda"))]
            return Err(Error::Invalid(
                "CUDA support requires the `cuda` feature".to_owned(),
            ));
        }
    }

    let session = if let Some(bytes) = bytes {
        builder.commit_from_memory(bytes)
    } else {
        builder.commit_from_file(path)
    }
    .map_err(|error| model_error(path, error))?;

    Ok(Candidate {
        backend,
        coreml_units: units,
        session,
    })
}

fn model_error(path: &Path, error: impl fmt::Display) -> Error {
    Error::Model {
        model: path.display().to_string(),
        message: error.to_string(),
    }
}

fn performance_core_count() -> usize {
    #[cfg(target_os = "macos")]
    if let Ok(output) = std::process::Command::new("sysctl")
        .args(["-n", "hw.perflevel0.physicalcpu"])
        .output()
        && output.status.success()
        && let Ok(count) = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<usize>()
        && count > 0
    {
        return count;
    }
    num_cpus::get_physical().max(1)
}

/// Returns an engine-specific cache directory under the developer model cache by default.
pub fn model_cache_dir(model_dir: &Path, model_id: &str) -> PathBuf {
    model_dir.join(".runtime-cache").join(model_id)
}
