//! `claude-pet.exe hook`：Claude Code 每个 hook 事件都会启动一次这个模式。
//! 读 stdin 的 JSON，只挑几个字段，用 UDP 发给正在运行的桌宠，然后立刻退出（总是 exit 0，
//! 绝不阻塞或影响 Claude Code 本身）。

use crate::config;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Read;
use std::net::UdpSocket;
use std::time::Duration;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Console::*;
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
use windows_sys::Win32::System::Threading::*;

pub const MUTEX_NAME: &str = "Local\\ClaudePetNativeSingleton";

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn app_running() -> bool {
    unsafe {
        let name = wide(MUTEX_NAME);
        let h = OpenMutexW(0x0010_0000 /* SYNCHRONIZE */, 0, name.as_ptr());
        if h.is_null() {
            false
        } else {
            CloseHandle(h);
            true
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

pub fn run() {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let v: Value = serde_json::from_str(input.trim_start_matches('\u{feff}')).unwrap_or(Value::Null);
    let event = v["hook_event_name"].as_str().unwrap_or("").to_string();
    if event.is_empty() {
        return;
    }
    let cfg = config::load();
    let msg = json!({
        "e": event,
        "sid": v["session_id"].as_str(),
        "tool": v["tool_name"].as_str(),
        "nt": v["notification_type"].as_str(),
        "msg": v["message"].as_str().map(|s| truncate(s, 160)),
        "pid": agent_pid(),
    });

    if !app_running() {
        // 会话里任何事件都能把桌宠拉起来（桌宠中途被关掉也能自己回来），会话结束时就不必了
        if !cfg.auto_launch || cfg.quit_by_user || event == "SessionEnd" {
            return;
        }
        launch_app();
        // 等桌宠把 UDP 端口开起来（最多 ~1.5 秒，只在桌宠没开时发生一次）
        for _ in 0..15 {
            std::thread::sleep(Duration::from_millis(100));
            if app_running() {
                std::thread::sleep(Duration::from_millis(150));
                break;
            }
        }
    }
    if let Ok(sock) = UdpSocket::bind("127.0.0.1:0") {
        let _ = sock.send_to(msg.to_string().as_bytes(), ("127.0.0.1", cfg.port));
    }
}

/// 启动桌宠本体。先把自己的标准句柄设为不可继承，否则子进程会继承 Claude Code 给 hook 的
/// stdout 管道，Claude Code 会一直等它关闭（= 卡住）。
fn launch_app() {
    unsafe {
        for std_h in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let h = GetStdHandle(std_h);
            if !h.is_null() && h != INVALID_HANDLE_VALUE {
                SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
    use std::os::windows::process::CommandExt;
    const DETACHED: u32 = 0x0000_0008 /* DETACHED_PROCESS */ | 0x0000_0200 /* CREATE_NEW_PROCESS_GROUP */;
    const BREAKAWAY: u32 = 0x0100_0000; // CREATE_BREAKAWAY_FROM_JOB：别跟着 Claude Code 的作业对象一起被关掉
    let spawn = |flags: u32| {
        std::process::Command::new(config::exe_path())
            .current_dir(config::base_dir())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    // 作业对象不允许脱离时 BREAKAWAY 会失败，退回普通方式
    if spawn(DETACHED | BREAKAWAY).is_err() {
        let _ = spawn(DETACHED);
    }
}

/// 往上找启动这个 hook 的 Claude Code 进程（claude.exe）。找不到就用最近的一个非 shell 祖先。
/// 桌宠用它判断会话是否还活着。
fn agent_pid() -> Option<u32> {
    let mut table: HashMap<u32, (u32, String)> = HashMap::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut pe: PROCESSENTRY32W = std::mem::zeroed();
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut pe) != 0 {
            loop {
                let len = pe.szExeFile.iter().position(|&c| c == 0).unwrap_or(pe.szExeFile.len());
                let name = String::from_utf16_lossy(&pe.szExeFile[..len]).to_lowercase();
                table.insert(pe.th32ProcessID, (pe.th32ParentProcessID, name));
                if Process32NextW(snap, &mut pe) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    const SHELLS: [&str; 8] =
        ["bash.exe", "sh.exe", "cmd.exe", "powershell.exe", "pwsh.exe", "conhost.exe", "env.exe", "claude-pet.exe"];
    let me = std::process::id();
    let mut cur = table.get(&me)?.0;
    let mut fallback = None;
    for _ in 0..12 {
        let (parent, name) = table.get(&cur)?;
        if name.starts_with("claude") && name != "claude-pet.exe" {
            return Some(cur);
        }
        if fallback.is_none() && !SHELLS.contains(&name.as_str()) {
            fallback = Some(cur);
        }
        if *parent == 0 || *parent == cur {
            break;
        }
        cur = *parent;
    }
    fallback
}

pub fn process_alive(pid: u32) -> bool {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            // 拿不到句柄：进程不存在（或权限不足——那就当它还活着，宁可不清）
            return GetLastError() != ERROR_INVALID_PARAMETER;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code);
        CloseHandle(h);
        ok == 0 || code == STILL_ACTIVE as u32
    }
}
