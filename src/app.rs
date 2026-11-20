//! 桌宠窗口：Win32 分层窗口（逐像素透明、置顶、不进任务栏、不抢焦点），
//! 用 UpdateLayeredWindow 直接贴预乘 BGRA 位图，不依赖任何浏览器内核。

use crate::chat;
use crate::config::{self, AppConfig};
use crate::desktop;
use crate::hook::{self, wide};
use crate::install;
use crate::pack::{Pack, Sheet};
use crate::state::{HookMsg, Store, WState};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Write;
use std::net::UdpSocket;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_HOOK_EVENT: u32 = WM_APP + 1;
const TIMER_FRAME: usize = 1;
const TIMER_HOUSE: usize = 2;
/// 对话框发送：等 Claude 窗口到前台、等页面渲染，然后打字
const TIMER_SEND: usize = 3;

const CMD_TOPMOST: usize = 1;
const CMD_IDLE_EXTRAS: usize = 2;
const CMD_HOOKS: usize = 3;
const CMD_RESET: usize = 4;
const CMD_RELOAD: usize = 5;
const CMD_OPEN_DIR: usize = 6;
const CMD_AUTOSTART: usize = 7;
const CMD_OPEN_LOG: usize = 8;
const CMD_QUIT: usize = 9;
const CMD_OPEN_CLAUDE: usize = 10;
const CMD_HIDE: usize = 11;
const CMD_CHAT: usize = 12;
/// 再次双击 exe 时，新进程发这个消息叫已在运行的桌宠现身
const WM_SHOW_PET: u32 = WM_APP + 2;
const CMD_STATE_BASE: usize = 100; // + WState 序号；+6 = 待机
const CMD_SCALE_BASE: usize = 200;
const CMD_PET_BASE: usize = 300;
const CMD_ANIM_BASE: usize = 1000;
const SCALES: [f32; 7] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 2.5];

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "ClaudePet";

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Base,
    Manual,
    Click,
    IdleExtra,
    Drag,
}

struct Play {
    name: String,
    sheet: Rc<Sheet>,
    frame: usize,
    kind: Kind,
}

struct Dib {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    w: usize,
    h: usize,
}

struct Drag {
    start_cursor: POINT,
    start_pos: (i32, i32),
    moved: bool,
    /// 上次鼠标 x、同方向累计的水平位移、当前朝向（-1 左 / 0 未定 / 1 右）
    last_x: i32,
    acc: i32,
    dir: i32,
}

enum SendPhase {
    /// 已经发了 claude:// 链接，等 Claude 窗口到前台
    Wait { deadline: Instant },
    /// 窗口到前台了，等会话页面渲染、输入框拿到焦点
    Settle { until: Instant },
}

struct PendingSend {
    text: String,
    /// 桌面版没开、需要冷启动
    cold: bool,
    phase: SendPhase,
}

struct App {
    hwnd: HWND,
    cfg: AppConfig,
    pack: Pack,
    cache: Vec<(String, Rc<Sheet>)>,
    store: Store,
    shown: Option<WState>,
    shown_since: Instant,
    base: Option<Play>,
    overlay: Option<Play>,
    next_idle_extra: Instant,
    dib: Option<Dib>,
    pos: (i32, i32),
    size: (i32, i32),
    drag: Option<Drag>,
    queue: Arc<Mutex<VecDeque<HookMsg>>>,
    rng: u64,
    frame_ms: u32,
    last_reap: Instant,
    menu_anims: Vec<String>,
    menu_pets: Vec<String>,
    udp_error: Option<String>,
    /// 右键「隐藏」后为 true：窗口不显示、不跑动画，但照常记录会话状态
    hidden: bool,
    /// 对话框里按了发送、还没打进 Claude 的那条
    pending: Option<PendingSend>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// 借用全局 App；正在被借用（重入）时返回 None，调用方走默认处理
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn msgbox(hwnd: HWND, text: &str, flags: u32) -> i32 {
    let (t, c) = (wide(text), wide("Claude 桌宠"));
    unsafe { MessageBoxW(hwnd, t.as_ptr(), c.as_ptr(), flags) }
}

fn now_str() -> String {
    unsafe {
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        format!("{:02}-{:02} {:02}:{:02}:{:02}.{:03}", t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds)
    }
}

fn log_line(cfg: &AppConfig, line: &str) {
    if !cfg.hook_log {
        return;
    }
    let path = config::base_dir().join("hook-events.log");
    let too_big = std::fs::metadata(&path).map(|m| m.len() > 512 * 1024).unwrap_or(false);
    let file = std::fs::OpenOptions::new().create(true).append(!too_big).write(true).truncate(too_big).open(&path);
    if let Ok(mut f) = file {
        let _ = writeln!(f, "{} {}", now_str(), line);
    }
}

impl App {
    fn rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn pick(&mut self, pool: &[String], avoid: Option<&str>) -> Option<String> {
        if pool.is_empty() {
            return None;
        }
        let cands: Vec<&String> = pool.iter().filter(|n| Some(n.as_str()) != avoid).collect();
        let cands = if cands.is_empty() { pool.iter().collect() } else { cands };
        let i = (self.rand() % cands.len() as u64) as usize;
        Some(cands[i].clone())
    }

    /// 某状态的动画池；没配就按链条回退，最后回退到第一段动画
    fn pool_for(&self, state: Option<WState>) -> Vec<String> {
        let chain: &[&str] = match state {
            None => &["idle"],
            Some(WState::Thinking) => &["thinking", "working", "idle"],
            Some(WState::Working) => &["working", "thinking", "idle"],
            Some(WState::Result) => &["result", "working", "idle"],
            Some(WState::Waiting) => &["waiting", "thinking", "idle"],
            Some(WState::Success) => &["success", "idle"],
            Some(WState::Error) => &["error", "idle"],
        };
        for k in chain {
            if let Some(v) = self.pack.states.get(*k) {
                if !v.is_empty() {
                    return v.clone();
                }
            }
        }
        vec![self.pack.anims[0].0.clone()]
    }

    fn sheet(&mut self, name: &str) -> Option<Rc<Sheet>> {
        if let Some(i) = self.cache.iter().position(|(n, _)| n == name) {
            let item = self.cache.remove(i);
            let s = item.1.clone();
            self.cache.push(item);
            return Some(s);
        }
        match self.pack.load_sheet(name) {
            Ok(s) => {
                let s = Rc::new(s);
                self.cache.push((name.to_string(), s.clone()));
                // 只留最近用的 3 段（当前状态 + 叠加动作 + 待机）
                while self.cache.len() > 3 {
                    self.cache.remove(0);
                }
                Some(s)
            }
            Err(e) => {
                log_line(&self.cfg, &format!("[错误] {e}"));
                None
            }
        }
    }

    fn start_base(&mut self, state: Option<WState>, avoid: Option<String>) {
        let pool = self.pool_for(state);
        if let Some(name) = self.pick(&pool, avoid.as_deref()) {
            if let Some(sheet) = self.sheet(&name) {
                self.base = Some(Play { name, sheet, frame: 0, kind: Kind::Base });
            }
        }
    }

    fn start_overlay(&mut self, name: &str, kind: Kind) {
        if let Some(sheet) = self.sheet(name) {
            self.overlay = Some(Play { name: name.to_string(), sheet, frame: 0, kind });
            self.reset_frame_timer();
            self.render();
        }
    }

    fn schedule_idle_extra(&mut self) {
        let ie = &self.pack.idle_extras;
        let (lo, hi) = (ie.min_secs.max(5), ie.max_secs.max(ie.min_secs.max(5)));
        let secs = lo + self.rand() % (hi - lo + 1);
        self.next_idle_extra = Instant::now() + Duration::from_secs(secs);
    }

    fn set_shown(&mut self, s: Option<WState>) {
        let from = self.shown.map(|x| x.label()).unwrap_or("待机");
        log_line(&self.cfg, &format!("  => 显示 {} → {}", from, s.map(|x| x.label()).unwrap_or("待机")));
        self.shown = s;
        self.shown_since = Instant::now();
        // 状态变化会打断「点击回应 / 待机小动作」，但不打断右键点播和拖拽
        if matches!(self.overlay.as_ref().map(|o| o.kind), Some(Kind::Click) | Some(Kind::IdleExtra)) {
            self.overlay = None;
        }
        self.start_base(s, None);
        if s.is_none() {
            self.schedule_idle_extra();
        }
        self.reset_frame_timer();
        self.render();
    }

    /// 按会话表重新决定显示哪个状态；降级（优先级变低）至少等 hold_ms
    fn reevaluate(&mut self) {
        let desired = self.store.display();
        if desired == self.shown {
            return;
        }
        let pd = desired.map(|s| s.prio()).unwrap_or(0);
        let ps = self.shown.map(|s| s.prio()).unwrap_or(0);
        if pd < ps && self.shown_since.elapsed() < Duration::from_millis(self.cfg.hold_ms) {
            return;
        }
        self.set_shown(desired);
    }

    fn current(&self) -> Option<&Play> {
        self.overlay.as_ref().or(self.base.as_ref())
    }

    fn reset_frame_timer(&mut self) {
        if self.hidden {
            return;
        }
        let ms = self.current().map(|p| p.sheet.frame_ms(p.frame)).unwrap_or(100).max(16);
        if ms != self.frame_ms {
            self.frame_ms = ms;
            unsafe { SetTimer(self.hwnd, TIMER_FRAME, ms, None) };
        }
    }

    fn anchor_rect(&self) -> RECT {
        RECT { left: self.pos.0, top: self.pos.1, right: self.pos.0 + self.size.0, bottom: self.pos.1 + self.size.1 }
    }

    /// 对话框里列的会话：桌面版最近的会话，带上桌宠记录到的状态
    fn chat_items(&self) -> Vec<chat::Item> {
        desktop::list_sessions(15)
            .into_iter()
            .map(|s| {
                let state = self.store.state_of(&s.cli_id).map(|st| st.label());
                chat::Item { session: s, state }
            })
            .collect()
    }

    /// 对话框按了发送：让桌面版切到目标会话，然后用定时器等它准备好再打字
    fn start_send(&mut self, req: chat::SendReq) {
        let (id, title, cli) = match &req.session {
            Some(s) => (s.id.as_str(), s.title.as_str(), s.cli_id.chars().take(8).collect::<String>()),
            None => ("last", "最近的会话", "-".to_string()),
        };
        log_line(&self.cfg, &format!("对话框发送 → {title}（会话 {cli}），{} 字", req.text.chars().count()));
        let cold = desktop::claude_window().is_none();
        desktop::open_session(id);
        let wait = Duration::from_secs(if cold { 20 } else { 4 });
        self.pending = Some(PendingSend { text: req.text, cold, phase: SendPhase::Wait { deadline: Instant::now() + wait } });
        unsafe { SetTimer(self.hwnd, TIMER_SEND, 100, None) };
    }

    /// 发送状态机；失败时返回要提示的话
    fn send_tick(&mut self) -> Option<String> {
        let Some(p) = self.pending.as_mut() else {
            unsafe { KillTimer(self.hwnd, TIMER_SEND) };
            return None;
        };
        let now = Instant::now();
        let ok = match p.phase {
            SendPhase::Wait { deadline } => {
                if desktop::foreground_is_claude() {
                    // 冷启动时桌面版还要加载会话列表才会跳转，多等一会
                    let extra = if p.cold { 2500 } else { 0 };
                    p.phase = SendPhase::Settle { until: now + Duration::from_millis(self.cfg.chat_delay_ms + extra) };
                    return None;
                }
                if now < deadline {
                    if let Some(w) = desktop::claude_window() {
                        desktop::bring_claude_front(w);
                    }
                    return None;
                }
                false
            }
            SendPhase::Settle { until } => {
                if now < until {
                    return None;
                }
                desktop::foreground_is_claude() && desktop::type_text(&p.text, self.cfg.chat_auto_send)
            }
        };
        let p = self.pending.take().unwrap();
        unsafe { KillTimer(self.hwnd, TIMER_SEND) };
        if ok {
            log_line(&self.cfg, "  （已打进 Claude 桌面版）");
            let pool = self.pack.states.get("click").cloned().unwrap_or_default();
            if let Some(name) = self.pick(&pool, None) {
                self.start_overlay(&name, Kind::Click);
            }
            None
        } else {
            log_line(&self.cfg, "  （没能切到 Claude 窗口，已复制到剪贴板）");
            let copied = desktop::copy_to_clipboard(self.hwnd, &p.text);
            Some(if copied {
                "没能切到 Claude 桌面版的窗口。\n要发的话已经复制到剪贴板，切过去按 Ctrl+V 就行。".into()
            } else {
                "没能切到 Claude 桌面版的窗口，复制到剪贴板也失败了。".into()
            })
        }
    }

    fn hide(&mut self) {
        chat::close(true);
        self.hidden = true;
        self.overlay = None;
        self.drag = None;
        unsafe {
            ShowWindow(self.hwnd, SW_HIDE);
            KillTimer(self.hwnd, TIMER_FRAME);
        }
        self.frame_ms = 0;
        log_line(&self.cfg, "  （隐藏桌宠）");
    }

    fn show(&mut self) {
        if !self.hidden {
            return;
        }
        self.hidden = false;
        log_line(&self.cfg, "  （重新显示桌宠）");
        self.reset_frame_timer();
        self.render();
        unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
        self.apply_topmost();
    }

    fn tick(&mut self) {
        if self.hidden {
            return;
        }
        if let Some(o) = self.overlay.as_mut() {
            o.frame += 1;
            if o.frame >= o.sheet.frames {
                if o.kind == Kind::Drag {
                    o.frame = 0;
                } else {
                    self.overlay = None;
                    if let Some(b) = self.base.as_mut() {
                        b.frame = 0;
                    }
                    if self.shown.is_none() {
                        self.schedule_idle_extra();
                    }
                }
            }
        } else if let Some(b) = self.base.as_mut() {
            b.frame += 1;
            if b.frame >= b.sheet.frames {
                // 一轮播完：同一状态的动画池里换一段（池里只有一段就重播）
                let avoid = Some(b.name.clone());
                self.start_base(self.shown, avoid);
            }
        }
        if self.overlay.is_none()
            && self.shown.is_none()
            && self.cfg.idle_extras
            && self.drag.is_none()
            && Instant::now() >= self.next_idle_extra
        {
            let pool = self.pack.idle_extras.pool.clone();
            match self.pick(&pool, None) {
                Some(name) => self.start_overlay(&name, Kind::IdleExtra),
                None => self.schedule_idle_extra(),
            }
        }
        self.reset_frame_timer();
        self.render();
    }

    fn eff_scale(&self) -> f32 {
        let dpi = unsafe { GetDpiForWindow(self.hwnd) }.max(96) as f32;
        self.cfg.scale.unwrap_or(self.pack.scale).clamp(0.2, 6.0) * dpi / 96.0
    }

    fn render(&mut self) {
        if self.hidden {
            return;
        }
        let Some(play) = self.current() else { return };
        let sheet = play.sheet.clone();
        let frame = play.frame;
        let scale = self.eff_scale();
        let dw = ((sheet.fw as f32 * scale).round() as usize).max(1);
        let dh = ((sheet.fh as f32 * scale).round() as usize).max(1);
        unsafe {
            if self.dib.as_ref().map(|d| d.w != dw || d.h != dh).unwrap_or(true) {
                self.free_dib();
                let screen = GetDC(std::ptr::null_mut());
                let dc = CreateCompatibleDC(screen);
                ReleaseDC(std::ptr::null_mut(), screen);
                let mut bmi: BITMAPINFO = std::mem::zeroed();
                bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                bmi.bmiHeader.biWidth = dw as i32;
                bmi.bmiHeader.biHeight = -(dh as i32);
                bmi.bmiHeader.biPlanes = 1;
                bmi.bmiHeader.biBitCount = 32;
                let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
                let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
                if bmp.is_null() || bits.is_null() {
                    DeleteDC(dc);
                    return;
                }
                let old = SelectObject(dc, bmp);
                self.dib = Some(Dib { dc, bmp, old, bits: bits as *mut u32, w: dw, h: dh });
            }
            let dib = self.dib.as_ref().unwrap();
            let buf = std::slice::from_raw_parts_mut(dib.bits, dw * dh);
            sheet.draw(frame, buf, dw, dh);

            // 尺寸变了：保持「底边中点」不动
            let (ow, oh) = self.size;
            if ow != dw as i32 || oh != dh as i32 {
                if ow > 0 {
                    self.pos.0 += (ow - dw as i32) / 2;
                    self.pos.1 += oh - dh as i32;
                }
                self.size = (dw as i32, dh as i32);
            }
            let pt = POINT { x: self.pos.0, y: self.pos.1 };
            let sz = SIZE { cx: dw as i32, cy: dh as i32 };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            UpdateLayeredWindow(self.hwnd, std::ptr::null_mut(), &pt, &sz, dib.dc, &src, 0, &blend, ULW_ALPHA);
        }
        chat::follow(self.anchor_rect());
    }

    fn free_dib(&mut self) {
        if let Some(d) = self.dib.take() {
            unsafe {
                SelectObject(d.dc, d.old);
                DeleteObject(d.bmp);
                DeleteDC(d.dc);
            }
        }
    }

    fn apply_topmost(&self) {
        let after = if self.cfg.topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
        unsafe { SetWindowPos(self.hwnd, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
    }

    fn handle_events(&mut self) {
        let msgs: Vec<HookMsg> = self.queue.lock().map(|mut q| q.drain(..).collect()).unwrap_or_default();
        for m in msgs {
            let sid: String = m.sid.as_deref().unwrap_or("-").chars().take(8).collect();
            let extra = [m.tool.as_deref(), m.nt.as_deref()].iter().flatten().cloned().collect::<Vec<_>>().join(" ");
            log_line(&self.cfg, &format!("{:<18} 会话 {sid} {extra}", m.e));
            self.store.apply(&m);
            if self.hidden && m.e == "UserPromptSubmit" {
                self.show();
            }
        }
        self.reevaluate();
    }

    fn housekeeping(&mut self) {
        let stale = (self.cfg.stale_minutes > 0).then(|| Duration::from_secs(self.cfg.stale_minutes * 60));
        self.store.expire(
            Duration::from_secs(self.cfg.success_secs),
            Duration::from_secs(self.cfg.error_secs),
            stale,
        );
        if self.last_reap.elapsed() >= Duration::from_secs(5) {
            self.last_reap = Instant::now();
            if self.store.reap(hook::process_alive) {
                log_line(&self.cfg, "  （Claude 进程已退出，清掉对应会话）");
            }
            if self.cfg.topmost && self.drag.is_none() {
                self.apply_topmost();
            }
        }
        self.reevaluate();
    }

    fn reload(&mut self) -> Result<(), String> {
        let cfg = config::load();
        let pack = Pack::load(&config::base_dir().join("pets").join(&cfg.pet))?;
        self.cfg = cfg;
        self.pack = pack;
        self.cache.clear();
        self.overlay = None;
        self.start_base(self.shown, None);
        self.reset_frame_timer();
        self.render();
        Ok(())
    }

    fn build_menu(&mut self) -> HMENU {
        unsafe {
            let m = CreatePopupMenu();
            let add = |menu: HMENU, flags: u32, id: usize, text: &str| {
                let t = wide(text);
                AppendMenuW(menu, flags, id, t.as_ptr());
            };
            let status = match self.shown {
                Some(s) => format!("当前：{}（{} 个会话进行中）", s.label(), self.store.active_count()),
                None => "当前：待机".to_string(),
            };
            add(m, MF_STRING | MF_GRAYED, 0, &status);
            if let Some(e) = &self.udp_error {
                add(m, MF_STRING | MF_GRAYED, 0, e);
            }
            add(m, MF_STRING, CMD_OPEN_CLAUDE, "打开 Claude Code");
            add(m, MF_STRING, CMD_CHAT, "和 Claude 说话…（双击桌宠也行）");
            AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());

            // 动作：按 pet.json 的 menu 分组，点哪个播哪个
            self.menu_anims.clear();
            let groups = self.pack.menu.clone();
            for g in &groups {
                let sub = CreatePopupMenu();
                for name in &g.items {
                    let id = CMD_ANIM_BASE + self.menu_anims.len();
                    self.menu_anims.push(name.clone());
                    add(sub, MF_STRING, id, name);
                }
                add(m, MF_POPUP, sub as usize, &format!("{}（{}）", g.group, g.items.len()));
            }
            let sub = CreatePopupMenu();
            for (i, s) in WState::ALL.iter().enumerate() {
                add(sub, MF_STRING, CMD_STATE_BASE + i, s.label());
            }
            add(sub, MF_STRING, CMD_STATE_BASE + 6, "待机");
            add(m, MF_POPUP, sub as usize, "按状态预览");
            AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());

            let sub = CreatePopupMenu();
            let cur = self.cfg.scale.unwrap_or(self.pack.scale);
            for (i, s) in SCALES.iter().enumerate() {
                let chk = if (cur - s).abs() < 0.01 { MF_CHECKED } else { 0 };
                add(sub, MF_STRING | chk, CMD_SCALE_BASE + i, &format!("{}%", (s * 100.0) as i32));
            }
            add(m, MF_POPUP, sub as usize, "大小");
            self.menu_pets.clear();
            let packs = crate::pack::list_packs(&config::base_dir().join("pets"));
            if packs.len() > 1 {
                let sub = CreatePopupMenu();
                for (i, (dir, name)) in packs.into_iter().take(99).enumerate() {
                    let chk = if dir == self.cfg.pet { MF_CHECKED } else { 0 };
                    add(sub, MF_STRING | chk, CMD_PET_BASE + i, &name);
                    self.menu_pets.push(dir);
                }
                add(m, MF_POPUP, sub as usize, "换宠物");
            }
            let chk = |b: bool| if b { MF_CHECKED } else { 0 };
            add(m, MF_STRING | chk(self.cfg.topmost), CMD_TOPMOST, "始终置顶");
            add(m, MF_STRING | chk(self.cfg.idle_extras), CMD_IDLE_EXTRAS, "待机时随机小动作");
            add(m, MF_STRING | chk(autostart_enabled()), CMD_AUTOSTART, "开机自启");
            AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
            add(m, MF_STRING | chk(install::is_installed()), CMD_HOOKS, "接入 Claude Code（hooks）");
            add(m, MF_STRING, CMD_RESET, "重置会话状态（回到待机）");
            add(m, MF_STRING, CMD_OPEN_LOG, "查看 hook 事件日志");
            add(m, MF_STRING, CMD_RELOAD, "重新加载配置和动画");
            add(m, MF_STRING, CMD_OPEN_DIR, "打开桌宠文件夹");
            AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
            add(m, MF_STRING, CMD_HIDE, "隐藏（下次对话时自动出现）");
            add(m, MF_STRING, CMD_QUIT, "彻底退出（不再自动启动）");
            m
        }
    }

    /// 处理菜单命令；返回需要在借用释放后弹出的提示
    fn command(&mut self, id: usize) -> Option<(String, u32)> {
        match id {
            CMD_TOPMOST => {
                self.cfg.topmost = !self.cfg.topmost;
                self.apply_topmost();
                config::save(&self.cfg);
            }
            CMD_IDLE_EXTRAS => {
                self.cfg.idle_extras = !self.cfg.idle_extras;
                config::save(&self.cfg);
            }
            CMD_AUTOSTART => set_autostart(!autostart_enabled()),
            CMD_HOOKS => {
                let r = if install::is_installed() { install::uninstall() } else { install::install() };
                return Some(match r {
                    Ok(s) => (s, MB_ICONINFORMATION),
                    Err(e) => (e, MB_ICONERROR),
                });
            }
            CMD_RESET => {
                self.store.clear();
                self.set_shown(None);
            }
            CMD_OPEN_LOG => open_path(&config::base_dir().join("hook-events.log")),
            CMD_RELOAD => {
                if let Err(e) = self.reload() {
                    return Some((e, MB_ICONERROR));
                }
            }
            CMD_OPEN_DIR => open_path(&config::base_dir()),
            CMD_OPEN_CLAUDE => {
                if !open_claude(&self.cfg.claude_app) {
                    return Some((
                        "没找到 Claude 桌面版。\n可以在 config.json 的 claude_app 里填它的路径，然后右键「重新加载配置和动画」。".into(),
                        MB_ICONWARNING,
                    ));
                }
            }
            CMD_HIDE => self.hide(),
            CMD_QUIT => {
                // 记下「是你自己关的」：hook 不会再自动拉起，直到你手动打开一次
                self.cfg.quit_by_user = true;
                config::save(&self.cfg);
                unsafe { DestroyWindow(self.hwnd) };
            }
            i if (CMD_STATE_BASE..CMD_STATE_BASE + 7).contains(&i) => {
                let st = WState::ALL.get(i - CMD_STATE_BASE).copied();
                let pool = self.pool_for(st);
                if let Some(name) = self.pick(&pool, None) {
                    self.start_overlay(&name, Kind::Manual);
                }
            }
            i if (CMD_SCALE_BASE..CMD_SCALE_BASE + SCALES.len()).contains(&i) => {
                self.cfg.scale = Some(SCALES[i - CMD_SCALE_BASE]);
                config::save(&self.cfg);
                self.render();
            }
            i if (CMD_PET_BASE..CMD_PET_BASE + 100).contains(&i) => {
                let Some(dir) = self.menu_pets.get(i - CMD_PET_BASE).cloned() else { return None };
                if dir == self.cfg.pet {
                    return None;
                }
                let (old_pet, old_scale) = (self.cfg.pet.clone(), self.cfg.scale);
                self.cfg.pet = dir;
                self.cfg.scale = None; // 每个宠物用自己的默认大小
                config::save(&self.cfg);
                if let Err(e) = self.reload() {
                    self.cfg.pet = old_pet;
                    self.cfg.scale = old_scale;
                    config::save(&self.cfg);
                    return Some((e, MB_ICONERROR));
                }
            }
            i if i >= CMD_ANIM_BASE => {
                if let Some(name) = self.menu_anims.get(i - CMD_ANIM_BASE).cloned() {
                    self.start_overlay(&name, Kind::Manual);
                }
            }
            _ => {}
        }
        None
    }
}

fn open_path(p: &std::path::Path) {
    let (op, f) = (wide("open"), wide(&p.display().to_string()));
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), f.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

/// 启动（或切到）Claude 桌面版。已经开着时，Windows 会把现有窗口带到前台。
fn open_claude(custom: &str) -> bool {
    let shell_open = desktop::shell_open;
    if !custom.trim().is_empty() {
        return shell_open(custom.trim());
    }
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    // 微软商店 / MSIX 版：装了才会有这个数据目录
    if std::path::Path::new(&local).join("Packages").join("Claude_pzs8sxrjxfjjc").is_dir()
        && shell_open(r"shell:AppsFolder\Claude_pzs8sxrjxfjjc!Claude")
    {
        return true;
    }
    // 官网安装包版
    let exe = std::path::Path::new(&local).join("AnthropicClaude").join("claude.exe");
    exe.is_file() && shell_open(&exe.display().to_string())
}

fn autostart_enabled() -> bool {
    let (k, v) = (wide(RUN_KEY), wide(RUN_VALUE));
    let mut size = 0u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut size)
            == ERROR_SUCCESS
    }
}

fn set_autostart(on: bool) {
    let (k, v) = (wide(RUN_KEY), wide(RUN_VALUE));
    unsafe {
        if on {
            let data = wide(&format!("\"{}\"", config::exe_path().display()));
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                k.as_ptr(),
                v.as_ptr(),
                REG_SZ,
                data.as_ptr() as *const _,
                (data.len() * 2) as u32,
            );
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr());
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let handled: Option<LRESULT> = match msg {
        WM_TIMER if wp == TIMER_FRAME => with_app(|a| a.tick()).map(|_| 0),
        WM_TIMER if wp == TIMER_HOUSE => with_app(|a| a.housekeeping()).map(|_| 0),
        WM_TIMER if wp == TIMER_SEND => {
            if let Some(Some(text)) = with_app(|a| a.send_tick()) {
                msgbox(hwnd, &text, MB_OK | MB_ICONWARNING);
            }
            Some(0)
        }
        chat::WM_CHAT_SEND => {
            if lp != 0 {
                let req = *Box::from_raw(lp as *mut chat::SendReq);
                with_app(|a| a.start_send(req));
            }
            Some(0)
        }
        WM_LBUTTONDBLCLK => {
            if chat::is_open() {
                chat::close(true);
            } else if let Some((anchor, items)) = with_app(|a| (a.anchor_rect(), a.chat_items())) {
                chat::open(hwnd, anchor, items);
            }
            Some(0)
        }
        WM_HOOK_EVENT => with_app(|a| a.handle_events()).map(|_| 0),
        WM_SHOW_PET => with_app(|a| a.show()).map(|_| 0),
        WM_LBUTTONDOWN => with_app(|a| {
            let mut c = POINT { x: 0, y: 0 };
            GetCursorPos(&mut c);
            a.drag = Some(Drag { start_cursor: c, start_pos: a.pos, moved: false, last_x: c.x, acc: 0, dir: 0 });
            SetCapture(hwnd);
            0
        }),
        WM_MOUSEMOVE => with_app(|a| {
            let Some(d) = a.drag.as_mut() else { return 0 };
            let mut c = POINT { x: 0, y: 0 };
            GetCursorPos(&mut c);
            let (dx, dy) = (c.x - d.start_cursor.x, c.y - d.start_cursor.y);
            if !d.moved && dx.abs() + dy.abs() < 5 {
                return 0;
            }
            let first = !d.moved;
            d.moved = true;
            // 朝向：同一方向累计移动超过 6 像素才换，避免手抖来回闪
            let step = c.x - d.last_x;
            d.last_x = c.x;
            if step != 0 && (step > 0) != (d.acc > 0) {
                d.acc = 0;
            }
            d.acc += step;
            let want = if d.acc >= 6 { 1 } else if d.acc <= -6 { -1 } else { d.dir };
            let turn = first || want != d.dir;
            d.dir = want;
            a.pos = (d.start_pos.0 + dx, d.start_pos.1 + dy);
            if turn {
                let key = match want {
                    1 => "drag_right",
                    -1 => "drag_left",
                    _ => "drag",
                };
                let pool = a
                    .pack
                    .states
                    .get(key)
                    .filter(|p| !p.is_empty())
                    .or_else(|| a.pack.states.get("drag"))
                    .cloned()
                    .unwrap_or_default();
                let playing = a.overlay.as_ref().filter(|o| o.kind == Kind::Drag).map(|o| o.name.clone());
                if playing.as_ref().map(|n| !pool.contains(n)).unwrap_or(true) {
                    if let Some(name) = a.pick(&pool, None) {
                        a.start_overlay(&name, Kind::Drag);
                    }
                }
            }
            SetWindowPos(hwnd, std::ptr::null_mut(), a.pos.0, a.pos.1, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            chat::follow(a.anchor_rect());
            0
        }),
        WM_LBUTTONUP => with_app(|a| {
            ReleaseCapture();
            if let Some(d) = a.drag.take() {
                if d.moved {
                    if a.overlay.as_ref().map(|o| o.kind == Kind::Drag).unwrap_or(false) {
                        a.overlay = None;
                    }
                    a.cfg.x = Some(a.pos.0);
                    a.cfg.y = Some(a.pos.1);
                    config::save(&a.cfg);
                    a.reset_frame_timer();
                    a.render();
                } else {
                    let pool = a.pack.states.get("click").cloned().unwrap_or_default();
                    if let Some(name) = a.pick(&pool, None) {
                        a.start_overlay(&name, Kind::Click);
                    }
                }
            }
            0
        }),
        WM_RBUTTONUP => {
            // 菜单是模态循环，期间还会收到定时器消息：建菜单和处理命令分两次借用
            if let Some(menu) = with_app(|a| a.build_menu()) {
                let mut c = POINT { x: 0, y: 0 };
                GetCursorPos(&mut c);
                SetForegroundWindow(hwnd);
                let id = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, c.x, c.y, 0, hwnd, std::ptr::null());
                DestroyMenu(menu);
                PostMessageW(hwnd, WM_NULL, 0, 0);
                if id as usize == CMD_CHAT {
                    if let Some((anchor, items)) = with_app(|a| (a.anchor_rect(), a.chat_items())) {
                        chat::open(hwnd, anchor, items);
                    }
                } else if id > 0 {
                    if let Some(Some((text, icon))) = with_app(|a| a.command(id as usize)) {
                        msgbox(hwnd, &text, MB_OK | icon);
                    }
                }
            }
            Some(0)
        }
        WM_DPICHANGED => with_app(|a| a.render()).map(|_| 0),
        WM_DESTROY => {
            with_app(|a| {
                a.cfg.x = Some(a.pos.0);
                a.cfg.y = Some(a.pos.1);
                config::save(&a.cfg);
                a.free_dib();
            });
            PostQuitMessage(0);
            Some(0)
        }
        _ => None,
    };
    handled.unwrap_or_else(|| DefWindowProcW(hwnd, msg, wp, lp))
}

fn initial_pos(cfg: &AppConfig, w: i32, h: i32) -> (i32, i32) {
    unsafe {
        if let (Some(x), Some(y)) = (cfg.x, cfg.y) {
            let p = POINT { x: x + w / 2, y: y + h / 2 };
            if !MonitorFromPoint(p, MONITOR_DEFAULTTONULL).is_null() {
                return (x, y);
            }
        }
        let mut wa = RECT { left: 0, top: 0, right: 1280, bottom: 720 };
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut RECT as *mut _, 0);
        (wa.right - w - 40, wa.bottom - h)
    }
}

pub fn run() {
    unsafe {
        let name = wide(hook::MUTEX_NAME);
        let mutex = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // 已经在运行（可能被隐藏了）：叫它现身
            let class = wide("ClaudePetNativeWindow");
            let other = FindWindowW(class.as_ptr(), std::ptr::null());
            if !other.is_null() {
                PostMessageW(other, WM_SHOW_PET, 0, 0);
            }
            return;
        }
        let _keep = mutex;
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let mut cfg = config::load();
        if cfg.quit_by_user {
            // 你手动打开了它：恢复 hook 自动拉起
            cfg.quit_by_user = false;
            config::save(&cfg);
        }
        let pack = match Pack::load(&config::base_dir().join("pets").join(&cfg.pet)) {
            Ok(p) => p,
            Err(e) => {
                msgbox(std::ptr::null_mut(), &format!("动画包加载失败：\n{e}"), MB_OK | MB_ICONERROR);
                return;
            }
        };

        let hinst = GetModuleHandleW(std::ptr::null());
        let class = wide("ClaudePetNativeWindow");
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_HAND),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let title = wide("Claude 桌宠");
        let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | if cfg.topmost { WS_EX_TOPMOST } else { 0 };
        let hwnd = CreateWindowExW(
            ex,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }

        // 接收 hook 事件的 UDP 线程
        let queue: Arc<Mutex<VecDeque<HookMsg>>> = Arc::new(Mutex::new(VecDeque::new()));
        let mut udp_error = None;
        match UdpSocket::bind(("127.0.0.1", cfg.port)) {
            Ok(sock) => {
                let q = queue.clone();
                let hw = hwnd as isize;
                std::thread::spawn(move || {
                    let mut buf = vec![0u8; 65536];
                    loop {
                        let Ok((n, _)) = sock.recv_from(&mut buf) else { continue };
                        if let Ok(m) = serde_json::from_slice::<HookMsg>(&buf[..n]) {
                            if let Ok(mut g) = q.lock() {
                                g.push_back(m);
                            }
                            PostMessageW(hw as HWND, WM_HOOK_EVENT, 0, 0);
                        }
                    }
                });
            }
            Err(e) => udp_error = Some(format!("⚠ 端口 {} 被占用，收不到 hook 事件：{e}", cfg.port)),
        }

        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1;
        let mut app = App {
            hwnd,
            cfg,
            pack,
            cache: Vec::new(),
            store: Store::default(),
            shown: None,
            shown_since: Instant::now(),
            base: None,
            overlay: None,
            next_idle_extra: Instant::now(),
            dib: None,
            pos: (0, 0),
            size: (0, 0),
            drag: None,
            queue,
            rng: seed,
            frame_ms: 0,
            last_reap: Instant::now(),
            menu_anims: Vec::new(),
            menu_pets: Vec::new(),
            udp_error,
            hidden: false,
            pending: None,
        };
        app.start_base(None, None);
        app.schedule_idle_extra();
        if let Some(p) = app.current() {
            let s = app.eff_scale();
            let (w, h) = ((p.sheet.fw as f32 * s) as i32, (p.sheet.fh as f32 * s) as i32);
            app.pos = initial_pos(&app.cfg, w, h);
        }
        log_line(&app.cfg, &format!("—— 桌宠启动，动画包「{}」，{} 段动画 ——", app.pack.name, app.pack.anims.len()));
        app.reset_frame_timer();
        app.render();
        let ask_hooks = !app.cfg.asked_hooks && !install::is_installed();
        APP.with(|a| *a.borrow_mut() = Some(app));

        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(hwnd, TIMER_HOUSE, 500, None);

        if ask_hooks {
            let r = msgbox(
                hwnd,
                "要把桌宠接入 Claude Code 吗？\n\n会在 ~/.claude/settings.json 里加入 hooks（先自动备份），\n之后新开的 Claude Code 会话就会驱动桌宠的动作。\n\n以后也可以在右键菜单里随时接入或断开。",
                MB_YESNO | MB_ICONQUESTION,
            );
            with_app(|a| {
                a.cfg.asked_hooks = true;
                config::save(&a.cfg);
            });
            if r == IDYES {
                let (text, icon) = match install::install() {
                    Ok(s) => (s, MB_ICONINFORMATION),
                    Err(e) => (e, MB_ICONERROR),
                };
                msgbox(hwnd, &text, MB_OK | icon);
            }
        }

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 调试用：打印当前已解码动画占的内存
#[allow(dead_code)]
pub fn cache_bytes() -> usize {
    APP.with(|a| a.borrow().as_ref().map(|a| a.cache.iter().map(|(_, s)| s.bytes()).sum()).unwrap_or(0))
}
