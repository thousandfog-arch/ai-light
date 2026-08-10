#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(target_os = "windows")]
use std::ffi::c_void;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(target_os = "windows")]
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn GetForegroundWindow() -> *mut c_void;
    fn GetWindowThreadProcessId(window: *mut c_void, process_id: *mut u32) -> u32;
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> *mut c_void;
    fn QueryFullProcessImageNameW(
        process: *mut c_void,
        flags: u32,
        file_name: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

#[derive(Debug, Deserialize)]
struct RuntimeConfig {
    http_port: u16,
}

#[derive(Debug, Serialize)]
struct HookEvent {
    event_type: String,
    session_id: String,
    cwd: Option<String>,
    tool_call: Option<String>,
    tool_source: String,
    origin: String,
    host_window: Option<i64>,
    terminal_tab_index: Option<usize>,
    terminal_tab_runtime_id: Vec<i32>,
}

#[derive(Debug, Default)]
struct HostContext {
    origin: String,
    host_window: Option<i64>,
    terminal_tab_index: Option<usize>,
    terminal_tab_runtime_id: Vec<i32>,
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut source = "claude-code".to_string();
    let mut event_type_arg: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--source" | "--tool-source" => {
                i += 1;
                if i < args.len() {
                    source = args[i].clone();
                }
            }
            "--help" => {
                eprintln!("Usage: ai-light-hook [--source <tool>] <event_type>");
                eprintln!("");
                eprintln!("Tools: claude-code (default), opencode, reasonix");
                return;
            }
            _ => {
                event_type_arg = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let Some(raw_event_type) = event_type_arg else {
        append_log("ignored: missing event type argument");
        return;
    };
    let event_type = normalize_event_type(&raw_event_type);

    let payload = match read_stdin_payload() {
        Ok(payload) => payload,
        Err(error) => {
            append_log(format!("ignored: invalid stdin payload: {error}"));
            return;
        }
    };

    let Some((target_url, target_source)) = resolve_event_url() else {
        append_log(format!(
            "ignored: no target url for event={event_type}; runtime_path={}",
            runtime_config_path().display()
        ));
        return;
    };

    let host = detect_host_context();
    let event = HookEvent {
        event_type,
        session_id: extract_string(&payload, &["session_id", "sessionId"])
            .unwrap_or_else(|| "unknown".to_string()),
        cwd: extract_string(&payload, &["cwd"]).or_else(|| {
            env::current_dir()
                .ok()
                .map(|path| path.to_string_lossy().to_string())
        }),
        tool_call: extract_string(&payload, &["tool_name", "tool", "toolName"]),
        tool_source: source,
        origin: host.origin,
        host_window: host.host_window,
        terminal_tab_index: host.terminal_tab_index,
        terminal_tab_runtime_id: host.terminal_tab_runtime_id,
    };

    match post_event(&target_url, &event) {
        Ok(status) => append_log(format!(
            "sent: event={} session={} target={} source={} status={}",
            event.event_type, event.session_id, target_url, target_source, status
        )),
        Err(error) => append_log(format!(
            "failed: event={} session={} target={} source={} error={}",
            event.event_type, event.session_id, target_url, target_source, error
        )),
    }
}

fn detect_host_context() -> HostContext {
    #[cfg(target_os = "windows")]
    {
        if let Some((host_window, process_name)) = foreground_process() {
            if process_name.starts_with("code") {
                return HostContext {
                    origin: "vscode".to_string(),
                    host_window: Some(host_window),
                    ..HostContext::default()
                };
            }

            if process_name.contains("windowsterminal") {
                return detect_terminal_context(host_window);
            }
        }
    }

    let origin = if env::var_os("VSCODE_PID").is_some()
        || env::var_os("VSCODE_IPC_HOOK_CLI").is_some()
        || env::var("TERM_PROGRAM").map(|value| value.eq_ignore_ascii_case("vscode")).unwrap_or(false)
    { "vscode" } else { "terminal" };
    HostContext { origin: origin.to_string(), ..HostContext::default() }
}

#[cfg(target_os = "windows")]
fn foreground_process() -> Option<(i64, String)> {
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return None;
    }

    let mut process_id = 0u32;
    unsafe { GetWindowThreadProcessId(window, &mut process_id) };
    if process_id == 0 {
        return None;
    }

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return None;
    }

    let mut path = vec![0u16; 32_768];
    let mut path_len = path.len() as u32;
    let query_succeeded = unsafe {
        QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut path_len) != 0
    };
    unsafe { CloseHandle(process) };
    if !query_succeeded {
        return None;
    }

    let executable = String::from_utf16_lossy(&path[..path_len as usize]);
    let process_name = executable
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(&executable)
        .trim_end_matches(".exe")
        .to_ascii_lowercase();
    Some((window as isize as i64, process_name))
}

#[cfg(target_os = "windows")]
fn detect_terminal_context(host_window: i64) -> HostContext {
    let script = r#"
Add-Type -AssemblyName UIAutomationClient
$hwnd = [IntPtr]::new(__HOST_WINDOW__)
$root = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
$tabIndex = ''
$runtimeId = ''
try {
    $condition = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::TabItem)
    $tabs = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition)
    for ($i = 0; $i -lt $tabs.Count; $i++) {
      $pattern = $tabs.Item($i).GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern)
      if ($pattern.Current.IsSelected) {
        $tabIndex = [string]$i
        $runtimeId = ($tabs.Item($i).GetRuntimeId() -join ',')
        break
      }
    }
} catch {}
'{0}|{1}' -f $tabIndex, $runtimeId
"#
    .replace("__HOST_WINDOW__", &host_window.to_string());
    if let Ok(output) = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout);
            let mut parts = value.trim().splitn(2, '|');
            let terminal_tab_index = parts.next().and_then(|value| value.parse().ok());
            let terminal_tab_runtime_id = parts
                .next()
                .unwrap_or_default()
                .split(',')
                .filter_map(|value| value.parse().ok())
                .collect();
            return HostContext {
                origin: "terminal".to_string(),
                host_window: Some(host_window),
                terminal_tab_index,
                terminal_tab_runtime_id,
            };
        }
    }
    HostContext {
        origin: "terminal".to_string(),
        host_window: Some(host_window),
        ..HostContext::default()
    }
}

fn read_stdin_payload() -> Result<serde_json::Value, String> {
    let mut stdin_content = String::new();
    io::stdin()
        .read_to_string(&mut stdin_content)
        .map_err(|error| error.to_string())?;

    if stdin_content.trim().is_empty() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }

    serde_json::from_str(&stdin_content).map_err(|error| error.to_string())
}

fn resolve_event_url() -> Option<(String, &'static str)> {
    if let Some(url) = env::var_os("AI_LIGHT_URL").and_then(|value| {
        let value = value.to_string_lossy().trim().to_string();
        (!value.is_empty()).then_some(value)
    }) {
        return Some((normalize_event_url(&url), "AI_LIGHT_URL"));
    }

    let config = load_runtime_config()?;
    Some((
        format!("http://127.0.0.1:{}/events", config.http_port),
        "runtime.json",
    ))
}

fn load_runtime_config() -> Option<RuntimeConfig> {
    let content = fs::read_to_string(runtime_config_path()).ok()?;
    serde_json::from_str(&content).ok()
}

fn runtime_config_path() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ai_light")
        .join("runtime.json")
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
}

fn extract_string(payload: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        payload
            .get(key)
            .and_then(|value| value.as_str())
            .map(ToString::to_string)
    })
}

fn normalize_event_type(event_type: &str) -> String {
    match event_type {
        "SessionStart" | "session_start" | "sessionstart" => "session-start",
        "UserPromptSubmit" | "prompt_submit" | "user-prompt-submit" | "userpromptsubmit" => {
            "prompt-submit"
        }
        "PreToolUse" | "pre_tool_use" | "pre-tool-use" | "pretooluse" => "pre-tool-use",
        "PermissionRequest" | "permission_request" | "permission-request" | "permissionrequest" => {
            "permission-request"
        }
        "PostToolUse" | "post_tool_use" | "post-tool-use" | "posttooluse" => "post-tool-use",
        "Notification" | "notification" => "notification",
        "Stop" | "stop" => "stop",
        "SessionEnd" | "session_end" | "sessionend" => "session-end",
        // opencode event compatibility
        "session_created" | "session.created" | "SessionCreated" => "session-start",
        "session_deleted" | "session.deleted" | "SessionDeleted" => "session-end",
        "session_idle" | "session.idle" | "SessionIdle" => "stop",
        "tool_execute_before" | "tool.execute.before" | "tool.execute_before" => "pre-tool-use",
        "tool_execute_after" | "tool.execute.after" | "tool.execute_after" => "post-tool-use",
        "message_updated" | "message.updated" | "MessageUpdated" => "prompt-submit",
        "tui_toast_show" | "tui.toast.show" | "TuiToastShow" => "notification",
        "permission_asked" | "permission.asked" | "PermissionAsked" => "permission-request",
        // reasonix event compatibility
        "TurnStart" | "turn_start" | "turnstart" | "turn.start" => "pre-tool-use",
        "TurnEnd" | "turn_end" | "turnend" | "turn.end" => "stop",
        "PreModelCall" | "pre_model_call" | "premodelcall" => "pre-tool-use",
        "PostModelCall" | "post_model_call" | "postmodelcall" => "post-tool-use",
        other => other,
    }
    .to_string()
}

fn post_event(url: &str, event: &HookEvent) -> Result<u16, String> {
    let client = reqwest::blocking::Client::new();

    let response = client
        .post(url)
        .json(event)
        .send()
        .map_err(|error| error.to_string())?;

    Ok(response.status().as_u16())
}

fn normalize_event_url(url: &str) -> String {
    if url.ends_with("/events") {
        url.to_string()
    } else {
        format!("{}/events", url.trim_end_matches('/'))
    }
}

fn append_log(message: impl AsRef<str>) {
    let Some(home) = home_dir() else {
        return;
    };

    let log_dir = home.join(".ai_light");
    if fs::create_dir_all(&log_dir).is_err() {
        return;
    }

    let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("hook.log"))
    else {
        return;
    };

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let _ = writeln!(file, "[{timestamp}] {}", message.as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_claude_hook_names() {
        assert_eq!(normalize_event_type("SessionStart"), "session-start");
        assert_eq!(normalize_event_type("UserPromptSubmit"), "prompt-submit");
        assert_eq!(normalize_event_type("PreToolUse"), "pre-tool-use");
        assert_eq!(
            normalize_event_type("PermissionRequest"),
            "permission-request"
        );
        assert_eq!(normalize_event_type("PostToolUse"), "post-tool-use");
        assert_eq!(normalize_event_type("SessionEnd"), "session-end");
    }

    #[test]
    fn normalizes_opencode_event_names() {
        assert_eq!(normalize_event_type("session.created"), "session-start");
        assert_eq!(normalize_event_type("session.deleted"), "session-end");
        assert_eq!(normalize_event_type("session.idle"), "stop");
        assert_eq!(normalize_event_type("tool.execute.before"), "pre-tool-use");
        assert_eq!(normalize_event_type("tool.execute.after"), "post-tool-use");
        assert_eq!(normalize_event_type("message.updated"), "prompt-submit");
        assert_eq!(normalize_event_type("tui.toast.show"), "notification");
        assert_eq!(normalize_event_type("permission.asked"), "permission-request");
    }

    #[test]
    fn normalizes_reasonix_event_names() {
        assert_eq!(normalize_event_type("TurnStart"), "pre-tool-use");
        assert_eq!(normalize_event_type("TurnEnd"), "stop");
        assert_eq!(normalize_event_type("PreModelCall"), "pre-tool-use");
        assert_eq!(normalize_event_type("PostModelCall"), "post-tool-use");
    }

    #[test]
    fn extracts_first_present_string_key() {
        let payload = serde_json::json!({
            "sessionId": "abc123",
            "cwd": "N:/AI/ai_light"
        });

        assert_eq!(
            extract_string(&payload, &["session_id", "sessionId"]),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn prefers_explicit_event_url_environment_variable() {
        let previous = env::var_os("AI_LIGHT_URL");
        env::set_var("AI_LIGHT_URL", "http://127.0.0.1:32123");

        assert_eq!(
            resolve_event_url(),
            Some(("http://127.0.0.1:32123/events".to_string(), "AI_LIGHT_URL"))
        );

        match previous {
            Some(value) => env::set_var("AI_LIGHT_URL", value),
            None => env::remove_var("AI_LIGHT_URL"),
        }
    }
}
