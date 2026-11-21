//! 把 hooks 写进 / 移出 ~/.claude/settings.json。只动自己那几条，其余配置原样保留，
//! 每次修改前都会备份一份 settings.json.claude-pet-<时间戳>.bak。

use crate::config;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

/// (事件名, 是否需要 matcher)
const EVENTS: [(&str, bool); 12] = [
    ("SessionStart", false),
    ("SessionEnd", false),
    ("UserPromptSubmit", false),
    ("PreToolUse", true),
    ("PostToolUse", true),
    ("PostToolUseFailure", true),
    ("PermissionRequest", true),
    ("Notification", false),
    ("Stop", false),
    ("StopFailure", false),
    ("SubagentStart", false),
    ("PreCompact", false),
];

fn settings_path() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".claude").join("settings.json")
}

fn our_command() -> String {
    // 正斜杠 + 引号：Git Bash 和 cmd 都认，避免反斜杠被 bash 吃掉
    format!("\"{}\" hook", config::exe_path().display().to_string().replace('\\', "/"))
}

fn is_ours(cmd: &str) -> bool {
    let c = cmd.to_lowercase().replace('\\', "/");
    c.contains("claude-pet.exe\" hook") || c.trim_end().ends_with("claude-pet.exe hook")
}

fn read() -> Result<Map<String, Value>, String> {
    let path = settings_path();
    match std::fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => match serde_json::from_str::<Value>(s.trim_start_matches('\u{feff}')) {
            Ok(Value::Object(m)) => Ok(m),
            _ => Err(format!("{} 不是合法的 JSON，没有改动它", path.display())),
        },
        _ => Ok(Map::new()),
    }
}

fn write(m: &Map<String, Value>) -> Result<(), String> {
    let path = settings_path();
    if path.exists() {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let bak = path.with_file_name(format!("settings.json.claude-pet-{ts}.bak"));
        std::fs::copy(&path, &bak).map_err(|e| format!("备份失败：{e}"))?;
    } else if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = serde_json::to_string_pretty(&Value::Object(m.clone())).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("写入 {} 失败：{e}", path.display()))
}

/// 删掉所有属于本程序的 hook 条目；返回删了几条
fn strip_ours(m: &mut Map<String, Value>) -> usize {
    let mut removed = 0;
    let Some(Value::Object(hooks)) = m.get_mut("hooks") else { return 0 };
    for (_, groups) in hooks.iter_mut() {
        let Value::Array(arr) = groups else { continue };
        for g in arr.iter_mut() {
            if let Some(Value::Array(hs)) = g.get_mut("hooks") {
                let before = hs.len();
                hs.retain(|h| !h.get("command").and_then(|c| c.as_str()).map(is_ours).unwrap_or(false));
                removed += before - hs.len();
            }
        }
        arr.retain(|g| g.get("hooks").and_then(|h| h.as_array()).map(|a| !a.is_empty()).unwrap_or(true));
    }
    hooks.retain(|_, v| v.as_array().map(|a| !a.is_empty()).unwrap_or(true));
    if hooks.is_empty() {
        m.remove("hooks");
    }
    removed
}

/// settings.json 里所有属于本程序的 hook 命令
fn our_commands(m: &Map<String, Value>) -> Vec<String> {
    let Some(Value::Object(hooks)) = m.get("hooks") else { return Vec::new() };
    hooks
        .values()
        .filter_map(|v| v.as_array())
        .flatten()
        .filter_map(|g| g.get("hooks").and_then(|h| h.as_array()))
        .flatten()
        .filter_map(|h| h.get("command").and_then(|c| c.as_str()))
        .filter(|c| is_ours(c))
        .map(str::to_string)
        .collect()
}

/// 已接入，但记录的 exe 路径和 `cmd` 不一致（文件夹被移动或换了一份拷贝）
fn path_is_stale(m: &Map<String, Value>, cmd: &str) -> bool {
    let norm = |s: &str| s.trim().to_lowercase().replace('\\', "/");
    let want = norm(cmd);
    our_commands(m).iter().any(|c| norm(c) != want)
}

pub fn is_installed() -> bool {
    read().map(|m| !our_commands(&m).is_empty()).unwrap_or(false)
}

/// 启动时调用：已接入但 hooks 指向的不是当前这个 exe 时，自动改成当前路径。
/// 没接入或路径没变时返回 None，不碰 settings.json。
pub fn repair_path() -> Option<Result<String, String>> {
    let m = read().ok()?;
    if !path_is_stale(&m, &our_command()) {
        return None;
    }
    Some(install())
}

pub fn install() -> Result<String, String> {
    let mut m = read()?;
    strip_ours(&mut m);
    let hooks = m.entry("hooks").or_insert_with(|| json!({}));
    let Value::Object(hooks) = hooks else { return Err("settings.json 里的 hooks 不是对象，没有改动".into()) };
    let cmd = our_command();
    for (event, matcher) in EVENTS {
        let mut group = Map::new();
        if matcher {
            group.insert("matcher".into(), json!("*"));
        }
        group.insert("hooks".into(), json!([{ "type": "command", "command": cmd, "timeout": 5 }]));
        let arr = hooks.entry(event).or_insert_with(|| json!([]));
        if let Value::Array(a) = arr {
            a.push(Value::Object(group));
        }
    }
    write(&m)?;
    Ok(format!(
        "已接入 Claude Code（{} 个事件）。\n\n新开的 Claude Code 会话才会生效。\n配置文件：{}",
        EVENTS.len(),
        settings_path().display()
    ))
}

pub fn uninstall() -> Result<String, String> {
    let mut m = read()?;
    let n = strip_ours(&mut m);
    if n == 0 {
        return Ok("settings.json 里没有桌宠的 hooks，无需移除。".into());
    }
    write(&m)?;
    Ok(format!("已从 Claude Code 断开（移除 {n} 条 hook）。"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with(cmds: &[&str]) -> Map<String, Value> {
        let groups: Vec<Value> = cmds.iter().map(|c| json!({ "hooks": [{ "type": "command", "command": c }] })).collect();
        let Value::Object(m) = json!({ "hooks": { "Stop": groups } }) else { unreachable!() };
        m
    }

    #[test]
    fn stale_path_detection() {
        let cur = "\"D:/pet/claude-pet.exe\" hook";
        // 没接入：不算过期
        assert!(!path_is_stale(&Map::new(), cur));
        assert!(!path_is_stale(&settings_with(&["other-tool --flag"]), cur));
        // 路径一致（大小写、斜杠不同也算一致）
        assert!(!path_is_stale(&settings_with(&[cur]), cur));
        assert!(!path_is_stale(&settings_with(&[r#""d:\PET\claude-pet.exe" hook"#]), cur));
        // 文件夹被移动过
        assert!(path_is_stale(&settings_with(&["\"C:/Users/a/Downloads/dist/claude-pet.exe\" hook"]), cur));
        assert!(path_is_stale(&settings_with(&[cur, "\"E:/old/claude-pet.exe\" hook"]), cur));
    }

    #[test]
    fn strip_keeps_other_hooks() {
        let mut m = settings_with(&["\"E:/old/claude-pet.exe\" hook", "other-tool"]);
        assert_eq!(strip_ours(&mut m), 1);
        assert_eq!(our_commands(&m).len(), 0);
        assert_eq!(m["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }
}
