use std::{path::PathBuf, time::Instant};

use anyhow::Context;
use clap::Args as ClapArgs;
use yc_core::{ports::VoiceActivityDetector, samples_to_ms};
use yc_hub::{
    catalog,
    catalog::{ModelEngine, ModelKind, ModelSpec},
    downloader::model_storage_dir,
};
use yc_ml::{
    AsrCatalogEngine, AsrEngine, AsrFileNames, AsrKind, AsrModelConfig, SileroVad, asr_config,
};

use super::{
    audio::decode_file,
    shared::{
        BackendArg, EngineArg, app_data_dir, fail_for_unsupported_backend, resolve_engine_dir,
        resolve_models_dir,
    },
};

/// Arguments for `yadacall transcribe`.
#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Input audio file.
    pub audio: PathBuf,
    /// Recognition engine.
    #[arg(long, value_enum, conflicts_with = "model")]
    pub engine: Option<EngineArg>,
    /// Stable catalog model id to use.
    #[arg(long, conflicts_with = "engine")]
    pub model: Option<String>,
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
    model_id: String,
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
    let (model_id, kind, config, model_dir, models_dir) = if let Some(id) = args.model.as_deref() {
        let catalog = catalog::embedded()?;
        let spec = catalog::model(&catalog, id)
            .with_context(|| format!("unknown catalog model id `{id}`"))?;
        if spec.kind != ModelKind::Asr {
            anyhow::bail!("catalog model `{id}` is not an ASR model");
        }
        if spec.engine.is_none() {
            anyhow::bail!("catalog model `{id}` has no supported ASR engine");
        }
        let app_data = app_data_dir()?;
        let hub_dir = model_storage_dir(&app_data, spec)?;
        let default_models_dir = app_data.join("models");
        let models_dir = args.models_dir.clone().unwrap_or(default_models_dir);
        let model_dir = if args.models_dir.is_some() {
            hub_dir
                .strip_prefix(app_data.join("models"))
                .map(|relative| models_dir.join(relative))
                .context("could not resolve catalog model install path")?
        } else {
            hub_dir
        };
        ensure_model_installed(spec, &model_dir)?;
        let (kind, config) = configured_catalog_asr(spec)?;
        (spec.id.clone(), kind, config, model_dir, models_dir)
    } else {
        let engine = args.engine.unwrap_or(EngineArg::Gigaam);
        let models_dir = resolve_models_dir(args.models_dir.clone());
        let model_dir = resolve_engine_dir(&models_dir, engine)?;
        let model_id = match engine {
            EngineArg::Gigaam => "gigaam-v3-e2e-rnnt-int8",
            EngineArg::Parakeet => "parakeet-tdt-0.6b-v3-int8",
        };
        (
            model_id.to_owned(),
            engine.kind(),
            yc_ml::AsrModelConfig {
                id: model_id.to_owned(),
                files: AsrFileNames::default(),
            },
            model_dir,
            models_dir,
        )
    };

    let load_started = Instant::now();
    let mut recognizer =
        AsrEngine::load_configured(kind, &model_dir, args.backend.backend(), config).with_context(
            || {
                format!(
                    "failed to load {} model from {}",
                    model_id,
                    model_dir.display()
                )
            },
        )?;
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
        model_id,
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
        eprintln!("stats: {}", serde_json::to_string(&stats)?);
    } else {
        for segment in &segments {
            println!("[{}] {}", timestamp(segment.start_ms), segment.text);
        }
        println!(
            "model {} | audio {:.2}s | wall {:.2}s | model load {}ms | RTF {:.4} ({:.2}x realtime) | backend {}",
            stats.model_id,
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

fn ensure_model_installed(spec: &ModelSpec, model_dir: &std::path::Path) -> anyhow::Result<()> {
    for file in spec.files.iter().filter(|file| !file.optional) {
        let path = model_dir.join(&file.path);
        let installed = std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() == file.size);
        if !installed {
            anyhow::bail!(
                "ASR model `{}` is not installed or is incomplete; missing or invalid file {}",
                spec.id,
                path.display()
            );
        }
    }
    Ok(())
}

fn configured_catalog_asr(spec: &ModelSpec) -> anyhow::Result<(AsrKind, AsrModelConfig)> {
    let engine = spec.engine.ok_or_else(|| {
        anyhow::anyhow!("catalog model `{}` has no supported ASR engine", spec.id)
    })?;
    let family = match engine {
        ModelEngine::GigaAmRnnt => AsrCatalogEngine::GigaAmRnnt,
        ModelEngine::GigaAmCtc => AsrCatalogEngine::GigaAmCtc,
        ModelEngine::ParakeetTdt => AsrCatalogEngine::ParakeetTdt,
    };
    let files = spec.engine_files.as_ref();
    Ok(asr_config(
        family,
        spec.id.clone(),
        AsrFileNames {
            preprocessor: files.and_then(|f| f.preprocessor.clone()),
            encoder: files.and_then(|f| f.encoder.clone()),
            decoder: files.and_then(|f| f.decoder.clone()),
            joint: files.and_then(|f| f.joint.clone()),
            decoder_joint: files.and_then(|f| f.decoder_joint.clone()),
            model: files.and_then(|f| f.model.clone()),
            vocabulary: files.and_then(|f| f.vocabulary.clone()),
        },
    ))
}

fn timestamp(milliseconds: u64) -> String {
    let seconds = milliseconds / 1000;
    let hours = seconds / 3600;
    let minutes = (seconds / 60) % 60;
    let seconds = seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parakeet_ultra_uses_catalog_file_overrides() {
        let catalog = catalog::embedded().expect("embedded catalog");
        let spec = catalog::model(&catalog, "parakeet-tdt-0.6b-v3-ultra-int8")
            .expect("Parakeet Ultra catalog entry");
        let (kind, config) = configured_catalog_asr(spec).expect("ASR config");

        assert_eq!(kind, AsrKind::Parakeet);
        assert_eq!(config.id, spec.id);
        assert_eq!(
            config.files.encoder.as_deref(),
            Some("encoder-model.int8.onnx")
        );
        assert_eq!(
            config.files.decoder_joint.as_deref(),
            Some("decoder_joint-model.int8.onnx")
        );
        assert_eq!(config.files.preprocessor.as_deref(), Some("nemo128.onnx"));
        assert_eq!(config.files.vocabulary.as_deref(), Some("vocab.txt"));
    }

    #[test]
    fn catalog_gigaam_multilingual_uses_ctc_config() {
        let catalog = catalog::embedded().expect("embedded catalog");
        let spec = catalog::model(&catalog, "gigaam-multilingual-ctc-int8")
            .expect("multilingual GigaAM catalog entry");
        let (kind, config) = configured_catalog_asr(spec).expect("ASR config");

        assert_eq!(kind, AsrKind::GigaAmCtc);
        assert_eq!(config.id, spec.id);
        assert_eq!(
            config.files.model.as_deref(),
            Some("multilingual_ctc.int8.onnx")
        );
        assert_eq!(
            config.files.vocabulary.as_deref(),
            Some("multilingual_vocab.txt")
        );
    }
}
