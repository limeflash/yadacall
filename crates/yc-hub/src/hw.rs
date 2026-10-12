//! Cross-platform hardware probe for catalog predictions.

use std::{path::Path, time::SystemTime};

#[cfg(target_os = "macos")]
use std::process::Command;

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {}

use serde::{Deserialize, Serialize};
use sysinfo::System;

use crate::{Error, Result};

/// Serializable snapshot of the host relevant to model inference.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareProfile {
    /// CPU marketing name or vendor brand string.
    pub cpu_brand: String,
    /// Physical CPU cores where the platform reports them.
    pub physical_cores: usize,
    /// Apple performance cores, where available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub performance_cores: Option<usize>,
    /// Apple efficiency cores, where available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efficiency_cores: Option<usize>,
    /// Logical CPU count.
    pub logical_cores: usize,
    /// Runtime-detected instruction set features.
    pub isa: Vec<String>,
    /// Total physical memory in bytes.
    pub total_ram_bytes: u64,
    /// Available physical memory in bytes at probe time.
    pub available_ram_bytes: u64,
    /// Operating system name.
    pub os: String,
    /// Target architecture name.
    pub arch: String,
    /// Detected GPU devices.
    pub gpus: Vec<GpuInfo>,
    /// Apple Silicon identity and memory limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apple: Option<AppleChipInfo>,
    /// Estimated unified/system memory bandwidth in decimal GB/s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bandwidth_gbps: Option<f64>,
    /// Whether the host exposes a neural accelerator suitable for Core ML.
    pub npu_present: bool,
    /// Available free bytes at the configured model directory.
    pub model_disk_free_bytes: u64,
    /// Unix seconds when the snapshot was made.
    pub probed_at_unix: u64,
}

/// One graphics or accelerator device.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuInfo {
    /// Display name reported by the system.
    pub name: String,
    /// Vendor string (`apple`, `nvidia`, `amd`, `intel`, or `unknown`).
    pub vendor: String,
    /// Dedicated VRAM, or `None` for a unified-memory GPU.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_bytes: Option<u64>,
    /// Whether this GPU shares host memory.
    pub unified_memory: bool,
    /// Best matching inference backend (`metal`, `cuda`, `vulkan`, `directml`).
    pub backend_hint: String,
}

/// Apple chip identity and Metal memory ceiling.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleChipInfo {
    /// Chip identity, for example `Apple M1 Pro`.
    pub chip_name: String,
    /// GPU core count, when the public platform API reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_cores: Option<usize>,
    /// Apple-published or conservatively inferred peak bandwidth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bandwidth_gbps: Option<f64>,
    /// Metal's `recommendedMaxWorkingSetSize`, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_max_working_set_bytes: Option<u64>,
    /// Apple Silicon has a Neural Engine; this indicates availability, not ORT usage.
    pub neural_engine_present: bool,
}

/// Probe this machine and the free space at `models_dir`.
pub fn probe(models_dir: &Path) -> Result<HardwareProfile> {
    std::fs::create_dir_all(models_dir)?;
    let mut system = System::new();
    system.refresh_cpu_all();
    system.refresh_memory();
    let cpu_brand = system
        .cpus()
        .first()
        .map(|cpu| cpu.brand().to_owned())
        .unwrap_or_else(|| "Unknown CPU".to_owned());
    let logical_cores = system.cpus().len().max(1);
    let physical_cores = System::physical_core_count().unwrap_or(logical_cores);
    #[cfg(target_os = "macos")]
    let (apple, perf_cores, eff_cores, gpus) = {
        let (profile_apple, gpu, perf, efficiency) = probe_macos(&cpu_brand);
        (
            profile_apple,
            perf,
            efficiency,
            gpu.into_iter().collect::<Vec<_>>(),
        )
    };

    #[cfg(target_os = "linux")]
    let (apple, perf_cores, eff_cores, gpus) = (None, None, None, probe_linux_gpus());

    #[cfg(target_os = "windows")]
    let (apple, perf_cores, eff_cores, gpus) = (None, None, None, probe_windows_gpus());

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let (apple, perf_cores, eff_cores, gpus) = (None, None, None, Vec::new());

    let memory_bandwidth_gbps = apple.as_ref().and_then(|chip| chip.memory_bandwidth_gbps);
    let npu_present = apple
        .as_ref()
        .is_some_and(|chip| chip.neural_engine_present);
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Ok(HardwareProfile {
        cpu_brand,
        physical_cores,
        performance_cores: perf_cores,
        efficiency_cores: eff_cores,
        logical_cores,
        isa: detect_isa(),
        total_ram_bytes: system.total_memory(),
        available_ram_bytes: available_ram_bytes(&system),
        os: System::name().unwrap_or_else(|| std::env::consts::OS.to_owned()),
        arch: std::env::consts::ARCH.to_owned(),
        gpus,
        apple,
        memory_bandwidth_gbps,
        npu_present,
        model_disk_free_bytes: fs2::available_space(models_dir)?,
        probed_at_unix: now,
    })
}

fn available_ram_bytes(system: &System) -> u64 {
    #[cfg(target_os = "macos")]
    if system.available_memory() == 0
        && let Some(available) = mac_available_ram_bytes()
    {
        return available;
    }
    system.available_memory()
}

/// Read a recent cached profile or probe and atomically cache a fresh one.
pub fn probe_cached(
    models_dir: &Path,
    cache_path: &Path,
    max_age_seconds: u64,
) -> Result<HardwareProfile> {
    if let Ok(bytes) = std::fs::read(cache_path)
        && let Ok(profile) = serde_json::from_slice::<HardwareProfile>(&bytes)
    {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now.saturating_sub(profile.probed_at_unix) <= max_age_seconds {
            return Ok(profile);
        }
    }
    let profile = probe(models_dir)?;
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = cache_path.with_extension("json.tmp");
    std::fs::write(
        &temp,
        serde_json::to_vec(&profile)
            .map_err(|error| Error::Storage(format!("serialize hardware profile: {error}")))?,
    )?;
    std::fs::rename(temp, cache_path)?;
    Ok(profile)
}

fn detect_isa() -> Vec<String> {
    #[cfg(target_arch = "aarch64")]
    {
        vec!["neon".to_owned()]
    }
    #[cfg(target_arch = "x86_64")]
    {
        let mut features = Vec::new();
        if std::is_x86_feature_detected!("avx2") {
            features.push("avx2".to_owned());
        }
        if std::is_x86_feature_detected!("avx512f") {
            features.push("avx512".to_owned());
        }
        if std::is_x86_feature_detected!("amx-tile") {
            features.push("amx".to_owned());
        }
        features
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
fn probe_macos(
    cpu_brand: &str,
) -> (
    Option<AppleChipInfo>,
    Option<GpuInfo>,
    Option<usize>,
    Option<usize>,
) {
    let sysctl_brand = command_value("sysctl", &["-n", "machdep.cpu.brand_string"]);
    let profiler = Command::new("system_profiler")
        .args(["-json", "SPHardwareDataType", "SPDisplaysDataType"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok());
    let chip_name = profiler
        .as_ref()
        .and_then(find_apple_chip)
        .or(sysctl_brand)
        .unwrap_or_else(|| cpu_brand.to_owned());
    let chip_name = if chip_name.contains("Apple") {
        chip_name
    } else {
        format!("Apple {chip_name}")
    };
    let gpu_cores = profiler.as_ref().and_then(find_gpu_cores);
    let bandwidth = apple_bandwidth_for_gpu(&chip_name, gpu_cores);
    let (gpu_name, recommended_max_working_set_bytes) = metal_device_info();
    let gpu = gpu_name.map(|name| GpuInfo {
        name,
        vendor: "apple".to_owned(),
        vram_bytes: None,
        unified_memory: true,
        backend_hint: "metal".to_owned(),
    });
    let apple = Some(AppleChipInfo {
        chip_name,
        gpu_cores,
        memory_bandwidth_gbps: bandwidth,
        recommended_max_working_set_bytes,
        neural_engine_present: true,
    });
    let perf_cores = sysctl_usize("hw.perflevel0.physicalcpu");
    let efficiency_cores = sysctl_usize("hw.perflevel1.physicalcpu");
    (apple, gpu, perf_cores, efficiency_cores)
}

#[cfg(target_os = "macos")]
fn metal_device_info() -> (Option<String>, Option<u64>) {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_foundation::NSString;
    use objc2_metal::MTLDevice;

    let device: Option<Retained<ProtocolObject<dyn MTLDevice>>> =
        objc2_metal::MTLCreateSystemDefaultDevice();
    device.map_or((None, None), |device| {
        let name: Retained<NSString> = device.name();
        let max_working_set = device.recommendedMaxWorkingSetSize();
        (
            Some(name.to_string()),
            (max_working_set > 0).then_some(max_working_set),
        )
    })
}

#[cfg(target_os = "macos")]
fn find_apple_chip(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) if text.contains("Apple M") => Some(text.clone()),
        serde_json::Value::Array(items) => items.iter().find_map(find_apple_chip),
        serde_json::Value::Object(items) => items.values().find_map(find_apple_chip),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn find_gpu_cores(value: &serde_json::Value) -> Option<usize> {
    match value {
        serde_json::Value::Array(items) => items.iter().find_map(find_gpu_cores),
        serde_json::Value::Object(items) => {
            if items
                .get("sppci_device_type")
                .and_then(serde_json::Value::as_str)
                == Some("spdisplays_gpu")
                && let Some(cores) = items
                    .get("sppci_cores")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| value.parse().ok())
            {
                return Some(cores);
            }
            items.values().find_map(find_gpu_cores)
        }
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn mac_available_ram_bytes() -> Option<u64> {
    let output = Command::new("vm_stat").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let page_size = text
        .split("page size of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    let pages = [
        "Pages free:",
        "Pages inactive:",
        "Pages speculative:",
        "Pages purgeable:",
    ]
    .iter()
    .filter_map(|name| {
        let line = text
            .lines()
            .find(|line| line.trim_start().starts_with(name))?;
        line.split_once(':')?
            .1
            .trim()
            .trim_end_matches('.')
            .replace(',', "")
            .parse::<u64>()
            .ok()
    })
    .sum::<u64>();
    Some(pages.saturating_mul(page_size))
}

#[cfg(target_os = "macos")]
fn command_value(program: &str, args: &[&str]) -> Option<String> {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|output| !output.is_empty())
}

#[cfg(target_os = "macos")]
fn sysctl_usize(name: &str) -> Option<usize> {
    command_value("sysctl", &["-n", name])?.parse().ok()
}

/// Return the known peak unified-memory bandwidth for the identified Apple chip.
///
/// Apple sources: [M1 Pro/Max](https://www.apple.com/newsroom/2021/10/introducing-m1-pro-and-m1-max-the-most-powerful-chips-apple-has-ever-built/),
/// [M1 Ultra](https://www.apple.com/newsroom/2022/03/apple-unveils-m1-ultra-the-worlds-most-powerful-chip-for-a-personal-computer/),
/// [M2](https://www.apple.com/newsroom/2022/06/apple-unveils-m2-with-breakthrough-performance-and-capabilities/),
/// [M2 Pro/Max](https://www.apple.com/newsroom/2023/01/apple-unveils-m2-pro-and-m2-max-next-generation-chips-for-next-level-workflows/),
/// [M2 Ultra](https://www.apple.com/newsroom/2023/06/apple-introduces-m2-ultra/),
/// [M3 Pro/Max](https://support.apple.com/en-us/117737), [M3 Ultra](https://www.apple.com/newsroom/2025/03/apple-reveals-m3-ultra-taking-apple-silicon-to-a-new-extreme/),
/// [M4](https://support.apple.com/en-us/121552), [M4 Pro/Max](https://support.apple.com/en-us/121554),
/// and [M5](https://www.apple.com/newsroom/2025/10/apple-unveils-new-14-inch-macbook-pro-powered-by-the-m5-chip/),
/// [M5 Pro/Max](https://www.apple.com/newsroom/2026/03/apple-introduces-macbook-pro-with-all-new-m5-pro-and-m5-max/).
/// The base M1's 68.25 GB/s is inferred from Apple's statement that M2's 100 GB/s is 50% faster.
pub fn apple_bandwidth_gbps(chip_name: &str) -> Option<f64> {
    let chip = chip_name.to_ascii_lowercase();
    let tier = ["ultra", "max", "pro"]
        .iter()
        .find(|tier| chip.contains(**tier))
        .copied()
        .unwrap_or("base");
    let generation = (1..=5).find(|generation| chip.contains(&format!("m{generation}")))?;
    let bandwidth = match (generation, tier) {
        (1, "ultra") | (2, "ultra") | (3, "ultra") => 800.0,
        (1, "max") | (2, "max") => 400.0,
        (1, "pro") | (2, "pro") => 200.0,
        (1, "base") => 68.25,
        (2, "base") | (3, "base") => 100.0,
        (3, "pro") => 150.0,
        (3, "max") => 300.0,
        (4, "base") => 120.0,
        (4, "pro") => 273.0,
        (4, "max") => 410.0,
        (5, "base") => 153.0,
        (5, "pro") => 307.0,
        (5, "max") => 614.0,
        _ => return None,
    };
    Some(bandwidth)
}

#[cfg(target_os = "macos")]
fn apple_bandwidth_for_gpu(chip_name: &str, gpu_cores: Option<usize>) -> Option<f64> {
    let chip = chip_name.to_ascii_lowercase();
    match (chip.contains("m3 max"), chip.contains("m4 max"), gpu_cores) {
        (true, _, Some(cores)) if cores >= 40 => Some(400.0),
        (_, true, Some(cores)) if cores >= 40 => Some(546.0),
        _ => apple_bandwidth_gbps(chip_name),
    }
}

#[cfg(target_os = "linux")]
fn probe_linux_gpus() -> Vec<GpuInfo> {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("card") || name.contains('-') {
                return None;
            }
            let device = entry.path().join("device");
            let vendor_id = std::fs::read_to_string(device.join("vendor")).ok()?;
            let vendor = match vendor_id.trim() {
                "0x10de" => "nvidia",
                "0x1002" => "amd",
                "0x8086" => "intel",
                _ => "unknown",
            };
            let vram = std::fs::read_to_string(device.join("mem_info_vram_total"))
                .ok()
                .and_then(|value| value.trim().parse().ok());
            Some(GpuInfo {
                name: name.clone(),
                vendor: vendor.to_owned(),
                vram_bytes: vram,
                unified_memory: false,
                backend_hint: if vendor == "nvidia" {
                    "cuda".to_owned()
                } else {
                    "vulkan".to_owned()
                },
            })
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn probe_windows_gpus() -> Vec<GpuInfo> {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
        DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIAdapter3, IDXGIFactory1,
    };
    use windows::core::Interface;

    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Vec::new();
    };
    let mut adapters = Vec::new();
    for index in 0..32 {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        let Ok(description) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if description.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let name = String::from_utf16_lossy(&description.Description)
            .trim_matches('\0')
            .trim()
            .to_owned();
        let vendor = match description.VendorId {
            0x10de => "nvidia",
            0x1002 => "amd",
            0x8086 => "intel",
            _ => "unknown",
        };
        let backend_hint = if vendor == "nvidia" {
            "cuda"
        } else {
            "directml"
        };
        let local_budget = adapter.cast::<IDXGIAdapter3>().ok().and_then(|adapter3| {
            let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
            unsafe {
                adapter3
                    .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info)
                    .ok()?;
            }
            Some(info.Budget)
        });
        let vram = if description.DedicatedVideoMemory > 0 {
            local_budget.or(Some(description.DedicatedVideoMemory as u64))
        } else {
            None
        };
        adapters.push(GpuInfo {
            name,
            vendor: vendor.to_owned(),
            vram_bytes: vram,
            unified_memory: description.DedicatedVideoMemory == 0,
            backend_hint: backend_hint.to_owned(),
        });
    }
    adapters
}

#[cfg(test)]
mod tests {
    use super::apple_bandwidth_gbps;

    #[test]
    fn returns_published_or_estimated_apple_memory_bandwidths() {
        assert_eq!(apple_bandwidth_gbps("Apple M1"), Some(68.25));
        assert_eq!(apple_bandwidth_gbps("Apple M1 Pro"), Some(200.0));
        assert_eq!(apple_bandwidth_gbps("Apple M2 Ultra"), Some(800.0));
        assert_eq!(apple_bandwidth_gbps("Apple M3 Max"), Some(300.0));
        assert_eq!(apple_bandwidth_gbps("Apple M4 Max"), Some(410.0));
        assert_eq!(apple_bandwidth_gbps("Apple M5"), Some(153.0));
        assert_eq!(apple_bandwidth_gbps("Apple M6"), None);
    }
}
