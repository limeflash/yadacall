//! `yadacall models` catalog, downloads, verification, and removal.

use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use clap::{Args, Subcommand, ValueEnum};
use indicatif::{ProgressBar, ProgressStyle};
use yc_core::events::{DownloadState, EngineEvent};
use yc_hub::{
    benchmark,
    calibration::Calibration,
    catalog,
    downloader::{DownloadControl, Downloader, ProgressCallback},
    hw, predictor,
};

/// Model command options.
#[derive(Debug, Args)]
pub struct ModelsArgs {
    /// Print JSON instead of a model table.
    #[arg(long)]
    pub json: bool,
    /// Model subcommand; with no subcommand the catalog is listed.
    #[command(subcommand)]
    pub action: Option<ModelAction>,
}

/// Model management action.
#[derive(Debug, Subcommand)]
pub enum ModelAction {
    /// Download all required files for a catalog model.
    Pull {
        /// Stable catalog model id.
        id: String,
        /// Prefer the Hugging Face mirror before the primary host.
        #[arg(long, value_enum)]
        mirror: Option<Mirror>,
    },
    /// Verify installed file sizes and SHA-256 digests.
    Verify {
        /// Stable catalog model id.
        id: String,
    },
    /// Remove installed model files.
    Rm {
        /// Stable catalog model id.
        id: String,
    },
    /// Refresh file sizes and digests from Hugging Face/GitHub APIs.
    RefreshCatalog {
        /// Output file (defaults to `catalog/models.json` in the current directory).
        #[arg(long, default_value = "catalog/models.json")]
        output: PathBuf,
    },
}

/// Hugging Face source preference.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Mirror {
    /// Standard Hugging Face first.
    Hf,
    /// hf-mirror.com first.
    HfMirror,
}

/// Dispatch a model command.
pub async fn run(args: ModelsArgs) -> anyhow::Result<()> {
    match args.action {
        None => list_models(args.json).await,
        Some(ModelAction::Pull { id, mirror }) => pull_model(&id, mirror).await,
        Some(ModelAction::Verify { id }) => verify_model(&id).await,
        Some(ModelAction::Rm { id }) => remove_model(&id).await,
        Some(ModelAction::RefreshCatalog { output }) => refresh_catalog(&output).await,
    }
}

async fn list_models(json: bool) -> anyhow::Result<()> {
    let catalog = catalog::embedded()?;
    let data_dir = app_data_dir()?;
    let models_dir = data_dir.join("models");
    let hardware_cache = data_dir.join("hardware.json");
    let profile =
        tokio::task::spawn_blocking(move || hw::probe_cached(&models_dir, &hardware_cache, 300))
            .await
            .context("hardware profile worker failed")??;
    let benchmarks = tokio::task::spawn_blocking(benchmark::run)
        .await
        .context("microbenchmark worker failed")?;
    let calibration = Calibration::load(&data_dir.join("calibration.json"))?;
    let rows = catalog
        .models
        .iter()
        .map(|model| {
            let prediction = predictor::predict(model, &profile, &benchmarks, &calibration);
            let size = model.files.iter().map(|file| file.size).sum::<u64>();
            (model, prediction, size)
        })
        .collect::<Vec<_>>();
    if json {
        let output = rows
            .iter()
            .map(|(model, prediction, size)| {
                serde_json::json!({
                    "id": model.id,
                    "name": model.name,
                    "kind": model.kind,
                    "sizeBytes": size,
                    "prediction": prediction
                })
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("{:<38} {:>9}  {:<8}  HEADLINE", "ID", "SIZE", "FIT");
        for (model, prediction, size) in rows {
            println!(
                "{:<38} {:>9}  {:<8?}  {}",
                model.id,
                format_bytes(size),
                prediction.verdict,
                prediction.headline_en
            );
        }
    }
    Ok(())
}

async fn pull_model(id: &str, mirror: Option<Mirror>) -> anyhow::Result<()> {
    let mut model = catalog::model(&catalog::embedded()?, id)?.clone();
    if matches!(mirror, Some(Mirror::HfMirror)) {
        for file in &mut model.files {
            file.urls
                .sort_by_key(|url| if url.contains("hf-mirror.com") { 0 } else { 1 });
        }
    }
    let data_dir = app_data_dir()?;
    let bar = ProgressBar::new(0);
    bar.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} {msg} [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} {bytes_per_sec} ETA {eta}",
        )?
        .progress_chars("=>-"),
    );
    let progress_bar = bar.clone();
    let progress: ProgressCallback = Arc::new(move |event| {
        if let EngineEvent::Download {
            item,
            done_bytes,
            total_bytes,
            bytes_per_sec,
            state,
        } = event
        {
            progress_bar.set_length(total_bytes);
            progress_bar.set_position(done_bytes);
            progress_bar.set_message(format!("{item} · {bytes_per_sec}/s"));
            if matches!(state, DownloadState::Paused) {
                progress_bar.set_message(format!("{item} · paused"));
            }
        }
    });
    let downloader = Downloader::with_progress(&data_dir, Some(progress))?;
    let control = DownloadControl::default();
    let installed = downloader.download_model(&model, &control).await?;
    bar.finish_with_message(format!("{} installed and verified", model.id));
    println!("Installed {} at {}", model.name, installed.display());
    Ok(())
}

async fn verify_model(id: &str) -> anyhow::Result<()> {
    let catalog = catalog::embedded()?;
    let model = catalog::model(&catalog, id)?;
    let downloader = Downloader::new(app_data_dir()?)?;
    if downloader.installed(model, true).await? {
        println!(
            "{}: installed; file sizes and SHA-256 digests match",
            model.id
        );
        Ok(())
    } else {
        anyhow::bail!("{} is missing or failed integrity verification", model.id)
    }
}

async fn remove_model(id: &str) -> anyhow::Result<()> {
    let catalog = catalog::embedded()?;
    let model = catalog::model(&catalog, id)?;
    let downloader = Downloader::new(app_data_dir()?)?;
    downloader.remove_model(model).await?;
    println!("Removed model {id}");
    Ok(())
}

async fn refresh_catalog(path: &std::path::Path) -> anyhow::Result<()> {
    let catalog = catalog::refresh_file(path).await?;
    println!(
        "Refreshed {} model entries and {} runtime packs into {}",
        catalog.models.len(),
        catalog.runtimes.len(),
        path.display()
    );
    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn app_data_dir() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("YC_DATA_DIR") {
        let path = PathBuf::from(path);
        std::fs::create_dir_all(&path).context("create configured YadaCall data directory")?;
        return Ok(path);
    }
    let root = dirs::data_local_dir().context("cannot resolve the local app data directory")?;
    let path = root.join("YadaCall");
    std::fs::create_dir_all(&path).context("create YadaCall app data directory")?;
    Ok(path)
}
