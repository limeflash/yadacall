//! macOS call detection through Core Audio process objects and targeted AppleScript tab reads.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cidre::core_audio as ca;

use super::procs::{self, BrowserApp};
use super::titles::call_tabs_from_output;
use super::{CallSources, MicProbe, MicUser, TabAccessError};

/// A denied browser is not asked again for this long.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(10 * 60);
const RETRY_AFTER_PERMISSION_DENIAL: Duration = Duration::from_secs(20);
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(4);

/// AppleScript runner seam used to test Automation errors without opening permission prompts.
trait AppleScriptRunner: Send + Sync {
    fn run(&self, browser: BrowserApp, script: &str) -> Result<String, ScriptFailure>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ScriptFailure {
    Exit(String),
    Timeout,
    Launch,
}

struct CommandRunner;

impl AppleScriptRunner for CommandRunner {
    fn run(&self, _browser: BrowserApp, script: &str) -> Result<String, ScriptFailure> {
        let mut child = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| ScriptFailure::Launch)?;
        let deadline = Instant::now() + SCRIPT_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut stdout = String::new();
                    let mut stderr = String::new();
                    if let Some(mut pipe) = child.stdout.take() {
                        pipe.read_to_string(&mut stdout)
                            .map_err(|_| ScriptFailure::Launch)?;
                    }
                    if let Some(mut pipe) = child.stderr.take() {
                        pipe.read_to_string(&mut stderr)
                            .map_err(|_| ScriptFailure::Launch)?;
                    }
                    return if status.success() {
                        Ok(stdout)
                    } else {
                        Err(ScriptFailure::Exit(stderr))
                    };
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ScriptFailure::Timeout);
                }
                Err(_) => return Err(ScriptFailure::Launch),
            }
        }
    }
}

/// AppleScript that reads only the active tab of one identified browser.
pub fn tabs_script(browser: BrowserApp) -> String {
    format!(
        "tell application id \"{}\"\n\
         if (count of windows) is 0 then return \"\"\n\
         set t to active tab of front window\n\
         return URL of t\n\
         end tell",
        browser.bundle_id
    )
}

/// Sources backed by Core Audio process objects and `osascript`.
pub struct MacSources {
    runner: Arc<dyn AppleScriptRunner>,
    failed_at: Mutex<HashMap<&'static str, (Instant, TabAccessError)>>,
}

impl Default for MacSources {
    fn default() -> Self {
        Self::with_runner(Arc::new(CommandRunner))
    }
}

impl MacSources {
    fn with_runner(runner: Arc<dyn AppleScriptRunner>) -> Self {
        Self {
            runner,
            failed_at: Mutex::new(HashMap::new()),
        }
    }

    fn read_tabs(&self, browser: BrowserApp) -> Result<Vec<super::titles::CallTab>, TabAccessError> {
        if let Some((at, error)) = self
            .failed_at
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(browser.bundle_id)
            .cloned()
            && at.elapsed() < retry_after(&error)
        {
            return Err(error);
        }

        match self.runner.run(browser, &tabs_script(browser)) {
            Ok(output) => {
                self.failed_at
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .remove(browser.bundle_id);
                Ok(call_tabs_from_output(&output, browser.name))
            }
            Err(failure) => {
                let error = match failure {
                    ScriptFailure::Exit(ref stderr) if is_automation_denied(stderr) => {
                        tracing::debug!(browser = browser.name, "Apple Events permission denied");
                        TabAccessError::PermissionDenied {
                            browser: browser.name.to_owned(),
                        }
                    }
                    _ => {
                        tracing::debug!(browser = browser.name, "could not read active browser tab");
                        TabAccessError::Unavailable
                    }
                };
                self.failed_at
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .insert(browser.bundle_id, (Instant::now(), error.clone()));
                Err(error)
            }
        }
    }
}

fn is_automation_denied(stderr: &str) -> bool {
    stderr.contains("-1743") || stderr.contains("-1744")
}

fn retry_after(error: &TabAccessError) -> Duration {
    match error {
        TabAccessError::PermissionDenied { .. } => RETRY_AFTER_PERMISSION_DENIAL,
        TabAccessError::Unavailable => RETRY_AFTER_FAILURE,
    }
}

fn core_audio_mic_users() -> MicProbe {
    let Ok(processes) = ca::System::processes() else {
        return MicProbe::Unknown;
    };
    let own_pid = std::process::id() as i32;
    let users = processes
        .into_iter()
        .filter_map(|process| {
            let pid = process.pid().ok()?;
            if pid == own_pid || !process.is_running_input().ok()? {
                return None;
            }
            let bundle_id = process.bundle_id().ok().map(|id| id.to_string());
            let name = bundle_id
                .as_deref()
                .and_then(procs::browser_for_bundle_id)
                .map(|browser| browser.name.to_owned())
                .or_else(|| {
                    bundle_id
                        .as_deref()
                        .and_then(procs::classify_bundle_id)
                        .map(app_label)
                })
                .or_else(|| bundle_id.as_deref().map(last_bundle_component))
                .unwrap_or_else(|| format!("process {pid}"));
            Some(MicUser {
                process_name: name,
                bundle_id,
                pid: Some(pid),
            })
        })
        .collect();
    MicProbe::Known(users)
}

fn last_bundle_component(bundle_id: &str) -> String {
    bundle_id
        .rsplit('.')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(bundle_id)
        .to_owned()
}

fn app_label(app: crate::types::DetectedApp) -> String {
    use crate::types::DetectedApp;
    match app {
        DetectedApp::GoogleMeet => "Google Meet".into(),
        DetectedApp::Zoom => "Zoom".into(),
        DetectedApp::Teams => "Microsoft Teams".into(),
        DetectedApp::Telegram => "Telegram".into(),
        DetectedApp::Slack => "Slack".into(),
        DetectedApp::Discord => "Discord".into(),
        DetectedApp::Telemost => "Yandex Telemost".into(),
        DetectedApp::Jitsi => "Jitsi".into(),
        DetectedApp::Other(name) => name,
    }
}

impl CallSources for MacSources {
    fn microphone_users(&self) -> MicProbe {
        core_audio_mic_users()
    }

    fn call_tabs(
        &self,
        user: &MicUser,
    ) -> Result<Vec<super::titles::CallTab>, TabAccessError> {
        let browser = user
            .bundle_id
            .as_deref()
            .and_then(procs::browser_for_bundle_id)
            .or_else(|| procs::browser_for_process(&user.process_name))
            .ok_or(TabAccessError::Unavailable)?;
        self.read_tabs(browser)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::procs::browser_for_bundle_id;

    struct MockRunner {
        result: Result<String, ScriptFailure>,
    }

    impl AppleScriptRunner for MockRunner {
        fn run(&self, _browser: BrowserApp, _script: &str) -> Result<String, ScriptFailure> {
            self.result.clone()
        }
    }

    #[test]
    fn script_targets_one_bundle_and_only_the_active_tab() {
        let browser = browser_for_bundle_id("com.brave.Browser").expect("known browser");
        let script = tabs_script(browser);
        assert!(script.contains("application id \"com.brave.Browser\""));
        assert!(script.contains("active tab of front window"));
        assert!(script.contains("URL of t"));
        assert!(!script.contains("title of t"));
        assert!(!script.contains("repeat with t in tabs"));
    }

    #[test]
    fn apple_events_permission_denial_is_reported_without_running_osascript() {
        let browser = browser_for_bundle_id("com.brave.Browser").expect("known browser");
        let sources = MacSources::with_runner(Arc::new(MockRunner {
            result: Err(ScriptFailure::Exit(
                "Not authorized to send Apple events to Brave (-1743)".into(),
            )),
        }));
        let user = MicUser {
            process_name: "Brave".into(),
            bundle_id: Some(browser.bundle_id.into()),
            pid: Some(42),
        };
        assert_eq!(
            sources.call_tabs(&user),
            Err(TabAccessError::PermissionDenied {
                browser: "Brave".into()
            })
        );
    }

    #[test]
    fn recognizes_both_automation_denial_codes() {
        assert!(is_automation_denied("error (-1743)"));
        assert!(is_automation_denied("error (-1744)"));
        assert!(!is_automation_denied("execution error"));
    }

    #[test]
    fn core_audio_process_query_is_well_formed() {
        // This property query does not request microphone or Automation permission.
        let _ = core_audio_mic_users();
    }
}
