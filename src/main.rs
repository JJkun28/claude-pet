#![windows_subsystem = "windows"]
//! Claude 桌宠（原生 Windows 版）
//!
//!   claude-pet.exe              启动桌宠
//!   claude-pet.exe hook         由 Claude Code 的 hooks 调用：读 stdin 事件转发给桌宠
//!   claude-pet.exe install      把 hooks 写进 ~/.claude/settings.json
//!   claude-pet.exe uninstall    从 settings.json 移除本程序的 hooks
//!   claude-pet.exe sessions     列出对话框能看到的桌面版会话（排查用）

mod app;
mod chat;
mod config;
mod desktop;
mod hook;
mod install;
mod pack;
mod state;
mod tray;

fn console_print(s: &str) {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    println!("{s}");
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    match arg.as_str() {
        "hook" => hook::run(),
        "install" => console_print(&install::install().unwrap_or_else(|e| format!("失败：{e}"))),
        "uninstall" => console_print(&install::uninstall().unwrap_or_else(|e| format!("失败：{e}"))),
        "sessions" => {
            let list = desktop::list_sessions(15);
            let mut out = format!("找到 {} 个会话（最近的在前）：", list.len());
            for s in &list {
                let cli: String = s.cli_id.chars().take(8).collect();
                out.push_str(&format!("\n  {}  {:<8}  {} · {}", s.id, cli, s.title, s.folder));
            }
            out.push_str(&format!("\nClaude 窗口：{}", if desktop::claude_window().is_some() { "找到了" } else { "没找到" }));
            console_print(&out);
        }
        _ => app::run(),
    }
}
