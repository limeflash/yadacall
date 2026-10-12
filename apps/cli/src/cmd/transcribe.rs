use std::{path::PathBuf, time::Instant};

use anyhow::Context;
use clap::Args as ClapArgs;
use yc_core::{ports::VoiceActivityDetector, samples_to_ms};
use yc_ml::{AsrEngine, SileroVad};

use super::{
    audio::decode_file,
    shared::{
        BackendArg, EngineArg, fail_for_unsupported_backend, resolve_engine_dir, resolve_models_dir,
    },
};

/// Arguments for `yadacall transcribe`.
#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Input audio file.
    pub audio: PathBuf,
    /// Recognition engine.
    #[arg(long, value_enum, default_value_t = EngineArg::Gigaam)]
    pub engine: EngineArg,
    /// Language hint (`ru`, `en`, or `auto`).
    #[arg(long, default_value = "ru")]
    pub lang: String,
    /// ONNX Runtime backend.
    #[arg(long, value_enum, default_value_t = BackendArg::Auto)]
    pub backend: BackendArg,
    /// Directory containing downloaded model repositories.
    #[arg(long)]
    pub models_dir: Option<PathBuf>,
    /// Print the recognized segments as a JSON array.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Stats {
    audio_duration_seconds: f64,
    wall_time_seconds: f64,
    model_load_ms: u128,
    realtime_factor: f64,
    x_realtime: f64,
    backend: String,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    fail_for_unsupported_backend(args.backend)?;
    let audio = decode_file(&args.audio)?;
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
    let mut vad = SileroVad::load(args.backend.backend(), &models_dir)
        .context("failed to load bundled Silero VAD")?;
    let model_load_ms = load_started.elapsed().as_millis();

    let transcription_started = Instant::now();
    let mut regions = vad.push(&audio.pcm)?;
    regions.extend(vad.flush());
    let mut segments = Vec::new();
    for region in regions {
        let start = usize::try_from(region.start)
            .unwrap_or(usize::MAX)
            .min(audio.pcm.len());
        let end = usize::try_from(region.end)
            .unwrap_or(usize::MAX)
            .min(audio.pcm.len());
        if end <= start {
            continue;
        }
        let base_ms = samples_to_ms(region.start);
        let mut recognized = recognizer.recognize(&audio.pcm[start..end], &args.lang)?;
        for item in &mut recognized {
            item.start_ms = item.start_ms.saturating_add(base_ms);
            item.end_ms = item.end_ms.saturating_add(base_ms);
            for word in &mut item.words {
                word.start_ms = word.start_ms.saturating_add(base_ms);
                word.end_ms = word.end_ms.saturating_add(base_ms);
            }
        }
        segments.extend(recognized);
    }
    let wall_time_seconds = transcription_started.elapsed().as_secs_f64();
    let audio_duration_seconds = audio.duration_seconds();
    let realtime_factor = if audio_duration_seconds > 0.0 {
        wall_time_seconds / audio_duration_seconds
    } else {
        0.0
    };
    let stats = Stats {
        audio_duration_seconds,
        wall_time_seconds,
        model_load_ms,
        realtime_factor,
        x_realtime: if realtime_factor > 0.0 {
            1.0 / realtime_factor
        } else {
            0.0
        },
        backend: recognizer.info().backend.clone(),
    };

    if args.json {
        serde_json::to_writer(std::io::stdout().lock(), &segments)?;
        println!();
        eprintln!(
            "stats: audio={:.2}s wall={:.2}s load={}ms RTF={:.4} ({:.2}x) backend={}",
            stats.audio_duration_seconds,
            stats.wall_time_seconds,
            stats.model_load_ms,
            stats.realtime_factor,
            stats.x_realtime,
            stats.backend
        );
    } else {
        for segment in &segments {
            println!("[{}] {}", timestamp(segment.start_ms), segment.text);
        }
        println!(
            "audio {:.2}s | wall {:.2}s | model load {}ms | RTF {:.4} ({:.2}x realtime) | backend {}",
            stats.audio_duration_seconds,
            stats.wall_time_seconds,
            stats.model_load_ms,
            stats.realtime_factor,
            stats.x_realtime,
            stats.backend
        );
    }
    Ok(())
}

fn timestamp(milliseconds: u64) -> String {
    let seconds = milliseconds / 1000;
    let hours = seconds / 3600;
    let minutes = (seconds / 60) % 60;
    let seconds = seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}
