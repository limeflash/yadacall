//! `yadacall hw` hardware and microbenchmark report.

use std::path::PathBuf;

use anyhow::Context;
use clap::Args;
use yc_hub::{benchmark, hw};

/// Hardware command options.
#[derive(Debug, Args)]
pub struct HwArgs {
    /// Print one JSON object instead of a human-readable report.
    #[arg(long)]
    pub json: bool,
}

/// Run the hardware probe and bounded microbenchmarks.
pub async fn run(args: HwArgs) -> anyhow::Result<()> {
    let data_dir = app_data_dir()?;
    let models_dir = data_dir.join("models");
    let cache_path = data_dir.join("hardware.json");
    let profile =
        tokio::task::spawn_blocking(move || hw::probe_cached(&models_dir, &cache_path, 300))
            .await
            .context("hardware profile worker failed")??;
    let benchmarks = tokio::task::spawn_blocking(benchmark::run)
        .await
        .context("microbenchmark worker failed")?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "profile": profile,
                "benchmarks": benchmarks
            }))?
        );
        return Ok(());
    }
    println!("CPU: {}", profile.cpu_brand);
    println!(
        "Cores: {} physical, {} logical{}{}",
        profile.physical_cores,
        profile.logical_cores,
        profile
            .performance_cores
            .map(|count| format!(", {count} performance"))
            .unwrap_or_default(),
        profile
            .efficiency_cores
            .map(|count| format!(", {count} efficiency"))
            .unwrap_or_default()
    );
    println!(
        "Memory: {:.1} GB total, {:.1} GB available",
        profile.total_ram_bytes as f64 / 1_000_000_000.0,
        profile.available_ram_bytes as f64 / 1_000_000_000.0
    );
    println!("ISA: {}", profile.isa.join(", "));
    if let Some(apple) = &profile.apple {
        println!("Apple chip: {}", apple.chip_name);
        if let Some(bandwidth) = apple.memory_bandwidth_gbps {
            println!("Unified memory bandwidth (peak): {bandwidth:.0} GB/s");
        }
        if let Some(maximum) = apple.recommended_max_working_set_bytes {
            println!(
                "Metal recommended max working set: {:.1} GB",
                maximum as f64 / 1_000_000_000.0
            );
        }
    }
    for gpu in &profile.gpus {
        println!(
            "GPU: {} [{}] backend={}",
            gpu.name, gpu.vendor, gpu.backend_hint
        );
    }
    println!(
        "Models disk free: {:.1} GB",
        profile.model_disk_free_bytes as f64 / 1_000_000_000.0
    );
    println!(
        "Benchmarks: {:.1} GB/s memory, {:.1} GFLOP/s f32 GEMM ({} ms)",
        benchmarks.memory_bandwidth_gbps, benchmarks.gemm_gflops, benchmarks.elapsed_ms
    );
    Ok(())
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
