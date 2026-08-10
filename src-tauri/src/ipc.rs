use ai_light::aggregator::StateAggregator;
use ai_light::config::{
    get_config_dir, get_config_path, get_lock_path, get_log_path, get_runtime_path,
    load_app_config, load_runtime_config, save_app_config, AppConfig,
};
use ai_light::hook_installer::{
    check_hooks_installed, check_opencode_integration, check_reasonix_integration,
    install_hooks, install_opencode_integration, install_reasonix_integration, preview_hook_config,
    remove_hooks, remove_opencode_integration, remove_reasonix_integration,
};
use ai_light::types::LightState;
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, Position, Size, State,
    WebviewWindow,
};

use crate::light_windows::LightWindowManager;

#[derive(Debug, Serialize)]
pub struct Diagnostics {
    pub config_dir: String,
    pub runtime_path: String,
    pub lock_path: String,
    pub log_path: String,
    pub claude_settings_path: String,
    pub hook_binary_path: String,
    pub codex_sessions_path: String,
    pub hooks_installed: bool,
    pub hook_binary_exists: bool,
    pub runtime_exists: bool,
    pub light_count: usize,
    pub recent_log: String,
    pub opencode_integration: bool,
    pub reasonix_integration: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfigView {
    pub config_path: String,
    pub http_bind: String,
    pub http_port: Option<u16>,
    pub runtime_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceView {
    pub light_width: u16,
    pub label_font_family: String,
    pub label_font_size: u16,
    pub label_color: String,
    pub label_font_weight: u16,
    pub panel_color: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceUpdate {
    pub light_width: u16,
    pub label_font_family: String,
    pub label_font_size: u16,
    pub label_color: String,
    pub label_font_weight: u16,
    pub panel_color: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfigUpdate {
    pub http_bind: String,
    pub http_port: Option<u16>,
}

#[tauri::command]
pub fn confirm_light(project_id: String, aggregator: State<Arc<StateAggregator>>) {
    aggregator.confirm_light(&project_id);
}

#[tauri::command]
pub fn remove_light(project_id: String, aggregator: State<Arc<StateAggregator>>) {
    aggregator.remove_light(&project_id);
}

#[tauri::command]
pub fn get_lights(aggregator: State<Arc<StateAggregator>>) -> Vec<LightState> {
    aggregator.get_lights()
}

#[tauri::command]
pub fn open_project(project_id: String) -> Result<(), String> {
    open_path(&project_id)
}

#[tauri::command]
pub fn open_codex(session_id: Option<String>) -> Result<(), String> {
    let session_id = session_id
        .filter(|session_id| !session_id.trim().is_empty())
        .ok_or_else(|| "No active Codex session is associated with this light.".to_string())?;
    open_codex_thread(&session_id)
}

#[tauri::command]
pub fn open_claude_session(
    project_path: String,
    session_id: Option<String>,
    origin: Option<String>,
    host_window: Option<i64>,
    terminal_tab_index: Option<usize>,
    terminal_tab_runtime_id: Option<Vec<i32>>,
) -> Result<(), String> {
    let project_path = project_path.trim();
    if project_path.is_empty() {
        return Err("No Claude project path is associated with this light.".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(host_window) = host_window {
            if focus_recorded_host(
                host_window,
                origin.as_deref().unwrap_or("unknown"),
                terminal_tab_index,
                terminal_tab_runtime_id.as_deref().unwrap_or_default(),
            ) {
                return Ok(());
            }
        }

        if origin.as_deref() == Some("vscode") {
            let mut code = std::process::Command::new("code.exe");
            code.args(["--reuse-window", project_path]);
            if code.spawn().is_ok() {
                return Ok(());
            }
        }

        let mut terminal = std::process::Command::new("wt.exe");
        terminal.args(["-w", "0", "-d", project_path, "claude"]);
        if let Some(session_id) = session_id.filter(|id| !id.trim().is_empty()) {
            terminal.arg("--resume").arg(session_id);
        }
        return terminal.spawn().map(|_| ()).map_err(|error| error.to_string());
    }

    open_path(project_path)
}

#[cfg(target_os = "windows")]
fn focus_recorded_host(
    host_window: i64,
    origin: &str,
    terminal_tab_index: Option<usize>,
    terminal_tab_runtime_id: &[i32],
) -> bool {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    if host_window <= 0 || (origin != "terminal" && origin != "vscode") {
        return false;
    }
    let runtime_id = terminal_tab_runtime_id
        .iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let tab_index = terminal_tab_index.map(|value| value.to_string()).unwrap_or_default();
    let script = format!(r#"
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class AiLightFocusNative {{
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr hWnd, int nCmdShow);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
}}
'@
$hwnd = [IntPtr]::new({host_window})
if (-not [AiLightFocusNative]::IsWindow($hwnd)) {{ '0'; exit }}
$ownerPid = [uint32]0
[AiLightFocusNative]::GetWindowThreadProcessId($hwnd, [ref]$ownerPid) | Out-Null
$process = Get-Process -Id $ownerPid -ErrorAction SilentlyContinue
$expected = '{origin}'
if (($expected -eq 'vscode' -and $process.ProcessName -notmatch '^Code') -or ($expected -eq 'terminal' -and $process.ProcessName -notmatch 'WindowsTerminal')) {{ '0'; exit }}
$selected = $expected -eq 'vscode'
if ($expected -eq 'terminal') {{
  try {{
    Add-Type -AssemblyName UIAutomationClient
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
    $condition = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::TabItem)
    $tabs = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition)
    $runtimeId = '{runtime_id}'
    $tabIndex = '{tab_index}'
    for ($i = 0; $i -lt $tabs.Count; $i++) {{
      $matchesRuntime = $runtimeId -ne '' -and (($tabs.Item($i).GetRuntimeId() -join ',') -eq $runtimeId)
      $matchesIndex = $runtimeId -eq '' -and $tabIndex -ne '' -and $i -eq [int]$tabIndex
      if ($matchesRuntime -or $matchesIndex) {{
        $pattern = $tabs.Item($i).GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern)
        $pattern.Select()
        $selected = $true
        break
      }}
    }}
  }} catch {{}}
}}
if (-not $selected) {{ '0'; exit }}
[AiLightFocusNative]::ShowWindowAsync($hwnd, 9) | Out-Null
[AiLightFocusNative]::SetForegroundWindow($hwnd) | Out-Null
'1'
"#);
    std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).trim().ends_with('1'))
}

#[tauri::command]
pub fn open_session_logs(project_id: String) -> Result<(), String> {
    let path = claude_project_log_dir(&project_id)?;
    open_path(&path.to_string_lossy())
}

#[tauri::command]
pub fn open_app_log() -> Result<(), String> {
    let log_path = get_log_path();
    if !log_path.exists() {
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(&log_path, "").map_err(|error| error.to_string())?;
    }

    open_path(&log_path.to_string_lossy())
}

#[tauri::command]
pub fn get_app_config() -> AppConfigView {
    let config = load_app_config();
    AppConfigView {
        config_path: get_config_path().to_string_lossy().to_string(),
        http_bind: config.http_bind,
        http_port: config.http_port,
        runtime_port: load_runtime_config().map(|runtime| runtime.http_port),
    }
}

#[tauri::command]
pub fn get_appearance() -> AppearanceView {
    appearance_view(&load_app_config())
}

#[tauri::command]
pub fn save_appearance(app: AppHandle, update: AppearanceUpdate) -> Result<AppearanceView, String> {
    if !(44..=100).contains(&update.light_width) {
        return Err("Light size must be between 44 and 100.".to_string());
    }
    if !(8..=24).contains(&update.label_font_size) {
        return Err("Label font size must be between 8 and 24.".to_string());
    }
    if !matches!(update.label_font_weight, 400 | 600 | 700) {
        return Err("Unsupported label font weight.".to_string());
    }
    if !valid_hex_color(&update.label_color) {
        return Err("Label color must use #RRGGBB format.".to_string());
    }
    if !valid_hex_color(&update.panel_color) {
        return Err("Panel color must use #RRGGBB format.".to_string());
    }
    let font_family = update.label_font_family.trim();
    if font_family.is_empty()
        || font_family.len() > 80
        || font_family
            .chars()
            .any(|character| character.is_control() || matches!(character, ';' | '{' | '}'))
    {
        return Err("Unsupported label font family.".to_string());
    }

    let mut config = load_app_config();
    config.light_width = update.light_width;
    config.label_font_family = font_family.to_string();
    config.label_font_size = update.label_font_size;
    config.label_color = update.label_color.to_ascii_lowercase();
    config.label_font_weight = update.label_font_weight;
    config.panel_color = update.panel_color.to_ascii_lowercase();
    save_app_config(&config).map_err(|error| error.to_string())?;

    let appearance = appearance_view(&config);
    app.emit("appearance-changed", appearance.clone())
        .map_err(|error| error.to_string())?;
    Ok(appearance)
}

#[tauri::command]
pub fn save_app_config_command(update: AppConfigUpdate) -> Result<(), String> {
    validate_http_bind(&update.http_bind)?;
    validate_http_port(update.http_port)?;

    let mut config = load_app_config();
    config.http_bind = update.http_bind;
    config.http_port = update.http_port;

    save_app_config(&config).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_diagnostics(aggregator: State<Arc<StateAggregator>>) -> Diagnostics {
    let log_path = get_log_path();
    let hook_binary_path = ai_light::hook_installer::get_hook_binary_path();
    Diagnostics {
        config_dir: get_config_dir().to_string_lossy().to_string(),
        runtime_path: get_runtime_path().to_string_lossy().to_string(),
        lock_path: get_lock_path().to_string_lossy().to_string(),
        log_path: log_path.to_string_lossy().to_string(),
        claude_settings_path: ai_light::hook_installer::get_claude_settings_path()
            .to_string_lossy()
            .to_string(),
        hook_binary_path: hook_binary_path.to_string_lossy().to_string(),
        codex_sessions_path: codex_sessions_dir().to_string_lossy().to_string(),
        hooks_installed: check_hooks_installed(),
        hook_binary_exists: hook_binary_path.exists(),
        runtime_exists: get_runtime_path().exists(),
        light_count: aggregator.get_lights().len(),
        recent_log: recent_log(&log_path),
        opencode_integration: check_opencode_integration().unwrap_or(false),
        reasonix_integration: check_reasonix_integration().unwrap_or(false),
    }
}

#[tauri::command]
pub fn copy_path(project_id: String) -> String {
    project_id
}

#[tauri::command]
pub fn pause_monitoring() {}

#[tauri::command]
pub fn resume_monitoring() {}

#[tauri::command]
pub fn open_settings(app: AppHandle) -> Result<(), String> {
    crate::tray::show_settings_window(&app)
}

#[tauri::command]
pub fn hide_main_window(app: AppHandle) -> Result<(), String> {
    crate::tray::hide_main_window(&app)
}

#[tauri::command]
pub fn hide_all_windows(
    app: AppHandle,
    manager: State<Arc<LightWindowManager>>,
) -> Result<(), String> {
    manager.hide_all(&app);
    Ok(())
}

#[tauri::command]
pub fn current_window_project(
    window: WebviewWindow,
    manager: State<Arc<LightWindowManager>>,
) -> Option<String> {
    manager.project_for_window(window.label())
}

#[tauri::command]
pub fn show_current_light(
    window: WebviewWindow,
    manager: State<Arc<LightWindowManager>>,
) -> Result<bool, String> {
    manager.show_when_ready(&window)
}

#[tauri::command]
pub fn detach_current_light(
    window: WebviewWindow,
    manager: State<Arc<LightWindowManager>>,
) -> bool {
    manager.detach(window.label())
}

#[tauri::command]
pub fn detach_current_light_with_nudge(
    app: AppHandle,
    window: WebviewWindow,
    manager: State<Arc<LightWindowManager>>,
) -> bool {
    manager.detach_with_nudge(&app, window.label())
}

#[tauri::command]
pub fn is_current_light_attached(
    window: WebviewWindow,
    manager: State<Arc<LightWindowManager>>,
) -> bool {
    manager.is_attached(window.label())
}

#[tauri::command]
pub fn resize_current_window(
    window: WebviewWindow,
    manager: State<Arc<LightWindowManager>>,
    width: f64,
    height: f64,
    keep_bottom: Option<bool>,
) -> Result<(), String> {
    let width = width.clamp(48.0, 360.0);
    let height = height.clamp(96.0, 600.0);
    if keep_bottom.unwrap_or(false) {
        let scale_factor = window.scale_factor().map_err(|error| error.to_string())?;
        let physical_width = (width * scale_factor).round() as i32;
        let physical_height = (height * scale_factor).round() as i32;
        if let Some((x, y)) = manager.prepare_bottom_anchored_resize(
            window.label(),
            physical_width,
            physical_height,
        ) {
            window
                .set_position(Position::Physical(PhysicalPosition::new(x, y)))
                .map_err(|error| error.to_string())?;
        }
    }
    window
        .set_size(Size::Logical(LogicalSize::new(width, height)))
        .map_err(|error| error.to_string())?;
    crate::window_state::ensure_window_visible(&window)
}

#[tauri::command]
pub fn set_current_window_always_on_top(
    window: WebviewWindow,
    always_on_top: bool,
) -> Result<bool, String> {
    window
        .set_always_on_top(always_on_top)
        .map_err(|error| error.to_string())?;
    Ok(always_on_top)
}

#[tauri::command]
pub fn resize_main_window(app: AppHandle, width: f64, height: f64) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is not available".to_string())?;

    let width = width.clamp(54.0, 1200.0);
    let height = height.clamp(64.0, 900.0);

    window
        .set_size(Size::Logical(LogicalSize::new(width, height)))
        .map_err(|error| error.to_string())?;

    crate::window_state::ensure_window_visible(&window)?;
    Ok(())
}

#[tauri::command]
pub fn set_main_window_always_on_top(app: AppHandle, always_on_top: bool) -> Result<bool, String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is not available".to_string())?;

    window
        .set_always_on_top(always_on_top)
        .map_err(|error| error.to_string())?;

    Ok(always_on_top)
}

fn validate_http_bind(bind: &str) -> Result<(), String> {
    bind.parse::<IpAddr>().map(|_| ()).map_err(|_| {
        "HTTP bind must be an IP address, for example 127.0.0.1 or 0.0.0.0".to_string()
    })
}

fn appearance_view(config: &AppConfig) -> AppearanceView {
    AppearanceView {
        light_width: config.light_width,
        label_font_family: config.label_font_family.clone(),
        label_font_size: config.label_font_size,
        label_color: config.label_color.clone(),
        label_font_weight: config.label_font_weight,
        panel_color: config.panel_color.clone(),
    }
}

fn valid_hex_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color[1..].chars().all(|character| character.is_ascii_hexdigit())
}

fn validate_http_port(port: Option<u16>) -> Result<(), String> {
    if matches!(port, Some(0)) {
        return Err("HTTP port must be blank or between 1 and 65535".to_string());
    }

    Ok(())
}

#[tauri::command]
pub fn check_hooks() -> bool {
    check_hooks_installed()
}

#[tauri::command]
pub fn install_hooks_command() -> Result<(), String> {
    install_hooks().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn remove_hooks_command() -> Result<(), String> {
    remove_hooks().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn preview_hook_config_command() -> Result<String, String> {
    preview_hook_config()
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

// --- opencode integration commands ---

#[tauri::command]
pub fn check_opencode() -> bool {
    check_opencode_integration().unwrap_or(false)
}

#[tauri::command]
pub fn install_opencode_command() -> Result<(), String> {
    install_opencode_integration().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn remove_opencode_command() -> Result<(), String> {
    remove_opencode_integration().map_err(|error| error.to_string())
}

// --- reasonix integration commands ---

#[tauri::command]
pub fn check_reasonix() -> bool {
    check_reasonix_integration().unwrap_or(false)
}

#[tauri::command]
pub fn install_reasonix_command() -> Result<(), String> {
    install_reasonix_integration().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn remove_reasonix_command() -> Result<(), String> {
    remove_reasonix_integration().map_err(|error| error.to_string())
}

fn open_path(path: &str) -> Result<(), String> {
    let mut command = platform_open_command(path)?;

    command.spawn().map_err(|error| error.to_string())?;
    Ok(())
}

fn open_codex_thread(session_id: &str) -> Result<(), String> {
    let session_id = session_id.trim();
    if !is_safe_codex_thread_id(session_id) {
        return Err("invalid Codex thread id".to_string());
    }

    open_url(&format!("codex://threads/{session_id}"))
}

fn open_codex_app() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .args(["-a", "Codex"])
            .spawn()
            .map_err(|error| error.to_string())?;
        return Ok(());
    }

    #[allow(unreachable_code)]
    open_url("codex://")
}

fn open_url(url: &str) -> Result<(), String> {
    let mut command = platform_open_command(url)?;
    command.spawn().map_err(|error| error.to_string())?;
    Ok(())
}

fn is_safe_codex_thread_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn claude_project_log_dir(project_id: &str) -> Result<PathBuf, String> {
    let home = home_dir().ok_or_else(|| "failed to resolve home directory".to_string())?;
    Ok(home
        .join(".claude")
        .join("projects")
        .join(encode_claude_project_dir(project_id)))
}

fn encode_claude_project_dir(project_id: &str) -> String {
    project_id
        .replace("\\\\?\\", "")
        .replace(':', "")
        .replace(['\\', '/'], "-")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn codex_sessions_dir() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex")
        .join("sessions")
}

fn recent_log(log_path: &PathBuf) -> String {
    let Ok(content) = fs::read_to_string(log_path) else {
        return String::new();
    };

    let lines: Vec<_> = content.lines().rev().take(20).collect();
    lines.into_iter().rev().collect::<Vec<_>>().join("\n")
}

fn platform_open_command(path: &str) -> Result<std::process::Command, String> {
    #[cfg(target_os = "windows")]
    {
        let mut command = std::process::Command::new("explorer");
        command.arg(path);
        return Ok(command);
    }

    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("open");
        command.arg(path);
        return Ok(command);
    }

    #[cfg(target_os = "linux")]
    {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(path);
        return Ok(command);
    }

    #[allow(unreachable_code)]
    Err("opening paths is not supported on this platform".to_string())
}
