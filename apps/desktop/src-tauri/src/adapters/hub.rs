//! The model catalog bound to the app data directory: where models live, what is installed, what the
//! adapters can actually run, and the hardware/benchmark/calibration context behind the predictor.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;
use yc_core::{Error, Result};
use yc_hub::benchmark::{self, Benchmarks};
use yc_hub::calibration::{Calibration, Measurement};
use yc_hub::catalog::{self, Catalog, ModelKind, ModelSpec};
use yc_hub::downloader::model_storage_dir;
use yc_hub::hw::{self, HardwareProfile};
use yc_hub::predictor::{self, Prediction};
use yc_hub::runtime::{self, RuntimeBackend};

/// GigaAM v3 e2e RNNT (Russian, the default for `ru`).
pub const ASR_GIGAAM: &str = "gigaam-v3-e2e-rnnt-int8";
/// GigaAM v3 e2e CTC.
pub const ASR_GIGAAM_CTC: &str = "gigaam-v3-e2e-ctc-int8";
/// Parakeet TDT 0.6B v3 (25 languages, the default for everything but `ru`).
pub const ASR_PARAKEET: &str = "parakeet-tdt-0.6b-v3-int8";
/// CAM++ speaker embeddings.
pub const SPEAKER_CAMPPLUS: &str = "speaker-campplus-zh-en-16k";
/// Default text-embedding model.
pub const EMBED_E5_SMALL: &str = "multilingual-e5-small-int8";

/// Models the adapters can run. Catalog entries outside this list (other embedding models, Silero —
/// which ships inside `yc-ml`) are not offered in the UI.
pub fn is_supported(spec: &ModelSpec) -> bool {
    matches!(
        spec.id.as_str(),
        ASR_GIGAAM | ASR_GIGAAM_CTC | ASR_PARAKEET | SPEAKER_CAMPPLUS | EMBED_E5_SMALL
    ) || spec.kind == ModelKind::Llm
}

/// The catalog plus the directory it installs into.
pub struct ModelHub {
    root: PathBuf,
    catalog: Catalog,
}

impl ModelHub {
    /// `root` is the app data directory: models go to `<root>/models`, runtimes to `<root>/runtimes`.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self {
            root: root.into(),
            catalog: catalog::embedded()?,
        })
    }

    /// The app data directory the hub installs into.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The embedded catalog.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Catalog entry by id.
    pub fn spec(&self, id: &str) -> Result<&ModelSpec> {
        catalog::model(&self.catalog, id)
    }

    /// Models the app offers, in catalog order.
    pub fn offered(&self) -> impl Iterator<Item = &ModelSpec> {
        self.catalog.models.iter().filter(|m| is_supported(m))
    }

    /// Install directory of a model.
    pub fn dir(&self, spec: &ModelSpec) -> Result<PathBuf> {
        model_storage_dir(&self.root, spec)
    }

    /// Cheap installed check (all required files present with their catalog length; no hashing).
    pub fn is_installed(&self, spec: &ModelSpec) -> bool {
        let Ok(dir) = self.dir(spec) else {
            return false;
        };
        spec.files
            .iter()
            .filter(|f| !f.optional)
            .all(|f| std::fs::metadata(dir.join(&f.path)).is_ok_and(|m| m.len() == f.size))
    }

    /// Installed models of a kind.
    pub fn installed_of(&self, kind: ModelKind) -> Vec<&ModelSpec> {
        self.offered()
            .filter(|m| m.kind == kind && self.is_installed(m))
            .collect()
    }

    /// Sum of the sizes of the files a download fetches.
    pub fn download_bytes(spec: &ModelSpec) -> u64 {
        spec.files.iter().filter(|f| !f.optional).map(|f| f.size).sum()
    }

    /// The default ASR model for a meeting language.
    pub fn default_asr(language: &str) -> &'static str {
        if language.eq_ignore_ascii_case("ru") {
            ASR_GIGAAM
        } else {
            ASR_PARAKEET
        }
    }

    /// The installed ASR model to use: the user's choice if installed, else the language default, else
    /// any installed one.
    pub fn pick_asr(&self, language: &str, chosen: Option<&str>) -> Result<&ModelSpec> {
        let candidates = chosen
            .into_iter()
            .chain([Self::default_asr(language), ASR_GIGAAM, ASR_PARAKEET]);
        for id in candidates {
            if let Ok(spec) = self.spec(id)
                && self.is_installed(spec)
            {
                return Ok(spec);
            }
        }
        Err(Error::ModelMissing(Self::default_asr(language).to_owned()))
    }

    /// The llama.cpp runtime pack for this machine.
    pub fn runtime_pack(&self, hardware: Option<&HardwareProfile>) -> Result<&yc_hub::catalog::RuntimeSpec> {
        let platform = runtime::current_platform();
        let backend = runtime_backend(hardware);
        runtime::select(&self.catalog.runtimes, platform, backend).or_else(|_| {
            runtime::select(&self.catalog.runtimes, platform, RuntimeBackend::Cpu)
        })
    }

    /// An already-extracted llama.cpp runtime for this platform: its pack, directory and server binary.
    /// Prefers the pack that suits the hardware, then any other installed one.
    pub fn installed_runtime(
        &self,
        hardware: Option<&HardwareProfile>,
    ) -> Option<(&yc_hub::catalog::RuntimeSpec, PathBuf, PathBuf)> {
        let platform = runtime::current_platform();
        let preferred = self.runtime_pack(hardware).ok().map(|p| p.id.clone());
        let mut packs: Vec<&yc_hub::catalog::RuntimeSpec> = self
            .catalog
            .runtimes
            .iter()
            .filter(|r| r.platform == platform)
            .collect();
        packs.sort_by_key(|r| Some(&r.id) != preferred.as_ref());
        packs.into_iter().find_map(|pack| {
            let dir = self.runtime_dir(pack);
            let exe = runtime::find_llama_server(&dir).ok()?;
            Some((pack, dir, exe))
        })
    }

    /// Directory of the extracted runtime pack.
    pub fn runtime_dir(&self, pack: &yc_hub::catalog::RuntimeSpec) -> PathBuf {
        self.root.join("runtimes").join(&pack.id)
    }

    /// Path of the GGUF file of an LLM entry.
    pub fn gguf_path(&self, spec: &ModelSpec) -> Result<PathBuf> {
        let file = spec
            .files
            .iter()
            .find(|f| f.path.ends_with(".gguf"))
            .ok_or_else(|| Error::Invalid(format!("model `{}` has no GGUF file", spec.id)))?;
        Ok(self.dir(spec)?.join(&file.path))
    }

    /// Total bytes under `<root>/models`.
    pub fn models_bytes(&self) -> u64 {
        dir_size(&self.root.join("models"))
    }

    /// Path of the calibration file shared with the CLI.
    pub fn calibration_path(&self) -> PathBuf {
        self.root.join("calibration.json")
    }
}

/// Runtime pack backend for the machine: Metal on Apple silicon, CUDA/Vulkan when a matching GPU is
/// present, else CPU.
pub fn runtime_backend(hardware: Option<&HardwareProfile>) -> RuntimeBackend {
    if runtime::current_platform() == "macos-arm64" {
        return RuntimeBackend::Metal;
    }
    match hardware.and_then(|h| h.gpus.iter().find(|g| !g.backend_hint.is_empty())) {
        Some(g) if g.backend_hint == "cuda" => RuntimeBackend::Cuda,
        Some(g) if g.backend_hint == "vulkan" => RuntimeBackend::Vulkan,
        _ => RuntimeBackend::Cpu,
    }
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredBenchmarks {
    app_version: String,
    benchmarks: Benchmarks,
}

/// Hardware profile and micro-benchmarks, measured once and cached; plus the persisted calibration.
pub struct Insights {
    root: PathBuf,
    hardware: OnceCell<HardwareProfile>,
    benchmarks: OnceCell<Benchmarks>,
    calibration: Mutex<Option<Calibration>>,
}

impl Insights {
    /// Insights cached under `root` (`hardware.json`, `microbench.json`, `calibration.json`).
    pub fn new(root: impl Into<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            root: root.into(),
            hardware: OnceCell::new(),
            benchmarks: OnceCell::new(),
            calibration: Mutex::new(None),
        })
    }

    /// The probed machine (cached on disk for a week; disk-free is refreshed with each probe).
    pub async fn hardware(&self) -> Result<&HardwareProfile> {
        self.hardware
            .get_or_try_init(|| async {
                let models = self.root.join("models");
                let cache = self.root.join("hardware.json");
                tokio::task::spawn_blocking(move || {
                    std::fs::create_dir_all(&models)?;
                    hw::probe_cached(&models, &cache, 7 * 24 * 3600)
                })
                .await
                .map_err(|e| Error::Storage(format!("hardware probe failed: {e}")))?
            })
            .await
    }

    /// The memory/GEMM micro-benchmarks, run once per app version.
    pub async fn benchmarks(&self) -> &Benchmarks {
        self.benchmarks
            .get_or_init(|| async {
                let path = self.root.join("microbench.json");
                let version = env!("CARGO_PKG_VERSION").to_owned();
                tokio::task::spawn_blocking(move || {
                    if let Ok(bytes) = std::fs::read(&path)
                        && let Ok(stored) = serde_json::from_slice::<StoredBenchmarks>(&bytes)
                        && stored.app_version == version
                    {
                        return stored.benchmarks;
                    }
                    let benchmarks = benchmark::run();
                    let stored = StoredBenchmarks {
                        app_version: version,
                        benchmarks,
                    };
                    if let Ok(bytes) = serde_json::to_vec(&stored) {
                        let _ = std::fs::write(&path, bytes);
                    }
                    benchmarks
                })
                .await
                .unwrap_or_default()
            })
            .await
    }

    /// The calibration as last saved.
    pub fn calibration(&self) -> Calibration {
        let mut slot = self.calibration.lock().unwrap_or_else(|e| e.into_inner());
        slot.get_or_insert_with(|| {
            Calibration::load(&self.root.join("calibration.json")).unwrap_or_default()
        })
        .clone()
    }

    /// Records a measurement and persists the calibration.
    pub fn record(&self, measurement: Measurement) -> Result<()> {
        let path = self.root.join("calibration.json");
        // Reload: the LLM benchmark in yc-hub writes the file itself.
        let mut calibration = Calibration::load(&path).unwrap_or_default();
        calibration.record(measurement);
        calibration.save(&path)?;
        *self.calibration.lock().unwrap_or_else(|e| e.into_inner()) = Some(calibration);
        Ok(())
    }

    /// Forgets the in-memory calibration so the next read sees what other writers saved.
    pub fn invalidate_calibration(&self) {
        *self.calibration.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Prediction for one model with the current calibration.
    pub async fn predict(&self, spec: &ModelSpec) -> Result<Prediction> {
        let hardware = self.hardware().await?;
        let benchmarks = self.benchmarks().await;
        Ok(predictor::predict(spec, hardware, benchmarks, &self.calibration()))
    }

    /// Prediction without calibration (the "predicted" half of a calibration measurement).
    pub async fn predict_uncalibrated(&self, spec: &ModelSpec) -> Result<Prediction> {
        let hardware = self.hardware().await?;
        let benchmarks = self.benchmarks().await;
        Ok(predictor::predict(spec, hardware, benchmarks, &Calibration::default()))
    }
}
