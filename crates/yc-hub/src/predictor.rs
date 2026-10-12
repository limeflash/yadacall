//! Analytical model-fit and throughput predictions.

use serde::{Deserialize, Serialize};

use crate::{
    benchmark::Benchmarks,
    calibration::Calibration,
    catalog::{ModelKind, ModelSpec},
    hw::HardwareProfile,
};

/// Coarse fit tier shown before a model download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Verdict {
    /// Comfortable memory fit and fast interactive/real-time throughput.
    Great,
    /// Usable for normal workloads, but with some latency.
    Ok,
    /// Runs slowly or only after the call.
    Slow,
    /// Does not fit or cannot sustain the minimum useful speed.
    WontRun,
}

/// Estimate for one model on one probed device.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prediction {
    /// Model id for table joins.
    pub model_id: String,
    /// Verdict tier.
    pub verdict: Verdict,
    /// Russian headline, ready for the primary UI.
    pub headline_ru: String,
    /// English headline.
    pub headline_en: String,
    /// ASR speed relative to audio duration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub est_realtime_x: Option<f64>,
    /// LLM generation rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub est_tok_s: Option<f64>,
    /// Estimated prompt processing rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub est_prefill_tok_s: Option<f64>,
    /// Estimated wall time for summarizing a one-hour meeting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub est_summary_secs_for_1h_meeting: Option<f64>,
    /// Estimated peak memory requirement in bytes.
    pub mem_needed: u64,
    /// Memory budget available to the selected runtime/backend.
    pub mem_available: u64,
    /// Human-readable notes describing formulas and detected constraints.
    pub reasons: Vec<String>,
    /// Backend assumed by the estimate.
    pub backend: String,
}

/// Predict how well a catalog item will run on the probed machine.
pub fn predict(
    model: &ModelSpec,
    hardware: &HardwareProfile,
    benchmarks: &Benchmarks,
    calibration: &Calibration,
) -> Prediction {
    let backend = select_backend(hardware, model.kind);
    let mut reasons = vec![format!(
        "estimate uses {} cores on {} ({})",
        hardware.physical_cores, hardware.cpu_brand, backend
    )];
    let mut mem_available = available_memory_for(hardware, &backend);
    let mut verdict: Verdict;
    let mut realtime = None;
    let mut tok_s = None;
    let mut prefill = None;
    let mut summary_secs = None;
    let mem_needed: u64;

    match model.kind {
        ModelKind::Asr => {
            let cost = model.cost.gflop_per_audio_sec.max(0.01);
            let raw = benchmarks.gemm_gflops.max(0.1) / cost * model.cost.efficiency.max(0.01);
            let factor = calibration.factor(ModelKind::Asr, &backend).unwrap_or(1.0);
            let speed = raw * factor;
            realtime = Some(speed);
            mem_needed = model
                .requires
                .min_ram_bytes
                .max(model.files.iter().map(|file| file.size).sum::<u64>() + 256 * MIB);
            verdict = if mem_needed > mem_available {
                Verdict::WontRun
            } else if speed >= 20.0 {
                Verdict::Great
            } else if speed >= 5.0 {
                Verdict::Ok
            } else if speed >= 1.5 {
                Verdict::Slow
            } else {
                Verdict::WontRun
            };
            reasons.push(format!(
                "ASR estimate = {:.1} GFLOP/s measured GEMM ÷ {:.2} GFLOP per audio second × {:.0}% backend efficiency",
                benchmarks.gemm_gflops, cost, model.cost.efficiency * 100.0
            ));
            if factor != 1.0 {
                reasons.push(format!(
                    "measured ASR calibration scales estimate by {factor:.2}×"
                ));
            }
            if mem_needed > mem_available {
                reasons.push("estimated working set exceeds the usable memory budget".to_owned());
            }
        }
        ModelKind::Embed => {
            let work = model.cost.gflop_per_passage.max(0.01);
            let raw_passages_s =
                benchmarks.gemm_gflops.max(0.1) / work * model.cost.efficiency.max(0.01);
            let factor = calibration
                .factor(ModelKind::Embed, &backend)
                .unwrap_or(1.0);
            let passages_s = raw_passages_s * factor;
            let one_hour_passages = 36.0;
            let seconds = one_hour_passages / passages_s.max(0.01);
            summary_secs = Some(seconds);
            mem_needed = model
                .requires
                .min_ram_bytes
                .max(model.files.iter().map(|file| file.size).sum::<u64>() + 256 * MIB);
            verdict = if mem_needed > mem_available {
                Verdict::WontRun
            } else if seconds <= 5.0 {
                Verdict::Great
            } else if seconds <= 20.0 {
                Verdict::Ok
            } else if seconds <= 90.0 {
                Verdict::Slow
            } else {
                Verdict::WontRun
            };
            reasons.push(format!(
                "embedding estimate uses {:.1} GFLOP/s GEMM over {:.2} GFLOP per 512-token passage; 36 passages represent a one-hour meeting",
                benchmarks.gemm_gflops, work
            ));
            if factor != 1.0 {
                reasons.push(format!(
                    "measured embedding calibration scales estimate by {factor:.2}×"
                ));
            }
        }
        ModelKind::Llm => {
            let bandwidth = benchmarks
                .memory_bandwidth_gbps
                .max(hardware.memory_bandwidth_gbps.unwrap_or(0.0));
            let bytes_per_token = model.cost.bytes_per_token.max(1) as f64;
            let raw = bandwidth * 1_000_000_000.0 * model.cost.efficiency / bytes_per_token;
            let factor = calibration.factor(ModelKind::Llm, &backend).unwrap_or(1.0);
            let mut speed = raw * factor;
            let params_b = model
                .cost
                .active_params
                .max(model.cost.total_params)
                .max(0.1);
            let prefill_speed = (benchmarks.gemm_gflops / (2.0 * params_b)).max(0.1);
            let weights = model.files.iter().map(|file| file.size).sum::<u64>();
            let context = u64::from(model.cost.context.max(1));
            let sliding = u64::from(model.cost.sliding_window);
            let local_fraction = model.cost.local_layer_fraction.clamp(0.0, 1.0);
            let global_fraction = if model.cost.global_layer_fraction > 0.0 {
                model.cost.global_layer_fraction.clamp(0.0, 1.0)
            } else {
                1.0 - local_fraction
            };
            let local_layers = model.cost.n_layers as f64 * local_fraction;
            let global_layers = model.cost.n_layers as f64 * global_fraction;
            let local_context = if sliding > 0 {
                context.min(sliding)
            } else {
                context
            };
            let local_head_dim = if model.cost.local_head_dim > 0 {
                model.cost.local_head_dim
            } else {
                model.cost.head_dim
            };
            let kv_bytes = (2.0
                * f64::from(model.cost.kv_heads)
                * 2.0
                * (local_layers * f64::from(local_head_dim) * local_context as f64
                    + global_layers * f64::from(model.cost.head_dim) * context as f64))
                .ceil() as u64;
            let effective_context = if model.cost.n_layers > 0 {
                ((local_layers * local_context as f64 + global_layers * context as f64)
                    / f64::from(model.cost.n_layers))
                .ceil() as u64
            } else {
                context
            };
            let compute_buffer =
                ((weights as f64 * model.cost.buffer_fraction.max(0.0)) as u64).max(512 * MIB);
            mem_needed = weights
                .saturating_add(kv_bytes)
                .saturating_add(compute_buffer);
            if mem_needed > mem_available && backend != "cpu" && hardware.apple.is_none() {
                let cpu_available = (hardware.available_ram_bytes as f64 * 0.75) as u64;
                if mem_needed <= cpu_available {
                    mem_available = cpu_available;
                    speed *= 0.35;
                    reasons.push(format!(
                        "model spills from {} to CPU/system memory; decode speed is reduced to 35%",
                        backend
                    ));
                }
            }
            tok_s = Some(speed);
            prefill = Some(prefill_speed);
            summary_secs = Some(9_000.0 / prefill_speed + 800.0 / speed.max(0.1));
            verdict = if mem_needed > mem_available {
                Verdict::WontRun
            } else if speed >= 25.0 {
                Verdict::Great
            } else if speed >= 10.0 {
                Verdict::Ok
            } else if speed >= 3.0 {
                Verdict::Slow
            } else {
                Verdict::WontRun
            };
            reasons.push(format!(
                "decode estimate = {:.0} GB/s memory bandwidth × {:.0}% efficiency ÷ {:.2} GB read per generated token",
                bandwidth, model.cost.efficiency * 100.0, bytes_per_token / 1_000_000_000.0
            ));
            reasons.push(format!(
                "memory = {:.2} GB weights + {:.2} GB KV cache at {} tokens + {:.2} GB execution buffer",
                weights as f64 / GIB,
                kv_bytes as f64 / GIB,
                effective_context,
                compute_buffer as f64 / GIB
            ));
            reasons.push(format!(
                "prefill estimate uses {:.1} GFLOP/s GEMM and {:.1}B active parameters",
                benchmarks.gemm_gflops, params_b
            ));
            if factor != 1.0 {
                reasons.push(format!(
                    "measured LLM calibration scales decode estimate by {factor:.2}×"
                ));
            }
        }
        ModelKind::Vad | ModelKind::Speaker | ModelKind::Runtime => {
            mem_needed = model.files.iter().map(|file| file.size).sum();
            verdict = if mem_needed <= mem_available {
                Verdict::Great
            } else {
                Verdict::WontRun
            };
        }
    }

    if model.requires.min_ram_bytes > hardware.total_ram_bytes {
        verdict = Verdict::WontRun;
        reasons.push(format!(
            "requires at least {:.1} GB RAM, host has {:.1} GB",
            model.requires.min_ram_bytes as f64 / GIB,
            hardware.total_ram_bytes as f64 / GIB
        ));
    }
    let headline_en = headline(HeadlineInputs {
        kind: model.kind,
        verdict,
        realtime,
        tok_s,
        summary_secs,
        memory: mem_needed,
        available: mem_available,
        total_ram: hardware.total_ram_bytes,
        russian: false,
    });
    let headline_ru = headline(HeadlineInputs {
        kind: model.kind,
        verdict,
        realtime,
        tok_s,
        summary_secs,
        memory: mem_needed,
        available: mem_available,
        total_ram: hardware.total_ram_bytes,
        russian: true,
    });
    Prediction {
        model_id: model.id.clone(),
        verdict,
        headline_ru,
        headline_en,
        est_realtime_x: realtime,
        est_tok_s: tok_s,
        est_prefill_tok_s: prefill,
        est_summary_secs_for_1h_meeting: summary_secs,
        mem_needed,
        mem_available,
        reasons,
        backend,
    }
}

const MIB: u64 = 1024 * 1024;
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

fn select_backend(hardware: &HardwareProfile, kind: ModelKind) -> String {
    if hardware.apple.is_some() {
        return if kind == ModelKind::Llm {
            "metal"
        } else {
            "coreml"
        }
        .to_owned();
    }
    hardware
        .gpus
        .iter()
        .find(|gpu| !gpu.backend_hint.is_empty())
        .map(|gpu| gpu.backend_hint.clone())
        .unwrap_or_else(|| "cpu".to_owned())
}

fn available_memory_for(hardware: &HardwareProfile, backend: &str) -> u64 {
    if hardware.apple.is_some() {
        if let Some(maximum) = hardware
            .apple
            .as_ref()
            .and_then(|apple| apple.recommended_max_working_set_bytes)
        {
            return maximum;
        }
        return (hardware.total_ram_bytes as f64 * 0.70) as u64;
    }
    if let Some(gpu) = hardware.gpus.iter().find(|gpu| gpu.backend_hint == backend)
        && let Some(vram) = gpu.vram_bytes
    {
        return vram.saturating_sub(512 * MIB);
    }
    (hardware.available_ram_bytes as f64 * 0.75) as u64
}

struct HeadlineInputs {
    kind: ModelKind,
    verdict: Verdict,
    realtime: Option<f64>,
    tok_s: Option<f64>,
    summary_secs: Option<f64>,
    memory: u64,
    available: u64,
    total_ram: u64,
    russian: bool,
}

fn headline(inputs: HeadlineInputs) -> String {
    let HeadlineInputs {
        kind,
        verdict,
        realtime,
        tok_s,
        summary_secs,
        memory,
        available,
        total_ram,
        russian,
    } = inputs;
    if verdict == Verdict::WontRun {
        return if russian {
            format!(
                "Не поместится · {:.1} из {:.1} ГБ",
                memory as f64 / GIB,
                available as f64 / GIB
            )
        } else {
            format!(
                "Won't fit · {:.1} of {:.1} GB",
                memory as f64 / GIB,
                available as f64 / GIB
            )
        };
    }
    match kind {
        ModelKind::Asr => {
            let rate = realtime.unwrap_or_default();
            if russian {
                format!("≈ {rate:.0}× реального времени")
            } else {
                format!("≈ {rate:.0}× realtime")
            }
        }
        ModelKind::Llm => {
            let rate = tok_s.unwrap_or_default();
            let memory = memory as f64 / GIB;
            let total_ram = total_ram as f64 / GIB;
            if russian {
                format!("≈ {rate:.0} ток/с · {memory:.1} из {total_ram:.0} ГБ")
            } else {
                format!("≈ {rate:.0} tok/s · {memory:.1} of {total_ram:.0} GB")
            }
        }
        ModelKind::Embed => {
            let seconds = summary_secs.unwrap_or_default();
            if russian {
                format!("индексация 1 ч встречи ≈ {seconds:.0} с")
            } else {
                format!("index 1 h meeting in ≈ {seconds:.0} s")
            }
        }
        _ => {
            if russian {
                "запустится".to_owned()
            } else {
                "will run".to_owned()
            }
        }
    }
}
