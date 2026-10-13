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

/// How a device is attached, as far as the mic-choice policy cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transport {
    BuiltIn,
    Bluetooth,
    Other,
}

/// Transport of a cpal device. On macOS cpal does not report it, so Core Audio is asked directly.
pub(crate) fn transport_of(d: &cpal::Device) -> Transport {
    #[cfg(target_os = "macos")]
    if let Some(t) = mac_transport(d) {
        return t;
    }
    match d.description().map(|x| x.interface_type()) {
        Ok(cpal::InterfaceType::BuiltIn) => Transport::BuiltIn,
        Ok(cpal::InterfaceType::Bluetooth) => Transport::Bluetooth,
        _ => Transport::Other,
    }
}

#[cfg(target_os = "macos")]
fn mac_transport(d: &cpal::Device) -> Option<Transport> {
    use cidre::core_audio::{self as ca, DeviceTransportType as T};
    let id = d.id().ok()?.to_string();
    let uid = id.strip_prefix("coreaudio:")?;
    let dev = ca::Device::with_uid(&cidre::cf::String::from_str(uid)).ok()?;
    if dev.is_unknown() {
        return None;
    }
    let t = dev.transport_type().ok()?;
    Some(if t == T::BUILT_IN {
        Transport::BuiltIn
    } else if t == T::BLUETOOTH || t == T::BLUETOOTH_LE {
        Transport::Bluetooth
    } else {
        Transport::Other
    })
}

fn device_name(d: &cpal::Device) -> String {
    d.description()
        .map(|x| x.name().to_owned())
        .or_else(|_| d.id().map(|i| i.to_string()))
        .unwrap_or_else(|_| "unknown device".into())
}

/// The input to record for "the default microphone": the system default, except that a Bluetooth
/// headset is replaced by a built-in microphone when `prefer_builtin` and one exists. The second
/// value is the name of the replaced headset.
pub(crate) fn pick_default_input(
    host: &cpal::Host,
    prefer_builtin: bool,
) -> Option<(cpal::Device, Option<String>)> {
    let default = host.default_input_device()?;
    if prefer_builtin
        && transport_of(&default) == Transport::Bluetooth
        && let Ok(mut all) = host.input_devices()
        && let Some(builtin) =
            all.find(|d| d.supports_input() && transport_of(d) == Transport::BuiltIn)
    {
        let headset = device_name(&default);
        return Some((builtin, Some(headset)));
    }
    Some((default, None))
}

/// What "the default microphone" resolves to right now (for the UI hint "AirPods stay in high
/// quality, recording the built-in microphone").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicResolution {
    /// Name of the microphone that will be recorded.
    pub device: String,
    /// The Bluetooth headset that is the system default input but is being avoided.
    pub substituted_for: Option<String>,
}

/// Resolves [`crate::DeviceSel::Default`] the way the recorder will.
pub fn resolve_default_mic(prefer_builtin: bool) -> Option<MicResolution> {
    let host = cpal::default_host();
    let (device, substituted_for) = pick_default_input(&host, prefer_builtin)?;
    Some(MicResolution {
        device: device_name(&device),
        substituted_for,
    })
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
        transport: match transport_of(d) {
            Transport::BuiltIn => Some("built-in".to_owned()),
            Transport::Bluetooth => Some("bluetooth".to_owned()),
            Transport::Other => desc
                .as_ref()
                .map(|x| format!("{:?}", x.interface_type()).to_lowercase())
                .filter(|t| t != "unknown"),
        },
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
