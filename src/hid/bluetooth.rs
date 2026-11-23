use std::collections::HashMap;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows_sys::core::GUID;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
    SetupDiGetDevicePropertyW, DIGCF_ALLCLASSES, DIGCF_PRESENT, HDEVINFO, SP_DEVINFO_DATA,
};
use windows_sys::Win32::Devices::Properties::{
    DEVPROPKEY, DEVPROPTYPE, DEVPROP_TYPE_BOOLEAN, DEVPROP_TYPE_BYTE, DEVPROP_TYPE_GUID,
    DEVPROP_TYPE_STRING,
};

const DEVPKEY_NAME: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0xB725F130_47EF_101A_A5F1_02608C9EEBAC),
    pid: 10,
};
const DEVPKEY_DEVICE_DESC: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0xA45C254E_DF1C_4EFD_8020_67D146A850E0),
    pid: 2,
};
const DEVPKEY_FRIENDLY_NAME: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0xA45C254E_DF1C_4EFD_8020_67D146A850E0),
    pid: 14,
};
const DEVPKEY_CONTAINER_ID: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x8C7ED206_3F8A_4827_B3AB_AE9E1FAEFC6C),
    pid: 2,
};
const DEVPKEY_IS_CONNECTED: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x83DA6326_97A6_4088_9453_A1923F573B29),
    pid: 15,
};
const DEVPKEY_BLUETOOTH_BATTERY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x104EA319_6EE2_4701_BD47_8DDBF425BBE5),
    pid: 2,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BluetoothDeviceInfo {
    pub name: String,
    pub all_names_lower: String,
    pub connected: bool,
    pub battery: Option<u8>,
}

#[derive(Default)]
struct BtGroup {
    friendly: Option<String>,
    names: Vec<String>,
    connected: bool,
    battery: Option<u8>,
}

struct DevInfoGuard(HDEVINFO);
impl Drop for DevInfoGuard {
    fn drop(&mut self) {
        if self.0 != 0 && self.0 != -1 {
            unsafe {
                SetupDiDestroyDeviceInfoList(self.0);
            }
        }
    }
}

fn read_string_prop(
    dev_info: HDEVINFO,
    dev_data: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
) -> Option<String> {
    let mut prop_type: DEVPROPTYPE = 0;
    let mut buf = [0u16; 256];
    let ok = unsafe {
        SetupDiGetDevicePropertyW(
            dev_info,
            dev_data,
            key,
            &mut prop_type,
            buf.as_mut_ptr() as *mut u8,
            (buf.len() * 2) as u32,
            null_mut(),
            0,
        )
    };
    if ok == 0 || prop_type != DEVPROP_TYPE_STRING {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let s = String::from_utf16_lossy(&buf[..end]).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn read_byte_prop(dev_info: HDEVINFO, dev_data: &SP_DEVINFO_DATA, key: &DEVPROPKEY) -> Option<u8> {
    let mut prop_type: DEVPROPTYPE = 0;
    let mut val = 0u8;
    let ok = unsafe {
        SetupDiGetDevicePropertyW(
            dev_info,
            dev_data,
            key,
            &mut prop_type,
            &mut val,
            1,
            null_mut(),
            0,
        )
    };
    if ok != 0 && prop_type == DEVPROP_TYPE_BYTE {
        Some(val)
    } else {
        None
    }
}

fn read_bool_prop(
    dev_info: HDEVINFO,
    dev_data: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
) -> Option<bool> {
    let mut prop_type: DEVPROPTYPE = 0;
    let mut val = 0u8;
    let ok = unsafe {
        SetupDiGetDevicePropertyW(
            dev_info,
            dev_data,
            key,
            &mut prop_type,
            &mut val,
            1,
            null_mut(),
            0,
        )
    };
    if ok != 0 && prop_type == DEVPROP_TYPE_BOOLEAN {
        Some(val != 0)
    } else {
        None
    }
}

fn read_guid_prop(
    dev_info: HDEVINFO,
    dev_data: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
) -> Option<[u8; 16]> {
    let mut prop_type: DEVPROPTYPE = 0;
    let mut val = [0u8; 16];
    let ok = unsafe {
        SetupDiGetDevicePropertyW(
            dev_info,
            dev_data,
            key,
            &mut prop_type,
            val.as_mut_ptr(),
            16,
            null_mut(),
            0,
        )
    };
    if ok != 0 && prop_type == DEVPROP_TYPE_GUID {
        Some(val)
    } else {
        None
    }
}

fn scan_enumerator(enumerator: &str, groups: &mut HashMap<[u8; 16], BtGroup>) {
    let wide_enum: Vec<u16> = enumerator.encode_utf16().chain(std::iter::once(0)).collect();
    let raw = unsafe {
        SetupDiGetClassDevsW(
            null(),
            wide_enum.as_ptr(),
            null_mut(),
            DIGCF_PRESENT | DIGCF_ALLCLASSES,
        )
    };
    if raw == 0 || raw == -1 {
        return;
    }
    let guard = DevInfoGuard(raw);

    let mut dev_data: SP_DEVINFO_DATA = unsafe { zeroed() };
    dev_data.cbSize = size_of::<SP_DEVINFO_DATA>() as u32;

    let mut idx = 0u32;
    loop {
        if unsafe { SetupDiEnumDeviceInfo(guard.0, idx, &mut dev_data) } == 0 {
            break;
        }
        idx += 1;

        let Some(cid) = read_guid_prop(guard.0, &dev_data, &DEVPKEY_CONTAINER_ID) else {
            continue;
        };
        let group = groups.entry(cid).or_default();

        if let Some(friendly) = read_string_prop(guard.0, &dev_data, &DEVPKEY_FRIENDLY_NAME) {
            if group.friendly.is_none() {
                group.friendly = Some(friendly.clone());
            }
            group.names.push(friendly);
        }
        for key in [&DEVPKEY_NAME, &DEVPKEY_DEVICE_DESC] {
            if let Some(name) = read_string_prop(guard.0, &dev_data, key) {
                group.names.push(name);
            }
        }
        if read_bool_prop(guard.0, &dev_data, &DEVPKEY_IS_CONNECTED).unwrap_or(false) {
            group.connected = true;
        }
        if let Some(batt) = read_byte_prop(guard.0, &dev_data, &DEVPKEY_BLUETOOTH_BATTERY) {
            if batt <= 100 {
                group.battery = Some(batt);
            }
        }
    }
}

/// Scans all present Windows Bluetooth PnP devices (LE and Classic), grouped by ContainerId.
pub fn scan_bluetooth() -> Vec<BluetoothDeviceInfo> {
    let mut groups: HashMap<[u8; 16], BtGroup> = HashMap::new();
    scan_enumerator("BTHLE", &mut groups);
    scan_enumerator("BTHENUM", &mut groups);

    let mut out: Vec<BluetoothDeviceInfo> = groups
        .into_values()
        .filter_map(|g| {
            let name = g
                .friendly
                .or_else(|| g.names.first().cloned())
                .unwrap_or_default();
            if name.is_empty() && g.battery.is_none() {
                return None;
            }
            Some(BluetoothDeviceInfo {
                name,
                all_names_lower: g.names.join(" ").to_ascii_lowercase(),
                connected: g.connected,
                battery: g.battery,
            })
        })
        .collect();

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Returns the battery percentage for the first connected Bluetooth device matching `needle`.
pub fn find_connected_battery(devices: &[BluetoothDeviceInfo], needle: &str) -> Option<u8> {
    let needle_lower = needle.to_ascii_lowercase();
    devices.iter().find_map(|d| {
        if d.connected && d.all_names_lower.contains(&needle_lower) {
            d.battery
        } else {
            None
        }
    })
}
