use std::ptr::null;

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, PostMessageW, SetForegroundWindow,
    TrackPopupMenu, MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, TPM_BOTTOMALIGN,
    TPM_RIGHTALIGN, TPM_RIGHTBUTTON, WM_NULL,
};

pub const IDM_REFRESH: usize = 1001;
pub const IDM_AUTOSTART: usize = 1002;
pub const IDM_LOW_ALERT: usize = 1003;
pub const IDM_EXIT: usize = 1004;

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn show_context_menu(
    hwnd: HWND,
    autostart_on: bool,
    low_alert_on: bool,
    low_threshold: u8,
) {
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }

        let lbl_refresh = to_wide("Refresh");
        let lbl_autostart = to_wide("Start with Windows");
        let lbl_alert = to_wide(&format!("Low Battery Alerts (<{}%)", low_threshold));
        let lbl_exit = to_wide("Exit");

        AppendMenuW(menu, MF_STRING, IDM_REFRESH, lbl_refresh.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(
            menu,
            MF_STRING | if autostart_on { MF_CHECKED } else { MF_UNCHECKED },
            IDM_AUTOSTART,
            lbl_autostart.as_ptr(),
        );
        AppendMenuW(
            menu,
            MF_STRING | if low_alert_on { MF_CHECKED } else { MF_UNCHECKED },
            IDM_LOW_ALERT,
            lbl_alert.as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(menu, MF_STRING, IDM_EXIT, lbl_exit.as_ptr());

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);
        TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_RIGHTALIGN,
            pt.x,
            pt.y,
            0,
            hwnd,
            null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
    }
}
