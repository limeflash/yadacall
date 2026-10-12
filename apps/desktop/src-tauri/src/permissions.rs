//! OS permissions the recorder needs.
//!
//! * macOS microphone: `AVCaptureDevice` authorization status / request (Objective-C through `objc2`).
//! * macOS system audio: there is no query API, so [`probe_system_audio`] runs a short real capture
//!   (with a system sound playing) and reports whether non-zero audio arrived; macOS shows its prompt
//!   the first time.
//! * Other platforms need no permission for either.
//!
//! [`open_system_settings`] opens the matching privacy pane.

use std::time::{Duration, Instant};

use yc_core::Result;
use yc_core::error::PermissionKind;
use yc_core::meeting::TrackKind;

use crate::api::PermissionState;

/// Current microphone permission.
pub fn microphone_state() -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        mac::microphone_state()
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionState::Granted
    }
}

/// Asks for the microphone permission (macOS shows its prompt once) and returns the outcome.
pub async fn request_microphone() -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        match microphone_state() {
            PermissionState::NotDetermined => {
                let granted = mac::request_microphone().await;
                if granted {
                    PermissionState::Granted
                } else {
                    PermissionState::Denied
                }
            }
            other => other,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionState::Granted
    }
}

/// Result of [`probe_system_audio`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemAudioProbe {
    /// Non-zero system audio arrived: the permission is granted.
    Heard,
    /// The capture ran but only digital silence arrived: macOS withheld the audio.
    Denied,
    /// The capture ran but the test sound could not be played, so silence proves nothing.
    Inconclusive,
}

/// Runs a 1-2 s system-audio capture to trigger the macOS permission prompt and checks that audio
/// arrives. Blocking: call from a blocking thread. Errors with `Error::Permission` when the capture
/// is refused outright.
pub fn probe_system_audio() -> Result<SystemAudioProbe> {
    #[cfg(not(target_os = "macos"))]
    {
        Ok(SystemAudioProbe::Heard)
    }
    #[cfg(target_os = "macos")]
    {
        let dir = std::env::temp_dir().join(format!("yadacall-audio-probe-{}", std::process::id()));
        let mut cfg = yc_audio::RecorderConfig::new(&dir);
        cfg.mic = None;
        cfg.system = true;
        cfg.prevent_sleep = false;
        cfg.skip_disk_check = true;
        let outcome = (|| {
            let handle = yc_audio::Recorder::start(cfg)?;
            let sound = std::process::Command::new("afplay")
                .args(["-v", "0.4", "/System/Library/Sounds/Tink.aiff"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok();
            let played = sound.is_some();
            let started = Instant::now();
            let mut heard = false;
            while started.elapsed() < Duration::from_millis(1800) && !heard {
                std::thread::sleep(Duration::from_millis(100));
                heard = handle
                    .levels()
                    .get(TrackKind::System)
                    .is_some_and(|l| l.peak > 0.0);
            }
            if let Some(mut child) = sound {
                let _ = child.kill();
                let _ = child.wait();
            }
            let _ = handle.stop();
            Ok(if heard {
                SystemAudioProbe::Heard
            } else if played {
                SystemAudioProbe::Denied
            } else {
                SystemAudioProbe::Inconclusive
            })
        })();
        let _ = std::fs::remove_dir_all(&dir);
        outcome
    }
}

/// Opens the OS privacy pane of a permission.
pub fn open_system_settings(kind: PermissionKind) {
    #[cfg(target_os = "macos")]
    {
        let pane = match kind {
            PermissionKind::Microphone => "Privacy_Microphone",
            PermissionKind::SystemAudio => "Privacy_AudioCapture",
        };
        let _ = std::process::Command::new("open")
            .arg(format!(
                "x-apple.systempreferences:com.apple.preference.security?{pane}"
            ))
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let page = match kind {
            PermissionKind::Microphone => "ms-settings:privacy-microphone",
            PermissionKind::SystemAudio => "ms-settings:sound",
        };
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", page])
            .spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = kind;
}

#[cfg(target_os = "macos")]
mod mac {
    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, Bool};
    use objc2_foundation::NSString;
    use tokio::sync::oneshot;

    use crate::api::PermissionState;

    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {}

    /// `AVMediaTypeAudio`.
    fn audio_media_type() -> objc2::rc::Retained<NSString> {
        NSString::from_str("soun")
    }

    pub fn microphone_state() -> PermissionState {
        let Some(class) = AnyClass::get(c"AVCaptureDevice") else {
            return PermissionState::NotDetermined;
        };
        let media = audio_media_type();
        // AVAuthorizationStatus: 0 not determined, 1 restricted, 2 denied, 3 authorized.
        let status: isize = unsafe { msg_send![class, authorizationStatusForMediaType: &*media] };
        match status {
            3 => PermissionState::Granted,
            1 | 2 => PermissionState::Denied,
            _ => PermissionState::NotDetermined,
        }
    }

    pub async fn request_microphone() -> bool {
        // The Objective-C objects must be gone before the first `.await` (they are not `Send`).
        let rx = {
            let Some(class) = AnyClass::get(c"AVCaptureDevice") else {
                return false;
            };
            let (tx, rx) = oneshot::channel::<bool>();
            let tx = std::sync::Mutex::new(Some(tx));
            let block = RcBlock::new(move |granted: Bool| {
                if let Some(tx) = tx.lock().ok().and_then(|mut t| t.take()) {
                    let _ = tx.send(granted.as_bool());
                }
            });
            let media = audio_media_type();
            let _: () = unsafe {
                msg_send![class, requestAccessForMediaType: &*media, completionHandler: &*block]
            };
            rx
        };
        rx.await.unwrap_or(false)
    }
}
