//! 程序配置：exe 同目录下的 config.json（便携，拷走整个文件夹即可）。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct AppConfig {
    /// 动画包目录名（pets/<pet>/pet.json）
    pub pet: String,
    /// 显示缩放；不写则用动画包自己的 scale
    pub scale: Option<f32>,
    /// 窗口位置（拖动后自动保存）
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub topmost: bool,
    /// 待机时偶尔播放随机小动作
    pub idle_extras: bool,
    /// 本机 UDP 端口：hook 进程把事件发到这里
    pub port: u16,
    /// 完成 / 出错动画展示多久后回到待机（秒）
    pub success_secs: u64,
    pub error_secs: u64,
    /// 状态降级前至少展示多久（毫秒），防止 工作→整理→工作 来回闪
    pub hold_ms: u64,
    /// 会话超过这么多分钟没有任何事件就清掉；0 = 不超时（只认明确信号和进程退出）
    pub stale_minutes: u64,
    /// 收到 hook 事件而桌宠没开时，自动启动桌宠
    pub auto_launch: bool,
    /// 右键「打开 Claude Code」启动什么：exe 路径或 shell:AppsFolder\… ；空 = 自动找 Claude 桌面版
    pub claude_app: String,
    /// 把收到的 hook 事件写进 hook-events.log，排查哪些 hook 没触发
    pub hook_log: bool,
    /// 首次启动是否已询问过接入 hooks
    pub asked_hooks: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            pet: "maid".into(),
            scale: None,
            x: None,
            y: None,
            topmost: true,
            idle_extras: true,
            port: 47615,
            success_secs: 10,
            error_secs: 12,
            hold_ms: 600,
            stale_minutes: 0,
            auto_launch: true,
            claude_app: String::new(),
            hook_log: true,
            asked_hooks: false,
        }
    }
}

pub fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("claude-pet.exe"))
}

pub fn base_dir() -> PathBuf {
    exe_path().parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
}

fn config_path() -> PathBuf {
    base_dir().join("config.json")
}

pub fn load() -> AppConfig {
    match std::fs::read_to_string(config_path()) {
        Ok(s) => serde_json::from_str(s.trim_start_matches('\u{feff}')).unwrap_or_default(),
        Err(_) => {
            let cfg = AppConfig::default();
            save(&cfg);
            cfg
        }
    }
}

pub fn save(cfg: &AppConfig) {
    if let Ok(s) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(config_path(), s);
    }
}
