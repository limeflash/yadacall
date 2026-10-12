//! YadaCall desktop shell: Tauri 2 app that hosts the Svelte UI (`../ui`) and forwards everything to a
//! [`api::Backend`]: the real [`engine_backend::EngineBackend`], or the in-memory [`demo::DemoBackend`] when
//! `YADACALL_DEMO=1` (UI work, screenshots).

pub mod adapters;
pub mod api;
pub mod commands;
pub mod demo;
pub mod engine_backend;
pub mod paths;
pub mod permissions;
pub mod shell;

use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager, RunEvent, WindowEvent};

use api::Backend;
use engine_backend::{BackendConfig, EngineBackend};

/// The engine, kept so it can be shut down (stops the local LLM server) when the app exits.
struct EngineHandle(Arc<EngineBackend>);

/// Every command exposed to the webviews (also the ACL allow-list generated in `build.rs`).
pub const COMMANDS: &[&str] = &[
    "list_meetings",
    "get_meeting",
    "rename_meeting",
    "delete_meeting",
    "rename_speaker",
    "set_task_done",
    "regenerate_summary",
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
];

pub fn run() {
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
        .manage::<shell::Shared>(Arc::new(Mutex::new(shell::ShellState::default())))
        .invoke_handler(tauri::generate_handler![
            commands::list_meetings,
            commands::get_meeting,
            commands::rename_meeting,
            commands::delete_meeting,
            commands::rename_speaker,
            commands::set_task_done,
            commands::regenerate_summary,
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
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            // Engine events -> every webview (`engine` event) and shell reactions (tray title, mini-player, notifications).
            let sink: api::EventSink = {
                let h = handle.clone();
                Arc::new(move |e| {
                    shell::on_engine_event(&h, &e);
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
                let engine =
                    tauri::async_runtime::block_on(EngineBackend::open(BackendConfig::new(
                        data_dir,
                        sink,
                    )))
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

            shell::create_popovers(&handle)?;
            shell::build_tray(&handle)?;
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
            if let RunEvent::Exit = event
                && let Some(h) = app.try_state::<EngineHandle>()
            {
                tauri::async_runtime::block_on(h.0.shutdown());
            }
            let _ = (app, event);
        });
}
