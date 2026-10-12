//! Device enumeration and the two hints the UI needs: "use headphones" and "Bluetooth mic".

use cpal::traits::{DeviceTrait, HostTrait};
use yc_core::{Error, Result};

/// One audio device as shown by `yadacall devices`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Stable id usable in [`crate::DeviceSel::Named`].
    pub id: String,
    pub name: String,
    pub is_default: bool,
    /// Default sample rate in Hz, when the host reports one.
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    /// `built-in`, `usb`, `bluetooth`, … when known.
    pub transport: Option<String>,
}

fn describe(d: &cpal::Device, default_id: Option<&str>, input: bool) -> DeviceInfo {
    let id = d.id().map(|i| i.to_string()).unwrap_or_default();
    let desc = d.description().ok();
    let cfg = if input {
        d.default_input_config()
    } else {
        d.default_output_config()
    }
    .ok();
    DeviceInfo {
        is_default: default_id == Some(id.as_str()),
        name: desc
            .as_ref()
            .map(|x| x.name().to_owned())
            .unwrap_or_else(|| id.clone()),
        transport: desc
            .as_ref()
            .map(|x| format!("{:?}", x.interface_type()).to_lowercase())
            .filter(|t| t != "unknown"),
        sample_rate: cfg.as_ref().map(|c| c.sample_rate()),
        channels: cfg.as_ref().map(|c| c.channels()),
        id,
    }
}

/// All input devices of the default host.
pub fn list_input_devices() -> Result<Vec<DeviceInfo>> {
    let host = cpal::default_host();
    let default_id = host
        .default_input_device()
        .and_then(|d| d.id().ok())
        .map(|i| i.to_string());
    let devices = host
        .input_devices()
        .map_err(|e| Error::Device(e.to_string()))?;
    Ok(devices
        .map(|d| describe(&d, default_id.as_deref(), true))
        .collect())
}

/// All output devices of the default host (the system-audio source follows the default one).
pub fn list_output_devices() -> Result<Vec<DeviceInfo>> {
    let host = cpal::default_host();
    let default_id = host
        .default_output_device()
        .and_then(|d| d.id().ok())
        .map(|i| i.to_string());
    let devices = host
        .output_devices()
        .map_err(|e| Error::Device(e.to_string()))?;
    Ok(devices
        .map(|d| describe(&d, default_id.as_deref(), false))
        .collect())
}

/// Whether system-audio capture is available on this machine; `Err` carries the reason.
pub fn system_audio_support() -> std::result::Result<&'static str, String> {
    #[cfg(target_os = "macos")]
    {
        if macos_version_at_least(14, 2) {
            Ok("Core Audio process tap")
        } else {
            Err("needs macOS 14.2 or newer".into())
        }
    }
    #[cfg(target_os = "windows")]
    {
        Ok("WASAPI loopback")
    }
    #[cfg(target_os = "linux")]
    {
        Ok("PulseAudio/PipeWire monitor via parec")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Err("unsupported platform".into())
    }
}

/// `Some(true)` when the default output is the Mac's built-in loudspeakers (so the other side of a
/// call is audible to the microphone — the UI suggests headphones). `None` if it cannot be told.
pub fn output_is_builtin_speakers() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        use cidre::core_audio::{self as ca, DeviceTransportType, PropSelector};
        let dev = ca::System::default_output_device().ok()?;
        if dev.transport_type().ok()? != DeviceTransportType::BUILT_IN {
            return Some(false);
        }
        // kAudioDevicePropertyDataSource ('ssrc'): 'ispk' internal speaker, 'hdpn' headphone jack.
        let data_source: Option<u32> = dev
            .prop(&PropSelector(u32::from_be_bytes(*b"ssrc")).output_addr())
            .ok();
        Some(match data_source {
            Some(v) => v == u32::from_be_bytes(*b"ispk"),
            None => true,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        use cpal::DeviceType;
        let d = cpal::default_host()
            .default_output_device()?
            .description()
            .ok()?;
        Some(
            d.interface_type() == cpal::InterfaceType::BuiltIn
                && d.device_type() == DeviceType::Speaker,
        )
    }
}

/// `Some(true)` when the default input is a Bluetooth device. Bluetooth headsets drop to the
/// narrow-band HFP profile while their microphone is in use, which degrades the whole call.
pub fn input_is_bluetooth() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        use cidre::core_audio::{self as ca, DeviceTransportType};
        let t = ca::System::default_input_device()
            .ok()?
            .transport_type()
            .ok()?;
        Some(t == DeviceTransportType::BLUETOOTH || t == DeviceTransportType::BLUETOOTH_LE)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let d = cpal::default_host()
            .default_input_device()?
            .description()
            .ok()?;
        Some(d.interface_type() == cpal::InterfaceType::Bluetooth)
    }
}

/// `true` when the running macOS is at least `major.minor` (via `sw_vers`, cached).
#[cfg(target_os = "macos")]
pub(crate) fn macos_version_at_least(major: u32, minor: u32) -> bool {
    use std::sync::OnceLock;
    static VERSION: OnceLock<Option<(u32, u32)>> = OnceLock::new();
    let v = VERSION.get_or_init(|| {
        let out = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()?;
        let s = String::from_utf8(out.stdout).ok()?;
        let mut it = s.trim().split('.').map(|p| p.parse::<u32>().ok());
        Some((it.next()??, it.next().flatten().unwrap_or(0)))
    });
    // If detection failed, try anyway: the tap call itself fails cleanly on unsupported systems.
    v.is_none_or(|v| v >= (major, minor))
}
