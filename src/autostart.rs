use std::ptr::null_mut;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_SZ,
};

const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "AulaBattery";

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

fn current_exe_quoted_wide() -> Option<Vec<u16>> {
    let mut buf = [0u16; 1024];
    let len = unsafe { GetModuleFileNameW(null_mut(), buf.as_mut_ptr(), buf.len() as u32) };
    if len == 0 || len as usize >= buf.len() {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    Some(to_wide(&format!("\"{path}\"")))
}

pub fn is_autostart_enabled() -> bool {
    let subkey = to_wide(RUN_SUBKEY);
    let val_name = to_wide(RUN_VALUE);
    let mut hkey: HKEY = null_mut();
    let res = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) };
    if res != ERROR_SUCCESS {
        return false;
    }
    let _guard = HKeyGuard(hkey);

    let mut reg_type = 0u32;
    let mut size = 0u32;
    let q = unsafe {
        RegQueryValueExW(
            hkey,
            val_name.as_ptr(),
            null_mut(),
            &mut reg_type,
            null_mut(),
            &mut size,
        )
    };
    q == ERROR_SUCCESS && reg_type == REG_SZ && size > 2
}

pub fn set_autostart_enabled(enable: bool) -> Result<(), u32> {
    let subkey = to_wide(RUN_SUBKEY);
    let val_name = to_wide(RUN_VALUE);
    let mut hkey: HKEY = null_mut();
    let res = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        )
    };
    if res != ERROR_SUCCESS {
        return Err(res);
    }
    let _guard = HKeyGuard(hkey);

    if enable {
        let wide_cmd = current_exe_quoted_wide().ok_or(1u32)?;
        let byte_len = (wide_cmd.len() * 2) as u32;
        let s = unsafe {
            RegSetValueExW(
                hkey,
                val_name.as_ptr(),
                0,
                REG_SZ,
                wide_cmd.as_ptr() as *const u8,
                byte_len,
            )
        };
        if s != ERROR_SUCCESS {
            return Err(s);
        }
    } else {
        let d = unsafe { RegDeleteValueW(hkey, val_name.as_ptr()) };
        if d != ERROR_SUCCESS && d != 2 {
            return Err(d);
        }
    }
    Ok(())
}
