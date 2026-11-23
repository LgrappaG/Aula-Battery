use std::mem::{size_of, zeroed};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_WARNING, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};

use crate::model::{AppState, DeviceState, DeviceStatusKind, DeviceVisualKey};
use crate::ui::icon::build_hicon;
use crate::ui::WM_APP_TRAY;

const TRAY_ICON_ID: u32 = 1;

fn copy_wide_truncated(dst: &mut [u16], text: &str) {
    dst.fill(0);
    if dst.is_empty() {
        return;
    }
    let max_units = dst.len() - 1;
    for (idx, unit) in text.encode_utf16().take(max_units).enumerate() {
        dst[idx] = unit;
    }
}

fn format_device_line(dev: &DeviceState) -> String {
    match dev.status {
        DeviceStatusKind::Disconnected => format!("{}: Offline", dev.name),
        DeviceStatusKind::Sleeping => {
            let mode = dev.mode.map(|m| m.short_label()).unwrap_or("2.4G");
            match dev.percent {
                Some(p) => format!("{}: {}% ({}, Sleep)", dev.name, p, mode),
                None => format!("{}: Sleep ({})", dev.name, mode),
            }
        }
        DeviceStatusKind::Active => {
            let mode = dev.mode.map(|m| m.short_label()).unwrap_or("");
            let bolt = if dev.charging { " ⚡" } else { "" };
            match dev.percent {
                Some(p) => format!("{}: {}% ({}){}", dev.name, p, mode, bolt),
                None if dev.charging => format!("{}: Charging ({}) ⚡", dev.name, mode),
                None => format!("{}: Active ({})", dev.name, mode),
            }
        }
    }
}

fn format_device_compact(dev: &DeviceState) -> String {
    match dev.status {
        DeviceStatusKind::Disconnected => format!("{}: Off", dev.name),
        DeviceStatusKind::Sleeping => match dev.percent {
            Some(p) => format!("{}: {}% (Sleep)", dev.name, p),
            None => format!("{}: Sleep", dev.name),
        },
        DeviceStatusKind::Active => {
            let bolt = if dev.charging { "⚡" } else { "" };
            match dev.percent {
                Some(p) => format!("{}: {}{}%", dev.name, bolt, p),
                None if dev.charging => format!("{}: ⚡", dev.name),
                None => format!("{}: On", dev.name),
            }
        }
    }
}

pub fn format_tooltip(devices: &[DeviceState]) -> String {
    let full = devices
        .iter()
        .map(format_device_line)
        .collect::<Vec<_>>()
        .join("\n");
    if full.encode_utf16().count() <= 127 {
        full
    } else {
        devices
            .iter()
            .map(format_device_compact)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub struct TrayIcon {
    hwnd: HWND,
    current_hicon: HICON,
    added: bool,
    last_sig: Option<Vec<DeviceVisualKey>>,
}

impl TrayIcon {
    pub fn new(hwnd: HWND, initial: &AppState) -> Self {
        let mut tray = Self {
            hwnd,
            current_hicon: std::ptr::null_mut(),
            added: false,
            last_sig: None,
        };
        tray.re_add(initial);
        tray
    }

    pub fn re_add(&mut self, state: &AppState) {
        let new_icon = build_hicon(&state.devices, state.settings.low_threshold);
        let tip = format_tooltip(&state.devices);

        let mut nid: NOTIFYICONDATAW = unsafe { zeroed() };
        nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = TRAY_ICON_ID;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = new_icon;
        copy_wide_truncated(&mut nid.szTip, &tip);

        let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &nid) };
        if ok != 0 {
            self.added = true;
            self.last_sig = Some(state.visual_signature());
            if !self.current_hicon.is_null() {
                unsafe { DestroyIcon(self.current_hicon) };
            }
            self.current_hicon = new_icon;
        } else if !new_icon.is_null() {
            unsafe { DestroyIcon(new_icon) };
        }
    }

    pub fn update(&mut self, state: &AppState) {
        if !self.added {
            self.re_add(state);
            return;
        }

        let sig = state.visual_signature();
        if self.last_sig.as_ref() == Some(&sig) {
            return;
        }

        let new_icon = build_hicon(&state.devices, state.settings.low_threshold);
        let tip = format_tooltip(&state.devices);

        let mut nid: NOTIFYICONDATAW = unsafe { zeroed() };
        nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = TRAY_ICON_ID;
        nid.uFlags = NIF_ICON | NIF_TIP;
        nid.hIcon = new_icon;
        copy_wide_truncated(&mut nid.szTip, &tip);

        let ok = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
        if ok != 0 {
            self.last_sig = Some(sig);
            if !self.current_hicon.is_null() {
                unsafe { DestroyIcon(self.current_hicon) };
            }
            self.current_hicon = new_icon;
        } else {
            if !new_icon.is_null() {
                unsafe { DestroyIcon(new_icon) };
            }
            self.re_add(state);
        }
    }

    pub fn show_low_battery_balloon(&self, device_name: &str, percent: u8) {
        if !self.added {
            return;
        }
        let title = format!("Low Battery: {}", device_name);
        let body = format!("Battery is at {}%. Consider charging soon.", percent);

        let mut nid: NOTIFYICONDATAW = unsafe { zeroed() };
        nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = TRAY_ICON_ID;
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_WARNING;
        copy_wide_truncated(&mut nid.szInfoTitle, &title);
        copy_wide_truncated(&mut nid.szInfo, &body);

        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        if self.added {
            let mut nid: NOTIFYICONDATAW = unsafe { zeroed() };
            nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = self.hwnd;
            nid.uID = TRAY_ICON_ID;
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &nid);
            }
            self.added = false;
        }
        if !self.current_hicon.is_null() {
            unsafe {
                DestroyIcon(self.current_hicon);
            }
            self.current_hicon = std::ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ConnectionMode, DeviceKind};
    use std::time::Instant;

    #[test]
    fn tooltip_english_format_fits_within_127_utf16_units() {
        let now = Instant::now();
        let mut kb = DeviceState::new("aula_f87", "Aula F87", DeviceKind::Keyboard);
        kb.apply_active_reading(Some(100), true, ConnectionMode::Wireless24G, now);
        let mut mouse = DeviceState::new("sc900_pro", "SC900 Pro", DeviceKind::Mouse);
        mouse.apply_active_reading(Some(85), false, ConnectionMode::Wireless24G8K, now);
        mouse.mark_sleeping(ConnectionMode::Wireless24G8K);

        let tip = format_tooltip(&[kb.clone(), mouse.clone()]);
        assert!(tip.contains("Aula F87: 100% (2.4G)"));
        assert!(tip.contains("SC900 Pro: 85% (2.4G 8K, Sleep)"));
        assert!(tip.encode_utf16().count() <= 127, "tooltip too long: {tip}");

        // 4 devices fall back cleanly to compact form so all devices fit within 127 UTF-16 units.
        let mut d3 = DeviceState::new("hs", "Headset Pro", DeviceKind::Headset);
        d3.apply_active_reading(Some(72), false, ConnectionMode::Bluetooth, now);
        let mut d4 = DeviceState::new("gp", "Gamepad X", DeviceKind::Gamepad);
        d4.apply_active_reading(Some(55), false, ConnectionMode::Wireless24G, now);
        let tip4 = format_tooltip(&[kb, mouse, d3, d4]);
        assert!(tip4.encode_utf16().count() <= 127, "4-dev tooltip too long: {tip4}");
    }
}
