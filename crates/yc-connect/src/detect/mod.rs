//! Detecting calls that are not in the calendar.
//!
//! A call app is reported only when the platform says that app holds the microphone. Browsers are
//! inspected only when that browser holds the microphone and browser access is enabled.

pub mod procs;
pub mod titles;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "linux")]
pub mod linux;

use crate::types::{DetectedApp, DetectedCall};

/// Is some process capturing the microphone?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicState {
    InUse,
    Idle,
    /// The platform probe could not determine microphone use.
    Unknown,
}

/// Process identity reported by the platform's microphone-use API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicUser {
    /// Process or application name.
    pub process_name: String,
    /// Application bundle ID when the platform provides one.
    pub bundle_id: Option<String>,
    /// Process ID when the platform provides one.
    pub pid: Option<i32>,
}

/// Result of a platform microphone-holder query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MicProbe {
    /// The query succeeded; an empty list means the microphone is idle.
    Known(Vec<MicUser>),
    /// The platform could not query microphone users.
    Unknown,
}

impl MicProbe {
    /// Coarse microphone state for status output.
    pub fn state(&self) -> MicState {
        match self {
            MicProbe::Known(users) if users.is_empty() => MicState::Idle,
            MicProbe::Known(_) => MicState::InUse,
            MicProbe::Unknown => MicState::Unknown,
        }
    }

    /// Processes holding the microphone, or an empty list when the probe failed.
    pub fn users(&self) -> &[MicUser] {
        match self {
            MicProbe::Known(users) => users,
            MicProbe::Unknown => &[],
        }
    }
}

/// Why the detector could not inspect a browser's active tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabAccessError {
    /// macOS denied Apple Events access to this browser.
    PermissionDenied { browser: String },
    /// The browser did not expose a readable tab or the platform has no tab adapter.
    Unavailable,
}

/// One blocking detection pass, including the process information used to make the decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionScan {
    /// Whether a microphone holder was found.
    pub microphone: MicState,
    /// Processes that the platform says currently hold microphone input.
    pub microphone_users: Vec<MicUser>,
    /// Detected call, if a known call app or browser is a microphone holder.
    pub call: Option<DetectedCall>,
}

/// Platform facts the detector needs.
pub trait CallSources: Send + Sync {
    /// Processes currently holding microphone input, or `Unknown` if the query failed.
    fn microphone_users(&self) -> MicProbe;
    /// Supported call pages in the active tab of one microphone-holding browser.
    fn call_tabs(&self, browser: &MicUser) -> Result<Vec<titles::CallTab>, TabAccessError>;
}

/// Orchestrates the detection steps.
pub struct Detector<S> {
    sources: S,
    inspect_browsers: bool,
}

impl<S: CallSources> Detector<S> {
    /// `inspect_browsers`: read the active tab of a browser that holds microphone input.
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
        self.sources.microphone_users().state()
    }

    /// One blocking detection pass and its microphone-holder evidence.
    pub fn scan(&self) -> DetectionScan {
        let probe = self.sources.microphone_users();
        let microphone = probe.state();
        let microphone_users = probe.users().to_vec();
        tracing::debug!(
            microphone = ?microphone,
            microphone_users = ?microphone_users,
            "call detection microphone snapshot"
        );

        let call = if microphone == MicState::InUse {
            self.detect_from_holders(&microphone_users)
        } else {
            None
        };
        if let Some(call) = &call {
            tracing::debug!(
                app = ?call.app,
                browser = ?call.browser,
                certain = call.certain,
                automation_permission_required = ?call.automation_permission_required,
                "call detection decision"
            );
        }
        DetectionScan {
            microphone,
            microphone_users,
            call,
        }
    }

    /// One blocking detection pass.
    pub fn detect(&self) -> Option<DetectedCall> {
        self.scan().call
    }

    fn detect_from_holders(&self, users: &[MicUser]) -> Option<DetectedCall> {
        // Native call apps have the strongest attribution and always came from the OS holder list.
        for user in users {
            if let Some(app) = user_call_app(user) {
                return Some(native_call(app));
            }
        }

        let mut first_browser = None;
        for user in users {
            let Some(browser) = browser_for_user(user) else {
                continue;
            };
            first_browser.get_or_insert((user, browser));
            if !self.inspect_browsers {
                continue;
            }
            match self.sources.call_tabs(user) {
                Ok(tabs) => {
                    if let Some(tab) = tabs.into_iter().next() {
                        return Some(call_from_tab(tab));
                    }
                }
                Err(TabAccessError::PermissionDenied { browser: denied }) => {
                    tracing::debug!(browser = denied, "browser tab access requires Automation permission");
                    if denied == browser.name {
                        return Some(generic_browser_call(browser, Some(denied)));
                    }
                }
                Err(TabAccessError::Unavailable) => {}
            }
        }
        first_browser.map(|(_, browser)| generic_browser_call(browser, None))
    }
}

fn user_call_app(user: &MicUser) -> Option<DetectedApp> {
    user.bundle_id
        .as_deref()
        .and_then(procs::classify_bundle_id)
        .or_else(|| procs::classify_process(&user.process_name))
}

fn browser_for_user(user: &MicUser) -> Option<procs::BrowserApp> {
    user.bundle_id
        .as_deref()
        .and_then(procs::browser_for_bundle_id)
        .or_else(|| procs::browser_for_process(&user.process_name))
}

fn native_call(app: DetectedApp) -> DetectedCall {
    let title = format!("Звонок в {}", app_name(&app));
    DetectedCall {
        app,
        meet_code: None,
        code: None,
        url: None,
        browser: None,
        title,
        automation_permission_required: None,
        matched_event_id: None,
        certain: true,
    }
}

fn call_from_tab(tab: titles::CallTab) -> DetectedCall {
    let title = tab
        .title
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| format!("Звонок в {}", app_name(&tab.app)));
    DetectedCall {
        app: tab.app,
        meet_code: tab.meet_code,
        code: tab.code,
        url: tab.url,
        browser: Some(tab.browser),
        title,
        automation_permission_required: None,
        matched_event_id: None,
        certain: true,
    }
}

fn generic_browser_call(
    browser: procs::BrowserApp,
    automation_permission_required: Option<String>,
) -> DetectedCall {
    DetectedCall {
        app: DetectedApp::Other(browser.name.to_owned()),
        meet_code: None,
        code: None,
        url: None,
        browser: Some(browser.name.to_owned()),
        title: format!("Звонок в {}", browser.name),
        automation_permission_required,
        matched_event_id: None,
        certain: true,
    }
}

fn app_name(app: &DetectedApp) -> &str {
    match app {
        DetectedApp::GoogleMeet => "Google Meet",
        DetectedApp::Zoom => "Zoom",
        DetectedApp::Teams => "Microsoft Teams",
        DetectedApp::Telegram => "Telegram",
        DetectedApp::Slack => "Slack",
        DetectedApp::Discord => "Discord",
        DetectedApp::Telemost => "Yandex Telemost",
        DetectedApp::Jitsi => "Jitsi",
        DetectedApp::Other(name) => name,
    }
}

/// The detector wired to the current OS.
pub fn system_detector(inspect_browsers: bool) -> Detector<Box<dyn CallSources>> {
    Detector::new(system_sources(), inspect_browsers)
}

impl CallSources for Box<dyn CallSources> {
    fn microphone_users(&self) -> MicProbe {
        (**self).microphone_users()
    }

    fn call_tabs(&self, browser: &MicUser) -> Result<Vec<titles::CallTab>, TabAccessError> {
        (**self).call_tabs(browser)
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

#[cfg(target_os = "linux")]
fn system_sources() -> Box<dyn CallSources> {
    Box::new(linux::LinuxSources)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn system_sources() -> Box<dyn CallSources> {
    struct Unsupported;
    impl CallSources for Unsupported {
        fn microphone_users(&self) -> MicProbe {
            MicProbe::Unknown
        }

        fn call_tabs(&self, _browser: &MicUser) -> Result<Vec<titles::CallTab>, TabAccessError> {
            Err(TabAccessError::Unavailable)
        }
    }
    Box::new(Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::procs::browser_for_bundle_id;

    struct Fake {
        probe: MicProbe,
        tabs: Vec<titles::CallTab>,
        tab_error: Option<TabAccessError>,
    }

    impl CallSources for Fake {
        fn microphone_users(&self) -> MicProbe {
            self.probe.clone()
        }

        fn call_tabs(&self, browser: &MicUser) -> Result<Vec<titles::CallTab>, TabAccessError> {
            assert_eq!(browser.bundle_id.as_deref(), Some("com.brave.Browser"));
            if let Some(error) = &self.tab_error {
                return Err(error.clone());
            }
            Ok(self.tabs.clone())
        }
    }

    fn brave() -> MicUser {
        MicUser {
            process_name: "Brave".into(),
            bundle_id: Some("com.brave.Browser".into()),
            pid: Some(123),
        }
    }

    fn fake(probe: MicProbe) -> Fake {
        Fake {
            probe,
            tabs: vec![titles::CallTab {
                app: DetectedApp::GoogleMeet,
                code: Some("abc-defg-hij".into()),
                meet_code: Some("abc-defg-hij".into()),
                url: Some("https://meet.google.com/abc-defg-hij".into()),
                browser: "Brave".into(),
                title: Some("Weekly sync".into()),
            }],
            tab_error: None,
        }
    }

    #[test]
    fn idle_or_unknown_mic_short_circuits() {
        assert!(Detector::new(fake(MicProbe::Known(vec![])), true)
            .scan()
            .call
            .is_none());
        assert!(Detector::new(fake(MicProbe::Unknown), true)
            .scan()
            .call
            .is_none());
    }

    #[test]
    fn matching_call_tab_wins_and_includes_page_details() {
        let scan = Detector::new(fake(MicProbe::Known(vec![brave()])), true).scan();
        let call = scan.call.expect("call tab");
        assert_eq!(call.app, DetectedApp::GoogleMeet);
        assert_eq!(call.meet_code.as_deref(), Some("abc-defg-hij"));
        assert_eq!(call.url.as_deref(), Some("https://meet.google.com/abc-defg-hij"));
        assert_eq!(call.title, "Weekly sync");
        assert_eq!(call.browser.as_deref(), Some("Brave"));
        assert_eq!(scan.microphone_users, vec![brave()]);
    }

    #[test]
    fn browser_mic_holder_gets_generic_fallback_when_tabs_are_disabled() {
        let scan = Detector::new(fake(MicProbe::Known(vec![brave()])), false).scan();
        let call = scan.call.expect("generic browser call");
        assert_eq!(call.app, DetectedApp::Other("Brave".into()));
        assert_eq!(call.url, None);
        assert_eq!(call.title, "Звонок в Brave");
        assert_eq!(call.browser.as_deref(), Some("Brave"));
        assert!(call.certain);
    }

    #[test]
    fn browser_permission_denial_still_emits_generic_call_and_permission_state() {
        let mut fake = fake(MicProbe::Known(vec![brave()]));
        fake.tab_error = Some(TabAccessError::PermissionDenied {
            browser: "Brave".into(),
        });
        let call = Detector::new(fake, true).detect().expect("generic browser call");
        assert_eq!(call.app, DetectedApp::Other("Brave".into()));
        assert_eq!(call.automation_permission_required.as_deref(), Some("Brave"));
    }

    #[test]
    fn a_running_call_app_is_not_attributed_unless_it_holds_the_mic() {
        let only_brave_holds_mic = fake(MicProbe::Known(vec![brave()]));
        let call = Detector::new(only_brave_holds_mic, false)
            .detect()
            .expect("browser call");
        assert_eq!(call.app, DetectedApp::Other("Brave".into()));

        let zoom = MicUser {
            process_name: "Zoom".into(),
            bundle_id: Some("us.zoom.xos".into()),
            pid: Some(456),
        };
        let call = Detector::new(fake(MicProbe::Known(vec![zoom])), false)
            .detect()
            .expect("Zoom call");
        assert_eq!(call.app, DetectedApp::Zoom);
    }

    #[test]
    fn bundle_id_browser_mapping_is_used_for_generic_fallback() {
        let mut user = brave();
        user.bundle_id = Some("com.vivaldi.Vivaldi".into());
        user.process_name = "Vivaldi".into();
        let call = generic_browser_call(
            browser_for_bundle_id("com.vivaldi.Vivaldi").expect("known browser"),
            None,
        );
        assert_eq!(call.app, DetectedApp::Other("Vivaldi".into()));
        assert_eq!(call.title, "Звонок в Vivaldi");
    }
}
