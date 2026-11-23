use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, GetDC,
    GetTextExtentPoint32W, InvalidateRect, LineTo, MoveToEx, ReleaseDC, RoundRect, SelectObject,
    SetBkMode, SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_PITCH, DT_END_ELLIPSIS, DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, FF_DONTCARE,
    FW_BOLD, FW_NORMAL, FW_SEMIBOLD, HBRUSH, HDC, HFONT, HPEN, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    PS_SOLID, SRCCOPY, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW,
    IsWindowVisible, LoadCursorW, RegisterClassW, SetCursor, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, SystemParametersInfoW, GWLP_USERDATA,
    HWND_TOPMOST, IDC_ARROW, IDC_HAND, SPI_GETWORKAREA, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE,
    WA_INACTIVE, WM_ACTIVATE, WM_CREATE, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_NCHITTEST, WM_PAINT, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::model::{AppState, DeviceState, DeviceStatusKind};

const WM_MOUSELEAVE: u32 = 0x02A3;

const MIN_WIDTH_DIP: i32 = 252;
const MAX_WIDTH_DIP: i32 = 340;
const TOP_PAD_DIP: i32 = 12;
const ROW_H_DIP: i32 = 44;
const FOOTER_H_DIP: i32 = 26;
const BOTTOM_PAD_DIP: i32 = 6;

const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWCP_ROUND: u32 = 2;

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn calc_height_dip(device_count: usize) -> i32 {
    TOP_PAD_DIP + (device_count.max(1) as i32) * ROW_H_DIP + FOOTER_H_DIP + BOTTOM_PAD_DIP
}

fn is_dark_theme() -> bool {
    let subkey = to_wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let val = to_wide("AppsUseLightTheme");
    let mut hkey: HKEY = null_mut();
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) } != 0 {
        return true;
    }
    let mut reg_type = 0u32;
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    let q = unsafe {
        RegQueryValueExW(
            hkey,
            val.as_ptr(),
            null_mut(),
            &mut reg_type,
            &mut data as *mut u32 as *mut u8,
            &mut size,
        )
    };
    unsafe { RegCloseKey(hkey) };
    if q == 0 && reg_type == REG_DWORD {
        data == 0
    } else {
        true
    }
}

struct ThemePalette {
    bg: u32,
    border: u32,
    divider: u32,
    text_primary: u32,
    text_muted: u32,
    text_hover: u32,
    track_bg: u32,
}

impl ThemePalette {
    fn current() -> Self {
        if is_dark_theme() {
            Self {
                bg: rgb(26, 28, 34),
                border: rgb(56, 60, 72),
                divider: rgb(42, 46, 56),
                text_primary: rgb(242, 244, 248),
                text_muted: rgb(148, 156, 170),
                text_hover: rgb(226, 232, 240),
                track_bg: rgb(46, 50, 62),
            }
        } else {
            Self {
                bg: rgb(250, 251, 253),
                border: rgb(216, 220, 228),
                divider: rgb(232, 235, 242),
                text_primary: rgb(22, 24, 30),
                text_muted: rgb(104, 112, 126),
                text_hover: rgb(24, 26, 32),
                track_bg: rgb(224, 228, 236),
            }
        }
    }
}

fn accent_color(dev: &DeviceState, low_threshold: u8) -> u32 {
    match dev.status {
        DeviceStatusKind::Disconnected => rgb(107, 114, 128),
        DeviceStatusKind::Sleeping => rgb(100, 116, 139),
        DeviceStatusKind::Active => {
            if dev.charging {
                rgb(56, 189, 248)
            } else {
                let pct = dev.percent.unwrap_or(0);
                if pct <= low_threshold {
                    rgb(239, 68, 68)
                } else if pct <= low_threshold.saturating_add(20) {
                    rgb(245, 158, 11)
                } else {
                    rgb(34, 197, 94)
                }
            }
        }
    }
}

fn device_meta_text(dev: &DeviceState) -> String {
    match dev.status {
        DeviceStatusKind::Disconnected => "Offline".to_string(),
        DeviceStatusKind::Sleeping => match dev.mode {
            Some(m) => format!("{} · Sleep", m.short_label()),
            None => "Sleep".to_string(),
        },
        DeviceStatusKind::Active => dev
            .mode
            .map(|m| m.short_label().to_string())
            .unwrap_or_default(),
    }
}

fn device_pct_text(dev: &DeviceState) -> String {
    match (dev.status, dev.percent) {
        (DeviceStatusKind::Disconnected, _) => "--".to_string(),
        (_, Some(p)) if dev.charging => format!("⚡{}%", p),
        (_, Some(p)) => format!("{}%", p),
        (_, None) if dev.charging => "⚡".to_string(),
        (_, None) => "--".to_string(),
    }
}

fn format_updated_label(last_poll: Option<Instant>) -> String {
    let Some(t) = last_poll else {
        return "Checking...".to_string();
    };
    let secs = t.elapsed().as_secs();
    if secs < 15 {
        "Just now".to_string()
    } else if secs < 60 {
        format!("{}s ago", secs)
    } else {
        format!("{}m ago", secs / 60)
    }
}

struct PopupContext {
    state: Arc<Mutex<AppState>>,
    on_refresh: Box<dyn Fn()>,
    refresh_rect: RECT,
    hover_refresh: bool,
    tracking_leave: bool,
}

pub struct PopupWindow {
    hwnd: HWND,
    state: Arc<Mutex<AppState>>,
}

impl PopupWindow {
    pub fn create(state: Arc<Mutex<AppState>>, on_refresh: Box<dyn Fn()>) -> Option<Self> {
        unsafe {
            let hinst = GetModuleHandleW(null());
            let class_name = to_wide("AulaBatteryPopupClass");
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(popup_wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinst,
                hIcon: null_mut(),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                hbrBackground: null_mut(),
                lpszMenuName: null(),
                lpszClassName: class_name.as_ptr(),
            };
            RegisterClassW(&wc);

            let count = state.lock().unwrap().devices.len();
            let ctx = Box::new(PopupContext {
                state: Arc::clone(&state),
                on_refresh,
                refresh_rect: RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
                hover_refresh: false,
                tracking_leave: false,
            });
            let ctx_ptr = Box::into_raw(ctx);

            let title = to_wide("Aula Battery");
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                0,
                0,
                MIN_WIDTH_DIP,
                calc_height_dip(count),
                null_mut(),
                null_mut(),
                hinst,
                ctx_ptr as *const c_void,
            );

            if hwnd.is_null() {
                let _ = Box::from_raw(ctx_ptr);
                return None;
            }

            let corner = DWMWCP_ROUND;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner as *const u32 as *const c_void,
                size_of::<u32>() as u32,
            );

            Some(Self { hwnd, state })
        }
    }

    unsafe fn position_and_size(&self, show: bool) {
        let devices = {
            let st = self.state.lock().unwrap();
            st.devices.clone()
        };
        let dpi = GetDpiForWindow(self.hwnd).max(96) as i32;
        let w = compute_width_px(self.hwnd, &devices, dpi);
        let h = (calc_height_dip(devices.len()) * dpi) / 96;
        let margin = (12 * dpi) / 96;

        let mut work = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work as *mut RECT as *mut c_void, 0);

        let x = (work.right - w - margin).max(work.left + margin);
        let y = (work.bottom - h - margin).max(work.top + margin);

        let dark: u32 = if is_dark_theme() { 1 } else { 0 };
        DwmSetWindowAttribute(
            self.hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const u32 as *const c_void,
            size_of::<u32>() as u32,
        );

        let flags = if show { SWP_SHOWWINDOW } else { SWP_NOACTIVATE };
        SetWindowPos(self.hwnd, HWND_TOPMOST, x, y, w, h, flags);
    }

    pub fn toggle(&self) {
        unsafe {
            if IsWindowVisible(self.hwnd) != 0 {
                ShowWindow(self.hwnd, SW_HIDE);
                return;
            }
            self.position_and_size(true);
            SetForegroundWindow(self.hwnd);
            InvalidateRect(self.hwnd, null(), 0);
        }
    }

    pub fn redraw_if_visible(&self) {
        unsafe {
            if IsWindowVisible(self.hwnd) != 0 {
                self.position_and_size(false);
                InvalidateRect(self.hwnd, null(), 0);
            }
        }
    }
}

impl Drop for PopupWindow {
    fn drop(&mut self) {
        unsafe {
            if !self.hwnd.is_null() {
                let ptr = GetWindowLongPtrW(self.hwnd, GWLP_USERDATA) as *mut PopupContext;
                SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
                DestroyWindow(self.hwnd);
                if !ptr.is_null() {
                    drop(Box::from_raw(ptr));
                }
                self.hwnd = null_mut();
            }
        }
    }
}

unsafe fn compute_width_px(hwnd: HWND, devices: &[DeviceState], dpi: i32) -> i32 {
    let scale = |v: i32| (v * dpi) / 96;
    let min_w = scale(MIN_WIDTH_DIP);
    let max_w = scale(MAX_WIDTH_DIP);

    let hdc = GetDC(hwnd);
    if hdc.is_null() {
        return min_w;
    }

    let font_name = make_font(scale(13), FW_SEMIBOLD as i32);
    let font_pct = make_font(scale(14), FW_BOLD as i32);
    let font_meta = make_font(scale(11), FW_NORMAL as i32);

    let pad = scale(14);
    let mut needed = min_w;

    for dev in devices {
        let name_w = measure_text_width(hdc, dev.name, font_name);
        let meta = device_meta_text(dev);
        let meta_w = if meta.is_empty() {
            0
        } else {
            scale(6) + measure_text_width(hdc, &meta, font_meta)
        };
        let pct_w = measure_text_width(hdc, &device_pct_text(dev), font_pct).max(scale(42));
        let row_w = pad * 2 + name_w + meta_w + scale(12) + pct_w;
        needed = needed.max(row_w);
    }

    DeleteObject(font_name);
    DeleteObject(font_pct);
    DeleteObject(font_meta);
    ReleaseDC(hwnd, hdc);

    needed.clamp(min_w, max_w)
}

unsafe fn make_font(height_px: i32, weight: i32) -> HFONT {
    let face = to_wide("Segoe UI");
    CreateFontW(
        -height_px,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        (DEFAULT_PITCH | FF_DONTCARE) as u32,
        face.as_ptr(),
    )
}

unsafe fn fill_round_rect(hdc: HDC, r: &RECT, color: u32, radius: i32) {
    let brush: HBRUSH = CreateSolidBrush(color);
    let pen: HPEN = CreatePen(PS_SOLID, 1, color);
    let old_b = SelectObject(hdc, brush);
    let old_p = SelectObject(hdc, pen);
    RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius, radius);
    SelectObject(hdc, old_b);
    SelectObject(hdc, old_p);
    DeleteObject(brush);
    DeleteObject(pen);
}

unsafe fn draw_hline(hdc: HDC, x0: i32, x1: i32, y: i32, color: u32) {
    let pen: HPEN = CreatePen(PS_SOLID, 1, color);
    let old_p = SelectObject(hdc, pen);
    MoveToEx(hdc, x0, y, null_mut());
    LineTo(hdc, x1, y);
    SelectObject(hdc, old_p);
    DeleteObject(pen);
}

unsafe fn draw_text_rect(hdc: HDC, text: &str, r: &RECT, font: HFONT, color: u32, flags: u32) {
    let wide = to_wide(text);
    let old_f = SelectObject(hdc, font);
    SetTextColor(hdc, color);
    let mut rc = *r;
    DrawTextW(hdc, wide.as_ptr(), (wide.len() - 1) as i32, &mut rc, flags);
    SelectObject(hdc, old_f);
}

unsafe fn measure_text_width(hdc: HDC, text: &str, font: HFONT) -> i32 {
    if text.is_empty() {
        return 0;
    }
    let wide = to_wide(text);
    let old_f = SelectObject(hdc, font);
    let mut sz = SIZE { cx: 0, cy: 0 };
    GetTextExtentPoint32W(hdc, wide.as_ptr(), (wide.len() - 1) as i32, &mut sz);
    SelectObject(hdc, old_f);
    sz.cx
}

struct RowBounds {
    left: i32,
    right: i32,
    top: i32,
}

struct RowFonts {
    name: HFONT,
    pct: HFONT,
    meta: HFONT,
}

unsafe fn draw_device_row(
    hdc: HDC,
    bounds: RowBounds,
    dev: &DeviceState,
    low_threshold: u8,
    palette: &ThemePalette,
    scale: &dyn Fn(i32) -> i32,
    fonts: &RowFonts,
) {
    let left = bounds.left;
    let right = bounds.right;
    let top = bounds.top;

    let pct_text = device_pct_text(dev);
    let pct_w = measure_text_width(hdc, &pct_text, fonts.pct).max(scale(40)) + scale(4);
    let meta_text = device_meta_text(dev);
    let name_w = measure_text_width(hdc, dev.name, fonts.name);
    let min_meta_reserve = if meta_text.is_empty() { 0 } else { scale(36) };

    let name_r = RECT {
        left,
        top,
        right: (left + name_w + scale(2)).min(right - pct_w - min_meta_reserve),
        bottom: top + scale(22),
    };
    draw_text_rect(
        hdc,
        dev.name,
        &name_r,
        fonts.name,
        palette.text_primary,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
    );

    if !meta_text.is_empty() {
        let meta_left = name_r.right + scale(6);
        let meta_right = right - pct_w - scale(4);
        if meta_left < meta_right {
            let meta_r = RECT {
                left: meta_left,
                top: top + scale(1),
                right: meta_right,
                bottom: top + scale(22),
            };
            draw_text_rect(
                hdc,
                &meta_text,
                &meta_r,
                fonts.meta,
                palette.text_muted,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
            );
        }
    }

    let color = accent_color(dev, low_threshold);
    let pct_color = if dev.status == DeviceStatusKind::Disconnected {
        palette.text_muted
    } else {
        color
    };
    let pct_r = RECT {
        left: right - pct_w,
        top,
        right,
        bottom: top + scale(22),
    };
    draw_text_rect(
        hdc,
        &pct_text,
        &pct_r,
        fonts.pct,
        pct_color,
        DT_RIGHT | DT_VCENTER | DT_SINGLELINE,
    );

    // Sleek 4px flat progress bar
    let bar_top = top + scale(26);
    let bar_bottom = bar_top + scale(4);
    let track = RECT {
        left,
        top: bar_top,
        right,
        bottom: bar_bottom,
    };
    fill_round_rect(hdc, &track, palette.track_bg, scale(3));

    let fill_pct = match (dev.status, dev.percent) {
        (DeviceStatusKind::Disconnected, _) => 0,
        (_, Some(p)) => p.min(100) as i32,
        (_, None) if dev.charging => 100,
        _ => 0,
    };
    if fill_pct > 0 {
        let track_w = track.right - track.left;
        let fill_w = ((track_w * fill_pct) / 100).max(scale(4));
        let fill_r = RECT {
            left: track.left,
            top: track.top,
            right: (track.left + fill_w).min(track.right),
            bottom: track.bottom,
        };
        fill_round_rect(hdc, &fill_r, color, scale(3));
    }
}

unsafe fn paint_popup(hwnd: HWND, ctx: &mut PopupContext) {
    let mut ps: PAINTSTRUCT = zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut client = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    GetClientRect(hwnd, &mut client);
    let w = (client.right - client.left).max(1);
    let h = (client.bottom - client.top).max(1);

    let mem_dc = CreateCompatibleDC(hdc);
    let mem_bmp = CreateCompatibleBitmap(hdc, w, h);
    let old_bmp = SelectObject(mem_dc, mem_bmp);
    SetBkMode(mem_dc, TRANSPARENT as i32);

    let dpi = GetDpiForWindow(hwnd).max(96) as i32;
    let scale = |v: i32| (v * dpi) / 96;
    let palette = ThemePalette::current();

    let bg_brush = CreateSolidBrush(palette.bg);
    FillRect(mem_dc, &client, bg_brush);
    DeleteObject(bg_brush);

    // Subtle 1px outer frame
    let border_pen = CreatePen(PS_SOLID, 1, palette.border);
    let old_p = SelectObject(mem_dc, border_pen);
    let null_brush = windows_sys::Win32::Graphics::Gdi::GetStockObject(
        windows_sys::Win32::Graphics::Gdi::NULL_BRUSH,
    );
    let old_b = SelectObject(mem_dc, null_brush);
    windows_sys::Win32::Graphics::Gdi::Rectangle(mem_dc, 0, 0, w, h);
    SelectObject(mem_dc, old_b);
    SelectObject(mem_dc, old_p);
    DeleteObject(border_pen);

    let font_name = make_font(scale(13), FW_SEMIBOLD as i32);
    let font_pct = make_font(scale(14), FW_BOLD as i32);
    let font_meta = make_font(scale(11), FW_NORMAL as i32);
    let fonts = RowFonts {
        name: font_name,
        pct: font_pct,
        meta: font_meta,
    };

    let pad = scale(14);
    let snapshot = ctx.state.lock().unwrap().clone();
    let low_threshold = snapshot.settings.low_threshold;

    let mut y = scale(TOP_PAD_DIP);
    for (idx, dev) in snapshot.devices.iter().enumerate() {
        if idx > 0 {
            draw_hline(mem_dc, pad, w - pad, y - scale(4), palette.divider);
        }
        draw_device_row(
            mem_dc,
            RowBounds {
                left: pad,
                right: w - pad,
                top: y,
            },
            dev,
            low_threshold,
            &palette,
            &scale,
            &fonts,
        );
        y += scale(ROW_H_DIP);
    }

    let footer_top = h - scale(FOOTER_H_DIP + BOTTOM_PAD_DIP);
    let footer_bottom = h - scale(BOTTOM_PAD_DIP);

    let updated_r = RECT {
        left: pad,
        top: footer_top,
        right: w - scale(72),
        bottom: footer_bottom,
    };
    draw_text_rect(
        mem_dc,
        &format_updated_label(snapshot.last_poll),
        &updated_r,
        font_meta,
        palette.text_muted,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE,
    );

    let refresh_r = RECT {
        left: w - pad - scale(56),
        top: footer_top,
        right: w - pad,
        bottom: footer_bottom,
    };
    ctx.refresh_rect = refresh_r;
    let refresh_color = if ctx.hover_refresh {
        palette.text_hover
    } else {
        palette.text_muted
    };
    draw_text_rect(
        mem_dc,
        "Refresh",
        &refresh_r,
        font_meta,
        refresh_color,
        DT_RIGHT | DT_VCENTER | DT_SINGLELINE,
    );

    BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);

    DeleteObject(font_name);
    DeleteObject(font_pct);
    DeleteObject(font_meta);
    SelectObject(mem_dc, old_bmp);
    DeleteObject(mem_bmp);
    DeleteDC(mem_dc);
    EndPaint(hwnd, &ps);
}

fn pt_in_rect(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
}

unsafe extern "system" fn popup_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CREATE {
        let cs = &*(lparam as *const windows_sys::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return 0;
    }

    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PopupContext;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let ctx = &mut *ptr;

    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint_popup(hwnd, ctx);
            0
        }
        WM_MOUSEMOVE => {
            if !ctx.tracking_leave {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                if TrackMouseEvent(&mut tme) != 0 {
                    ctx.tracking_leave = true;
                }
            }
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            let hovering = pt_in_rect(&ctx.refresh_rect, x, y);
            SetCursor(LoadCursorW(
                null_mut(),
                if hovering { IDC_HAND } else { IDC_ARROW },
            ));
            if hovering != ctx.hover_refresh {
                ctx.hover_refresh = hovering;
                InvalidateRect(hwnd, &ctx.refresh_rect, 0);
            }
            0
        }
        WM_MOUSELEAVE => {
            ctx.tracking_leave = false;
            if ctx.hover_refresh {
                ctx.hover_refresh = false;
                InvalidateRect(hwnd, &ctx.refresh_rect, 0);
            }
            0
        }
        WM_LBUTTONUP => {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            if pt_in_rect(&ctx.refresh_rect, x, y) {
                (ctx.on_refresh)();
            }
            0
        }
        WM_ACTIVATE => {
            if (wparam & 0xFFFF) as u32 == WA_INACTIVE {
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_KEYDOWN => {
            if wparam as u16 == VK_ESCAPE {
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_NCHITTEST => 1,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
