//! Models screen and onboarding: catalog + installed state + predictor verdicts, resumable downloads
//! with aggregated progress, and the post-install benchmark that calibrates later predictions.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use yc_core::events::{DownloadState, EngineEvent};
use yc_core::{Error, Result};
use yc_hub::catalog::{ModelKind as CatKind, ModelSpec};
use yc_hub::downloader::{DownloadControl, Downloader};
use yc_hub::predictor::Verdict as HubVerdict;
use yc_hub::runtime;

use super::EngineBackend;
use crate::adapters::bench;
use crate::adapters::hub::{EMBED_E5_SMALL, ModelHub, SPEAKER_CAMPPLUS, runtime_backend};
use crate::adapters::read_prefs;
use crate::api::{
    BenchmarkResult, EventSink, HardwareProfile, ModelDownload, ModelEntry, ModelKind,
    ModelsOverview, RuntimeInfo, Verdict,
};

/// A download in flight (or paused): its control and where its events go.
pub(super) struct ActiveDownload {
    control: DownloadControl,
    sink: Arc<Mutex<EventSink>>,
}

/// Shared by the progress callback of one download.
struct Ctx {
    model_id: String,
    base: AtomicU64,
    total: AtomicU64,
    started: Instant,
    sink: Arc<Mutex<EventSink>>,
    backend: Weak<EngineBackend>,
    control: DownloadControl,
}

impl Ctx {
    fn emit(&self, done: u64, bytes_per_sec: u64, state: DownloadState) {
        let total = self.total.load(Ordering::Relaxed);
        let dl = ModelDownload {
            state: state.clone(),
            done_bytes: done,
            total_bytes: total,
            bytes_per_sec,
            eta_secs: (bytes_per_sec > 0).then(|| total.saturating_sub(done) / bytes_per_sec),
        };
        if let Some(b) = self.backend.upgrade() {
            let mut g = b.lock();
            if matches!(state, DownloadState::Done) {
                g.progress.remove(&self.model_id);
            } else {
                g.progress.insert(self.model_id.clone(), dl);
            }
        }
        let sink = self.sink.lock().unwrap_or_else(|e| e.into_inner()).clone();
        sink(EngineEvent::Download {
            item: self.model_id.clone(),
            done_bytes: done,
            total_bytes: total,
            bytes_per_sec,
            state,
        });
    }

    /// Callback handed to the hub downloader: per-file events become one stream for the whole model.
    fn on_hub_event(&self, ev: EngineEvent) {
        let EngineEvent::Download {
            done_bytes,
            total_bytes,
            bytes_per_sec,
            state,
            ..
        } = ev
        else {
            return;
        };
        match state {
            // A file finished: the next one starts after it.
            DownloadState::Done => {
                self.base.fetch_add(total_bytes, Ordering::Relaxed);
            }
            // The user cancelled: the screen just drops the row.
            DownloadState::Failed { .. } if self.control.is_cancelled() => {}
            state => {
                let done = self.base.load(Ordering::Relaxed) + done_bytes;
                let bps = if bytes_per_sec == 0 && matches!(state, DownloadState::Running) {
                    let secs = self.started.elapsed().as_secs_f64().max(0.5);
                    (done as f64 / secs) as u64
                } else {
                    bytes_per_sec
                };
                self.emit(done, bps, state);
            }
        }
    }
}

fn map_verdict(v: HubVerdict) -> Verdict {
    match v {
        HubVerdict::Great => Verdict::Great,
        HubVerdict::Ok => Verdict::Ok,
        HubVerdict::Slow => Verdict::Slow,
        HubVerdict::WontRun => Verdict::WontRun,
    }
}

fn map_kind(k: CatKind) -> Option<ModelKind> {
    match k {
        CatKind::Asr => Some(ModelKind::Asr),
        CatKind::Speaker => Some(ModelKind::Voice),
        CatKind::Embed => Some(ModelKind::Embed),
        CatKind::Llm => Some(ModelKind::Llm),
        CatKind::Vad | CatKind::Runtime => None,
    }
}

fn recommendation_group(model: &ModelEntry) -> Option<String> {
    if model.kind != ModelKind::Asr {
        return Some(format!("{:?}", model.kind));
    }
    if model
        .languages
        .iter()
        .any(|language| language == "multilingual")
    {
        Some("asr:multilingual".to_owned())
    } else if model.languages.iter().any(|language| language == "ru") {
        Some("asr:ru".to_owned())
    } else if model.languages.iter().any(|language| language == "en")
        && model.tags.iter().any(|tag| tag == "english-only")
    {
        Some("asr:en".to_owned())
    } else {
        None
    }
}

fn mark_recommended(models: &mut [ModelEntry]) {
    let mut groups = std::collections::BTreeSet::new();
    for model in models.iter() {
        if let Some(group) = recommendation_group(model) {
            groups.insert(group);
        }
    }
    for group in groups {
        let candidates = models
            .iter()
            .enumerate()
            .filter(|(_, model)| recommendation_group(model).as_deref() == Some(group.as_str()));
        let great = candidates
            .clone()
            .filter(|(_, model)| model.verdict == Verdict::Great);
        let eligible = great.collect::<Vec<_>>();
        let candidates = if eligible.is_empty() {
            models
                .iter()
                .enumerate()
                .filter(|(_, model)| recommendation_group(model).as_deref() == Some(group.as_str()))
                .filter(|(_, model)| model.verdict == Verdict::Ok)
                .collect::<Vec<_>>()
        } else {
            eligible
        };
        let best = candidates.into_iter().max_by(|(_, left), (_, right)| {
            left.quality_rank
                .cmp(&right.quality_rank)
                .then_with(|| right.size_bytes.cmp(&left.size_bytes))
        });
        if let Some((index, _)) = best {
            models[index].recommended = true;
        }
    }
}

impl EngineBackend {
    /// Catalog id of the model of `kind` in use (explicit choice, else the automatic one).
    pub(super) async fn active_id(&self, kind: CatKind) -> Option<String> {
        let prefs = read_prefs(&self.prefs);
        match kind {
            CatKind::Asr => Some(prefs.asr.unwrap_or_else(|| {
                ModelHub::default_asr(self.engine().settings().meeting_language.code()).to_owned()
            })),
            CatKind::Embed => Some(
                prefs
                    .text_embedding
                    .unwrap_or_else(|| EMBED_E5_SMALL.to_owned()),
            ),
            CatKind::Speaker => Some(SPEAKER_CAMPPLUS.to_owned()),
            CatKind::Llm => match prefs.llm {
                Some(id) if self.hub.spec(&id).is_ok_and(|s| self.hub.is_installed(s)) => Some(id),
                _ => self.llm.chosen_installed().await,
            },
            _ => None,
        }
    }

    pub(super) async fn overview(&self) -> Result<ModelsOverview> {
        let hardware = self.insights.hardware().await?.clone();
        let russian = self.engine().settings().russian();
        let mut models = Vec::new();
        for spec in self.hub.offered() {
            let Some(kind) = map_kind(spec.kind) else {
                continue;
            };
            let pred = self.insights.predict(spec).await?;
            let installed = self.hub.is_installed(spec);
            let active = self.active_id(spec.kind).await.as_deref() == Some(spec.id.as_str());
            let download = self.lock().progress.get(&spec.id).cloned();
            models.push(ModelEntry {
                id: spec.id.clone(),
                kind,
                name: spec.name.clone(),
                size_bytes: ModelHub::download_bytes(spec),
                blurb: if russian {
                    spec.description_ru.clone()
                } else {
                    spec.description_en.clone()
                },
                installed,
                active,
                verdict: map_verdict(pred.verdict),
                estimate: if russian {
                    pred.headline_ru
                } else {
                    pred.headline_en
                },
                vendor: spec.vendor.clone(),
                family: spec.family.clone(),
                languages: spec.languages.clone(),
                tags: spec.tags.clone(),
                params: if spec.cost.total_params > 0.0 {
                    Some(spec.cost.total_params)
                } else if spec.cost.params > 0.0 {
                    Some(spec.cost.params)
                } else {
                    None
                },
                active_params: (spec.cost.active_params > 0.0
                    && spec.cost.active_params < spec.cost.total_params)
                    .then_some(spec.cost.active_params),
                license: spec.license.clone(),
                license_url: spec.license_url.clone(),
                license_custom: spec.license_custom,
                homepage: Some(spec.homepage.clone()),
                recommended: false,
                quality_rank: spec.quality_rank,
                benchmarkable: bench::benchmarkable(spec.kind),
                download,
            });
        }
        mark_recommended(&mut models);
        let backend = match runtime_backend(Some(&hardware)) {
            runtime::RuntimeBackend::Metal => "Metal",
            runtime::RuntimeBackend::Vulkan => "Vulkan",
            runtime::RuntimeBackend::Cuda => "CUDA",
            runtime::RuntimeBackend::Cpu => "CPU",
        };
        let chip = hardware
            .apple
            .as_ref()
            .map(|a| a.chip_name.clone())
            .unwrap_or_else(|| hardware.cpu_brand.clone());
        let ram_gb = (hardware.total_ram_bytes as f64 / (1u64 << 30) as f64).round() as u32;
        std::fs::create_dir_all(self.hub.root().join("models")).ok();
        let runtime = self
            .keeper
            .status()
            .await
            .map(|(pack, installed)| RuntimeInfo {
                name: format!("llama.cpp {}", pack.release_tag),
                size_bytes: pack.size + pack.extra_assets.iter().map(|a| a.size).sum::<u64>(),
                id: pack.id,
                installed,
                busy: self.keeper.busy(),
            });
        Ok(ModelsOverview {
            runtime,
            hardware: HardwareProfile {
                label: format!("{chip} · {ram_gb} {}", if russian { "ГБ" } else { "GB" }),
                chip,
                ram_gb,
                backend: backend.to_owned(),
                disk_free_bytes: fs2::available_space(self.hub.root().join("models"))
                    .unwrap_or(hardware.model_disk_free_bytes),
                models_bytes: self.hub.models_bytes(),
            },
            models,
        })
    }

    /// Starts or resumes a download. Returns immediately; progress goes to `sink`.
    pub(super) async fn start_download(self: &Arc<Self>, id: &str, sink: EventSink) -> Result<()> {
        if let Some((pack, _)) = self.keeper.status().await
            && pack.id == id
        {
            // "Переустановить" on the runtime row: runs in the background, progress goes to `sink`.
            let keeper = Arc::clone(&self.keeper);
            tokio::spawn(async move {
                if let Err(e) = keeper.reinstall(sink).await {
                    tracing::warn!("runtime reinstall failed: {e}");
                }
            });
            return Ok(());
        }
        let spec = self.hub.spec(id)?.clone();
        {
            let g = self.lock();
            if let Some(active) = g.downloads.get(id) {
                *active.sink.lock().unwrap_or_else(|e| e.into_inner()) = sink;
                active.control.resume();
                return Ok(());
            }
        }
        if self.hub.is_installed(&spec) {
            let size = ModelHub::download_bytes(&spec);
            sink(EngineEvent::Download {
                item: spec.id.clone(),
                done_bytes: size,
                total_bytes: size,
                bytes_per_sec: 0,
                state: DownloadState::Done,
            });
            return Ok(());
        }
        let control = DownloadControl::default();
        let sink = Arc::new(Mutex::new(sink));
        self.lock().downloads.insert(
            id.to_owned(),
            ActiveDownload {
                control: control.clone(),
                sink: Arc::clone(&sink),
            },
        );
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let ctx = Arc::new(Ctx {
                model_id: spec.id.clone(),
                base: AtomicU64::new(0),
                total: AtomicU64::new(ModelHub::download_bytes(&spec)),
                started: Instant::now(),
                sink,
                backend: Arc::downgrade(&me),
                control: control.clone(),
            });
            ctx.emit(0, 0, DownloadState::Queued);
            let result = me.download_all(&spec, &control, &ctx).await;
            me.lock().downloads.remove(&spec.id);
            match result {
                Ok(()) => {
                    let total = ctx.total.load(Ordering::Relaxed);
                    ctx.emit(total, 0, DownloadState::Done);
                    me.after_install(&spec);
                }
                Err(Error::Cancelled) => {
                    me.lock().progress.remove(&spec.id);
                }
                Err(e) => {
                    tracing::warn!("download of {} failed: {e}", spec.id);
                    let done = ctx.base.load(Ordering::Relaxed);
                    ctx.emit(
                        done,
                        0,
                        DownloadState::Failed {
                            message: e.to_string(),
                            resumable: true,
                        },
                    );
                }
            }
        });
        Ok(())
    }

    async fn download_all(
        &self,
        spec: &ModelSpec,
        control: &DownloadControl,
        ctx: &Arc<Ctx>,
    ) -> Result<()> {
        let cb_ctx = Arc::clone(ctx);
        let downloader = Downloader::with_progress(
            self.hub.root(),
            Some(Arc::new(move |ev| cb_ctx.on_hub_event(ev))),
        )?;
        // The on-device LLM needs the llama.cpp runtime next to the weights.
        if spec.kind == CatKind::Llm {
            let hardware = self.insights.hardware().await.ok();
            if self.hub.installed_runtime(hardware).is_none() {
                let pack = self.hub.runtime_pack(hardware)?.clone();
                let extra = pack.size + pack.extra_assets.iter().map(|a| a.size).sum::<u64>();
                ctx.total.fetch_add(extra, Ordering::Relaxed);
                runtime::pull(self.hub.root(), &pack, &downloader, control).await?;
            }
        }
        let dir = self.hub.dir(spec)?;
        let mut pending = Vec::new();
        for f in spec.files.iter().filter(|f| !f.optional) {
            if file_complete(&dir, &f.path, f.size) {
                ctx.base.fetch_add(f.size, Ordering::Relaxed);
            } else {
                pending.push(f);
            }
        }
        let need: u64 = pending.iter().map(|f| f.size).sum();
        downloader.preflight_space(&dir, need)?;
        for f in pending {
            downloader.download_file(&spec.id, f, &dir, control).await?;
        }
        Ok(())
    }

    /// Hooks that run once a model is installed.
    fn after_install(self: &Arc<Self>, spec: &ModelSpec) {
        let me = Arc::clone(self);
        let spec = spec.clone();
        tokio::spawn(async move {
            if spec.kind == CatKind::Asr {
                me.queue_default_embedder().await;
            }
            if spec.kind == CatKind::Llm
                && let Err(e) = me.keeper.ensure().await
            {
                tracing::warn!("llama.cpp runtime is not usable: {e}");
            }
            if spec.kind == CatKind::Embed {
                me.settle_embedder();
                // Resets the engine's cached "no embedder" and indexes the meetings that exist.
                let id = spec.id.clone();
                let engine = me.engine();
                let applied = async {
                    engine
                        .update_settings(|s| s.models.text_embedding = None)
                        .await?;
                    engine
                        .update_settings(|s| s.models.text_embedding = Some(id.clone()))
                        .await?;
                    me.sync_prefs();
                    engine.reindex_all().await
                }
                .await;
                if let Err(e) = applied {
                    tracing::warn!("could not enable {}: {e}", spec.id);
                }
            }
            if matches!(spec.kind, CatKind::Asr | CatKind::Embed | CatKind::Llm)
                && let Err(e) = me.benchmark(&spec, true).await
            {
                tracing::info!("post-install benchmark of {} skipped: {e}", spec.id);
            }
        });
    }

    /// Remembers that the default embedding model needs no automatic download any more.
    fn settle_embedder(&self) {
        let mut g = self.lock();
        if !g.app.embedder_settled {
            g.app.embedder_settled = true;
            let app = g.app.clone();
            drop(g);
            self.save_app_prefs(&app);
        }
    }

    /// Downloads the default embedding model in the background once speech recognition is installed,
    /// so semantic search works without a visit to Models. Progress goes to the global event stream.
    /// Does nothing after the model was installed, removed or cancelled by the user.
    pub(super) async fn queue_default_embedder(self: &Arc<Self>) {
        let Ok(spec) = self.hub.spec(EMBED_E5_SMALL).cloned() else {
            return;
        };
        let busy = {
            let g = self.lock();
            g.app.embedder_settled || g.downloads.contains_key(&spec.id)
        };
        if busy {
            return;
        }
        if self.hub.is_installed(&spec) {
            return self.settle_embedder();
        }
        if self.hub.installed_of(CatKind::Asr).is_empty() {
            return;
        }
        tracing::info!("queueing the default search model {}", spec.id);
        if let Err(e) = self
            .start_download(&spec.id, Arc::clone(&self.events))
            .await
        {
            tracing::warn!("could not queue {}: {e}", spec.id);
        }
    }

    /// Runs the real benchmark of `spec` and stores the calibration. With `background` it yields to a
    /// recording in progress instead of competing with it for the CPU.
    pub(super) async fn benchmark(&self, spec: &ModelSpec, background: bool) -> Result<f64> {
        if !self.hub.is_installed(spec) {
            return Err(Error::ModelMissing(spec.id.clone()));
        }
        let _one_at_a_time = self.bench_lock.lock().await;
        if background && self.engine().recording().is_some() {
            return Err(Error::Invalid("a recording is in progress".into()));
        }
        let measured = bench::run(&self.hub, &self.insights, &self.keeper, spec).await?;
        tracing::info!("benchmark {}: {:.1}", spec.id, measured.value);
        Ok(measured.value)
    }

    pub(super) async fn benchmark_by_id(&self, id: &str) -> Result<BenchmarkResult> {
        let spec = self.hub.spec(id)?.clone();
        let value = self.benchmark(&spec, false).await?;
        Ok(BenchmarkResult {
            model_id: id.to_owned(),
            realtime_factor: value as f32,
        })
    }

    pub(super) fn pause_download(&self, id: &str) {
        if let Some(d) = self.lock().downloads.get(id) {
            d.control.pause();
        }
    }

    pub(super) fn cancel_download(&self, id: &str) {
        if id == EMBED_E5_SMALL {
            self.settle_embedder();
        }
        let mut g = self.lock();
        if let Some(d) = g.downloads.remove(id) {
            d.control.cancel();
        }
        g.progress.remove(id);
    }

    /// Deletes the files of one model (siblings sharing a directory stay).
    pub(super) async fn remove_model(&self, id: &str) -> Result<()> {
        let spec = self.hub.spec(id)?.clone();
        self.cancel_download(id);
        if self.engine().recording().is_some() {
            return Err(Error::Invalid("a recording is in progress".into()));
        }
        Downloader::new(self.hub.root())?
            .remove_model(&spec)
            .await?;
        self.engine()
            .update_settings(|s| {
                for slot in [
                    &mut s.models.asr,
                    &mut s.models.speaker,
                    &mut s.models.text_embedding,
                    &mut s.models.llm,
                ] {
                    if slot.as_deref() == Some(id) {
                        *slot = None;
                    }
                }
            })
            .await?;
        self.sync_prefs();
        if spec.kind == CatKind::Llm {
            self.llm.stop_server().await;
        }
        Ok(())
    }

    /// Makes an installed model the one in use for its kind.
    pub(super) async fn set_active(&self, id: &str) -> Result<()> {
        let spec = self.hub.spec(id)?.clone();
        if !self.hub.is_installed(&spec) {
            return Err(Error::ModelMissing(id.to_owned()));
        }
        let engine = self.engine();
        let id = id.to_owned();
        let kind = spec.kind;
        engine
            .update_settings(|s| match kind {
                CatKind::Asr => s.models.asr = Some(id.clone()),
                CatKind::Speaker => s.models.speaker = Some(id.clone()),
                CatKind::Embed => s.models.text_embedding = Some(id.clone()),
                CatKind::Llm => s.models.llm = Some(id.clone()),
                _ => {}
            })
            .await?;
        self.sync_prefs();
        match kind {
            CatKind::Embed => {
                engine.reindex_all().await?;
            }
            CatKind::Llm => self.llm.stop_server().await,
            _ => {}
        }
        Ok(())
    }
}

fn file_complete(dir: &Path, rel: &str, size: u64) -> bool {
    std::fs::metadata(dir.join(rel)).is_ok_and(|m| m.len() == size)
}

#[cfg(test)]
mod tests {
    use super::mark_recommended;
    use crate::api::{ModelEntry, ModelKind, Verdict};

    fn entry(
        id: &str,
        kind: ModelKind,
        verdict: Verdict,
        rank: i32,
        languages: &[&str],
        tags: &[&str],
    ) -> ModelEntry {
        ModelEntry {
            id: id.to_owned(),
            kind,
            name: id.to_owned(),
            size_bytes: 1,
            blurb: String::new(),
            installed: false,
            active: false,
            verdict,
            estimate: String::new(),
            vendor: String::new(),
            family: String::new(),
            languages: languages.iter().map(|value| (*value).to_owned()).collect(),
            tags: tags.iter().map(|value| (*value).to_owned()).collect(),
            params: None,
            active_params: None,
            license: String::new(),
            license_url: None,
            license_custom: false,
            homepage: None,
            recommended: false,
            quality_rank: rank,
            benchmarkable: true,
            download: None,
        }
    }

    #[test]
    fn recommendation_uses_rank_within_kind_and_asr_language_group() {
        let mut models = vec![
            entry("llm-ok", ModelKind::Llm, Verdict::Ok, 100, &[], &[]),
            entry(
                "llm-great-low",
                ModelKind::Llm,
                Verdict::Great,
                10,
                &[],
                &[],
            ),
            entry(
                "llm-great-high",
                ModelKind::Llm,
                Verdict::Great,
                20,
                &[],
                &[],
            ),
            entry("asr-ru", ModelKind::Asr, Verdict::Ok, 30, &["ru"], &[]),
            entry(
                "asr-multilingual",
                ModelKind::Asr,
                Verdict::Great,
                40,
                &["multilingual"],
                &[],
            ),
            entry(
                "asr-en",
                ModelKind::Asr,
                Verdict::Ok,
                50,
                &["en"],
                &["english-only"],
            ),
            entry(
                "asr-no-group",
                ModelKind::Asr,
                Verdict::Great,
                99,
                &["en"],
                &[],
            ),
        ];
        mark_recommended(&mut models);
        let recommended = models
            .iter()
            .filter(|model| model.recommended)
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            recommended,
            ["llm-great-high", "asr-ru", "asr-multilingual", "asr-en"]
        );
    }
}
