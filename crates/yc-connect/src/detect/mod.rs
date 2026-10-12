//! Detecting calls that are not in the calendar.
//!
//! Cheap gate first: only when the microphone is in use by some process do we look further —
//! for a Google Meet tab/window (browsers) and for call apps (Zoom, Teams, Telegram).
//!
//! Everything platform-specific hides behind [`CallSources`], so the orchestration in
//! [`Detector`] is tested with fakes. Inspecting browser tabs on macOS needs the user's
//! Automation permission per browser, so it is off unless the caller enables it, and any failure
//! is "unknown", never retried in a loop.

pub mod procs;
pub mod titles;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

use crate::links::meet_code_of;
use crate::types::{DetectedApp, DetectedCall};

/// Is some process capturing the microphone?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicState {
    InUse,
    Idle,
    /// Could not be determined; treated as "do not look further".
    Unknown,
}

/// A Meet room open in a browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetTab {
    pub browser: String,
    pub code: Option<String>,
    pub url: Option<String>,
}

/// Platform facts the detector needs.
pub trait CallSources: Send + Sync {
    /// Whether the microphone is in use by some process (cheap).
    fn mic_state(&self) -> MicState;
    /// Names of the apps holding the microphone, when the OS says (Windows); empty otherwise.
    fn mic_users(&self) -> Vec<String>;
    /// Meet rooms open in running browsers. May need OS permission; failures yield an empty list.
    fn meet_tabs(&self) -> Vec<MeetTab>;
    /// Names of running processes.
    fn running_processes(&self) -> Vec<String>;
}

/// Orchestrates the detection steps.
pub struct Detector<S> {
    sources: S,
    inspect_browsers: bool,
}

impl<S: CallSources> Detector<S> {
    /// `inspect_browsers`: look into browser tabs/windows for Meet (needs the user's opt-in on
    /// macOS because of the per-browser Automation prompt).
    pub fn new(sources: S, inspect_browsers: bool) -> Self {
        Self {
            sources,
            inspect_browsers,
        }
    }

    /// Enables or disables browser inspection.
    pub fn set_inspect_browsers(&mut self, on: bool) {
        self.inspect_browsers = on;
    }

    /// The microphone gate alone (for diagnostics).
    pub fn mic_state(&self) -> MicState {
        self.sources.mic_state()
    }

    /// One blocking detection pass.
    pub fn detect(&self) -> Option<DetectedCall> {
        if self.sources.mic_state() != MicState::InUse {
            return None;
        }
        if self.inspect_browsers
            && let Some(tab) = self.sources.meet_tabs().into_iter().next()
        {
            let url = tab
                .code
                .as_ref()
                .map(|c| format!("https://meet.google.com/{c}"))
                .or(tab.url);
            let meet_code = tab.code.or_else(|| url.as_deref().and_then(meet_code_of));
            return Some(DetectedCall {
                app: DetectedApp::GoogleMeet,
                meet_code,
                url,
                browser: Some(tab.browser),
                matched_event_id: None,
                certain: true,
            });
        }
        let holders = self.sources.mic_users();
        let (names, certain) = if holders.is_empty() {
            (self.sources.running_processes(), false)
        } else {
            (holders, true)
        };
        names
            .iter()
            .find_map(|n| procs::classify_process(n))
            .map(|app| DetectedCall {
                app,
                meet_code: None,
                url: None,
                browser: None,
                matched_event_id: None,
                certain,
            })
    }
}

/// The detector wired to the current OS (an always-idle one on unsupported platforms).
pub fn system_detector(inspect_browsers: bool) -> Detector<Box<dyn CallSources>> {
    Detector::new(system_sources(), inspect_browsers)
}

impl CallSources for Box<dyn CallSources> {
    fn mic_state(&self) -> MicState {
        (**self).mic_state()
    }
    fn mic_users(&self) -> Vec<String> {
        (**self).mic_users()
    }
    fn meet_tabs(&self) -> Vec<MeetTab> {
        (**self).meet_tabs()
    }
    fn running_processes(&self) -> Vec<String> {
        (**self).running_processes()
    }
}

#[cfg(target_os = "macos")]
fn system_sources() -> Box<dyn CallSources> {
    Box::new(macos::MacSources::default())
}

#[cfg(target_os = "windows")]
fn system_sources() -> Box<dyn CallSources> {
    Box::new(windows::WindowsSources)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn system_sources() -> Box<dyn CallSources> {
    struct Unsupported;
    impl CallSources for Unsupported {
        fn mic_state(&self) -> MicState {
            MicState::Unknown
        }
        fn mic_users(&self) -> Vec<String> {
            Vec::new()
        }
        fn meet_tabs(&self) -> Vec<MeetTab> {
            Vec::new()
        }
        fn running_processes(&self) -> Vec<String> {
            Vec::new()
        }
    }
    Box::new(Unsupported)
}

/// Names of running processes via `sysinfo` (cheap: no CPU/memory sampling).
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(crate) fn process_names() -> Vec<String> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    sys.processes()
        .values()
        .map(|p| p.name().to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        mic: MicState,
        users: Vec<&'static str>,
        tabs: Vec<MeetTab>,
        procs: Vec<&'static str>,
    }

    impl CallSources for Fake {
        fn mic_state(&self) -> MicState {
            self.mic
        }
        fn mic_users(&self) -> Vec<String> {
            self.users.iter().map(|s| s.to_string()).collect()
        }
        fn meet_tabs(&self) -> Vec<MeetTab> {
            self.tabs.clone()
        }
        fn running_processes(&self) -> Vec<String> {
            self.procs.iter().map(|s| s.to_string()).collect()
        }
    }

    fn fake(mic: MicState) -> Fake {
        Fake {
            mic,
            users: vec![],
            tabs: vec![MeetTab {
                browser: "Chrome".into(),
                code: Some("abc-defg-hij".into()),
                url: None,
            }],
            procs: vec!["Finder", "zoom.us"],
        }
    }

    #[test]
    fn idle_or_unknown_mic_short_circuits() {
        assert!(Detector::new(fake(MicState::Idle), true).detect().is_none());
        assert!(
            Detector::new(fake(MicState::Unknown), true)
                .detect()
                .is_none()
        );
    }

    #[test]
    fn meet_tab_wins_and_is_certain() {
        let call = Detector::new(fake(MicState::InUse), true).detect().unwrap();
        assert_eq!(call.app, DetectedApp::GoogleMeet);
        assert_eq!(call.meet_code.as_deref(), Some("abc-defg-hij"));
        assert_eq!(
            call.url.as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
        assert!(call.certain);
    }

    #[test]
    fn tabs_are_not_inspected_unless_enabled() {
        let call = Detector::new(fake(MicState::InUse), false)
            .detect()
            .unwrap();
        assert_eq!(call.app, DetectedApp::Zoom);
        assert!(!call.certain, "mic in use + app running is only likely");
    }

    #[test]
    fn mic_holders_make_native_detection_certain() {
        let mut f = fake(MicState::InUse);
        f.tabs.clear();
        f.users = vec!["chrome.exe", "Zoom.exe"];
        f.procs = vec!["Telegram"];
        let call = Detector::new(f, true).detect().unwrap();
        assert_eq!(call.app, DetectedApp::Zoom);
        assert!(call.certain);
    }

    #[test]
    fn nothing_recognisable_yields_none() {
        let mut f = fake(MicState::InUse);
        f.tabs.clear();
        f.procs = vec!["Finder"];
        assert!(Detector::new(f, true).detect().is_none());
    }
}
