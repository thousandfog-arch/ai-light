#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "windows")]
use std::env;
#[cfg(target_os = "windows")]
use std::ffi::c_void;
#[cfg(all(test, target_os = "windows"))]
use std::ffi::OsStr;
#[cfg(target_os = "windows")]
use std::mem::{size_of, zeroed};
#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;
#[cfg(target_os = "windows")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(target_os = "windows")]
const INFINITE: u32 = 0xffff_ffff;
#[cfg(target_os = "windows")]
const STARTF_USESTDHANDLES: u32 = 0x0000_0100;
#[cfg(target_os = "windows")]
const STD_INPUT_HANDLE: u32 = -10i32 as u32;
#[cfg(target_os = "windows")]
const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
#[cfg(target_os = "windows")]
const STD_ERROR_HANDLE: u32 = -12i32 as u32;

#[cfg(target_os = "windows")]
const AI_LIGHT_HOOK_EVENTS: [&str; 8] = [
    "session-start",
    "prompt-submit",
    "pre-tool-use",
    "permission-request",
    "post-tool-use",
    "notification",
    "stop",
    "session-end",
];

#[cfg(target_os = "windows")]
type Handle = *mut c_void;

#[cfg(target_os = "windows")]
#[repr(C)]
struct StartupInfoW {
    cb: u32,
    lp_reserved: *mut u16,
    lp_desktop: *mut u16,
    lp_title: *mut u16,
    dw_x: u32,
    dw_y: u32,
    dw_x_size: u32,
    dw_y_size: u32,
    dw_x_count_chars: u32,
    dw_y_count_chars: u32,
    dw_fill_attribute: u32,
    dw_flags: u32,
    w_show_window: u16,
    cb_reserved2: u16,
    lp_reserved2: *mut u8,
    h_std_input: Handle,
    h_std_output: Handle,
    h_std_error: Handle,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    process_id: u32,
    thread_id: u32,
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
extern "system" {
    fn GetCommandLineW() -> *mut u16;
    fn GetStdHandle(std_handle: u32) -> Handle;
    fn CreateProcessW(
        application_name: *const u16,
        command_line: *mut u16,
        process_attributes: *const c_void,
        thread_attributes: *const c_void,
        inherit_handles: i32,
        creation_flags: u32,
        environment: *const c_void,
        current_directory: *const u16,
        startup_info: *const StartupInfoW,
        process_information: *mut ProcessInformation,
    ) -> i32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn GetExitCodeProcess(process: Handle, exit_code: *mut u32) -> i32;
    fn CloseHandle(object: Handle) -> i32;
}

fn main() {
    std::process::exit(run());
}

#[cfg(target_os = "windows")]
fn run() -> i32 {
    let command_line = current_command_line();
    let raw_tail = raw_args_tail(&command_line);
    if let Some(event) = ai_light_hook_event(raw_tail) {
        if let Some(exit_code) = run_ai_light_hook(event) {
            return exit_code;
        }
    }

    run_cmd_tail(raw_tail)
}

#[cfg(not(target_os = "windows"))]
fn run() -> i32 {
    1
}

#[cfg(target_os = "windows")]
fn run_cmd_tail(raw_tail: &[u16]) -> i32 {
    let cmd_path = system_cmd_path();
    env::set_var("ComSpec", &cmd_path);

    run_process(&cmd_path, raw_tail, CREATE_NO_WINDOW)
}

#[cfg(target_os = "windows")]
fn run_ai_light_hook(event: &str) -> Option<i32> {
    let hook_path = env::current_exe()
        .ok()?
        .parent()?
        .join("ai-light-hook.exe");
    if !hook_path.exists() {
        return None;
    }

    let mut raw_tail = vec![b' ' as u16];
    raw_tail.extend(event.encode_utf16());
    Some(run_process(&hook_path, &raw_tail, CREATE_NO_WINDOW))
}

#[cfg(target_os = "windows")]
fn run_process(application_path: &Path, raw_tail: &[u16], creation_flags: u32) -> i32 {
    let mut application: Vec<u16> = application_path.as_os_str().encode_wide().collect();
    application.push(0);

    let mut command_line = Vec::with_capacity(application.len() + raw_tail.len() + 2);
    command_line.push(b'"' as u16);
    command_line.extend_from_slice(&application[..application.len() - 1]);
    command_line.push(b'"' as u16);
    command_line.extend_from_slice(raw_tail);
    command_line.push(0);

    let mut startup: StartupInfoW = unsafe { zeroed() };
    startup.cb = size_of::<StartupInfoW>() as u32;
    startup.dw_flags = STARTF_USESTDHANDLES;
    startup.h_std_input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    startup.h_std_output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    startup.h_std_error = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    let mut process: ProcessInformation = unsafe { zeroed() };
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            creation_flags,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        )
    };
    if created == 0 {
        return 1;
    }

    unsafe { WaitForSingleObject(process.process, INFINITE) };
    let mut exit_code = 1u32;
    let got_exit_code = unsafe { GetExitCodeProcess(process.process, &mut exit_code) };
    unsafe {
        CloseHandle(process.thread);
        CloseHandle(process.process);
    }

    if got_exit_code == 0 {
        1
    } else {
        exit_code as i32
    }
}

#[cfg(target_os = "windows")]
fn ai_light_hook_event(raw_tail: &[u16]) -> Option<&'static str> {
    let command = String::from_utf16_lossy(raw_tail).to_ascii_lowercase();
    if !command.contains("--ai-light-direct") {
        return None;
    }

    AI_LIGHT_HOOK_EVENTS.iter().copied().find(|event| {
        command
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '-')
            .any(|token| token == *event)
    })
}

#[cfg(target_os = "windows")]
fn current_command_line() -> Vec<u16> {
    let pointer = unsafe { GetCommandLineW() };
    if pointer.is_null() {
        return Vec::new();
    }

    let mut length = 0usize;
    unsafe {
        while *pointer.add(length) != 0 {
            length += 1;
        }
        std::slice::from_raw_parts(pointer, length).to_vec()
    }
}

#[cfg(target_os = "windows")]
fn raw_args_tail(command_line: &[u16]) -> &[u16] {
    let mut index = command_line
        .iter()
        .position(|character| !is_whitespace(*character))
        .unwrap_or(command_line.len());

    if command_line.get(index) == Some(&(b'"' as u16)) {
        index += 1;
        while index < command_line.len() && command_line[index] != b'"' as u16 {
            index += 1;
        }
        if index < command_line.len() {
            index += 1;
        }
    } else {
        while index < command_line.len() && !is_whitespace(command_line[index]) {
            index += 1;
        }
    }

    &command_line[index..]
}

#[cfg(target_os = "windows")]
fn is_whitespace(character: u16) -> bool {
    character == b' ' as u16 || character == b'\t' as u16
}

#[cfg(target_os = "windows")]
fn system_cmd_path() -> PathBuf {
    env::var_os("SystemRoot")
        .or_else(|| env::var_os("WINDIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32")
        .join("cmd.exe")
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value).encode_wide().collect()
    }

    #[test]
    fn keeps_the_raw_arguments_after_a_quoted_executable() {
        let command_line = wide(
            r#""C:\Users\Test User\ai-light-cmd-proxy.exe" /d /s /c "echo a b""#,
        );

        assert_eq!(
            String::from_utf16(raw_args_tail(&command_line)).unwrap(),
            r#" /d /s /c "echo a b""#
        );
    }

    #[test]
    fn forwards_the_real_cmd_exit_code() {
        assert_eq!(run_cmd_tail(&wide(r#" /d /s /c "exit /b 37""#)), 37);
    }

    #[test]
    fn detects_an_ai_light_hook_without_parsing_stdin() {
        let command = wide(
            r#" /d /s /c "C:\Users\kemp\.ai_light\bin\ai-light-hook.exe --ai-light-direct pre-tool-use""#,
        );

        assert_eq!(ai_light_hook_event(&command), Some("pre-tool-use"));
    }

    #[test]
    fn leaves_other_command_hooks_on_the_real_cmd_path() {
        let command = wide(r#" /d /s /c "C:\Tools\other-hook.exe stop""#);

        assert_eq!(ai_light_hook_event(&command), None);
    }

    #[test]
    fn does_not_intercept_a_legacy_ai_light_command_without_the_marker() {
        let command = wide(
            r#" /d /s /c "C:\Users\kemp\.ai_light\bin\ai-light-hook.exe pre-tool-use""#,
        );

        assert_eq!(ai_light_hook_event(&command), None);
    }

    #[test]
    fn preserves_quoted_command_arguments() {
        let command = wide(
            r#" /d /s /c "if "a b"=="a b" (exit /b 0) else (exit /b 1)""#,
        );

        assert_eq!(run_cmd_tail(&command), 0);
    }
}
