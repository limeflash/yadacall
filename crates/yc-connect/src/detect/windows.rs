//! Windows call-detection sources: the microphone consent store in the registry and window
//! titles via `EnumWindows`.

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
};
use windows::core::BOOL;
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

use super::procs::{self, mic_users_from_consent};
use super::titles::parse_meet_title;
use super::{CallSources, MicProbe, MicUser, TabAccessError, titles};

const MIC_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

/// `(key name, LastUsedTimeStart, LastUsedTimeStop)` of every app under the microphone key,
/// including the `NonPackaged` desktop apps.
fn consent_entries() -> Option<Vec<(String, u64, u64)>> {
    let root = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(MIC_KEY)
        .ok()?;
    let mut out = Vec::new();
    let mut read = |key: &RegKey, name: String| {
        let start: u64 = key.get_value("LastUsedTimeStart").unwrap_or(0);
        let stop: u64 = key.get_value("LastUsedTimeStop").unwrap_or(0);
        out.push((name, start, stop));
    };
    for name in root.enum_keys().flatten() {
        let Ok(sub) = root.open_subkey(&name) else {
            continue;
        };
        if name == "NonPackaged" {
            for app in sub.enum_keys().flatten() {
                if let Ok(key) = sub.open_subkey(&app) {
                    read(&key, app);
                }
            }
        } else {
            read(&sub, name);
        }
    }
    Some(out)
}

unsafe extern "system" fn collect_title(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the address of the `Vec<String>` owned by `window_titles`.
    let titles = unsafe { &mut *(lparam.0 as *mut Vec<String>) };
    // SAFETY: plain Win32 calls on a window handle handed to us by `EnumWindows`.
    unsafe {
        if IsWindowVisible(hwnd).as_bool() {
            let len = GetWindowTextLengthW(hwnd);
            if len > 0 {
                let mut buf = vec![0u16; len as usize + 1];
                let n = GetWindowTextW(hwnd, &mut buf);
                if n > 0 {
                    titles.push(String::from_utf16_lossy(&buf[..n as usize]));
                }
            }
        }
    }
    BOOL(1)
}

/// Titles of all visible top-level windows.
pub fn window_titles() -> Vec<String> {
    let mut titles: Vec<String> = Vec::new();
    // SAFETY: the callback only dereferences `lparam` as this local `Vec` during the call.
    let _ = unsafe { EnumWindows(Some(collect_title), LPARAM(&mut titles as *mut _ as isize)) };
    titles
}

/// Sources backed by the microphone ConsentStore and visible window titles.
#[derive(Debug, Default)]
pub struct WindowsSources;

impl CallSources for WindowsSources {
    fn microphone_users(&self) -> MicProbe {
        match consent_entries() {
            Some(entries) => MicProbe::Known(
                mic_users_from_consent(&entries)
                    .into_iter()
                    .map(|process_name| MicUser {
                        process_name,
                        bundle_id: None,
                        pid: None,
                    })
                    .collect(),
            ),
            None => MicProbe::Unknown,
        }
    }

    fn call_tabs(&self, process: &MicUser) -> Result<Vec<titles::CallTab>, TabAccessError> {
        let Some(browser) = procs::browser_for_process(&process.process_name) else {
            return Err(TabAccessError::Unavailable);
        };
        Ok(window_titles()
            .iter()
            .filter_map(|title| {
                let found = parse_meet_title(title)?;
                (found.browser.as_deref() == Some(browser.name)).then(|| titles::CallTab {
                    app: crate::types::DetectedApp::GoogleMeet,
                    code: found.code.clone(),
                    meet_code: found.code,
                    url: None,
                    browser: browser.name.to_owned(),
                    title: Some(title.clone()),
                })
            })
            .collect())
    }
}
