//! 对话输入框：桌宠上方（放不下就下方）弹出的小窗口。
//! 第一行是发送目标（点一下换会话），下面是输入框：Enter 发送、Shift+Enter 换行、Esc 或点别处关闭。
//! 按下发送后把请求 PostMessage 给桌宠窗口，由桌宠去切换 Claude 桌面版并打字。

use crate::desktop::DeskSession;
use crate::hook::wide;
use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// 桌宠窗口收到它：lparam 是 Box<SendReq> 的裸指针
pub const WM_CHAT_SEND: u32 = WM_APP + 3;
/// 输入框内部：内容变了，重新算行数
const WM_RELAYOUT: u32 = WM_APP + 10;

const EM_SETSEL: u32 = 177;
const EM_GETLINECOUNT: u32 = 186;
const EM_SETMARGINS: u32 = 211;
const EM_LINESCROLL: u32 = 182;
const EM_SCROLLCARET: u32 = 183;
const EM_GETFIRSTVISIBLELINE: u32 = 206;
const EDIT_ID: usize = 1;
const MAX_LINES: i32 = 5;
const CMD_SESSION_BASE: usize = 100;

pub struct SendReq {
    pub session: Option<DeskSession>,
    pub text: String,
}

/// 菜单和标题行里显示的一项
pub struct Item {
    pub session: DeskSession,
    /// 桌宠记录到的状态（思考中 / 工作中 …）
    pub state: Option<&'static str>,
}

impl Item {
    fn label(&self) -> String {
        let mut s = self.session.title.clone();
        if !self.session.folder.is_empty() {
            s.push_str(" · ");
            s.push_str(&self.session.folder);
        }
        if let Some(st) = self.state {
            s.push_str(" · ");
            s.push_str(st);
        }
        s
    }
}

struct Chat {
    hwnd: HWND,
    edit: HWND,
    owner: HWND,
    items: Vec<Item>,
    sel: usize,
    font: HFONT,
    small: HFONT,
    edit_bg: HBRUSH,
    /// 物理像素
    scale: f32,
    lines: i32,
    line_h: i32,
    /// 桌宠窗口的屏幕矩形
    anchor: RECT,
    menu_open: bool,
}

/// EDIT 控件原来的窗口过程。不放进 CHAT：CHAT 被借用期间（比如 layout 里挪 EDIT、SetFocus）
/// EDIT 也会收到消息，那时也必须能转给原过程，否则 EDIT 收不到 WM_SIZE / WM_SETFOCUS，就打不了字。
static OLD_EDIT_PROC: AtomicIsize = AtomicIsize::new(0);

thread_local! {
    static CHAT: RefCell<Option<Chat>> = const { RefCell::new(None) };
    /// 没发出去就关掉的草稿，下次打开还在
    static DRAFT: RefCell<String> = const { RefCell::new(String::new()) };
}

fn with_chat<R>(f: impl FnOnce(&mut Chat) -> R) -> Option<R> {
    CHAT.with(|c| c.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

pub fn is_open() -> bool {
    CHAT.with(|c| c.try_borrow().map(|g| g.is_some()).unwrap_or(true))
}

fn window_text(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h);
        if n <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..got.max(0) as usize])
    }
}

impl Chat {
    fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }
    fn pad(&self) -> i32 {
        self.px(10.0)
    }
    fn header_h(&self) -> i32 {
        self.px(22.0)
    }
    fn footer_h(&self) -> i32 {
        self.px(18.0)
    }
    fn width(&self) -> i32 {
        self.px(360.0)
    }
    fn edit_box_h(&self) -> i32 {
        // 多留几像素：EDIT 只画能完整放下的行，差一像素最后一行就不显示
        self.lines * self.line_h + self.px(13.0)
    }
    fn height(&self) -> i32 {
        self.pad() + self.header_h() + self.px(4.0) + self.edit_box_h() + self.footer_h() + self.pad() / 2
    }
    fn header_rect(&self) -> RECT {
        let p = self.pad();
        RECT { left: p, top: p, right: self.width() - p, bottom: p + self.header_h() }
    }
    fn edit_box_rect(&self) -> RECT {
        let p = self.pad();
        let top = p + self.header_h() + self.px(4.0);
        RECT { left: p, top, right: self.width() - p, bottom: top + self.edit_box_h() }
    }

    /// 按桌宠位置和当前行数摆放窗口和输入框
    fn layout(&mut self) {
        let (w, h) = (self.width(), self.height());
        let gap = self.px(6.0);
        let a = self.anchor;
        let mut wa = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
        unsafe {
            let mon = MonitorFromWindow(self.owner, MONITOR_DEFAULTTONEAREST);
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(mon, &mut mi) != 0 {
                wa = mi.rcWork;
            }
        }
        // 上方放不下就放到桌宠下方
        let below = a.top - gap - h < wa.top;
        let y = if below { (a.bottom + gap).min(wa.bottom - h) } else { a.top - gap - h };
        let x = ((a.left + a.right) / 2 - w / 2).clamp(wa.left, (wa.right - w).max(wa.left));
        let eb = self.edit_box_rect();
        let m = self.px(5.0);
        unsafe {
            SetWindowPos(self.hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE);
            SetWindowPos(
                self.edit,
                std::ptr::null_mut(),
                eb.left + m,
                eb.top + m,
                eb.right - eb.left - 2 * m,
                eb.bottom - eb.top - 2 * m,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            InvalidateRect(self.hwnd, std::ptr::null(), 1);
            InvalidateRect(self.edit, std::ptr::null(), 1);
        }
    }

    fn paint(&self) {
        unsafe {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(self.hwnd, &mut ps);
            let mut rc = RECT { left: 0, top: 0, right: self.width(), bottom: self.height() };
            let bg = CreateSolidBrush(rgb(255, 255, 255));
            FillRect(dc, &rc, bg);
            DeleteObject(bg);
            let border = CreateSolidBrush(rgb(200, 200, 206));
            FrameRect(dc, &rc, border);
            let eb = self.edit_box_rect();
            FillRect(dc, &eb, self.edit_bg);
            FrameRect(dc, &eb, border);
            DeleteObject(border);
            SetBkMode(dc, TRANSPARENT as i32);

            // 标题行：→ 会话名 · 文件夹 · 状态 ▾
            let old = SelectObject(dc, self.font);
            let mut hr = self.header_rect();
            let arrow = wide("▾");
            SetTextColor(dc, rgb(120, 120, 128));
            let mut ar = hr;
            DrawTextW(dc, arrow.as_ptr(), -1, &mut ar, DT_RIGHT | DT_VCENTER | DT_SINGLELINE);
            hr.right -= self.px(16.0);
            let label = match self.items.get(self.sel) {
                Some(it) => format!("发送到：{}", it.label()),
                None => "发送到：最近的会话（读不到桌面版的会话列表）".into(),
            };
            let t = wide(&label);
            SetTextColor(dc, rgb(40, 40, 48));
            DrawTextW(dc, t.as_ptr(), -1, &mut hr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);

            // 底部提示
            SelectObject(dc, self.small);
            rc.left = self.pad();
            rc.right = self.width() - self.pad();
            rc.top = eb.bottom;
            rc.bottom = eb.bottom + self.footer_h();
            let hint = if self.items.len() > 1 {
                "Enter 发送 · Shift+Enter 换行 · ↑↓ 或点标题换会话 · Esc 关闭"
            } else {
                "Enter 发送 · Shift+Enter 换行 · Esc 关闭"
            };
            let t = wide(hint);
            SetTextColor(dc, rgb(150, 150, 158));
            DrawTextW(dc, t.as_ptr(), -1, &mut rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
            SelectObject(dc, old);
            EndPaint(self.hwnd, &ps);
        }
    }

    fn step_sel(&mut self, d: i32) {
        let n = self.items.len() as i32;
        if n > 1 {
            self.sel = ((self.sel as i32 + d).rem_euclid(n)) as usize;
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 1) };
        }
    }

    fn update_lines(&mut self) {
        let total = unsafe { SendMessageW(self.edit, EM_GETLINECOUNT, 0, 0) } as i32;
        let n = total.clamp(1, MAX_LINES);
        if n != self.lines {
            self.lines = n;
            self.layout();
        }
        unsafe {
            // 字刚折到新一行时，输入框还没长高，EDIT 为了让光标可见已经把第一行滚上去了；
            // 长高后要滚回顶部，否则前面的字看不见。超过 MAX_LINES 行才真的需要滚动。
            let first = SendMessageW(self.edit, EM_GETFIRSTVISIBLELINE, 0, 0) as i32;
            if total <= MAX_LINES {
                if first > 0 {
                    SendMessageW(self.edit, EM_LINESCROLL, 0, -(first as isize) as LPARAM);
                }
            } else {
                SendMessageW(self.edit, EM_SCROLLCARET, 0, 0);
            }
        }
    }
}

/// 弹出会话菜单（在 CHAT 借用之外调用，菜单是模态循环）
fn session_menu(hwnd: HWND) {
    let Some((labels, sel, wr, above_pet)) = with_chat(|c| {
        c.menu_open = true;
        let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetWindowRect(c.hwnd, &mut wr) };
        let above_pet = wr.bottom <= c.anchor.top + 1;
        (c.items.iter().map(|i| i.label()).collect::<Vec<_>>(), c.sel, wr, above_pet)
    }) else {
        return;
    };
    unsafe {
        let m = CreatePopupMenu();
        if labels.is_empty() {
            let t = wide("读不到桌面版的会话列表，会发到最近的会话");
            AppendMenuW(m, MF_STRING | MF_GRAYED, 0, t.as_ptr());
        }
        for (i, l) in labels.iter().enumerate() {
            let t = wide(l);
            let chk = if i == sel { MF_CHECKED } else { 0 };
            AppendMenuW(m, MF_STRING | chk, CMD_SESSION_BASE + i, t.as_ptr());
        }
        // 菜单贴着输入框外沿弹出，并把输入框整块设为禁区，系统放不下时会翻到另一边，但不会盖住输入框。
        // 输入框在桌宠上方 → 菜单往上弹（不挡桌宠）；在下方 → 往下弹。
        let tp = TPMPARAMS { cbSize: std::mem::size_of::<TPMPARAMS>() as u32, rcExclude: wr };
        let (y, align) = if above_pet { (wr.top, TPM_BOTTOMALIGN) } else { (wr.bottom, TPM_TOPALIGN) };
        let id = TrackPopupMenuEx(m, TPM_RETURNCMD | TPM_NONOTIFY | TPM_VERTICAL | align, wr.left, y, hwnd, &tp);
        DestroyMenu(m);
        with_chat(|c| {
            c.menu_open = false;
            if id as usize >= CMD_SESSION_BASE && (id as usize - CMD_SESSION_BASE) < c.items.len() {
                c.sel = id as usize - CMD_SESSION_BASE;
                InvalidateRect(c.hwnd, std::ptr::null(), 1);
            }
            SetFocus(c.edit);
        });
    }
}

/// 关闭输入框；keep_draft = 没发出去的字留着下次用
pub fn close(keep_draft: bool) {
    let taken = CHAT.with(|c| c.try_borrow_mut().ok().and_then(|mut g| g.take()));
    if let Some(c) = taken {
        let text = window_text(c.edit);
        DRAFT.with(|d| *d.borrow_mut() = if keep_draft { text } else { String::new() });
        unsafe {
            DestroyWindow(c.hwnd);
            DeleteObject(c.font);
            DeleteObject(c.small);
            DeleteObject(c.edit_bg);
        }
    }
}

fn submit() {
    let req = with_chat(|c| {
        let text = window_text(c.edit);
        let text = text.trim().to_string();
        (!text.is_empty()).then(|| (c.owner, SendReq { session: c.items.get(c.sel).map(|i| i.session.clone()), text }))
    })
    .flatten();
    if let Some((owner, req)) = req {
        close(false);
        let p = Box::into_raw(Box::new(req));
        unsafe {
            if PostMessageW(owner, WM_CHAT_SEND, 0, p as LPARAM) == 0 {
                drop(Box::from_raw(p));
            }
        }
    }
}

unsafe extern "system" fn edit_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let old = OLD_EDIT_PROC.load(Ordering::Relaxed);
    let shift = GetKeyState(VK_SHIFT as i32) < 0;
    match msg {
        WM_KEYDOWN if wp == VK_RETURN as usize && !shift => {
            submit();
            return 0;
        }
        WM_KEYDOWN if wp == VK_ESCAPE as usize => {
            close(true);
            return 0;
        }
        WM_KEYDOWN if (wp == VK_UP as usize || wp == VK_DOWN as usize) => {
            // 只有一行时 ↑↓ 用来换会话，多行时照常移动光标
            if SendMessageW(hwnd, EM_GETLINECOUNT, 0, 0) <= 1 {
                with_chat(|c| c.step_sel(if wp == VK_UP as usize { -1 } else { 1 }));
                return 0;
            }
        }
        // 吞掉 Enter / Esc 对应的字符，免得多出一个换行或响一声
        WM_CHAR if (wp == '\r' as usize && !shift) || wp == 0x1b => return 0,
        _ => {}
    }
    if old == 0 {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let prev: WNDPROC = std::mem::transmute::<isize, WNDPROC>(old);
    CallWindowProcW(prev, hwnd, msg, wp, lp)
}

unsafe extern "system" fn chat_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            if with_chat(|c| c.paint()).is_some() {
                return 0;
            }
        }
        WM_ERASEBKGND => return 1,
        WM_LBUTTONDOWN => {
            let pt = POINT { x: (lp & 0xffff) as i16 as i32, y: ((lp >> 16) & 0xffff) as i16 as i32 };
            let in_header = with_chat(|c| {
                let r = c.header_rect();
                pt.x >= r.left && pt.x < r.right && pt.y >= r.top - c.pad() / 2 && pt.y < r.bottom
            })
            .unwrap_or(false);
            if in_header {
                session_menu(hwnd);
            } else {
                with_chat(|c| SetFocus(c.edit));
            }
            return 0;
        }
        WM_SETCURSOR => {
            let mut c = POINT { x: 0, y: 0 };
            GetCursorPos(&mut c);
            ScreenToClient(hwnd, &mut c);
            let hand = with_chat(|ch| {
                let r = ch.header_rect();
                c.x >= r.left && c.x < r.right && c.y >= r.top && c.y < r.bottom
            })
            .unwrap_or(false);
            if hand && (lp & 0xffff) as u32 == HTCLIENT {
                SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_HAND));
                return 1;
            }
        }
        WM_COMMAND if (wp & 0xffff) == EDIT_ID && ((wp >> 16) & 0xffff) as u32 == EN_CHANGE => {
            // EN_CHANGE 发出后 EDIT 还会再把光标滚进视野，所以等它这一轮处理完再调高度、滚回顶部
            PostMessageW(hwnd, WM_RELAYOUT, 0, 0);
            return 0;
        }
        WM_RELAYOUT => {
            with_chat(|c| c.update_lines());
            return 0;
        }
        WM_CTLCOLOREDIT => {
            if let Some(br) = with_chat(|c| c.edit_bg) {
                let dc = wp as HDC;
                SetBkColor(dc, rgb(246, 246, 248));
                SetTextColor(dc, rgb(30, 30, 36));
                return br as LRESULT;
            }
        }
        WM_ACTIVATE => {
            if (wp & 0xffff) as u32 == WA_INACTIVE {
                // 点到别处就关；自己弹的会话菜单不算
                let menu = with_chat(|c| c.menu_open).unwrap_or(true);
                if !menu {
                    close(true);
                }
            } else {
                with_chat(|c| SetFocus(c.edit));
            }
            return 0;
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// 打开输入框。anchor = 桌宠窗口的屏幕矩形；items 已按最近活动排好，第一个是默认目标。
pub fn open(owner: HWND, anchor: RECT, items: Vec<Item>) {
    if is_open() {
        with_chat(|c| unsafe {
            SetForegroundWindow(c.hwnd);
            SetFocus(c.edit);
        });
        return;
    }
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let class = wide("ClaudePetChatWindow");
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(chat_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc); // 第二次注册会失败，无所谓
        let title = wide("和 Claude 说话");
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_CLIPCHILDREN, // 画背景时绕开输入框，免得盖住它
            0,
            0,
            10,
            10,
            owner,
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }
        let scale = GetDpiForWindow(owner).max(96) as f32 / 96.0;
        let face = wide("Microsoft YaHei UI");
        let mk = |pt: f32| {
            CreateFontW(-((pt * scale).round() as i32), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5 /* CLEARTYPE_QUALITY */, 0, face.as_ptr())
        };
        let font = mk(14.0);
        let small = mk(11.5);
        let edit_cls = wide("EDIT");
        let style = WS_CHILD | WS_VISIBLE | (ES_MULTILINE | ES_AUTOVSCROLL) as u32;
        let edit = CreateWindowExW(
            0,
            edit_cls.as_ptr(),
            std::ptr::null(),
            style,
            0,
            0,
            10,
            10,
            hwnd,
            EDIT_ID as HMENU,
            hinst,
            std::ptr::null(),
        );
        SendMessageW(edit, WM_SETFONT, font as WPARAM, 1);
        SendMessageW(edit, EM_SETMARGINS, (EC_LEFTMARGIN | EC_RIGHTMARGIN) as WPARAM, 0);
        // 行高 = 字体高度
        let dc = GetDC(edit);
        let old = SelectObject(dc, font);
        let mut tm: TEXTMETRICW = std::mem::zeroed();
        GetTextMetricsW(dc, &mut tm);
        SelectObject(dc, old);
        ReleaseDC(edit, dc);
        let old_proc = SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as *const () as isize);
        OLD_EDIT_PROC.store(old_proc, Ordering::Relaxed);

        let chat = Chat {
            hwnd,
            edit,
            owner,
            items,
            sel: 0,
            font,
            small,
            edit_bg: CreateSolidBrush(rgb(246, 246, 248)),
            scale,
            lines: 1,
            line_h: (tm.tmHeight + tm.tmExternalLeading).max(8),
            anchor,
            menu_open: false,
        };
        CHAT.with(|c| *c.borrow_mut() = Some(chat));

        let draft = DRAFT.with(|d| std::mem::take(&mut *d.borrow_mut()));
        if !draft.is_empty() {
            let t = wide(&draft);
            SetWindowTextW(edit, t.as_ptr());
            SendMessageW(edit, EM_SETSEL, 0, -1isize as LPARAM);
        }
        // 先按真实宽度摆好 EDIT，再数行数（10×10 时数出来的行数不对）
        with_chat(|c| {
            c.layout();
            c.update_lines();
        });
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetFocus(edit);
    }
}

/// 桌宠挪动或改大小时，输入框跟着走
pub fn follow(anchor: RECT) {
    with_chat(|c| {
        if c.anchor.left != anchor.left
            || c.anchor.top != anchor.top
            || c.anchor.right != anchor.right
            || c.anchor.bottom != anchor.bottom
        {
            c.anchor = anchor;
            c.layout();
        }
    });
}
