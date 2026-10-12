use std::path::PathBuf;
use std::time::Instant;

use anyhow::Context;
use clap::Args as ClapArgs;
use serde::Serialize;
use yc_core::ports::SpeechRecognizer;
use yc_ml::AsrEngine;

use super::{
    audio::decode_bytes,
    shared::{
        BackendArg, EngineArg, fail_for_unsupported_backend, resolve_engine_dir, resolve_models_dir,
    },
};

const BENCH_SAMPLE: &[u8] = include_bytes!("../../assets/bench_sample.m4a");
const BENCH_SAMPLE_EXTENSION: &str = "m4a";
const RUN_COUNT: usize = 3;

/// Arguments for `yadacall bench-asr`.
#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Recognition engine.
    #[arg(long, value_enum, default_value_t = EngineArg::Gigaam)]
    pub engine: EngineArg,
    /// ONNX Runtime backend.
    #[arg(long, value_enum, default_value_t = BackendArg::Auto)]
    pub backend: BackendArg,
    /// Directory containing downloaded model repositories.
    #[arg(long)]
    pub models_dir: Option<PathBuf>,
    /// Print a JSON report for the performance predictor.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchReport {
    engine: String,
    requested_backend: String,
    backend: String,
    audio_duration_seconds: f64,
    model_load_ms: u128,
    warmup_seconds: f64,
    run_seconds: Vec<f64>,
    mean_seconds: f64,
    realtime_factor: f64,
    x_realtime: f64,
    peak_rss_bytes: Option<u64>,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    fail_for_unsupported_backend(args.backend)?;
    let sample = decode_bytes(BENCH_SAMPLE, BENCH_SAMPLE_EXTENSION)
        .context("failed to decode benchmark audio")?;
    let models_dir = resolve_models_dir(args.models_dir);
    let model_dir = resolve_engine_dir(&models_dir, args.engine)?;

    let load_started = Instant::now();
    let mut recognizer = AsrEngine::load(args.engine.kind(), &model_dir, args.backend.backend())
        .with_context(|| {
            format!(
                "failed to load {} model from {}",
                args.engine.model_directory(),
                model_dir.display()
            )
        })?;
    let model_load_ms = load_started.elapsed().as_millis();

    let warmup_started = Instant::now();
    recognize_all(&mut *recognizer, &sample.pcm)?;
    let warmup_seconds = warmup_started.elapsed().as_secs_f64();

    let mut run_seconds = Vec::with_capacity(RUN_COUNT);
    for _ in 0..RUN_COUNT {
        let started = Instant::now();
        recognize_all(&mut *recognizer, &sample.pcm)?;
        run_seconds.push(started.elapsed().as_secs_f64());
    }
    let mean_seconds = run_seconds.iter().sum::<f64>() / run_seconds.len() as f64;
    let duration_seconds = sample.duration_seconds();
    let realtime_factor = if duration_seconds > 0.0 {
        mean_seconds / duration_seconds
    } else {
        0.0
    };
    let report = BenchReport {
        engine: args.engine.kind().as_str().to_owned(),
        requested_backend: args.backend.backend().as_str().to_owned(),
        backend: recognizer.info().backend.clone(),
        audio_duration_seconds: duration_seconds,
        model_load_ms,
        warmup_seconds,
        run_seconds,
        mean_seconds,
        realtime_factor,
        x_realtime: if realtime_factor > 0.0 {
            1.0 / realtime_factor
        } else {
            0.0
        },
        peak_rss_bytes: peak_rss_bytes(),
    };
    if args.json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        println!(
            "{} / {}: {:.2}x realtime (RTF {:.4}), load {}ms, warm-up {:.2}s, peak RSS {}",
            report.engine,
            report.backend,
            report.x_realtime,
            report.realtime_factor,
            report.model_load_ms,
            report.warmup_seconds,
            report
                .peak_rss_bytes
                .map(|bytes| format!("{:.2} GiB", bytes as f64 / 1024.0 / 1024.0 / 1024.0))
                .unwrap_or_else(|| "unavailable".to_owned())
        );
        for (index, elapsed) in report.run_seconds.iter().enumerate() {
            println!(
                "run {}: {:.2}s ({:.2}x realtime)",
                index + 1,
                elapsed,
                duration_seconds / elapsed
            );
        }
    }
    Ok(())
}

fn recognize_all(recognizer: &mut dyn SpeechRecognizer, pcm: &[f32]) -> anyhow::Result<()> {
    let samples_per_chunk = recognizer.max_chunk_ms() as usize * 16;
    for chunk in pcm.chunks(samples_per_chunk.max(1)) {
        recognizer
            .recognize(chunk, "ru")
            .context("ASR inference failed")?;
    }
    Ok(())
}

#[cfg(unix)]
fn peak_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    let usage = unsafe { usage.assume_init() };
    let max_rss = u64::try_from(usage.ru_maxrss).ok()?;
    Some(if cfg!(target_os = "macos") {
        max_rss
    } else {
        max_rss.saturating_mul(1024)
    })
}

#[cfg(not(unix))]
fn peak_rss_bytes() -> Option<u64> {
    None
}
