//! Keeps the llama.cpp runtime pack usable: installs it when missing, reinstalls it when
//! `llama-server --version` fails, and reports progress as ordinary download events (item = pack id).
//! Every local-LLM use (summary, answer, benchmark) goes through [`RuntimeKeeper::ensure`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use yc_core::Result;
use yc_core::events::{DownloadState, EngineEvent};
use yc_hub::catalog::RuntimeSpec;
use yc_hub::downloader::{DownloadControl, Downloader};
use yc_hub::runtime;

use super::hub::{Insights, ModelHub};
use crate::api::EventSink;

/// An installed, verified runtime: its pack and `llama-server` binary.
#[derive(Debug, Clone)]
pub struct Ready {
    pub pack: RuntimeSpec,
    pub exe: PathBuf,
}

/// Installs and verifies the runtime; shared by the LLM adapter and the models backend.
pub struct RuntimeKeeper {
    hub: Arc<ModelHub>,
    insights: Arc<Insights>,
    events: EventSink,
    gate: tokio::sync::Mutex<()>,
    busy: AtomicBool,
    /// `--version` succeeded for this pack with this binary modification time.
    verified: Mutex<HashMap<String, SystemTime>>,
}

impl RuntimeKeeper {
    /// `events` receives the progress of installs the keeper starts on its own.
    pub fn new(hub: Arc<ModelHub>, insights: Arc<Insights>, events: EventSink) -> Arc<Self> {
        Arc::new(Self {
            hub,
            insights,
            events,
            gate: tokio::sync::Mutex::new(()),
            busy: AtomicBool::new(false),
            verified: Mutex::new(HashMap::new()),
        })
    }

    /// Whether an install is running right now.
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Relaxed)
    }

    /// The runtime pack for this machine and whether it is extracted on disk.
    pub async fn status(&self) -> Option<(RuntimeSpec, bool)> {
        let hardware = self.insights.hardware().await.ok();
        let pack = self.hub.runtime_pack(hardware).ok()?.clone();
        let installed = self.hub.installed_runtime(hardware).is_some();
        Some((pack, installed))
    }

    /// A working runtime, installing or repairing it first when needed.
    pub async fn ensure(&self) -> Result<Ready> {
        self.ensure_with(Arc::clone(&self.events), false).await
    }

    /// Wipes and installs the runtime again (the "переустановить" button).
    pub async fn reinstall(&self, sink: EventSink) -> Result<Ready> {
        self.ensure_with(sink, true).await
    }

    async fn ensure_with(&self, sink: EventSink, force: bool) -> Result<Ready> {
        let _one_at_a_time = self.gate.lock().await;
        let hardware = self.insights.hardware().await.ok();
        if !force && let Some((pack, _, exe)) = self.hub.installed_runtime(hardware) {
            let pack = pack.clone();
            if self.check(&pack, &exe).await {
                return Ok(Ready { pack, exe });
            }
            tracing::warn!("llama.cpp runtime {} is broken; reinstalling", pack.id);
        }
        let pack = self.hub.runtime_pack(hardware)?.clone();
        self.busy.store(true, Ordering::Relaxed);
        let result = self.install(&pack, &sink).await;
        self.busy.store(false, Ordering::Relaxed);
        let exe = result?;
        if let Ok(modified) = std::fs::metadata(&exe).and_then(|m| m.modified()) {
            self.remember(&pack.id, modified);
        }
        Ok(Ready { pack, exe })
    }

    fn remember(&self, id: &str, modified: SystemTime) {
        self.verified
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_owned(), modified);
    }

    /// `llama-server --version` works; remembered per pack id and binary modification time.
    async fn check(&self, pack: &RuntimeSpec, exe: &std::path::Path) -> bool {
        let modified = std::fs::metadata(exe).and_then(|m| m.modified()).ok();
        let known = self
            .verified
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&pack.id)
            .copied();
        if modified.is_some() && modified == known {
            return true;
        }
        let exe = exe.to_owned();
        let ok = tokio::task::spawn_blocking(move || {
            std::process::Command::new(exe)
                .arg("--version")
                .output()
                .is_ok_and(|o| o.status.success())
        })
        .await
        .unwrap_or(false);
        if ok && let Some(modified) = modified {
            self.remember(&pack.id, modified);
        }
        ok
    }

    async fn install(&self, pack: &RuntimeSpec, sink: &EventSink) -> Result<PathBuf> {
        let total = pack.size + pack.extra_assets.iter().map(|a| a.size).sum::<u64>();
        let emit = {
            let sink = Arc::clone(sink);
            let item = pack.id.clone();
            move |done: u64, bytes_per_sec: u64, state: DownloadState| {
                sink(EngineEvent::Download {
                    item: item.clone(),
                    done_bytes: done,
                    total_bytes: total,
                    bytes_per_sec,
                    state,
                });
            }
        };
        emit(0, 0, DownloadState::Queued);
        let base = Arc::new(AtomicU64::new(0));
        let started = Instant::now();
        let progress = {
            let base = Arc::clone(&base);
            let emit = emit.clone();
            Arc::new(move |ev: EngineEvent| {
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
                    DownloadState::Done => {
                        base.fetch_add(total_bytes, Ordering::Relaxed);
                    }
                    DownloadState::Failed { .. } => {}
                    state => {
                        let done = base.load(Ordering::Relaxed) + done_bytes;
                        let bps = if bytes_per_sec == 0 {
                            (done as f64 / started.elapsed().as_secs_f64().max(0.5)) as u64
                        } else {
                            bytes_per_sec
                        };
                        emit(done, bps, state);
                    }
                }
            })
        };
        let downloader = Downloader::with_progress(self.hub.root(), Some(progress))?;
        let dir = self.hub.runtime_dir(pack);
        let _ = std::fs::remove_dir_all(&dir);
        match runtime::pull(
            self.hub.root(),
            pack,
            &downloader,
            &DownloadControl::default(),
        )
        .await
        {
            Ok(exe) => {
                emit(total, 0, DownloadState::Done);
                Ok(exe)
            }
            Err(e) => {
                tracing::warn!("installing runtime {} failed: {e}", pack.id);
                emit(
                    base.load(Ordering::Relaxed),
                    0,
                    DownloadState::Failed {
                        message: e.to_string(),
                        resumable: true,
                    },
                );
                Err(e)
            }
        }
    }

    /// App start: when an LLM is installed (or the runtime is), make sure the runtime works.
    pub async fn validate_in_background(self: Arc<Self>) {
        let wanted = !self
            .hub
            .installed_of(yc_hub::catalog::ModelKind::Llm)
            .is_empty();
        let present = self.status().await.is_some_and(|(_, installed)| installed);
        if (wanted || present)
            && let Err(e) = self.ensure().await
        {
            tracing::warn!("llama.cpp runtime check failed: {e}");
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Downloads the real llama.cpp pack (tens of MB) over the network.
    #[tokio::test]
    #[ignore = "downloads the llama.cpp runtime"]
    async fn a_broken_runtime_is_reinstalled_with_progress() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let hub = Arc::new(ModelHub::new(tmp.path()).unwrap());
        let insights = Insights::new(tmp.path());
        let events: Arc<Mutex<Vec<EngineEvent>>> = Arc::default();
        let sink: EventSink = {
            let events = Arc::clone(&events);
            Arc::new(move |e| events.lock().unwrap().push(e))
        };
        let keeper = RuntimeKeeper::new(Arc::clone(&hub), insights, sink);
        let (pack, installed) = keeper.status().await.unwrap();
        assert!(!installed);
        // A broken install: the binary exists but fails.
        let dir = hub.runtime_dir(&pack);
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("llama-server");
        std::fs::write(&fake, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ready = keeper.ensure().await.unwrap();
        let out = std::process::Command::new(&ready.exe)
            .arg("--version")
            .output()
            .unwrap();
        assert!(out.status.success());
        {
            let seen = events.lock().unwrap();
            assert!(seen.iter().any(|e| matches!(
                e,
                EngineEvent::Download { item, state: DownloadState::Done, .. } if *item == pack.id
            )));
            assert!(
                seen.iter().any(|e| matches!(
                    e,
                    EngineEvent::Download { done_bytes, .. } if *done_bytes > 0
                )),
                "progress was reported"
            );
        }
        // A healthy runtime is not downloaded again.
        events.lock().unwrap().clear();
        keeper.ensure().await.unwrap();
        assert!(events.lock().unwrap().is_empty());
    }
}
