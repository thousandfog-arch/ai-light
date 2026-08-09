use tauri::{
    menu::{Menu, MenuItem, Submenu},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Emitter, Manager,
};

use ai_light::config::{load_app_config, save_app_config};
use std::sync::Arc;

use crate::light_windows::LightWindowManager;

const TOGGLE_MAIN_ID: &str = "toggle-main";
const SETTINGS_ID: &str = "settings";
const QUIT_ID: &str = "quit";
const SIZE_SMALL_ID: &str = "size-small";
const SIZE_MEDIUM_ID: &str = "size-medium";
const SIZE_LARGE_ID: &str = "size-large";

pub fn create_tray(app: &mut App) -> tauri::Result<()> {
    let toggle_item = MenuItem::with_id(
        app,
        TOGGLE_MAIN_ID,
        "Show/Hide AI Light",
        true,
        None::<&str>,
    )?;
    let settings_item =
        MenuItem::with_id(app, SETTINGS_ID, "Settings", true, None::<&str>)?;
    let small_item = MenuItem::with_id(app, SIZE_SMALL_ID, "Small", true, None::<&str>)?;
    let medium_item = MenuItem::with_id(app, SIZE_MEDIUM_ID, "Medium", true, None::<&str>)?;
    let large_item = MenuItem::with_id(app, SIZE_LARGE_ID, "Large", true, None::<&str>)?;
    let size_menu = Submenu::with_items(
        app,
        "Size",
        true,
        &[&small_item, &medium_item, &large_item],
    )?;
    let quit_item = MenuItem::with_id(app, QUIT_ID, "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle_item, &size_menu, &settings_item, &quit_item])?;

    let mut builder = TrayIconBuilder::with_id("ai-light-tray")
        .tooltip("AI Light")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            TOGGLE_MAIN_ID => {
                let _ = toggle_main_window(app);
            }
            SETTINGS_ID => {
                let _ = show_settings_window(app);
            }
            SIZE_SMALL_ID => {
                let _ = set_light_size(app, "small");
            }
            SIZE_MEDIUM_ID => {
                let _ = set_light_size(app, "medium");
            }
            SIZE_LARGE_ID => {
                let _ = set_light_size(app, "large");
            }
            QUIT_ID => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } = event
            {
                let _ = toggle_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}

pub fn hide_main_window(app: &AppHandle) -> Result<(), String> {
    let manager = app.state::<Arc<LightWindowManager>>();
    manager.hide_all(app);
    Ok(())
}

pub fn toggle_main_window(app: &AppHandle) -> Result<(), String> {
    app.state::<Arc<LightWindowManager>>().toggle_all(app)
}

fn set_light_size(app: &AppHandle, size: &str) -> Result<(), String> {
    let mut config = load_app_config();
    config.light_size = size.to_string();
    save_app_config(&config).map_err(|error| error.to_string())?;
    app.emit("light-size-changed", size)
        .map_err(|error| error.to_string())
}

fn show_settings_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("settings")
        .ok_or_else(|| "settings window is not available".to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}
