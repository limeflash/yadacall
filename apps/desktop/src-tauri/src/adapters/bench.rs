//! Short real benchmarks run after a model is installed. The measured speed is stored as a `yc-hub`
//! calibration so later predictions are measured, not guessed.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use yc_core::ports::{EmbedKind, SpeakerEmbedder, SpeechRecognizer};
use yc_core::{Error, Result};
use yc_hub::calibration::{CalibrationMetric, Measurement};
use yc_hub::catalog::{ModelKind, ModelSpec};
use yc_hub::runtime;
use yc_ml::{AsrEngine, Backend, CampPlusEmbedder};

use super::hub::{Insights, ModelHub, runtime_backend};
use super::ml::{asr_kind, load_text_embedder};
use super::runtime_keeper::RuntimeKeeper;

/// 12 s of Russian speech (the same clip `yadacall bench-asr` uses).
const SAMPLE: &[u8] = include_bytes!("../../assets/bench_sample.m4a");
const RUNS: usize = 2;

/// Whether [`run`] can benchmark models of this kind (VAD and runtime bundles cannot).
pub fn benchmarkable(kind: ModelKind) -> bool {
    matches!(
        kind,
        ModelKind::Asr | ModelKind::Embed | ModelKind::Llm | ModelKind::Speaker
    )
}

/// What a benchmark measured.
#[derive(Debug, Clone, Copy)]
pub struct Measured {
    /// ASR and speaker embeddings: times faster than real time. Embeddings: passages per second. LLM: generated tokens per second.
    pub value: f64,
}

/// Benchmarks `spec` (which must be installed) and stores the calibration.
pub async fn run(
    hub: &ModelHub,
    insights: &Insights,
    keeper: &RuntimeKeeper,
    spec: &ModelSpec,
) -> Result<Measured> {
    match spec.kind {
        ModelKind::Asr => asr(hub, insights, spec).await,
        ModelKind::Embed => embed(hub, insights, spec).await,
        ModelKind::Llm => llm(hub, insights, keeper, spec).await,
        ModelKind::Speaker => speaker(hub, insights, spec).await,
        _ => Err(Error::Invalid(format!(
            "`{}` cannot be benchmarked",
            spec.id
        ))),
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Decodes the bundled speech sample to 16 kHz mono through a temp file.
fn decode_sample(tmp: &std::path::Path) -> Result<Vec<f32>> {
    std::fs::create_dir_all(tmp)?;
    let sample = tmp.join("bench_sample.m4a");
    std::fs::write(&sample, SAMPLE)?;
    let mut pcm: Vec<f32> = Vec::new();
    let decoded = yc_audio::decode_to_16k_mono(&sample, &mut |b| {
        pcm.extend_from_slice(b);
        Ok(())
    });
    let _ = std::fs::remove_file(&sample);
    decoded?;
    Ok(pcm)
}

async fn asr(hub: &ModelHub, insights: &Insights, spec: &ModelSpec) -> Result<Measured> {
    let predicted = insights.predict_uncalibrated(spec).await?;
    let kind = asr_kind(&spec.id).ok_or_else(|| Error::Invalid(spec.id.clone()))?;
    let dir = hub.dir(spec)?;
    let tmp = hub.root().join("tmp");
    let x = tokio::task::spawn_blocking(move || -> Result<f64> {
        let pcm = decode_sample(&tmp)?;
        let mut asr = AsrEngine::load(kind, &dir, Backend::Auto)?;
        recognise_all(asr.as_mut(), &pcm)?; // warm-up (graph optimisation, caches)
        let started = Instant::now();
        for _ in 0..RUNS {
            recognise_all(asr.as_mut(), &pcm)?;
        }
        let mean = started.elapsed().as_secs_f64() / RUNS as f64;
        Ok((pcm.len() as f64 / 16_000.0) / mean.max(1e-6))
    })
    .await
    .map_err(|e| Error::Storage(format!("benchmark failed: {e}")))??;
    insights.record(Measurement {
        model_id: spec.id.clone(),
        kind: ModelKind::Asr,
        metric: CalibrationMetric::Primary,
        backend: predicted.backend.clone(),
        predicted: predicted.est_realtime_x.unwrap_or_default(),
        measured: x,
        measured_at_unix: now_unix(),
    })?;
    Ok(Measured { value: x })
}

fn recognise_all(asr: &mut dyn SpeechRecognizer, pcm: &[f32]) -> Result<()> {
    let chunk = (asr.max_chunk_ms() as usize * 16).max(16_000);
    for part in pcm.chunks(chunk) {
        asr.recognize(part, "ru")?;
    }
    Ok(())
}

async fn embed(hub: &ModelHub, insights: &Insights, spec: &ModelSpec) -> Result<Measured> {
    let predicted = insights.predict_uncalibrated(spec).await?;
    let mut embedder = load_text_embedder(hub, &spec.id)?;
    let passages = tokio::task::spawn_blocking(move || -> Result<f64> {
        let line = "Договорились созвониться во вторник и обсудить бюджет проекта, сроки поставки и план релиза. ";
        let text = line.repeat(24); // ~512 tokens
        let batch: Vec<&str> = std::iter::repeat_n(text.as_str(), 6).collect();
        embedder.embed(&batch[..1], EmbedKind::Passage)?; // warm-up
        let started = Instant::now();
        embedder.embed(&batch, EmbedKind::Passage)?;
        Ok(batch.len() as f64 / started.elapsed().as_secs_f64().max(1e-6))
    })
    .await
    .map_err(|e| Error::Storage(format!("benchmark failed: {e}")))??;
    // The predictor stores 36 passages per hour of meeting in `est_summary_secs_for_1h_meeting`.
    let predicted_rate = predicted
        .est_summary_secs_for_1h_meeting
        .filter(|s| *s > 0.0)
        .map(|s| 36.0 / s)
        .unwrap_or_default();
    insights.record(Measurement {
        model_id: spec.id.clone(),
        kind: ModelKind::Embed,
        metric: CalibrationMetric::Primary,
        backend: predicted.backend.clone(),
        predicted: predicted_rate,
        measured: passages,
        measured_at_unix: now_unix(),
    })?;
    Ok(Measured { value: passages })
}

async fn llm(
    hub: &ModelHub,
    insights: &Insights,
    keeper: &RuntimeKeeper,
    spec: &ModelSpec,
) -> Result<Measured> {
    let hardware = insights.hardware().await?;
    let exe = keeper.ensure().await?.exe;
    let backend = match runtime_backend(Some(hardware)) {
        runtime::RuntimeBackend::Metal => "metal",
        runtime::RuntimeBackend::Vulkan => "vulkan",
        runtime::RuntimeBackend::Cuda => "cuda",
        runtime::RuntimeBackend::Cpu => "cpu",
    };
    // yc-hub starts a server, asks for a short completion, reads llama.cpp's own timings and writes
    // the calibration file itself.
    let result = runtime::benchmark_model(hub.root(), spec, &exe, backend, 64).await?;
    insights.invalidate_calibration();
    Ok(Measured {
        value: result.measured_tok_s,
    })
}

async fn speaker(hub: &ModelHub, insights: &Insights, spec: &ModelSpec) -> Result<Measured> {
    let dir = hub.dir(spec)?;
    let tmp = hub.root().join("tmp");
    let x = tokio::task::spawn_blocking(move || -> Result<f64> {
        let mut pcm = decode_sample(&tmp)?;
        pcm.truncate(10 * 16_000);
        let mut embedder = CampPlusEmbedder::load(&dir, Backend::Auto)?;
        embedder.embed(&pcm)?; // warm-up
        let started = Instant::now();
        for _ in 0..RUNS {
            embedder.embed(&pcm)?;
        }
        let mean = started.elapsed().as_secs_f64() / RUNS as f64;
        Ok((pcm.len() as f64 / 16_000.0) / mean.max(1e-6))
    })
    .await
    .map_err(|e| Error::Storage(format!("benchmark failed: {e}")))??;
    // The predictor has no speed estimate for speaker models, so there is nothing to compare with.
    insights.record(Measurement {
        model_id: spec.id.clone(),
        kind: ModelKind::Speaker,
        metric: CalibrationMetric::Primary,
        backend: "auto".to_owned(),
        predicted: 0.0,
        measured: x,
        measured_at_unix: now_unix(),
    })?;
    Ok(Measured { value: x })
}

#[cfg(test)]
mod tests {
    #[cfg_attr(not(target_os = "macos"), allow(unused_imports))]
    use std::sync::Arc;

    use super::*;

    #[test]
    fn only_runnable_kinds_are_benchmarkable() {
        assert!(benchmarkable(ModelKind::Speaker));
        assert!(benchmarkable(ModelKind::Asr));
        assert!(!benchmarkable(ModelKind::Vad));
        assert!(!benchmarkable(ModelKind::Runtime));
    }

    /// Needs the CAM++ model under `YC_MODELS_DIR/speaker-campplus-zh-en-16k`.
    #[tokio::test]
    #[ignore = "needs downloaded models (YC_MODELS_DIR)"]
    #[cfg(unix)]
    async fn campplus_benchmark_reports_a_realtime_factor() {
        let models = std::env::var("YC_MODELS_DIR").expect("YC_MODELS_DIR");
        let tmp = tempfile::tempdir().unwrap();
        let hub = ModelHub::new(tmp.path()).unwrap();
        let spec = hub.spec("speaker-campplus-zh-en-16k").unwrap().clone();
        let dir = hub.dir(&spec).unwrap();
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(std::path::Path::new(&models).join(&spec.id), &dir).unwrap();
        let insights = Insights::new(tmp.path());
        let keeper = RuntimeKeeper::new(
            Arc::new(ModelHub::new(tmp.path()).unwrap()),
            Arc::clone(&insights),
            Arc::new(|_| {}),
        );
        let m = run(&hub, &insights, &keeper, &spec).await.unwrap();
        eprintln!("campplus: {:.1}x real time", m.value);
        assert!(m.value > 1.0);
    }
}
