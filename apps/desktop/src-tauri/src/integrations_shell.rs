//! Shell side of the "before the call" feature: a one-second scheduler that watches the agenda, shows the
//! menu-bar countdown ("1:1 с Аней 1:41") and the "звонок без записи" state, fires reminders, and turns
//! [`IntegrationEvent`]s into native notifications.
//!
//! Desktop notifications cannot carry buttons (`tauri-plugin-notification` only supports actions on mobile), so
//! the notification texts below list the actions and the tray popover — which shows the same "Следующая
//! встреча" card with the real buttons — is where they are actually pressed.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt as _;

use crate::api::{Backend, RecorderStartOptions};
use crate::integrations_api::*;
use crate::shell;

/// How long before the start the menu bar switches to the countdown.
const TITLE_WINDOW_SECS: i64 = 600;
/// How often the agenda is refetched even without an `AgendaChanged` event.
const REFRESH_EVERY_SECS: u64 = 30;
/// Longest call title shown in the menu bar.
const TITLE_MAX_CHARS: usize = 18;

/// Mutable scheduler state, kept in Tauri state.
#[derive(Default)]
pub struct IntegrationShell {
    dirty: AtomicBool,
    /// "Звонок без записи" is shown until this instant.
    no_record_until: Mutex<Option<DateTime<Utc>>>,
}

/// The sink handed to the [`Integrations`] implementation: forwards to every webview and reacts in the shell.
pub fn sink(app: &AppHandle) -> IntegrationSink {
    let app = app.clone();
    Arc::new(move |e| {
        on_event(&app, &e);
        let _ = app.emit("integrations", &e);
    })
}

/// Starts the scheduler. Called once from `setup`, after the integrations object is in state.
pub fn start(app: &AppHandle) {
    app.manage(IntegrationShell::default());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut events: Vec<AgendaEvent> = Vec::new();
        let mut remind = ReminderSettings::default().remind_minutes;
        let mut reminded: HashSet<String> = HashSet::new();
        let mut since_refresh = REFRESH_EVERY_SECS;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            let sh = app.state::<IntegrationShell>();
            since_refresh += 1;
            if since_refresh >= REFRESH_EVERY_SECS || sh.dirty.swap(false, Ordering::Relaxed) {
                since_refresh = 0;
                let i = app.state::<Arc<dyn Integrations>>();
                let now = Utc::now();
                let range = TimeRange {
                    from: now - chrono::Duration::hours(1),
                    to: now + chrono::Duration::hours(24),
                };
                if let Ok(ev) = i.agenda(range).await {
                    events = ev;
                }
                if let Ok(s) = i.reminder_settings().await {
                    remind = s.remind_minutes;
                }
            }
            let now = Utc::now();
            let recording = shell::is_recording(&app);
            let no_record = {
                let mut g = sh.no_record_until.lock().unwrap_or_else(|e| e.into_inner());
                if recording || g.is_some_and(|t| t <= now) {
                    *g = None;
                }
                g.is_some()
            };
            let p = plan(now, &events, remind, &reminded, recording, no_record);
            if let Some((ev, minutes)) = p.remind {
                reminded.insert(ev.id.clone());
                let (label, email) = account_of(&app, &ev.account_id).await;
                sink(&app)(IntegrationEvent::ReminderDue {
                    event: ev,
                    account_label: label,
                    account_email: email,
                    minutes_before: minutes,
                });
            }
            shell::set_idle_title(&app, p.title);
        }
    });
}

async fn account_of(app: &AppHandle, id: &str) -> (String, String) {
    let i = app.state::<Arc<dyn Integrations>>();
    i.accounts()
        .await
        .ok()
        .and_then(|a| a.into_iter().find(|a| a.id == id))
        .map(|a| (a.label, a.email))
        .unwrap_or_default()
}

/// What one scheduler tick decided.
#[derive(Debug, Default)]
pub struct Plan {
    /// Menu-bar title while idle (`None`: nothing to show).
    pub title: Option<String>,
    /// A reminder to fire now.
    pub remind: Option<(AgendaEvent, u8)>,
}

/// Pure decision step of the scheduler (unit-tested).
pub fn plan(
    now: DateTime<Utc>,
    events: &[AgendaEvent],
    remind_minutes: u8,
    reminded: &HashSet<String>,
    recording: bool,
    no_record: bool,
) -> Plan {
    let next = events
        .iter()
        .filter(|e| e.start > now)
        .min_by_key(|e| e.start);
    let left = next.map(|e| (e.start - now).num_seconds());
    let remind = next
        .zip(left)
        .filter(|(e, l)| {
            !recording && *l <= i64::from(remind_minutes) * 60 && !reminded.contains(&e.id)
        })
        .map(|(e, _)| (e.clone(), remind_minutes));
    let title = if recording {
        None
    } else if no_record {
        Some("звонок без записи".to_string())
    } else {
        next.zip(left)
            .filter(|(_, l)| *l <= TITLE_WINDOW_SECS)
            .map(|(e, l)| menu_title(&e.title, l))
    };
    Plan { title, remind }
}

/// "1:1 с Аней 1:41" — the call title, shortened, and the time left as `m:ss`.
pub fn menu_title(title: &str, left_secs: i64) -> String {
    let t: String = if title.chars().count() > TITLE_MAX_CHARS {
        title.chars().take(TITLE_MAX_CHARS - 1).collect::<String>() + "…"
    } else {
        title.to_string()
    };
    let l = left_secs.max(0);
    format!("{t} {}:{:02}", l / 60, l % 60)
}

// ---- notifications ------------------------------------------------------------------------------------

/// Text of a native notification and the actions it offers (pressed in the tray popover on desktop).
#[derive(Debug, PartialEq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    pub actions: Vec<&'static str>,
}

/// "минуту / минуты / минут" for 1, 2, 5.
fn minutes_word(n: u8) -> &'static str {
    match n {
        1 => "минуту",
        2..=4 => "минуты",
        _ => "минут",
    }
}

/// Reminder: macOS "Через 2 минуты / 1:1 с Аней / Рабочий · lime@company.com", Windows "1:1 с Аней — через 2 минуты".
pub fn reminder_notice(
    title: &str,
    minutes: u8,
    label: &str,
    email: &str,
    windows: bool,
) -> Notice {
    let who = if label.is_empty() {
        email.to_string()
    } else {
        format!("{label} · {email}")
    };
    let actions = vec!["Войти и записать", "Без записи"];
    if windows {
        Notice {
            title: format!("{title} — через {minutes} {}", minutes_word(minutes)),
            body: who,
            actions,
        }
    } else {
        Notice {
            title: format!("Через {minutes} {}", minutes_word(minutes)),
            body: format!("{title}\n{who}"),
            actions,
        }
    }
}

/// "Ты в звонке Google Meet — Записать его?" for calls that are not on any calendar.
pub fn detected_notice(call: &DetectedCall) -> Notice {
    Notice {
        title: format!("Ты в звонке {}", call.platform),
        body: format!("Записать его?\n{}", call.url),
        actions: vec!["Записать", "Не сейчас"],
    }
}

fn show(app: &AppHandle, n: &Notice) {
    let _ = app
        .notification()
        .builder()
        .title(&n.title)
        .body(&n.body)
        .show();
}

/// Shell reaction to an integration event (the webviews get the same event through `integrations`).
fn on_event(app: &AppHandle, e: &IntegrationEvent) {
    match e {
        IntegrationEvent::ReminderDue {
            event,
            account_label,
            account_email,
            minutes_before,
        } => show(
            app,
            &reminder_notice(
                &event.title,
                *minutes_before,
                account_label,
                account_email,
                cfg!(target_os = "windows"),
            ),
        ),
        IntegrationEvent::CallDetected { call } => show(app, &detected_notice(call)),
        IntegrationEvent::AuthExpired { email, .. } => show(
            app,
            &Notice {
                title: "Нужно войти в Google снова".into(),
                body: format!("{email} — встречи из этого календаря пока не видны"),
                actions: vec!["Войти снова"],
            },
        ),
        IntegrationEvent::AgendaChanged => {
            if let Some(sh) = app.try_state::<IntegrationShell>() {
                sh.dirty.store(true, Ordering::Relaxed);
            }
        }
        IntegrationEvent::AddProgress { .. } => {}
    }
}

// ---- joining ------------------------------------------------------------------------------------------

/// Carries out the decision of "Войти и записать" / "Без записи": opens the call, starts the recorder or
/// switches the menu bar to "звонок без записи".
pub async fn apply_join(app: &AppHandle, backend: &Arc<dyn Backend>, out: &JoinOutcome) {
    if let Some(url) = &out.meet_url {
        let _ = tauri_plugin_opener::open_url(url, None::<&str>);
    }
    if let Some(sh) = app.try_state::<IntegrationShell>() {
        let mut g = sh.no_record_until.lock().unwrap_or_else(|e| e.into_inner());
        *g = (!out.record).then_some(out.ends_at);
        sh.dirty.store(true, Ordering::Relaxed);
    }
    if out.record {
        let opts = RecorderStartOptions {
            title: Some(out.title.clone()),
            ..Default::default()
        };
        let _ = backend.recorder_start(opts).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration as D, TimeZone};

    fn ev(id: &str, title: &str, start: DateTime<Utc>) -> AgendaEvent {
        AgendaEvent {
            id: id.into(),
            account_id: "work".into(),
            title: title.into(),
            start,
            end: start + D::minutes(30),
            meet_url: None,
            attendees: vec![],
            record_policy: RecordPolicy::Always,
            will_record: true,
            overlaps: vec![],
            meeting_id: None,
        }
    }
    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 12, 11, 55, 0).unwrap()
    }

    #[test]
    fn countdown_title_appears_within_ten_minutes() {
        let e = vec![ev("a", "1:1 с Аней", now() + D::seconds(101))];
        let p = plan(now(), &e, 2, &HashSet::new(), false, false);
        assert_eq!(p.title.as_deref(), Some("1:1 с Аней 1:41"));
        let far = vec![ev("a", "1:1 с Аней", now() + D::minutes(11))];
        assert_eq!(
            plan(now(), &far, 2, &HashSet::new(), false, false).title,
            None
        );
    }

    #[test]
    fn reminder_fires_once_in_the_window() {
        let e = vec![ev("a", "1:1 с Аней", now() + D::seconds(119))];
        let p = plan(now(), &e, 2, &HashSet::new(), false, false);
        assert_eq!(p.remind.as_ref().map(|r| r.1), Some(2));
        let done: HashSet<String> = ["a".to_string()].into();
        assert!(plan(now(), &e, 2, &done, false, false).remind.is_none());
        let early = vec![ev("a", "x", now() + D::seconds(121))];
        assert!(
            plan(now(), &early, 2, &HashSet::new(), false, false)
                .remind
                .is_none()
        );
        assert!(
            plan(now(), &e, 2, &HashSet::new(), true, false)
                .remind
                .is_none()
        );
    }

    #[test]
    fn recording_beats_the_title_and_no_record_beats_the_countdown() {
        let e = vec![ev("a", "1:1 с Аней", now() + D::seconds(60))];
        assert_eq!(plan(now(), &e, 2, &HashSet::new(), true, false).title, None);
        assert_eq!(
            plan(now(), &e, 2, &HashSet::new(), false, true)
                .title
                .as_deref(),
            Some("звонок без записи")
        );
    }

    #[test]
    fn long_titles_are_shortened() {
        assert_eq!(
            menu_title("Ревью макетов у заказчика", 65),
            "Ревью макетов у з… 1:05"
        );
        assert_eq!(menu_title("Синк", -3), "Синк 0:00");
    }

    #[test]
    fn notice_texts_follow_the_boards() {
        let n = reminder_notice("1:1 с Аней", 2, "Рабочий", "lime@company.com", false);
        assert_eq!(n.title, "Через 2 минуты");
        assert_eq!(n.body, "1:1 с Аней\nРабочий · lime@company.com");
        assert_eq!(n.actions, vec!["Войти и записать", "Без записи"]);
        let w = reminder_notice("1:1 с Аней", 1, "Рабочий", "lime@company.com", true);
        assert_eq!(w.title, "1:1 с Аней — через 1 минуту");
        assert_eq!(w.body, "Рабочий · lime@company.com");
        let c = detected_notice(&DetectedCall {
            id: "c".into(),
            platform: "Google Meet".into(),
            url: "meet.google.com/kqv-tdre-xmn".into(),
            detected_at: now(),
        });
        assert_eq!(c.title, "Ты в звонке Google Meet");
        assert_eq!(c.actions, vec!["Записать", "Не сейчас"]);
        assert_eq!(
            reminder_notice("x", 5, "", "a@b.c", false).title,
            "Через 5 минут"
        );
    }
}
