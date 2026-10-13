//! Window / tray / hotkey plumbing around the main window:
//! * `tray` — frameless transparent popover anchored to the tray icon, hides on blur,
//! * `mini` — always-on-top mini-player shown while recording when the main window is not in front,
//!   draggable, snaps to screen edges,
//! * tray icon (macOS menu-bar title carries the timer, Windows tooltip), global shortcuts, autostart,
//!   "summary ready" notifications.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::Wry;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tauri_plugin_notification::NotificationExt as _;
use tauri_plugin_positioner::{Position, WindowExt};
use yc_core::error::PermissionKind;
use yc_core::events::{EngineEvent, RecorderState};
use yc_core::meeting::{MeetingStatus, Stage};

use crate::api::{Backend, RecorderPhase, RecorderStartOptions, Settings};

pub const TRAY_ID: &str = "yadacall";
const TRAY_W: f64 = if cfg!(target_os = "macos") {
    320.0
} else {
    360.0
};
const MINI_W: f64 = 300.0;
const SNAP_MARGIN: f64 = 12.0;
const SNAP_DISTANCE: f64 = 28.0;
/// A popover window hidden for this long is destroyed (its WebContent process is ~100 MB).
const POPOVER_IDLE: Duration = Duration::from_secs(60);

/// Mutable shell state shared by event handlers.
#[derive(Default)]
pub struct ShellState {
    recording: bool,
    mini_enabled: bool,
    record_key: Option<Shortcut>,
    bookmark_key: Option<Shortcut>,
    /// Menu-bar text while idle: "1:1 с Аней 1:41" before a call, "звонок без записи" (see `integrations_shell`).
    idle_title: Option<String>,
    /// Meetings processed in this session that are not done yet (tray "working" indicator).
    processing: HashSet<String>,
    /// Meetings seen in the transcription stage; the first later stage means "transcript ready".
    transcribing: HashSet<String>,
    /// Meetings whose "transcript ready" notification was shown.
    announced: HashSet<String>,
    /// A job failed and the user has not looked yet (tray error state).
    has_error: bool,
    /// Something finished and the user has not looked yet (tray "ready" badge).
    ready_badge: bool,
    /// The meeting the tray menu's "open last" entry points at.
    last_ready: Option<String>,
    last_item: Option<MenuItem<Wry>>,
    /// The tray menu and its "update" entry (present only while an update is available).
    tray_menu: Option<Menu<Wry>>,
    update_item: Option<MenuItem<Wry>>,
    /// Popover windows created on demand whose page has not reported its size yet: shown on the first
    /// `popover_fit` (or after a short timeout) so no blank frame appears.
    pending_show: HashSet<String>,
    /// Bumped whenever a popover is shown or hidden; a destroy timer only fires for the hide it belongs to.
    popover_seq: std::collections::HashMap<String, u64>,
    /// Where the user left the mini-player; restored when the window is created again.
    mini_pos: Option<PhysicalPosition<i32>>,
}

pub type Shared = Arc<Mutex<ShellState>>;
static MOVE_SEQ: AtomicU64 = AtomicU64::new(0);

fn state(app: &AppHandle) -> Shared {
    app.state::<Shared>().inner().clone()
}

// ---- windows ------------------------------------------------------------------------------------------

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    #[cfg(target_os = "macos")]
    let _ = app.show();
    hide_popover_window(app, "tray");
}

fn popover(
    app: &AppHandle,
    label: &str,
    query: &str,
    w: f64,
    h: f64,
) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(
        app,
        label,
        WebviewUrl::App(format!("index.html?w={query}").into()),
    )
    .title("YadaCall")
    .inner_size(w, h)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .resizable(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .focused(false)
    .build()
}

/// The `tray` / `mini` popover window, created on first use. The second value tells whether it was just
/// created (its page is still loading). Popovers are not created at startup: each one is a WebContent process.
fn ensure_popover(app: &AppHandle, label: &str) -> Option<(WebviewWindow, bool)> {
    if let Some(w) = app.get_webview_window(label) {
        return Some((w, false));
    }
    let w = match label {
        "tray" => popover(app, "tray", "tray", TRAY_W, 440.0),
        _ => popover(app, "mini", "mini", MINI_W, 64.0),
    }
    .map_err(|e| tracing::warn!("could not create the {label} window: {e}"))
    .ok()?;
    let handle = app.clone();
    if label == "tray" {
        w.on_window_event(move |e| {
            if let WindowEvent::Focused(false) = e {
                hide_popover_window(&handle, "tray");
            }
        });
    } else {
        let m = w.clone();
        w.on_window_event(move |e| {
            if let WindowEvent::Moved(_) = e {
                // Snap after the drag settles: debounce on a sequence number.
                let seq = MOVE_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
                let (m, handle) = (m.clone(), handle.clone());
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(260)).await;
                    if MOVE_SEQ.load(Ordering::SeqCst) == seq {
                        snap_to_edges(&m);
                        remember_mini_position(&handle, &m);
                    }
                });
            }
        });
    }
    Some((w, true))
}

fn remember_mini_position(app: &AppHandle, w: &WebviewWindow) {
    if let Ok(pos) = w.outer_position() {
        state(app)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mini_pos = Some(pos);
    }
}

/// Hides a popover and destroys it after [`POPOVER_IDLE`] unless it is shown again meanwhile.
pub fn hide_popover_window(app: &AppHandle, label: &str) {
    let Some(w) = app.get_webview_window(label) else {
        return;
    };
    if label == "mini" {
        remember_mini_position(app, &w);
    }
    let _ = w.hide();
    // A window still waiting for its first `popover_fit` must not appear after it was hidden.
    state(app)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pending_show
        .remove(label);
    let seq = bump_popover_seq(app, label);
    let (handle, label) = (app.clone(), label.to_owned());
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(POPOVER_IDLE).await;
        let current = state(&handle)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .popover_seq
            .get(&label)
            .copied();
        if current == Some(seq)
            && let Some(w) = handle.get_webview_window(&label)
            && !w.is_visible().unwrap_or(true)
        {
            tracing::debug!("destroying the idle {label} window");
            let _ = w.destroy();
        }
    });
}

fn bump_popover_seq(app: &AppHandle, label: &str) -> u64 {
    let shared = state(app);
    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
    let n = g.popover_seq.entry(label.to_owned()).or_insert(0);
    *n += 1;
    *n
}

/// Shows a popover that was created a moment ago once its page is ready (first `popover_fit`), or after
/// a timeout if the page never reports.
fn show_when_ready(app: &AppHandle, label: &str) {
    state(app)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pending_show
        .insert(label.to_owned());
    let (handle, label) = (app.clone(), label.to_owned());
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(2500)).await;
        reveal_pending(&handle, &label);
    });
}

fn reveal_pending(app: &AppHandle, label: &str) {
    let pending = state(app)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pending_show
        .remove(label);
    if !pending {
        return;
    }
    if label == "tray" {
        if let Some(w) = app.get_webview_window("tray") {
            anchor_tray_window(&w);
            let _ = w.show();
            let _ = w.set_focus();
        }
    } else {
        place_and_show_mini(app);
    }
}

/// Pulls the mini-player to the nearest screen edge when it was dropped close to one.
fn snap_to_edges(w: &WebviewWindow) {
    let (Ok(pos), Ok(size), Ok(Some(mon))) =
        (w.outer_position(), w.outer_size(), w.current_monitor())
    else {
        return;
    };
    let scale = mon.scale_factor();
    let (mp, ms) = (mon.position(), mon.size());
    let (x, y) = (
        f64::from(pos.x - mp.x) / scale,
        f64::from(pos.y - mp.y) / scale,
    );
    let (ww, wh) = (
        f64::from(size.width) / scale,
        f64::from(size.height) / scale,
    );
    let (sw, sh) = (f64::from(ms.width) / scale, f64::from(ms.height) / scale);
    let snap = |v: f64, lo: f64, hi: f64| -> f64 {
        if (v - lo).abs() <= SNAP_DISTANCE {
            lo
        } else if (hi - v).abs() <= SNAP_DISTANCE {
            hi
        } else {
            v
        }
    };
    let nx = snap(x, SNAP_MARGIN, sw - ww - SNAP_MARGIN);
    let ny = snap(y, SNAP_MARGIN, sh - wh - SNAP_MARGIN);
    if (nx - x).abs() > 0.5 || (ny - y).abs() > 0.5 {
        let _ = w.set_position(PhysicalPosition::new(
            ((nx * scale) as i32) + mp.x,
            ((ny * scale) as i32) + mp.y,
        ));
    }
}

/// Popover windows report their content height; fit and keep them anchored to the tray icon.
pub fn fit_popover(app: &AppHandle, label: &str, height: f64) {
    let Some(w) = app.get_webview_window(label) else {
        return;
    };
    let width = if label == "tray" { TRAY_W } else { MINI_W };
    let _ = w.set_size(LogicalSize::new(width, height.clamp(48.0, 720.0)));
    if label == "tray" && w.is_visible().unwrap_or(false) {
        anchor_tray_window(&w);
    }
    reveal_pending(app, label);
}

fn anchor_tray_window(w: &WebviewWindow) {
    let pos = if cfg!(target_os = "macos") {
        Position::TrayCenter
    } else if cfg!(target_os = "windows") {
        Position::TrayBottomCenter
    } else {
        Position::TopRight
    };
    let _ = w.move_window(pos);
}

fn toggle_tray_popover(app: &AppHandle) {
    let Some((w, created)) = ensure_popover(app, "tray") else {
        return;
    };
    if created {
        bump_popover_seq(app, "tray");
        show_when_ready(app, "tray");
    } else if w.is_visible().unwrap_or(false) {
        hide_popover_window(app, "tray");
    } else {
        bump_popover_seq(app, "tray");
        anchor_tray_window(&w);
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn show_mini(app: &AppHandle) {
    let Some((w, created)) = ensure_popover(app, "mini") else {
        return;
    };
    bump_popover_seq(app, "mini");
    if created {
        show_when_ready(app, "mini");
    } else if !w.is_visible().unwrap_or(false) {
        place_and_show_mini(app);
    }
}

/// Puts the mini-player where the user left it (first time: bottom-right corner inside the snap margin) and shows it.
fn place_and_show_mini(app: &AppHandle) {
    let Some(w) = app.get_webview_window("mini") else {
        return;
    };
    let remembered = state(app)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .mini_pos;
    if let Some(pos) = remembered {
        let _ = w.set_position(pos);
    } else if let Ok(Some(mon)) = w.current_monitor().or_else(|_| app.primary_monitor()) {
        let s = mon.scale_factor();
        let (sw, sh) = (
            f64::from(mon.size().width) / s,
            f64::from(mon.size().height) / s,
        );
        let p = mon.position();
        let _ = w.set_position(PhysicalPosition::new(
            ((sw - MINI_W - SNAP_MARGIN) * s) as i32 + p.x,
            ((sh - 64.0 - SNAP_MARGIN - 40.0) * s) as i32 + p.y,
        ));
    }
    let _ = w.show();
}

fn hide_mini(app: &AppHandle) {
    hide_popover_window(app, "mini");
}

/// Main window focus decides whether the mini-player is useful: it appears when you leave the window mid-recording.
pub fn on_main_focus(app: &AppHandle, focused: bool) {
    let st = state(app);
    let (rec, enabled) = {
        let mut g = st.lock().unwrap_or_else(|e| e.into_inner());
        if focused && (g.ready_badge || g.has_error) {
            g.ready_badge = false;
            g.has_error = false;
            drop(g);
            refresh_idle_tray(app);
            g = st.lock().unwrap_or_else(|e| e.into_inner());
        }
        (g.recording, g.mini_enabled)
    };
    if !rec || !enabled {
        return;
    }
    if focused {
        hide_mini(app)
    } else {
        show_mini(app)
    }
}

// ---- tray icon ----------------------------------------------------------------------------------------

/// Shows "Обновить YadaCall до <version>" in the tray menu while an update is available, removes it otherwise.
pub fn set_update_available(app: &AppHandle, version: Option<&str>) {
    let shared = state(app);
    let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
    let Some(menu) = g.tray_menu.clone() else {
        return;
    };
    match (version, g.update_item.clone()) {
        (Some(v), Some(item)) => {
            let _ = item.set_text(format!("Обновить YadaCall до {v}"));
        }
        (Some(v), None) => {
            if let Ok(item) = MenuItem::with_id(
                app,
                "update",
                format!("Обновить YadaCall до {v}"),
                true,
                None::<&str>,
            ) && menu.insert(&item, 3).is_ok()
            {
                g.update_item = Some(item);
            }
        }
        (None, Some(item)) => {
            let _ = menu.remove(&item);
            g.update_item = None;
        }
        (None, None) => {}
    }
}

pub fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let record = MenuItem::with_id(app, "record", "Записать встречу", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Открыть YadaCall", true, None::<&str>)?;
    let last = MenuItem::with_id(
        app,
        "last",
        "Открыть последнюю встречу",
        false,
        None::<&str>,
    )?;
    state(app)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .last_item = Some(last.clone());
    let settings = MenuItem::with_id(app, "settings", "Настройки", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &record,
            &open,
            &last,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &quit,
        ],
    )?;

    state(app)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .tray_menu = Some(menu.clone());

    #[cfg(target_os = "macos")]
    let icon = Image::from_bytes(include_bytes!("../icons/tray-template.png"))?;
    #[cfg(not(target_os = "macos"))]
    let icon = Image::from_bytes(include_bytes!("../icons/tray-idle.png"))?;

    let b = TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("YadaCall")
        .menu(&menu)
        .show_menu_on_left_click(false);
    #[cfg(target_os = "macos")]
    let b = b.icon_as_template(true);
    b.on_menu_event(|app, ev| match ev.id().as_ref() {
        "record" => toggle_recording(app),
        "open" => show_main(app),
        "last" => {
            let id = state(app)
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .last_ready
                .clone();
            if let Some(id) = id {
                show_main(app);
                let _ = tauri::Emitter::emit_to(
                    app,
                    "main",
                    "navigate",
                    format!("#/meeting/{id}/summary"),
                );
            }
        }
        "settings" => {
            show_main(app);
            let _ = tauri::Emitter::emit_to(app, "main", "navigate", "#/settings");
        }
        "update" => {
            show_main(app);
            let _ = tauri::Emitter::emit_to(app, "main", "navigate", "#/settings");
            let _ = tauri::Emitter::emit_to(
                app,
                "main",
                "update",
                crate::updates::UpdateEvent::OpenSheet,
            );
        }
        "quit" => app.exit(0),
        _ => {}
    })
    .on_tray_icon_event(|tray, ev| {
        tauri_plugin_positioner::on_tray_event(tray.app_handle(), &ev);
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = ev
        {
            toggle_tray_popover(tray.app_handle());
        }
    })
    .build(app)?;
    Ok(())
}

/// Whether a recording is running or paused.
pub fn is_recording(app: &AppHandle) -> bool {
    state(app)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .recording
}

/// Sets the idle menu-bar text (call countdown); a running recording keeps its own timer.
pub fn set_idle_title(app: &AppHandle, title: Option<String>) {
    let (changed, rec) = {
        let st = state(app);
        let mut g = st.lock().unwrap_or_else(|e| e.into_inner());
        let changed = g.idle_title != title;
        g.idle_title = title;
        (changed, g.recording)
    };
    if changed && !rec {
        set_tray_state(app, false, 0, false);
    }
}

/// What the idle tray shows: working, error or "something is ready" (in that priority).
fn idle_badge(g: &ShellState) -> Option<&'static str> {
    if g.has_error {
        Some("!")
    } else if !g.processing.is_empty() {
        Some("⋯")
    } else if g.ready_badge {
        Some("✓")
    } else {
        None
    }
}

/// Re-draws the tray for the not-recording state (after a processing / error / ready change).
fn refresh_idle_tray(app: &AppHandle) {
    let recording = state(app)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .recording;
    if !recording {
        set_tray_state(app, false, 0, false);
    }
}

fn set_tray_state(app: &AppHandle, recording: bool, elapsed_ms: u64, paused: bool) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let idle = state(app)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .idle_title
        .clone();
    let badge = idle_badge(&state(app).lock().unwrap_or_else(|e| e.into_inner()));
    let secs = elapsed_ms / 1000;
    let clock = if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    } else {
        format!("{:02}:{:02}", secs / 60, secs % 60)
    };
    #[cfg(target_os = "macos")]
    {
        let _ = tray.set_title(if recording {
            Some(format!("{}{clock}", if paused { "⏸ " } else { "● " }))
        } else {
            match (badge, idle.clone()) {
                (Some(b), Some(t)) => Some(format!("{b} {t}")),
                (Some(b), None) => Some(b.to_owned()),
                (None, t) => t,
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = tray.set_tooltip(Some(if recording {
            format!("YadaCall · {clock}{}", if paused { " ⏸" } else { "" })
        } else if let Some(t) = idle {
            format!("YadaCall · {t}")
        } else {
            match badge {
                Some("!") => "YadaCall · ошибка обработки".to_string(),
                Some("⋯") => "YadaCall · обработка…".to_string(),
                Some(_) => "YadaCall · готово".to_string(),
                None => "YadaCall".to_string(),
            }
        }));
        let bytes: &[u8] = if recording {
            include_bytes!("../icons/tray-rec.png")
        } else {
            include_bytes!("../icons/tray-idle.png")
        };
        if let Ok(img) = Image::from_bytes(bytes) {
            let _ = tray.set_icon(Some(img));
        }
    }
    let _ = (paused, &clock, badge);
}

// ---- recording hotkeys --------------------------------------------------------------------------------

/// Start/stop from the tray menu or the hotkey.
pub fn toggle_recording(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let b = app.state::<Arc<dyn Backend>>();
        match b.recorder_state().await.map(|s| s.phase) {
            Ok(RecorderPhase::Idle) => {
                let _ = b.recorder_start(RecorderStartOptions::default()).await;
            }
            Ok(_) => {
                let _ = b.recorder_stop().await;
            }
            Err(_) => {}
        }
    });
}

fn add_bookmark(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = app
            .state::<Arc<dyn Backend>>()
            .recorder_bookmark(None)
            .await;
    });
}

/// (Re)registers the two global shortcuts from settings. A failed registration (taken by another app) is ignored;
/// the UI still has buttons.
fn register_hotkeys(app: &AppHandle, s: &Settings) {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let parse = |a: &str| a.parse::<Shortcut>().ok();
    let (rec, mark) = (parse(&s.hotkey_record), parse(&s.hotkey_bookmark));
    for sc in [rec, mark].into_iter().flatten() {
        let _ = gs.register(sc);
    }
    let st = state(app);
    let mut g = st.lock().unwrap_or_else(|e| e.into_inner());
    g.record_key = rec;
    g.bookmark_key = mark;
}

/// The plugin-level handler (set once in the builder): dispatches by shortcut id.
pub fn hotkey_handler(
    app: &AppHandle,
    shortcut: &Shortcut,
    event: tauri_plugin_global_shortcut::ShortcutEvent,
) {
    if event.state() != ShortcutState::Pressed {
        return;
    }
    let st = state(app);
    let (is_rec, is_mark) = {
        let g = st.lock().unwrap_or_else(|e| e.into_inner());
        (
            g.record_key
                .as_ref()
                .is_some_and(|k| k.id() == shortcut.id()),
            g.bookmark_key
                .as_ref()
                .is_some_and(|k| k.id() == shortcut.id()),
        )
    };
    if is_rec {
        toggle_recording(app)
    } else if is_mark {
        add_bookmark(app)
    }
}

/// Applies settings that live in the OS: hotkeys, launch at login, mini-player toggle.
pub fn apply_settings(app: &AppHandle, s: &Settings) {
    register_hotkeys(app, s);
    let al = app.autolaunch();
    let _ = if s.launch_at_login {
        al.enable()
    } else {
        al.disable()
    };
    let st = state(app);
    st.lock().unwrap_or_else(|e| e.into_inner()).mini_enabled = s.mini_player;
    if !s.mini_player {
        hide_mini(app);
    }
}

// ---- engine events ------------------------------------------------------------------------------------

/// Reacts to engine events at the shell level (tray title, mini-player, notifications). The UI gets the same
/// events through the `engine` event.
pub fn on_engine_event(app: &AppHandle, e: &EngineEvent) {
    match e {
        EngineEvent::Recorder {
            state: rs,
            elapsed_ms,
            ..
        } => {
            let rec = matches!(rs, RecorderState::Recording | RecorderState::Paused);
            let was = {
                let st = state(app);
                let mut g = st.lock().unwrap_or_else(|e| e.into_inner());
                let was = g.recording;
                g.recording = rec;
                (was, g.mini_enabled)
            };
            set_tray_state(app, rec, *elapsed_ms, matches!(rs, RecorderState::Paused));
            if rec && !was.0 && was.1 {
                let main_focused = app
                    .get_webview_window("main")
                    .and_then(|w| w.is_focused().ok())
                    .unwrap_or(false);
                if !main_focused {
                    show_mini(app);
                }
            }
            if !rec {
                hide_mini(app);
            }
        }
        EngineEvent::Status { meeting, status } => on_status(app, meeting.to_string(), status),
        EngineEvent::Notice { message, .. } => {
            let _ = app
                .notification()
                .builder()
                .title("YadaCall")
                .body(message)
                .show();
        }
        _ => {}
    }
}

/// Opens the OS privacy pane of a permission (macOS: `Privacy_Microphone` / `Privacy_AudioCapture`).
pub fn open_os_privacy_settings(kind: PermissionKind) {
    crate::permissions::open_system_settings(kind);
}

/// What a status change should tell the user.
enum Announce {
    TranscriptReady,
    Ready,
    Failed(String),
}

/// Tracks processing / ready / error for the tray and shows "Расшифровка готова" / "Итоги готовы".
fn on_status(app: &AppHandle, id: String, status: &MeetingStatus) {
    let announce = {
        let st = state(app);
        let mut g = st.lock().unwrap_or_else(|e| e.into_inner());
        match status {
            MeetingStatus::Processing { stage, .. } => {
                g.processing.insert(id.clone());
                if *stage == Stage::Transcribe {
                    g.transcribing.insert(id.clone());
                    None
                } else if g.transcribing.remove(&id) {
                    g.announced.insert(id.clone());
                    Some(Announce::TranscriptReady)
                } else {
                    None
                }
            }
            MeetingStatus::Ready => {
                if g.processing.remove(&id) {
                    g.transcribing.remove(&id);
                    g.ready_badge = true;
                    g.last_ready = Some(id.clone());
                    if let Some(item) = &g.last_item {
                        let _ = item.set_enabled(true);
                    }
                    Some(Announce::Ready)
                } else {
                    None
                }
            }
            MeetingStatus::Failed { message, .. } => {
                g.processing.remove(&id);
                g.transcribing.remove(&id);
                g.has_error = true;
                Some(Announce::Failed(message.clone()))
            }
            MeetingStatus::Recording => None,
        }
    };
    refresh_idle_tray(app);
    let Some(announce) = announce else { return };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let backend = app.state::<Arc<dyn Backend>>();
        let russian = backend
            .get_settings()
            .await
            .map(|s| matches!(s.ui_language, crate::api::UiLanguage::Ru))
            .unwrap_or(true);
        let (title, tasks, has_summary) = match backend.get_meeting(&id).await {
            Ok(d) => (
                d.meeting.title,
                d.summary.as_ref().map_or(0, |s| s.tasks.len()),
                d.summary.is_some(),
            ),
            Err(_) => (String::new(), 0, false),
        };
        let tr = |ru: &'static str, en: &'static str| if russian { ru } else { en };
        let (head, body) = match announce {
            Announce::TranscriptReady => (tr("Расшифровка готова", "Transcript ready"), title),
            Announce::Ready if has_summary => {
                state(&app)
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .announced
                    .remove(&id);
                let body = if tasks > 0 {
                    if russian {
                        format!("{title}\n{tasks} задач(и) в итогах")
                    } else {
                        format!("{title}\n{tasks} task(s) in the summary")
                    }
                } else {
                    title
                };
                (tr("Итоги готовы", "Summary ready"), body)
            }
            Announce::Ready => {
                let announced = {
                    let st = state(&app);
                    let mut g = st.lock().unwrap_or_else(|e| e.into_inner());
                    g.announced.remove(&id)
                };
                if announced {
                    return;
                }
                (tr("Расшифровка готова", "Transcript ready"), title)
            }
            Announce::Failed(message) => (
                tr(
                    "Не удалось обработать встречу",
                    "Could not process the meeting",
                ),
                message,
            ),
        };
        let _ = app.notification().builder().title(head).body(body).show();
    });
}

/// Initial shell state from the settings the backend currently holds.
pub fn init_state(app: &AppHandle, s: &Settings) {
    state(app)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .mini_enabled = s.mini_player;
    apply_settings(app, s);
}
