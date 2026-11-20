//! 会话状态聚合：每个 Claude Code 会话一个条目，展示优先级最高的那个。
//!
//! 规则（桌宠只在收到明确信号时才回待机）：
//!   - 思考 / 工作 / 整理 / 等待 一直保持，直到同一会话来了下一个事件；
//!   - 完成（Stop）/ 出错（StopFailure）展示一段时间后清掉 → 回待机；
//!   - 被 Esc 中断：Claude Code 不发 Stop，但会在空闲时发 Notification(idle_prompt)，据此清掉；
//!   - SessionEnd 清掉；会话所属的 Claude 进程退出了也清掉（防止关窗口后卡在「等待」）。

use serde::Deserialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WState {
    Thinking,
    Working,
    Result,
    Waiting,
    Success,
    Error,
}

impl WState {
    pub const ALL: [WState; 6] =
        [WState::Thinking, WState::Working, WState::Result, WState::Waiting, WState::Success, WState::Error];

    /// 等待授权 > 出错 > 工作 > 思考 > 整理 > 完成
    pub fn prio(self) -> u8 {
        match self {
            WState::Waiting => 60,
            WState::Error => 50,
            WState::Working => 40,
            WState::Thinking => 30,
            WState::Result => 25,
            WState::Success => 20,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            WState::Thinking => "思考中",
            WState::Working => "工作中",
            WState::Result => "整理结果",
            WState::Waiting => "等待你确认",
            WState::Success => "完成",
            WState::Error => "出错",
        }
    }
    fn terminal(self) -> bool {
        matches!(self, WState::Success | WState::Error)
    }
}

/// hook 进程发来的精简事件
#[derive(Deserialize, Debug, Clone, Default)]
pub struct HookMsg {
    pub e: String,
    #[serde(default)]
    pub sid: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub nt: Option<String>,
    #[serde(default)]
    pub msg: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
}

enum Action {
    Set(WState),
    Clear,
    Ignore,
}

fn classify(m: &HookMsg) -> Action {
    match m.e.as_str() {
        "UserPromptSubmit" => Action::Set(WState::Thinking),
        "PreToolUse" => {
            if m.tool.as_deref() == Some("AskUserQuestion") {
                Action::Set(WState::Waiting)
            } else {
                Action::Set(WState::Working)
            }
        }
        "PostToolUse" | "PostToolUseFailure" | "PostToolBatch" => Action::Set(WState::Result),
        "PermissionRequest" | "Elicitation" => Action::Set(WState::Waiting),
        "ElicitationResult" | "SubagentStart" | "PreCompact" => Action::Set(WState::Working),
        "Notification" => {
            let nt = m.nt.as_deref().unwrap_or("");
            let msg = m.msg.as_deref().unwrap_or("").to_lowercase();
            if nt == "permission_prompt" || nt == "elicitation_dialog" || (nt.is_empty() && msg.contains("permission")) {
                Action::Set(WState::Waiting)
            } else if nt == "idle_prompt" || (nt.is_empty() && msg.contains("waiting for your input")) {
                Action::Clear
            } else {
                Action::Ignore
            }
        }
        "Stop" => Action::Set(WState::Success),
        "StopFailure" => Action::Set(WState::Error),
        "SessionEnd" => Action::Clear,
        _ => Action::Ignore,
    }
}

struct Entry {
    state: WState,
    seq: u64,
    last: Instant,
}

#[derive(Default)]
pub struct Store {
    sessions: HashMap<String, Entry>,
    pids: HashMap<String, u32>,
    seq: u64,
}

impl Store {
    /// 返回 true 表示会话表有变化
    pub fn apply(&mut self, m: &HookMsg) -> bool {
        let sid = m.sid.clone().unwrap_or_else(|| "default".into());
        if let Some(pid) = m.pid {
            self.pids.insert(sid.clone(), pid);
        }
        if m.e == "SessionEnd" {
            self.pids.remove(&sid);
        }
        match classify(m) {
            Action::Set(s) => {
                self.seq += 1;
                let changed = self.sessions.get(&sid).map(|e| e.state != s).unwrap_or(true);
                self.sessions.insert(sid, Entry { state: s, seq: self.seq, last: Instant::now() });
                changed
            }
            Action::Clear => self.sessions.remove(&sid).is_some(),
            Action::Ignore => {
                if let Some(e) = self.sessions.get_mut(&sid) {
                    e.last = Instant::now();
                }
                false
            }
        }
    }

    /// 完成 / 出错过期、可选的长时间无事件超时
    pub fn expire(&mut self, success: Duration, error: Duration, stale: Option<Duration>) -> bool {
        let before = self.sessions.len();
        self.sessions.retain(|_, e| {
            let age = e.last.elapsed();
            match e.state {
                WState::Success => age < success,
                WState::Error => age < error,
                _ => stale.map(|s| age < s).unwrap_or(true),
            }
        });
        self.sessions.len() != before
    }

    /// 清掉 Claude 进程已经退出的会话
    pub fn reap(&mut self, alive: impl Fn(u32) -> bool) -> bool {
        let dead: Vec<String> =
            self.pids.iter().filter(|(_, &pid)| !alive(pid)).map(|(s, _)| s.clone()).collect();
        let mut changed = false;
        for sid in dead {
            self.pids.remove(&sid);
            changed |= self.sessions.remove(&sid).is_some();
        }
        changed
    }

    pub fn display(&self) -> Option<WState> {
        self.sessions
            .values()
            .max_by(|a, b| a.state.prio().cmp(&b.state.prio()).then(a.seq.cmp(&b.seq)))
            .map(|e| e.state)
    }

    /// 某个会话（hook 的 session_id）当前的状态
    pub fn state_of(&self, sid: &str) -> Option<WState> {
        self.sessions.get(sid).map(|e| e.state)
    }

    pub fn active_count(&self) -> usize {
        self.sessions.values().filter(|e| !e.state.terminal()).count()
    }

    pub fn clear(&mut self) {
        self.sessions.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(e: &str, sid: &str) -> HookMsg {
        HookMsg { e: e.into(), sid: Some(sid.into()), ..Default::default() }
    }

    #[test]
    fn priority_and_no_early_celebration() {
        let mut s = Store::default();
        s.apply(&ev("UserPromptSubmit", "a"));
        assert_eq!(s.display(), Some(WState::Thinking));
        s.apply(&ev("PreToolUse", "a"));
        assert_eq!(s.display(), Some(WState::Working));
        s.apply(&ev("UserPromptSubmit", "b"));
        assert_eq!(s.display(), Some(WState::Working));
        s.apply(&ev("Stop", "a"));
        assert_eq!(s.display(), Some(WState::Thinking), "a 完成但 b 还在思考，不该庆祝");
        let mut n = ev("Notification", "b");
        n.nt = Some("permission_prompt".into());
        s.apply(&n);
        assert_eq!(s.display(), Some(WState::Waiting));
        s.apply(&ev("PreToolUse", "b"));
        assert_eq!(s.display(), Some(WState::Working));
        s.apply(&ev("Stop", "b"));
        assert_eq!(s.display(), Some(WState::Success));
    }

    #[test]
    fn interrupt_and_session_end_clear() {
        let mut s = Store::default();
        s.apply(&ev("PreToolUse", "a"));
        let mut n = ev("Notification", "a");
        n.nt = Some("idle_prompt".into());
        s.apply(&n);
        assert_eq!(s.display(), None);
        s.apply(&ev("PermissionRequest", "a"));
        assert_eq!(s.display(), Some(WState::Waiting));
        s.apply(&ev("SessionEnd", "a"));
        assert_eq!(s.display(), None);
    }

    #[test]
    fn dead_process_clears_waiting() {
        let mut s = Store::default();
        let mut m = ev("PermissionRequest", "a");
        m.pid = Some(42);
        s.apply(&m);
        assert!(s.reap(|_| false));
        assert_eq!(s.display(), None);
    }

    #[test]
    fn terminal_expires() {
        let mut s = Store::default();
        s.apply(&ev("Stop", "a"));
        s.expire(Duration::ZERO, Duration::ZERO, None);
        assert_eq!(s.display(), None);
    }
}
