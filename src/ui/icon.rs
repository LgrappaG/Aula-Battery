use std::mem::zeroed;
use std::ptr::{copy_nonoverlapping, null_mut};

use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

use crate::model::{DeviceState, DeviceStatusKind};

pub const ICON_SIZE: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Bgra {
    b: u8,
    g: u8,
    r: u8,
    a: u8,
}

impl Bgra {
    const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { b, g, r, a: 255 }
    }
    const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { b, g, r, a }
    }
}

const TRANSPARENT: Bgra = Bgra::rgba(0, 0, 0, 0);
const TRACK_BG: Bgra = Bgra::rgba(18, 22, 28, 175);
const BORDER_NORMAL: Bgra = Bgra::rgba(220, 226, 235, 175);
const BORDER_LOW: Bgra = Bgra::rgba(248, 113, 113, 230);
const BORDER_DIM: Bgra = Bgra::rgba(120, 128, 142, 135);
const OFFLINE_DASH: Bgra = Bgra::rgb(139, 148, 158);

const COLOR_GREEN: Bgra = Bgra::rgb(34, 197, 94);
const COLOR_AMBER: Bgra = Bgra::rgb(245, 158, 11);
const COLOR_RED: Bgra = Bgra::rgb(239, 68, 68);
const COLOR_CYAN: Bgra = Bgra::rgb(56, 189, 248);
const COLOR_SLATE: Bgra = Bgra::rgb(100, 116, 139);
const COLOR_BOLT: Bgra = Bgra::rgb(255, 250, 210);

fn colors_for(dev: &DeviceState, low_threshold: u8) -> (Bgra, Bgra) {
    match dev.status {
        DeviceStatusKind::Disconnected => (TRANSPARENT, BORDER_DIM),
        DeviceStatusKind::Sleeping => (COLOR_SLATE, BORDER_DIM),
        DeviceStatusKind::Active => {
            if dev.charging {
                (COLOR_CYAN, BORDER_NORMAL)
            } else {
                let pct = dev.percent.unwrap_or(0);
                if pct <= low_threshold {
                    (COLOR_RED, BORDER_LOW)
                } else if pct <= low_threshold.saturating_add(20) {
                    (COLOR_AMBER, BORDER_NORMAL)
                } else {
                    (COLOR_GREEN, BORDER_NORMAL)
                }
            }
        }
    }
}

fn set_px(buf: &mut [u8; ICON_SIZE * ICON_SIZE * 4], x: usize, y: usize, c: Bgra) {
    if x >= ICON_SIZE || y >= ICON_SIZE {
        return;
    }
    let idx = (y * ICON_SIZE + x) * 4;
    buf[idx] = c.b;
    buf[idx + 1] = c.g;
    buf[idx + 2] = c.r;
    buf[idx + 3] = c.a;
}

fn draw_rect(
    buf: &mut [u8; ICON_SIZE * ICON_SIZE * 4],
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    c: Bgra,
) {
    for y in y0..=y1 {
        for x in x0..=x1 {
            set_px(buf, x, y, c);
        }
    }
}

fn draw_flat_bar(
    buf: &mut [u8; ICON_SIZE * ICON_SIZE * 4],
    x0: usize,
    x1: usize,
    dev: &DeviceState,
    low_threshold: u8,
) {
    let y0 = 3usize;
    let y1 = 28usize;
    let (fill_col, border_col) = colors_for(dev, low_threshold);

    // Subtle 1px rounded-corner outline + semi-transparent slate track.
    for y in y0..=y1 {
        for x in x0..=x1 {
            let is_corner = (x == x0 || x == x1) && (y == y0 || y == y1);
            if is_corner {
                continue;
            }
            let is_edge = x == x0 || x == x1 || y == y0 || y == y1;
            set_px(buf, x, y, if is_edge { border_col } else { TRACK_BG });
        }
    }

    // Interior fill bounds
    let ix0 = x0 + 2;
    let ix1 = x1 - 2;
    let iy0 = y0 + 2;
    let iy1 = y1 - 2;
    let well_h = iy1 - iy0 + 1;

    if dev.status == DeviceStatusKind::Disconnected {
        let cy = iy0 + well_h / 2;
        if ix1 > ix0 + 2 {
            draw_rect(buf, ix0 + 1, cy, ix1 - 1, cy + 1, OFFLINE_DASH);
        }
        return;
    }

    let fill_rows = if let Some(p) = dev.percent {
        let pct = p.min(100) as usize;
        if pct == 0 {
            1
        } else {
            ((pct * well_h) + 50) / 100
        }
        .clamp(1, well_h)
    } else if dev.charging {
        well_h
    } else {
        0
    };

    if fill_rows > 0 {
        let fy0 = iy1 + 1 - fill_rows;
        draw_rect(buf, ix0, fy0, ix1, iy1, fill_col);
    }

    if dev.charging && dev.status == DeviceStatusKind::Active && (ix1 - ix0 + 1) >= 5 {
        const BOLT: [&str; 9] = [
            "..##.",
            ".##..",
            "##...",
            "#####",
            "#####",
            "...##",
            "..##.",
            ".##..",
            ".#...",
        ];
        let bx = ix0 + (ix1 - ix0 + 1 - 5) / 2;
        let by = iy0 + (well_h - BOLT.len()) / 2;
        for (dy, row) in BOLT.iter().enumerate() {
            for (dx, ch) in row.bytes().enumerate() {
                if ch == b'#' {
                    set_px(buf, bx + dx, by + dy, COLOR_BOLT);
                }
            }
        }
    }
}

fn pick_summary_device(devices: &[DeviceState]) -> Option<&DeviceState> {
    devices
        .iter()
        .filter(|d| d.status != DeviceStatusKind::Disconnected)
        .min_by_key(|d| d.percent.unwrap_or(101))
        .or_else(|| devices.first())
}

/// Renders a crisp flat N-bar 32x32 BGRA tray icon for any number of registered devices.
pub fn render_bars_bgra(devices: &[DeviceState], low_threshold: u8) -> [u8; ICON_SIZE * ICON_SIZE * 4] {
    let mut buf = [0u8; ICON_SIZE * ICON_SIZE * 4];
    match devices.len() {
        0 => {}
        1 => draw_flat_bar(&mut buf, 9, 22, &devices[0], low_threshold),
        2 => {
            draw_flat_bar(&mut buf, 3, 13, &devices[0], low_threshold);
            draw_flat_bar(&mut buf, 18, 28, &devices[1], low_threshold);
        }
        3 => {
            draw_flat_bar(&mut buf, 2, 9, &devices[0], low_threshold);
            draw_flat_bar(&mut buf, 12, 19, &devices[1], low_threshold);
            draw_flat_bar(&mut buf, 22, 29, &devices[2], low_threshold);
        }
        _ => {
            if let Some(summary) = pick_summary_device(devices) {
                draw_flat_bar(&mut buf, 9, 22, summary, low_threshold);
            }
        }
    }
    buf
}

/// Creates a Win32 `HICON` from the rendered 32x32 BGRA bitmap.
/// Caller owns the returned `HICON` and must release it with `DestroyIcon`.
pub fn build_hicon(devices: &[DeviceState], low_threshold: u8) -> HICON {
    let pixels = render_bars_bgra(devices, low_threshold);
    unsafe {
        let mut bi: BITMAPINFO = zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = ICON_SIZE as i32;
        bi.bmiHeader.biHeight = -(ICON_SIZE as i32);
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;

        let hdc = GetDC(null_mut());
        let mut bits_ptr: *mut std::ffi::c_void = null_mut();
        let color_bmp: HBITMAP =
            CreateDIBSection(hdc, &bi, DIB_RGB_COLORS, &mut bits_ptr, null_mut(), 0);
        if !hdc.is_null() {
            ReleaseDC(null_mut(), hdc);
        }
        if color_bmp.is_null() || bits_ptr.is_null() {
            return null_mut();
        }
        copy_nonoverlapping(pixels.as_ptr(), bits_ptr as *mut u8, pixels.len());

        let mask_bytes = [0u8; (ICON_SIZE / 8) * ICON_SIZE];
        let mask_bmp: HBITMAP = CreateBitmap(
            ICON_SIZE as i32,
            ICON_SIZE as i32,
            1,
            1,
            mask_bytes.as_ptr() as *const _,
        );

        let info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask_bmp,
            hbmColor: color_bmp,
        };
        let hicon = CreateIconIndirect(&info);

        if !mask_bmp.is_null() {
            DeleteObject(mask_bmp);
        }
        DeleteObject(color_bmp);
        hicon
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ConnectionMode, DeviceKind};
    use std::time::Instant;

    fn sample_pixel(buf: &[u8; ICON_SIZE * ICON_SIZE * 4], x: usize, y: usize) -> (u8, u8, u8, u8) {
        let idx = (y * ICON_SIZE + x) * 4;
        (buf[idx + 2], buf[idx + 1], buf[idx], buf[idx + 3]) // R, G, B, A
    }

    #[test]
    fn renders_two_flat_bars_with_distinct_colors_and_heights() {
        let now = Instant::now();
        let mut kb = DeviceState::new("aula_f87", "Aula F87", DeviceKind::Keyboard);
        kb.apply_active_reading(Some(90), false, ConnectionMode::Wireless24G, now);

        let mut mouse = DeviceState::new("sc900_pro", "SC900 Pro", DeviceKind::Mouse);
        mouse.apply_active_reading(Some(15), false, ConnectionMode::Wireless24G8K, now);

        let buf = render_bars_bgra(&[kb, mouse], 20);

        // Rounded corner pixel at (3, 3) stays transparent.
        assert_eq!(sample_pixel(&buf, 3, 3), (0, 0, 0, 0));
        // Left bar interior near top (x=8, y=8) is green at 90%.
        assert_eq!(sample_pixel(&buf, 8, 8), (34, 197, 94, 255));
        // Right bar interior near bottom (x=23, y=25) is red at 15%, top (y=8) is track bg.
        assert_eq!(sample_pixel(&buf, 23, 25), (239, 68, 68, 255));
        assert_eq!(sample_pixel(&buf, 23, 8), (18, 22, 28, 175));
    }

    #[test]
    fn renders_charging_bolt_sleeping_slate_and_three_bars() {
        let now = Instant::now();
        let mut kb = DeviceState::new("aula_f87", "Aula F87", DeviceKind::Keyboard);
        kb.apply_active_reading(Some(50), true, ConnectionMode::Wired, now);

        let mut mouse = DeviceState::new("sc900_pro", "SC900 Pro", DeviceKind::Mouse);
        mouse.apply_active_reading(Some(75), false, ConnectionMode::Wireless24G8K, now);
        mouse.mark_sleeping(ConnectionMode::Wireless24G8K);

        let buf = render_bars_bgra(&[kb.clone(), mouse.clone()], 20);
        assert_eq!(sample_pixel(&buf, 23, 18), (100, 116, 139, 255));
        let has_bolt =
            (5..=26).any(|y| (5..=11).any(|x| sample_pixel(&buf, x, y) == (255, 250, 210, 255)));
        assert!(has_bolt);

        let mut headset = DeviceState::new("hs", "Headset", DeviceKind::Headset);
        headset.apply_active_reading(Some(80), false, ConnectionMode::Bluetooth, now);
        let buf3 = render_bars_bgra(&[kb, mouse, headset], 20);
        // Third bar centered around x=25 should be green.
        assert_eq!(sample_pixel(&buf3, 25, 16), (34, 197, 94, 255));
    }
}
