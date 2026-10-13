//! Analytical model-fit and throughput predictions.

use serde::{Deserialize, Serialize};

use crate::{
    benchmark::Benchmarks,
    calibration::Calibration,
    catalog::{AttentionCost, ModelCost, ModelKind, ModelSpec},
    hw::{GpuInfo, HardwareProfile},
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
            // Indexing runs in the background after the call, so tens of seconds per hour of
            // meeting is perfectly fine; only minutes start to feel slow.
            verdict = if mem_needed > mem_available {
                Verdict::WontRun
            } else if seconds <= 30.0 {
                Verdict::Great
            } else if seconds <= 120.0 {
                Verdict::Ok
            } else if seconds <= 600.0 {
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
            let weights = model.files.iter().map(|file| file.size).sum::<u64>();
            let (bytes_per_token, using_weight_fallback) = decode_bytes_per_token(model, weights);
            let raw = bandwidth * 1_000_000_000.0 * model.cost.efficiency / bytes_per_token;
            let factor = calibration.factor(ModelKind::Llm, &backend).unwrap_or(1.0);
            let mut speed = raw * factor;
            let (params_b, parameter_basis) = if model.cost.active_params > 0.0 {
                (
                    model.cost.active_params / 1_000_000_000.0,
                    "active parameters",
                )
            } else {
                (
                    model.cost.total_params / 1_000_000_000.0,
                    "total parameters because activeParams is unset",
                )
            };
            let params_b = params_b.max(0.1);
            let (prefill_gflops, prefill_method) =
                prefill_throughput_gflops(hardware, benchmarks.gemm_gflops, &backend);
            let raw_prefill = (prefill_gflops / (2.0 * params_b)).max(0.1);
            let prefill_factor = calibration
                .prefill_factor(ModelKind::Llm, &backend)
                .unwrap_or(1.0);
            let prefill_speed = raw_prefill * prefill_factor;
            let cache = estimate_kv_cache(&model.cost);
            let compute_buffer =
                ((weights as f64 * model.cost.buffer_fraction.max(0.0)) as u64).max(512 * MIB);
            mem_needed = weights
                .saturating_add(cache.total_bytes)
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
            if using_weight_fallback {
                reasons.push(format!(
                    "catalog has no bytesPerToken estimate; decode falls back to {:.2} GB total model files",
                    bytes_per_token / 1_000_000_000.0
                ));
            }
            reasons.push(format!(
                "memory = {:.2} GB total weights + {:.2} GB {} + {:.2} GB execution buffer",
                weights as f64 / GIB,
                cache.total_bytes as f64 / GIB,
                cache.description,
                compute_buffer as f64 / GIB
            ));
            reasons.push(format!(
                "prefill estimate uses {prefill_method}, 2 FLOP per parameter per token, and {params_b:.1}B {parameter_basis}"
            ));
            reasons.push(format!(
                "one-hour summary estimate uses 9,000 prompt tokens ÷ {prefill_speed:.1} prefill tok/s + 800 output tokens ÷ {:.1} decode tok/s",
                speed.max(0.1)
            ));
            if factor != 1.0 {
                reasons.push(format!(
                    "measured LLM calibration scales decode estimate by {factor:.2}×"
                ));
            }
            if prefill_factor != 1.0 {
                reasons.push(format!(
                    "measured LLM prefill calibration scales estimate by {prefill_factor:.2}×"
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
const Q8_CACHE_BYTES_PER_ELEMENT: f64 = 1.06;
const APPLE_GPU_EFFICIENCY: f64 = 0.55;

fn decode_bytes_per_token(model: &ModelSpec, total_weights: u64) -> (f64, bool) {
    if model.cost.bytes_per_token > 0 {
        (model.cost.bytes_per_token as f64, false)
    } else if total_weights > 0 {
        (total_weights as f64, true)
    } else {
        (1.0, true)
    }
}

struct KvCacheEstimate {
    total_bytes: u64,
    description: String,
}

fn estimate_kv_cache(cost: &ModelCost) -> KvCacheEstimate {
    let context = u64::from(cost.context.max(1));
    let (context_cache_bytes, fixed_state_bytes, description) = match &cost.attention {
        AttentionCost::Standard => {
            let sliding = u64::from(cost.sliding_window);
            let local_fraction = cost.local_layer_fraction.clamp(0.0, 1.0);
            let global_fraction = if cost.global_layer_fraction > 0.0 {
                cost.global_layer_fraction.clamp(0.0, 1.0)
            } else {
                1.0 - local_fraction
            };
            let local_layers = f64::from(cost.n_layers) * local_fraction;
            let global_layers = f64::from(cost.n_layers) * global_fraction;
            let local_context = if sliding > 0 {
                context.min(sliding)
            } else {
                context
            };
            let local_head_dim = if cost.local_head_dim > 0 {
                cost.local_head_dim
            } else {
                cost.head_dim
            };
            let element_count = f64::from(cost.kv_heads)
                * (local_layers * f64::from(local_head_dim) * local_context as f64
                    + global_layers * f64::from(cost.head_dim) * context as f64);
            let bytes = (2.0 * Q8_CACHE_BYTES_PER_ELEMENT * element_count).ceil() as u64;
            let effective_context = if cost.n_layers > 0 {
                ((local_layers * local_context as f64 + global_layers * context as f64)
                    / f64::from(cost.n_layers))
                .ceil() as u64
            } else {
                context
            };
            (
                bytes,
                0,
                format!(
                    "q8_0 K/V cache at {effective_context} effective context tokens (1.06 bytes per element)"
                ),
            )
        }
        AttentionCost::Mla {
            kv_lora_rank,
            qk_rope_head_dim,
        } => {
            let elements_per_token = u64::from(*kv_lora_rank + *qk_rope_head_dim);
            let bytes = (f64::from(cost.n_layers)
                * elements_per_token as f64
                * context as f64
                * Q8_CACHE_BYTES_PER_ELEMENT)
                .ceil() as u64;
            (
                bytes,
                0,
                format!(
                    "q8_0 MLA cache at {context} tokens across {} layers ({} latent + {} RoPE elements per token)",
                    cost.n_layers, kv_lora_rank, qk_rope_head_dim
                ),
            )
        }
        AttentionCost::Hybrid {
            full_attention_layers,
            linear_state_bytes,
        } => {
            let full_layers = if cost.n_layers > 0 {
                full_attention_layers.min(&cost.n_layers)
            } else {
                full_attention_layers
            };
            let elements = f64::from(*full_layers)
                * f64::from(cost.kv_heads)
                * f64::from(cost.head_dim)
                * context as f64;
            let bytes = (2.0 * Q8_CACHE_BYTES_PER_ELEMENT * elements).ceil() as u64;
            (
                bytes,
                *linear_state_bytes,
                format!(
                    "q8_0 full-attention cache at {context} tokens across {full_layers} layers plus {:.2} GB fixed linear state",
                    *linear_state_bytes as f64 / GIB
                ),
            )
        }
    };
    KvCacheEstimate {
        total_bytes: context_cache_bytes.saturating_add(fixed_state_bytes),
        description,
    }
}

/// Return effective f16 throughput, using broad sustained-throughput tiers rather than marketing
/// peak rates for CUDA/Vulkan devices. Metal uses a peak estimate with an explicit efficiency.
fn prefill_throughput_gflops(
    hardware: &HardwareProfile,
    cpu_gemm_gflops: f64,
    backend: &str,
) -> (f64, String) {
    if backend == "metal"
        && let Some(gpu_cores) = hardware.apple.as_ref().and_then(|apple| apple.gpu_cores)
    {
        let peak = gpu_cores as f64 * 128.0 * 2.0 * 1.3;
        let effective = peak * APPLE_GPU_EFFICIENCY;
        return (
            effective,
            format!(
                "Metal f16 estimate uses {gpu_cores} GPU cores × 128 ALUs × 2 FLOP × 1.3 GHz × {:.0}% efficiency = {effective:.0} GFLOP/s",
                APPLE_GPU_EFFICIENCY * 100.0
            ),
        );
    }
    if matches!(backend, "cuda" | "vulkan")
        && let Some(gpu) = hardware.gpus.iter().find(|gpu| gpu.backend_hint == backend)
        && let Some(effective) = gpu_prefill_tier(gpu)
    {
        return (
            effective,
            format!(
                "conservative {backend} f16 tier for {} {} = {effective:.0} GFLOP/s",
                gpu.vendor, gpu.name
            ),
        );
    }
    let effective = cpu_gemm_gflops.max(0.1);
    (
        effective,
        format!("CPU f32 GEMM probe = {effective:.1} GFLOP/s"),
    )
}

/// Conservative effective f16 GFLOP/s tiers keyed by reported vendor, VRAM, and GPU name.
fn gpu_prefill_tier(gpu: &GpuInfo) -> Option<f64> {
    let name = gpu.name.to_ascii_lowercase();
    let vram_gb = gpu.vram_bytes.unwrap_or_default() as f64 / 1_000_000_000.0;
    match gpu.vendor.as_str() {
        "nvidia" => Some(if name.contains("rtx 50") {
            18_000.0
        } else if name.contains("rtx 40") {
            12_000.0
        } else if name.contains("rtx 30") {
            7_000.0
        } else if name.contains("rtx") {
            5_000.0
        } else if name.contains("gtx") {
            1_800.0
        } else if vram_gb >= 24.0 {
            14_000.0
        } else if vram_gb >= 12.0 {
            9_000.0
        } else if vram_gb >= 8.0 {
            6_000.0
        } else if vram_gb >= 4.0 {
            2_500.0
        } else {
            1_200.0
        }),
        "amd" => Some(if name.contains("rx 9") || name.contains("rx 7") {
            8_000.0
        } else if name.contains("rx 6") {
            4_500.0
        } else if name.contains("rx") {
            2_000.0
        } else if vram_gb >= 16.0 {
            8_000.0
        } else if vram_gb >= 8.0 {
            5_000.0
        } else if vram_gb >= 4.0 {
            2_200.0
        } else {
            900.0
        }),
        "intel" => Some(if name.contains("arc") {
            3_000.0
        } else if name.contains("iris") {
            800.0
        } else if name.contains("uhd") {
            350.0
        } else if vram_gb >= 16.0 {
            4_000.0
        } else if vram_gb >= 8.0 {
            2_500.0
        } else if vram_gb >= 4.0 {
            1_200.0
        } else {
            550.0
        }),
        _ => None,
    }
}

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
    if verdict == Verdict::WontRun && memory <= available {
        return if russian {
            "Не потянет · слишком медленно на этом компьютере".to_owned()
        } else {
            "Too slow on this computer".to_owned()
        };
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        benchmark::Benchmarks,
        catalog::{self, AttentionCost},
        hw::AppleChipInfo,
    };

    const RAM_16_GIB: u64 = 16 * 1024 * 1024 * 1024;

    fn model(id: &str) -> ModelSpec {
        catalog::embedded()
            .expect("embedded catalog")
            .models
            .into_iter()
            .find(|model| model.id == id)
            .expect("catalog model")
    }

    fn m1_pro_profile() -> HardwareProfile {
        HardwareProfile {
            cpu_brand: "Apple M1 Pro".to_owned(),
            physical_cores: 8,
            logical_cores: 16,
            total_ram_bytes: RAM_16_GIB,
            available_ram_bytes: RAM_16_GIB,
            apple: Some(AppleChipInfo {
                chip_name: "Apple M1 Pro".to_owned(),
                gpu_cores: Some(16),
                memory_bandwidth_gbps: Some(200.0),
                recommended_max_working_set_bytes: Some(RAM_16_GIB),
                neural_engine_present: true,
            }),
            memory_bandwidth_gbps: Some(200.0),
            ..HardwareProfile::default()
        }
    }

    fn cpu_windows_profile() -> HardwareProfile {
        HardwareProfile {
            cpu_brand: "8-core Windows CPU".to_owned(),
            physical_cores: 8,
            logical_cores: 8,
            total_ram_bytes: RAM_16_GIB,
            available_ram_bytes: RAM_16_GIB,
            os: "windows".to_owned(),
            arch: "x86_64".to_owned(),
            ..HardwareProfile::default()
        }
    }

    #[test]
    fn gemma_e2b_prefill_uses_m1_pro_gpu_and_decode_stays_in_band() {
        let model = model("gemma-4-e2b-q4-0");
        let benchmarks = Benchmarks {
            memory_bandwidth_gbps: 200.0,
            gemm_gflops: 278.0,
            ..Benchmarks::default()
        };
        let prediction = predict(
            &model,
            &m1_pro_profile(),
            &benchmarks,
            &Calibration::default(),
        );

        let prefill = prediction.est_prefill_tok_s.expect("prefill estimate");
        assert!((315.0..=1_260.0).contains(&prefill), "prefill={prefill}");
        assert!(
            (35.0..=48.0).contains(&prediction.est_tok_s.expect("decode estimate")),
            "decode={:?}",
            prediction.est_tok_s
        );
        assert_eq!(prediction.backend, "metal");
    }

    #[test]
    fn gemma_e2b_cpu_profile_has_a_sane_slower_prefill_estimate() {
        let model = model("gemma-4-e2b-q4-0");
        let prediction = predict(
            &model,
            &cpu_windows_profile(),
            &Benchmarks {
                memory_bandwidth_gbps: 27.0,
                gemm_gflops: 81.0,
                ..Benchmarks::default()
            },
            &Calibration::default(),
        );
        let prefill = prediction.est_prefill_tok_s.expect("prefill estimate");
        assert!((10.0..30.0).contains(&prefill), "prefill={prefill}");
        assert!(prefill < 630.0);
        assert_eq!(prediction.backend, "cpu");
    }

    #[test]
    fn gemma_moe_decode_uses_active_weight_bytes_but_memory_uses_all_weights() {
        let moe = model("gemma-4-26b-a4b-q4-0");
        assert_eq!(moe.cost.bytes_per_token, 2_204_323_577);
        let mut dense = moe.clone();
        dense.cost.active_params = dense.cost.total_params;
        dense.cost.bytes_per_token = dense.files.iter().map(|file| file.size).sum();
        let hardware = m1_pro_profile();
        let benchmarks = Benchmarks {
            memory_bandwidth_gbps: 200.0,
            gemm_gflops: 278.0,
            ..Benchmarks::default()
        };
        let moe_prediction = predict(&moe, &hardware, &benchmarks, &Calibration::default());
        let dense_prediction = predict(&dense, &hardware, &benchmarks, &Calibration::default());
        assert!(
            moe_prediction.est_tok_s > dense_prediction.est_tok_s,
            "MoE={:?}, dense={:?}",
            moe_prediction.est_tok_s,
            dense_prediction.est_tok_s
        );
        assert_eq!(moe_prediction.mem_needed, dense_prediction.mem_needed);
        assert!(moe_prediction.mem_needed > moe.files[0].size);
    }

    #[test]
    fn mla_and_hybrid_cache_sizes_match_their_layouts() {
        let mla = ModelCost {
            n_layers: 2,
            context: 100,
            attention: AttentionCost::Mla {
                kv_lora_rank: 512,
                qk_rope_head_dim: 64,
            },
            ..ModelCost::default()
        };
        assert_eq!(estimate_kv_cache(&mla).total_bytes, 122_112);

        let hybrid = ModelCost {
            n_layers: 6,
            kv_heads: 2,
            head_dim: 16,
            context: 100,
            attention: AttentionCost::Hybrid {
                full_attention_layers: 2,
                linear_state_bytes: 4_096,
            },
            ..ModelCost::default()
        };
        assert_eq!(estimate_kv_cache(&hybrid).total_bytes, 17_664);
    }

    #[test]
    fn dense_gemma_memory_uses_total_weights_and_q8_kv_size() {
        let model = model("gemma-4-e2b-q4-0");
        let weights = model.files.iter().map(|file| file.size).sum::<u64>();
        let cost = &model.cost;
        let context = f64::from(cost.context.max(1));
        let local_fraction = cost.local_layer_fraction.clamp(0.0, 1.0);
        let global_fraction = if cost.global_layer_fraction > 0.0 {
            cost.global_layer_fraction.clamp(0.0, 1.0)
        } else {
            1.0 - local_fraction
        };
        let local_context = if cost.sliding_window > 0 {
            context.min(f64::from(cost.sliding_window))
        } else {
            context
        };
        let local_dim = if cost.local_head_dim > 0 {
            cost.local_head_dim
        } else {
            cost.head_dim
        };
        let cached_elements = f64::from(cost.kv_heads)
            * (f64::from(cost.n_layers) * local_fraction * f64::from(local_dim) * local_context
                + f64::from(cost.n_layers) * global_fraction * f64::from(cost.head_dim) * context);
        let expected_kv_bytes = (2.0 * 1.06 * cached_elements).ceil() as u64;
        let compute_buffer = ((weights as f64 * cost.buffer_fraction) as u64).max(512 * MIB);
        let prediction = predict(
            &model,
            &m1_pro_profile(),
            &Benchmarks {
                memory_bandwidth_gbps: 200.0,
                gemm_gflops: 278.0,
                ..Benchmarks::default()
            },
            &Calibration::default(),
        );
        assert_eq!(
            prediction.mem_needed,
            weights + expected_kv_bytes + compute_buffer
        );
    }
}
