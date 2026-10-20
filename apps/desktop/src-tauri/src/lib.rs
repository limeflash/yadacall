//! YadaCall desktop shell: Tauri 2 app that hosts the Svelte UI (`../ui`) and forwards everything to a
//! [`api::Backend`]: the real [`engine_backend::EngineBackend`], or the in-memory [`demo::DemoBackend`] when
//! `YADACALL_DEMO=1` (UI work, screenshots).

pub mod adapters;
pub mod api;
pub mod commands;
pub mod demo;
pub mod engine_backend;
pub mod integrations_api;
pub mod integrations_commands;
pub mod integrations_demo;
pub mod integrations_real;
pub mod integrations_shell;
pub mod paths;
pub mod permissions;
pub mod shell;
pub mod updates;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager, RunEvent, WindowEvent};

use api::Backend;
use engine_backend::{BackendConfig, EngineBackend};
use integrations_api::Integrations;

/// The engine, kept so it can be shut down (stops the local LLM server) when the app exits.
struct EngineHandle(Arc<EngineBackend>);

/// Every command exposed to the webviews (also the ACL allow-list generated in `build.rs`).
pub const COMMANDS: &[&str] = &[
    "list_meetings",
    "list_folders",
    "create_folder",
    "update_folder",
    "delete_folder",
    "set_meeting_folder",
    "set_meeting_favorite",
    "add_bookmark",
    "update_bookmark",
    "remove_bookmark",
    "get_meeting",
    "rename_meeting",
    "delete_meeting",
    "rename_speaker",
    "set_task_done",
    "regenerate_summary",
    "meeting_retranscribe",
    "export_meeting",
    "reveal_meeting",
    "copy_for_claude",
    "recorder_state",
    "recorder_start",
    "recorder_pause",
    "recorder_resume",
    "recorder_stop",
    "recorder_cancel",
    "recorder_bookmark",
    "list_input_devices",
    "search",
    "ask",
    "list_tasks",
    "import_audio",
    "list_imports",
    "models_overview",
    "model_download",
    "model_pause",
    "model_cancel",
    "model_remove",
    "model_set_active",
    "model_benchmark",
    "ollama_status",
    "test_summary_provider",
    "get_settings",
    "update_settings",
    "permissions",
    "request_permission",
    "open_system_settings",
    "show_main_window",
    "open_in_main",
    "popover_fit",
    "hide_popover",
    "quit_app",
    "check_update",
    "install_update",
    "reveal_logs",
    "open_automation_settings",
];

/// Commands of the integrations (`integrations_commands.rs`); same ACL rules as [`COMMANDS`].
pub const INTEGRATION_COMMANDS: &[&str] = &[
    "accounts",
    "add_google_account",
    "cancel_add_account",
    "remove_account",
    "reauth_account",
    "set_account_policy",
    "set_account_visible",
    "agenda",
    "set_event_recording",
    "join_and_record",
    "join_without_recording",
    "connections",
    "toggle_want",
    "reminder_settings",
    "set_reminder_settings",
    "detected_call",
    "resolve_detected_call",
];

pub fn run() {
    let _log_guard = init_file_logging();
    let builder = tauri::Builder::default()
        // must be first: a second launch just focuses the running instance
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            shell::show_main(app)
        }))
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_denylist(&["tray", "mini"])
                .build(),
        )
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(shell::hotkey_handler)
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage::<shell::Shared>(Arc::new(Mutex::new(shell::ShellState::default())))
        .invoke_handler(tauri::generate_handler![
            commands::list_meetings,
            commands::list_folders,
            commands::create_folder,
            commands::update_folder,
            commands::delete_folder,
            commands::set_meeting_folder,
            commands::set_meeting_favorite,
            commands::add_bookmark,
            commands::update_bookmark,
            commands::remove_bookmark,
            commands::get_meeting,
            commands::rename_meeting,
            commands::delete_meeting,
            commands::rename_speaker,
            commands::set_task_done,
            commands::regenerate_summary,
            commands::meeting_retranscribe,
            commands::export_meeting,
            commands::reveal_meeting,
            commands::copy_for_claude,
            commands::recorder_state,
            commands::recorder_start,
            commands::recorder_pause,
            commands::recorder_resume,
            commands::recorder_stop,
            commands::recorder_cancel,
            commands::recorder_bookmark,
            commands::list_input_devices,
            commands::search,
            commands::ask,
            commands::list_tasks,
            commands::import_audio,
            commands::list_imports,
            commands::models_overview,
            commands::model_download,
            commands::model_pause,
            commands::model_cancel,
            commands::model_remove,
            commands::model_set_active,
            commands::model_benchmark,
            commands::ollama_status,
            commands::test_summary_provider,
            commands::get_settings,
            commands::update_settings,
            commands::permissions,
            commands::request_permission,
            commands::open_system_settings,
            commands::show_main_window,
            commands::open_in_main,
            commands::popover_fit,
            commands::hide_popover,
            commands::quit_app,
            commands::check_update,
            commands::install_update,
            commands::reveal_logs,
            commands::open_automation_settings,
            integrations_commands::accounts,
            integrations_commands::add_google_account,
            integrations_commands::cancel_add_account,
            integrations_commands::remove_account,
            integrations_commands::reauth_account,
            integrations_commands::set_account_policy,
            integrations_commands::set_account_visible,
            integrations_commands::agenda,
            integrations_commands::set_event_recording,
            integrations_commands::join_and_record,
            integrations_commands::join_without_recording,
            integrations_commands::connections,
            integrations_commands::toggle_want,
            integrations_commands::reminder_settings,
            integrations_commands::set_reminder_settings,
            integrations_commands::detected_call,
            integrations_commands::resolve_detected_call,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            // Engine events -> every webview (`engine` event) and shell reactions (tray title, mini-player, notifications).
            let sink: api::EventSink = {
                let h = handle.clone();
                Arc::new(move |e| {
                    shell::on_engine_event(&h, &e);
                    integrations_shell::on_engine_event(&h, &e);
                    let _ = h.emit("engine", &e);
                })
            };
            if std::env::var_os("YADACALL_DEMO").is_some_and(|v| v != "0" && !v.is_empty()) {
                let demo = demo::DemoBackend::new(sink);
                let backend: Arc<dyn Backend> = demo.clone();
                app.manage(backend);
                tauri::async_runtime::spawn(demo.run());
            } else {
                let data_dir = paths::data_dir(|| {
                    handle
                        .path()
                        .app_data_dir()
                        .unwrap_or_else(|_| paths::home_dir().join(".yadacall"))
                });
                let engine = tauri::async_runtime::block_on(EngineBackend::open(
                    BackendConfig::new(data_dir, sink),
                ))
                .map_err(|e| format!("could not start the YadaCall engine: {e}"))?;
                // The player loads audio through the asset protocol, limited to the library and the mix cache.
                for dir in engine.asset_roots() {
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = app.asset_protocol_scope().allow_directory(&dir, true);
                }
                let backend: Arc<dyn Backend> = engine.clone();
                app.manage(backend);
                app.manage(EngineHandle(engine));
            }

            // Integrations live next to the backend, never inside it. `YADACALL_DEMO=1` keeps the in-memory ones.
            let sink = integrations_shell::sink(&handle);
            let demo = std::env::var_os("YADACALL_DEMO").is_some_and(|v| v != "0" && !v.is_empty());
            if demo {
                let integrations = integrations_demo::DemoIntegrations::new(sink);
                let i: Arc<dyn Integrations> = integrations.clone();
                app.manage(i);
                tauri::async_runtime::spawn(integrations.run());
            } else {
                let data_dir = paths::data_dir(|| {
                    handle
                        .path()
                        .app_data_dir()
                        .unwrap_or_else(|_| paths::home_dir().join(".yadacall"))
                });
                let open: integrations_real::OpenUrl = Arc::new(|url| {
                    let _ = tauri_plugin_opener::open_url(url, None::<&str>);
                });
                let real = integrations_real::RealIntegrations::open(&data_dir, sink, open)
                    .map_err(|e| format!("could not start the calendar integrations: {e}"))?;
                let i: Arc<dyn Integrations> = real.clone();
                app.manage(i);
                app.manage(integrations_shell::ConnectHandle(real.clone()));
                // `AutoRecord` (policy "Всегда") starts the recorder the same way "Войти и записать" does.
                let auto: integrations_real::AutoRecord = {
                    let h = handle.clone();
                    Arc::new(move |out| {
                        let h = h.clone();
                        tauri::async_runtime::spawn(async move {
                            let backend = h.state::<Arc<dyn Backend>>();
                            integrations_shell::apply_join(&h, backend.inner(), &out).await;
                        });
                    })
                };
                tauri::async_runtime::spawn(real.clone().run(auto));
            }
            integrations_shell::start(&handle, demo);

            shell::build_tray(&handle)?;
            updates::start_background_check(handle.clone());
            let h2 = handle.clone();
            tauri::async_runtime::spawn(async move {
                let backend = h2.state::<Arc<dyn Backend>>();
                if let Ok(s) = backend.get_settings().await {
                    shell::init_state(&h2, &s);
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| match (window.label(), event) {
            // Closing the main window keeps the app (and an active recording) alive in the tray.
            ("main", WindowEvent::CloseRequested { api, .. }) => {
                api.prevent_close();
                let _ = window.hide();
            }
            ("main", WindowEvent::Focused(f)) => shell::on_main_focus(window.app_handle(), *f),
            _ => {}
        });

    #[allow(unused_variables)]
    builder
        .build(tauri::generate_context!())
        .expect("failed to build YadaCall")
        .run(|app, event| {
            // macOS: clicking the Dock icon with the window hidden brings it back.
            #[cfg(target_os = "macos")]
            if let RunEvent::Reopen { .. } = event {
                shell::show_main(app);
            }
            if let RunEvent::Exit = event {
                if let Some(c) = app.try_state::<integrations_shell::ConnectHandle>() {
                    c.0.stop();
                }
                if let Some(h) = app.try_state::<EngineHandle>() {
                    tauri::async_runtime::block_on(h.0.shutdown());
                }
            }
            let _ = (app, event);
        });
}

fn init_file_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_appender::rolling::{Builder, Rotation};
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let Some(directory) = log_directory() else {
        init_stderr_logging(filter);
        return None;
    };
    if let Err(error) = std::fs::create_dir_all(&directory) {
        eprintln!(
            "could not create YadaCall log directory {}: {error}",
            directory.display()
        );
        init_stderr_logging(filter);
        return None;
    }
    let appender = match Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("yadacall.log")
        .max_log_files(7)
        .build(&directory)
    {
        Ok(appender) => appender,
        Err(error) => {
            eprintln!("could not open YadaCall log files: {error}");
            init_stderr_logging(filter);
            return None;
        }
    };
    let (writer, guard) = tracing_appender::non_blocking(appender);
    if let Err(error) = tracing_subscriber::fmt()
        .with_env_filter(filter.clone())
        .with_writer(writer)
        .with_ansi(false)
        .try_init()
    {
        eprintln!("could not initialize YadaCall file logging: {error}");
        init_stderr_logging(filter);
        return None;
    }
    Some(guard)
}

fn init_stderr_logging(filter: tracing_subscriber::EnvFilter) {
    if let Err(error) = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init()
    {
        eprintln!("could not initialize YadaCall logging: {error}");
    }
}

#[cfg(target_os = "macos")]
fn log_directory() -> Option<PathBuf> {
    Some(paths::home_dir().join("Library/Logs/YadaCall"))
}

#[cfg(target_os = "windows")]
fn log_directory() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| Some(paths::home_dir().join("AppData/Local")))
        .map(|base| base.join("YadaCall").join("logs"))
}

#[cfg(target_os = "linux")]
fn log_directory() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(paths::home_dir().join(".local/state")))
        .map(|base| base.join("yadacall"))
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn log_directory() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod log_tests {
    use super::log_directory;

    #[test]
    fn log_directory_is_inside_the_per_platform_log_location() {
        let dir = log_directory().expect("a log directory on desktop platforms");
        let text = dir.to_string_lossy().replace('\\', "/");
        #[cfg(target_os = "macos")]
        assert!(text.ends_with("Library/Logs/YadaCall"), "{text}");
        #[cfg(target_os = "windows")]
        assert!(text.ends_with("YadaCall/logs"), "{text}");
        #[cfg(target_os = "linux")]
        assert!(text.ends_with("yadacall"), "{text}");
    }
}
