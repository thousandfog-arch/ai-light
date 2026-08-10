#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ai_light::aggregator::StateAggregator;
use ai_light::app_lock::AppLock;
use ai_light::config::load_app_config;
use ai_light::http_server::{existing_instance_is_healthy, start_http_server};
use std::sync::Arc;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

mod ipc;
mod light_windows;
mod tray;
mod window_state;

fn main() {
    let app_lock = match AppLock::acquire() {
        Ok(Some(lock)) => lock,
        Ok(None) => return,
        Err(error) => {
            eprintln!("failed to acquire app lock: {error}");
            return;
        }
    };

    let app_config = load_app_config();
    let aggregator = Arc::new(StateAggregator::new());
    let server_aggregator = Arc::clone(&aggregator);
    let light_window_manager = Arc::new(light_windows::LightWindowManager::new());

    tauri::Builder::default()
        .manage(Arc::clone(&aggregator))
        .manage(Arc::clone(&light_window_manager))
        .manage(app_lock)
        .invoke_handler(tauri::generate_handler![
            ipc::confirm_light,
            ipc::remove_light,
            ipc::get_lights,
            ipc::get_diagnostics,
            ipc::open_project,
            ipc::open_codex,
            ipc::open_claude_session,
            ipc::open_session_logs,
            ipc::open_app_log,
            ipc::get_app_config,
            ipc::get_appearance,
            ipc::save_appearance,
            ipc::save_app_config_command,
            ipc::copy_path,
            ipc::pause_monitoring,
            ipc::resume_monitoring,
            ipc::open_settings,
            ipc::hide_main_window,
            ipc::hide_all_windows,
            ipc::current_window_project,
            ipc::detach_current_light,
            ipc::detach_current_light_with_nudge,
            ipc::is_current_light_attached,
            ipc::resize_current_window,
            ipc::set_current_window_always_on_top,
            ipc::resize_main_window,
            ipc::set_main_window_always_on_top,
            ipc::check_hooks,
            ipc::install_hooks_command,
            ipc::remove_hooks_command,
            ipc::preview_hook_config_command,
            ipc::quit_app,
            ipc::check_opencode,
            ipc::install_opencode_command,
            ipc::remove_opencode_command,
            ipc::check_reasonix,
            ipc::install_reasonix_command,
            ipc::remove_reasonix_command,
        ])
        .setup(move |app| {
            if existing_instance_is_healthy() {
                app.handle().exit(0);
                return Ok(());
            }

            let window = app
                .get_webview_window("main")
                .expect("main window should exist");

            tray::create_tray(app)?;
            window_state::restore_main_window_position(&window, &app_config)
                .map_err(std::io::Error::other)?;

            let close_manager = Arc::clone(&light_window_manager);
            let close_app = app.handle().clone();
            window.on_window_event(move |event| match event {
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    close_manager.hide_all(&close_app);
                }
                WindowEvent::Moved(position) => {
                    let _ = window_state::save_position(position.x, position.y);
                }
                _ => {}
            });

            for label in ["settings", "appearance"] {
                if let Some(utility_window) = app.get_webview_window(label) {
                    let window_to_hide = utility_window.clone();
                    utility_window.on_window_event(move |event| {
                        if let WindowEvent::CloseRequested { api, .. } = event {
                            api.prevent_close();
                            let _ = window_to_hide.hide();
                        }
                    });
                }
            }

            let emit_aggregator = Arc::clone(&aggregator);
            let sync_manager = Arc::clone(&light_window_manager);
            let sync_app = app.handle().clone();
            let sync_x = app_config.window_x;
            let sync_y = app_config.window_y;

            aggregator.set_on_change(move || {
                let lights = emit_aggregator.get_lights();
                let manager = Arc::clone(&sync_manager);
                let app_handle = sync_app.clone();
                let scheduler = sync_app.clone();
                let _ = scheduler.run_on_main_thread(move || {
                    let appearance = load_app_config();
                    let _ = manager.sync(
                        &app_handle,
                        &lights,
                        appearance.light_width,
                        appearance.label_font_size,
                        sync_x,
                        sync_y,
                    );
                });
            });

            start_http_server(Arc::clone(&server_aggregator), &app_config)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            ai_light::codex_watcher::start_codex_watcher(Arc::clone(&aggregator))?;

            light_window_manager
                .sync(
                    app.handle(),
                    &aggregator.get_lights(),
                    app_config.light_width,
                    app_config.label_font_size,
                    app_config.window_x,
                    app_config.window_y,
                )
                .map_err(std::io::Error::other)?;

            if let Ok(resource_dir) = app.path().resource_dir() {
                let _ = ai_light::hook_installer::install_hook_binary_from_resource(&resource_dir);
            }

            if !ai_light::hook_installer::check_hooks_installed() {
                WebviewWindowBuilder::new(
                    app,
                    "install-hooks",
                    WebviewUrl::App("install-hooks.html".into()),
                )
                .title("Claude Code Integration")
                .inner_size(560.0, 340.0)
                .resizable(false)
                .center()
                .build()?;
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
