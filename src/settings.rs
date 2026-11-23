use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE,
};

use crate::model::AppSettings;

const APP_SUBKEY: &str = "Software\\AulaBattery";
const ALERT_VALUE: &str = "LowBatteryAlerts";

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct HKeyGuard(HKEY);
impl Drop for HKeyGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }
}

fn load_low_battery_alerts() -> bool {
    let subkey = to_wide(APP_SUBKEY);
    let val_name = to_wide(ALERT_VALUE);
    let mut hkey: HKEY = null_mut();
    let res = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) };
    if res != ERROR_SUCCESS {
        return true;
    }
    let _guard = HKeyGuard(hkey);

    let mut reg_type = 0u32;
    let mut data = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let q = unsafe {
        RegQueryValueExW(
            hkey,
            val_name.as_ptr(),
            null_mut(),
            &mut reg_type,
            &mut data as *mut u32 as *mut u8,
            &mut size,
        )
    };
    if q == ERROR_SUCCESS && reg_type == REG_DWORD {
        data != 0
    } else {
        true
    }
}

pub fn load_settings() -> AppSettings {
    AppSettings {
        low_battery_alerts: load_low_battery_alerts(),
        ..AppSettings::default()
    }
}

pub fn save_low_battery_alerts(enabled: bool) {
    let subkey = to_wide(APP_SUBKEY);
    let val_name = to_wide(ALERT_VALUE);
    let mut hkey: HKEY = null_mut();
    let res = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            null(),
            &mut hkey,
            null_mut(),
        )
    };
    if res != ERROR_SUCCESS {
        return;
    }
    let _guard = HKeyGuard(hkey);
    let val: u32 = if enabled { 1 } else { 0 };
    unsafe {
        RegSetValueExW(
            hkey,
            val_name.as_ptr(),
            0,
            REG_DWORD,
            &val as *const u32 as *const u8,
            std::mem::size_of::<u32>() as u32,
        );
    }
}
