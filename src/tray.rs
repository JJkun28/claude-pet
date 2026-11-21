//! 通知区域（托盘）图标：左键显示 / 隐藏桌宠，右键弹出和桌宠一样的菜单，悬停显示当前状态。
//! 图标不打包进 exe，而是用当前宠物待机动画的第一帧现场生成，换宠物时跟着换。

use crate::hook::wide;
use crate::pack::Sheet;
use std::cell::Cell;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// 托盘图标的回调消息；lParam 是鼠标消息（WM_LBUTTONUP 等）
pub const WM_TRAY: u32 = WM_APP + 4;
const ID: u32 = 1;

thread_local! {
    static ICON: Cell<HICON> = const { Cell::new(std::ptr::null_mut()) };
}

/// 资源管理器重启后会广播这个消息，需要重新添加图标
pub fn taskbar_created() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) })
}

fn data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut d: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    d.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    d.hWnd = hwnd;
    d.uID = ID;
    d
}

fn put_tip(d: &mut NOTIFYICONDATAW, tip: &str) {
    let w: Vec<u16> = tip.encode_utf16().take(d.szTip.len() - 1).collect();
    d.szTip[..w.len()].copy_from_slice(&w);
    d.szTip[w.len()] = 0;
    d.uFlags |= NIF_TIP;
}

/// 换上新图标，销毁旧的
fn swap_icon(new: HICON) -> HICON {
    let old = ICON.with(|c| c.replace(new));
    if !old.is_null() {
        unsafe { DestroyIcon(old) };
    }
    new
}

fn icon_or_default(sheet: Option<&Sheet>) -> HICON {
    let made = sheet.and_then(make_icon).unwrap_or(std::ptr::null_mut());
    if made.is_null() {
        swap_icon(std::ptr::null_mut());
        unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) }
    } else {
        swap_icon(made)
    }
}

pub fn add(hwnd: HWND, sheet: Option<&Sheet>, tip: &str) {
    let mut d = data(hwnd);
    d.uFlags = NIF_MESSAGE | NIF_ICON;
    d.uCallbackMessage = WM_TRAY;
    d.hIcon = icon_or_default(sheet);
    put_tip(&mut d, tip);
    unsafe {
        // 已经有了（例如重新加载配置）就先删掉，保证幂等
        Shell_NotifyIconW(NIM_DELETE, &data(hwnd));
        Shell_NotifyIconW(NIM_ADD, &d);
    }
}

pub fn set_icon(hwnd: HWND, sheet: Option<&Sheet>, tip: &str) {
    let mut d = data(hwnd);
    d.uFlags = NIF_ICON;
    d.hIcon = icon_or_default(sheet);
    put_tip(&mut d, tip);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) };
}

pub fn set_tip(hwnd: HWND, tip: &str) {
    let mut d = data(hwnd);
    put_tip(&mut d, tip);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) };
}

pub fn remove(hwnd: HWND) {
    unsafe { Shell_NotifyIconW(NIM_DELETE, &data(hwnd)) };
    swap_icon(std::ptr::null_mut());
}

/// 取第 0 帧，裁掉透明边、居中成正方形，再缩到小图标尺寸（区域平均），生成 HICON
fn make_icon(sheet: &Sheet) -> Option<HICON> {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64) as usize;
    let px = icon_pixels(sheet, size)?;
    unsafe {
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = size as i32;
        bmi.bmiHeader.biHeight = -(size as i32);
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(std::ptr::null_mut(), &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if color.is_null() || bits.is_null() {
            return None;
        }
        std::slice::from_raw_parts_mut(bits as *mut u32, size * size).copy_from_slice(&px);
        let mask_bytes = vec![0u8; size.div_ceil(16) * 2 * size];
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, mask_bytes.as_ptr() as *const _);
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color);
        DeleteObject(mask);
        (!icon.is_null()).then_some(icon)
    }
}

/// 图标像素（非预乘 BGRA，自上而下，size×size）；整帧透明时返回 None
fn icon_pixels(sheet: &Sheet, size: usize) -> Option<Vec<u32>> {
    let (fw, fh) = (sheet.fw, sheet.fh);
    let mut src = vec![0u32; fw * fh];
    sheet.draw(0, &mut src, fw, fh);

    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..fh {
        for x in 0..fw {
            if src[y * fw + x] >> 24 > 16 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x0 >= x1 {
        return None;
    }
    // 居中的正方形区域（可能超出帧边界，超出部分当透明）
    let side = (x1 - x0).max(y1 - y0) as isize;
    let left = (x0 + x1) as isize / 2 - side / 2;
    let top = (y0 + y1) as isize / 2 - side / 2;

    let mut out = vec![0u32; size * size];
    for dy in 0..size {
        let sy0 = top + (dy as isize * side) / size as isize;
        let sy1 = (top + ((dy as isize + 1) * side) / size as isize).max(sy0 + 1);
        for dx in 0..size {
            let sx0 = left + (dx as isize * side) / size as isize;
            let sx1 = (left + ((dx as isize + 1) * side) / size as isize).max(sx0 + 1);
            let (mut acc, mut n) = ([0u32; 4], 0u32);
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    n += 1;
                    if sy < 0 || sx < 0 || sy >= fh as isize || sx >= fw as isize {
                        continue;
                    }
                    let p = src[sy as usize * fw + sx as usize];
                    for (i, a) in acc.iter_mut().enumerate() {
                        *a += (p >> (i * 8)) & 0xff;
                    }
                }
            }
            // 平均后的预乘值 → 非预乘
            let [b, g, r, a] = acc.map(|v| v / n.max(1));
            let un = |c: u32| if a == 0 { 0 } else { (c * 255 / a).min(255) };
            out[dy * size + dx] = un(b) | (un(g) << 8) | (un(r) << 16) | (a << 24);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::Pack;

    #[test]
    fn icon_from_every_bundled_pack() {
        for dir in ["ghost-squeak", "zyf", "maid"] {
            let pack = Pack::load(&std::path::Path::new("dist/pets").join(dir)).unwrap();
            let idle = &pack.states["idle"][0];
            let sheet = pack.load_sheet(idle).unwrap();
            let px = icon_pixels(&sheet, 32).unwrap();
            assert_eq!(px.len(), 32 * 32);
            let opaque = px.iter().filter(|p| *p >> 24 > 200).count();
            // 裁掉透明边之后，宠物应该占图标的相当一部分
            assert!(opaque > 32 * 32 / 6, "{dir}: 只有 {opaque} 个不透明像素");
        }
    }
}
