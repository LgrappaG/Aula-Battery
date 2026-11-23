pub mod icon;
pub mod menu;
pub mod popup;
pub mod tray;

use windows_sys::Win32::UI::WindowsAndMessaging::WM_APP;

pub const WM_APP_TRAY: u32 = WM_APP + 1;
pub const WM_APP_STATE_UPDATED: u32 = WM_APP + 2;
pub const WM_APP_LOW_BATTERY: u32 = WM_APP + 3;
