//! macOS call-detection sources: Core Audio for the microphone gate, `osascript` for browser
//! tabs.
//!
//! Reading the browser's tabs needs the user's *Automation* permission for each browser (the
//! first `osascript` call per browser shows the system prompt). Therefore: only running browsers
//! are asked, a browser that errors (typically `-1743`, not authorised) is left alone for a
//! while, and every failure is just "unknown".

use std::collections::HashMap;
use std::ffi::c_void;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::titles::meet_rooms_from_urls;
use super::{CallSources, MeetTab, MicState, process_names};

/// Browsers that expose their tabs through AppleScript: `(process name, script kind)`.
const BROWSERS: [(&str, &str); 5] = [
    ("Google Chrome", "Google Chrome"),
    ("Arc", "Arc"),
    ("Brave Browser", "Brave Browser"),
    ("Microsoft Edge", "Microsoft Edge"),
    ("Safari", "Safari"),
];

/// A browser that failed is not asked again for this long (so a denied permission never nags).
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(10 * 60);
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(4);

#[repr(C)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
        data: *mut c_void,
    ) -> i32;
}

const SYSTEM_OBJECT: u32 = 1;
const SCOPE_GLOBAL: u32 = u32::from_be_bytes(*b"glob");
const DEFAULT_INPUT_DEVICE: u32 = u32::from_be_bytes(*b"dIn ");
const IS_RUNNING_SOMEWHERE: u32 = u32::from_be_bytes(*b"gone");

fn property_u32(object: u32, selector: u32) -> Option<u32> {
    let address = PropertyAddress {
        selector,
        scope: SCOPE_GLOBAL,
        element: 0,
    };
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: `address`, `size` and `value` outlive the call; `size` matches `value`'s type.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &address,
            0,
            std::ptr::null(),
            &mut size,
            (&mut value as *mut u32).cast(),
        )
    };
    (status == 0).then_some(value)
}

/// `kAudioDevicePropertyDeviceIsRunningSomewhere` on the default input device.
pub fn mic_running_somewhere() -> MicState {
    let Some(device) = property_u32(SYSTEM_OBJECT, DEFAULT_INPUT_DEVICE).filter(|d| *d != 0) else {
        return MicState::Unknown;
    };
    match property_u32(device, IS_RUNNING_SOMEWHERE) {
        Some(0) => MicState::Idle,
        Some(_) => MicState::InUse,
        None => MicState::Unknown,
    }
}

/// The AppleScript that lists a browser's tab URLs, one per line.
pub fn tabs_script(app: &str) -> String {
    format!(
        "tell application \"{app}\"\n\
         set out to \"\"\n\
         repeat with w in windows\n\
         repeat with t in tabs of w\n\
         set out to out & (URL of t) & linefeed\n\
         end repeat\n\
         end repeat\n\
         return out\n\
         end tell"
    )
}

/// Sources backed by Core Audio, `sysinfo` and `osascript`.
#[derive(Debug, Default)]
pub struct MacSources {
    failed_at: Mutex<HashMap<&'static str, Instant>>,
}

impl MacSources {
    fn run_script(script: &str) -> Option<String> {
        let mut child = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + SCRIPT_TIMEOUT;
        loop {
            match child.try_wait().ok()? {
                Some(status) if status.success() => {
                    let mut out = String::new();
                    child.stdout.take()?.read_to_string(&mut out).ok()?;
                    return Some(out);
                }
                Some(_) => return None,
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                None => std::thread::sleep(Duration::from_millis(25)),
            }
        }
    }
}

impl CallSources for MacSources {
    fn mic_state(&self) -> MicState {
        mic_running_somewhere()
    }

    fn mic_users(&self) -> Vec<String> {
        Vec::new() // macOS does not tell cheaply which process holds the microphone
    }

    fn meet_tabs(&self) -> Vec<MeetTab> {
        let running = process_names();
        let mut tabs = Vec::new();
        for (process, app) in BROWSERS {
            if !running.iter().any(|p| p == process) {
                continue; // never launch a browser just to look into it
            }
            {
                let failed = self.failed_at.lock().unwrap_or_else(|p| p.into_inner());
                if failed
                    .get(process)
                    .is_some_and(|t| t.elapsed() < RETRY_AFTER_FAILURE)
                {
                    continue;
                }
            }
            match Self::run_script(&tabs_script(app)) {
                Some(output) => {
                    for (code, url) in meet_rooms_from_urls(&output) {
                        tabs.push(MeetTab {
                            browser: process.to_string(),
                            code: Some(code),
                            url: Some(url),
                        });
                    }
                }
                None => {
                    tracing::debug!("cannot read tabs of {process} (permission or error)");
                    self.failed_at
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .insert(process, Instant::now());
                }
            }
        }
        tabs
    }

    fn running_processes(&self) -> Vec<String> {
        process_names()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_char_codes_match_core_audio_headers() {
        assert_eq!(SCOPE_GLOBAL, 0x676C_6F62);
        assert_eq!(DEFAULT_INPUT_DEVICE, 0x6449_6E20);
        assert_eq!(IS_RUNNING_SOMEWHERE, 0x676F_6E65);
    }

    #[test]
    fn script_targets_the_given_app() {
        let s = tabs_script("Google Chrome");
        assert!(s.starts_with("tell application \"Google Chrome\""));
        assert!(s.contains("URL of t"));
    }

    #[test]
    fn mic_probe_does_not_crash() {
        // Value depends on the machine; this only checks the FFI call is well-formed.
        let _ = mic_running_somewhere();
    }
}
