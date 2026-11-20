//! 和 Claude 桌面版打交道：读它的会话列表、用 claude:// 链接切到某个会话、往它的输入框里打字。
//!
//! 桌面版每个 Code 会话都有一份元数据 local_<uuid>.json（标题、文件夹、最近活动时间、
//! 对应的 CLI session_id 等），放在 …\claude-code-sessions\<账号>\<组织>\ 下面。
//! `claude://code/continue?session=local_<uuid>` 会让桌面版跳到那个会话。

use crate::hook::wide;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Debug, PartialEq)]
pub struct DeskSession {
    /// 桌面版的会话 id：local_<uuid>
    pub id: String,
    /// 对应 hook 里的 session_id
    pub cli_id: String,
    pub title: String,
    /// 工作文件夹名（cwd 的最后一级）
    pub folder: String,
    pub last_activity: u64,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawSession {
    session_id: String,
    cli_session_id: Option<String>,
    title: Option<String>,
    cwd: Option<String>,
    last_activity_at: Option<u64>,
    is_archived: bool,
}

/// 桌面版 id 格式（和桌面版自己的校验一致）：local_ 后面 1~64 个字母、数字或连字符
pub fn valid_id(id: &str) -> bool {
    id.strip_prefix("local_")
        .map(|r| (1..=64).contains(&r.len()) && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .unwrap_or(false)
}

/// 可能存放会话元数据的目录：商店版、官网安装包版
fn session_roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        // 商店版：%LOCALAPPDATA%\Claude-Data\claude_<发布者>\claude-code-sessions
        if let Ok(rd) = std::fs::read_dir(local.join("Claude-Data")) {
            for e in rd.flatten() {
                v.push(e.path().join("claude-code-sessions"));
            }
        }
        if let Ok(rd) = std::fs::read_dir(local.join("Packages")) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with("Claude_") {
                    v.push(e.path().join("LocalCache").join("Roaming").join("Claude").join("claude-code-sessions"));
                }
            }
        }
    }
    if let Some(roaming) = std::env::var_os("APPDATA").map(PathBuf::from) {
        v.push(roaming.join("Claude").join("claude-code-sessions"));
    }
    v.retain(|p| p.is_dir());
    v
}

fn collect_files(dir: &Path, depth: u32, out: &mut Vec<(std::time::SystemTime, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            if depth > 0 {
                collect_files(&p, depth - 1, out);
            }
        } else {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("local_") && name.ends_with(".json") {
                let mtime = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                out.push((mtime, p));
            }
        }
    }
}

fn parse(text: &str) -> Option<DeskSession> {
    let r: RawSession = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    if r.is_archived || !valid_id(&r.session_id) {
        return None;
    }
    let folder = r
        .cwd
        .as_deref()
        .map(|c| c.trim_end_matches(['\\', '/']))
        .and_then(|c| c.rsplit(['\\', '/']).next())
        .unwrap_or("")
        .to_string();
    Some(DeskSession {
        id: r.session_id,
        cli_id: r.cli_session_id.unwrap_or_default(),
        title: r.title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| "（未命名会话）".into()),
        folder,
        last_activity: r.last_activity_at.unwrap_or(0),
    })
}

/// 从指定目录读会话：先按文件修改时间挑最近的一批再解析，按最近活动时间排序
pub fn list_sessions_in(roots: &[PathBuf], limit: usize) -> Vec<DeskSession> {
    let mut files = Vec::new();
    for r in roots {
        collect_files(r, 3, &mut files);
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let mut v: Vec<DeskSession> = files
        .iter()
        .take(limit * 3 + 10)
        .filter_map(|(_, p)| std::fs::read_to_string(p).ok())
        .filter_map(|t| parse(&t))
        .collect();
    v.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
    v.dedup_by(|a, b| a.id == b.id);
    v.truncate(limit);
    v
}

/// 桌面版里最近的若干个未归档 Code 会话，最近活动的在前
pub fn list_sessions(limit: usize) -> Vec<DeskSession> {
    list_sessions_in(&session_roots(), limit)
}

pub fn shell_open(target: &str) -> bool {
    let (op, f) = (wide("open"), wide(target));
    let r = unsafe {
        ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), f.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL)
    };
    r as isize > 32
}

/// 让桌面版切到这个会话（id 不合法就切到最近的会话）。允许它把窗口抢到前台。
pub fn open_session(id: &str) -> bool {
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    let sid = if valid_id(id) { id } else { "last" };
    shell_open(&format!("claude://code/continue?session={sid}"))
}

fn exe_of_pid(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// 这个 exe 路径是不是 Claude 桌面版本体（不是 Claude Code CLI，也不是桌宠自己）
fn is_desktop_exe(path: &str) -> bool {
    let p = path.to_lowercase().replace('/', "\\");
    p.rsplit('\\').next() == Some("claude.exe") && !p.contains("\\claude-code\\")
}

fn window_is_claude(hwnd: HWND) -> bool {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid != 0 && exe_of_pid(pid).map(|p| is_desktop_exe(&p)).unwrap_or(false)
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lp: LPARAM) -> BOOL {
    let out = &mut *(lp as *mut HWND);
    if IsWindowVisible(hwnd) != 0
        && GetWindow(hwnd, GW_OWNER).is_null()
        && GetWindowTextLengthW(hwnd) > 0
        && window_is_claude(hwnd)
    {
        *out = hwnd;
        return 0;
    }
    1
}

/// Claude 桌面版的主窗口
pub fn claude_window() -> Option<HWND> {
    let mut found: HWND = std::ptr::null_mut();
    unsafe { EnumWindows(Some(enum_proc), &mut found as *mut HWND as LPARAM) };
    (!found.is_null()).then_some(found)
}

pub fn foreground_is_claude() -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        !fg.is_null() && window_is_claude(GetAncestor(fg, GA_ROOTOWNER))
    }
}

/// 把 Claude 窗口带到前台（最小化了就先还原）
pub fn bring_claude_front(hwnd: HWND) {
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd);
    }
}

fn key(vk: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn unit(c: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: c,
                dwFlags: KEYEVENTF_UNICODE | if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// 往当前有焦点的输入框里打字：换行用 Shift+Enter，最后按需按 Enter 发送。不经过剪贴板。
pub fn type_text(text: &str, press_enter: bool) -> bool {
    let mut v: Vec<INPUT> = Vec::with_capacity(text.len() * 2 + 8);
    // 先松开可能还按着的修饰键，免得和字符拼成快捷键
    for vk in [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN] {
        if unsafe { GetKeyState(vk as i32) } < 0 {
            v.push(key(vk, true));
        }
    }
    for c in text.replace("\r\n", "\n").encode_utf16() {
        if c == '\n' as u16 {
            v.extend([key(VK_SHIFT, false), key(VK_RETURN, false), key(VK_RETURN, true), key(VK_SHIFT, true)]);
        } else if c != '\r' as u16 {
            v.extend([unit(c, false), unit(c, true)]);
        }
    }
    if press_enter {
        v.extend([key(VK_RETURN, false), key(VK_RETURN, true)]);
    }
    let n = unsafe { SendInput(v.len() as u32, v.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    n as usize == v.len()
}

/// 兜底：把文字放进剪贴板
pub fn copy_to_clipboard(owner: HWND, text: &str) -> bool {
    let data: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(owner) == 0 {
            return false;
        }
        EmptyClipboard();
        let bytes = data.len() * 2;
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
        let mut ok = false;
        if !h.is_null() {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
                GlobalUnlock(h);
                ok = !SetClipboardData(CF_UNICODETEXT as u32, h).is_null();
            }
            if !ok {
                GlobalFree(h);
            }
        }
        CloseClipboard();
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_validation() {
        assert!(valid_id("local_9ca2bd48-f513-47ba-b86f-ae1d04d423c0"));
        assert!(!valid_id("local_"));
        assert!(!valid_id("9ca2bd48"));
        assert!(!valid_id("local_abc&x=1"));
        assert!(!valid_id(&format!("local_{}", "a".repeat(65))));
    }

    #[test]
    fn desktop_exe_detection() {
        assert!(is_desktop_exe(r"C:\Program Files\WindowsApps\Claude_5.1_x64__pzs8sxrjxfjjc\app\Claude.exe"));
        assert!(is_desktop_exe(r"C:\Users\a\AppData\Local\AnthropicClaude\app-1.0\claude.exe"));
        assert!(!is_desktop_exe(r"C:\Users\a\AppData\Local\Claude-Data\claude_x\claude-code\2.1\abc\claude.exe"));
        assert!(!is_desktop_exe(r"D:\pet\claude-pet.exe"));
    }

    #[test]
    fn list_sorts_and_filters() {
        let dir = std::env::temp_dir().join(format!("claude-pet-test-{}", std::process::id()));
        let sub = dir.join("acct").join("org");
        std::fs::create_dir_all(&sub).unwrap();
        let w = |name: &str, body: &str| std::fs::write(sub.join(name), body).unwrap();
        w("local_a.json", r#"{"sessionId":"local_a","cliSessionId":"c-a","title":"旧的","cwd":"D:\\work\\proj-a","lastActivityAt":100,"isArchived":false}"#);
        w("local_b.json", r#"{"sessionId":"local_b","cliSessionId":"c-b","title":"新的","cwd":"D:\\work\\proj-b\\","lastActivityAt":300,"extra":{"x":1}}"#);
        w("local_c.json", r#"{"sessionId":"local_c","title":"归档了","lastActivityAt":500,"isArchived":true}"#);
        w("local_d.json", r#"{"sessionId":"local_d","title":"","lastActivityAt":200}"#);
        w("local_bad.json", "not json");
        w("other.json", r#"{"sessionId":"local_z","lastActivityAt":999}"#);
        let v = list_sessions_in(&[dir.clone()], 10);
        let _ = std::fs::remove_dir_all(&dir);
        let ids: Vec<&str> = v.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["local_b", "local_d", "local_a"]);
        assert_eq!(v[0].folder, "proj-b");
        assert_eq!(v[0].cli_id, "c-b");
        assert_eq!(v[1].title, "（未命名会话）");
    }
}
