#![windows_subsystem = "windows"]
//! Claude 桌宠（原生 Windows 版）
//!
//!   claude-pet.exe              启动桌宠
//!   claude-pet.exe hook         由 Claude Code 的 hooks 调用：读 stdin 事件转发给桌宠
//!   claude-pet.exe install      把 hooks 写进 ~/.claude/settings.json
//!   claude-pet.exe uninstall    从 settings.json 移除本程序的 hooks

mod app;
mod config;
mod hook;
mod install;
mod pack;
mod state;

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
        _ => app::run(),
    }
}
