pub mod bluetooth;

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows_sys::core::GUID;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetFeature, HidD_GetHidGuid,
    HidD_GetPreparsedData, HidD_GetProductString, HidD_SetFeature, HidD_SetOutputReport,
    HidP_GetCaps, HIDD_ATTRIBUTES, HIDP_CAPS, HIDP_STATUS_SUCCESS, PHIDP_PREPARSED_DATA,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, ResetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

#[derive(Debug, Clone)]
pub struct HidCollectionInfo {
    pub path: Vec<u16>,
    pub vid: u16,
    pub pid: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub input_len: usize,
    pub output_len: usize,
    pub feature_len: usize,
    pub product: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOutcome {
    Data(usize),
    Timeout,
    Stopped,
    IoError,
}

struct DevInfoSet(HDEVINFO);

impl Drop for DevInfoSet {
    fn drop(&mut self) {
        if self.0 != 0 && self.0 != -1 {
            unsafe {
                SetupDiDestroyDeviceInfoList(self.0);
            }
        }
    }
}

struct PreparsedData(PHIDP_PREPARSED_DATA);

impl Drop for PreparsedData {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                HidD_FreePreparsedData(self.0);
            }
        }
    }
}

/// RAII wrapper around a Win32 HANDLE.
pub struct WinHandle(HANDLE);

unsafe impl Send for WinHandle {}
unsafe impl Sync for WinHandle {}

impl WinHandle {
    pub fn from_raw(h: HANDLE) -> Option<Self> {
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(Self(h))
        }
    }

    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for WinHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// RAII wrapper around a Win32 HID device handle plus optional overlapped event.
pub struct HidHandle {
    handle: WinHandle,
    event: Option<WinHandle>,
}

impl HidHandle {
    fn open_with(path: &[u16], desired_access: u32, overlapped: bool) -> Option<Self> {
        let flags = if overlapped { FILE_FLAG_OVERLAPPED } else { 0 };
        let h = unsafe {
            CreateFileW(
                path.as_ptr(),
                desired_access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                flags,
                null_mut(),
            )
        };
        let handle = WinHandle::from_raw(h)?;
        let event = if overlapped {
            let ev = unsafe { CreateEventW(null(), 1, 0, null()) };
            Some(WinHandle::from_raw(ev)?)
        } else {
            None
        };
        Some(Self { handle, event })
    }

    pub fn open_rw_overlapped(path: &[u16]) -> Option<Self> {
        Self::open_with(path, GENERIC_READ | GENERIC_WRITE, true)
    }

    pub fn open_read_overlapped(path: &[u16]) -> Option<Self> {
        Self::open_with(path, GENERIC_READ, true)
    }

    pub fn open_feature_only(path: &[u16]) -> Option<Self> {
        Self::open_with(path, 0, false)
    }

    /// Reads an HID input report with timeout, optionally aborting immediately on `stop_event`.
    /// Guarantees any pending async I/O is cancelled and completed before returning so `buf` and
    /// `OVERLAPPED` are never dropped while in flight.
    pub fn read_timeout(
        &self,
        buf: &mut [u8],
        timeout_ms: u32,
        stop_event: Option<HANDLE>,
    ) -> ReadOutcome {
        let Some(ev) = self.event.as_ref() else {
            return ReadOutcome::IoError;
        };
        unsafe {
            ResetEvent(ev.raw());
            let mut ov: OVERLAPPED = zeroed();
            ov.hEvent = ev.raw();
            let mut read: u32 = 0;

            let ok = ReadFile(
                self.handle.raw(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut read,
                &mut ov,
            );
            if ok != 0 {
                return if read > 0 {
                    ReadOutcome::Data(read as usize)
                } else {
                    ReadOutcome::IoError
                };
            }

            let err = GetLastError();
            if err != ERROR_IO_PENDING {
                return ReadOutcome::IoError;
            }

            let wait_res = if let Some(stop) = stop_event {
                let handles = [ev.raw(), stop];
                WaitForMultipleObjects(2, handles.as_ptr(), 0, timeout_ms)
            } else {
                WaitForSingleObject(ev.raw(), timeout_ms)
            };

            if wait_res == WAIT_OBJECT_0 {
                let done = GetOverlappedResult(self.handle.raw(), &ov, &mut read, 0);
                if done != 0 && read > 0 {
                    ReadOutcome::Data(read as usize)
                } else {
                    ReadOutcome::IoError
                }
            } else {
                CancelIoEx(self.handle.raw(), &ov);
                let _ = GetOverlappedResult(self.handle.raw(), &ov, &mut read, 1);
                if stop_event.is_some() && wait_res == WAIT_OBJECT_0 + 1 {
                    ReadOutcome::Stopped
                } else if wait_res == WAIT_TIMEOUT {
                    ReadOutcome::Timeout
                } else {
                    ReadOutcome::IoError
                }
            }
        }
    }

    pub fn write_timeout(&self, data: &[u8], timeout_ms: u32) -> bool {
        let Some(ev) = self.event.as_ref() else {
            return false;
        };
        unsafe {
            ResetEvent(ev.raw());
            let mut ov: OVERLAPPED = zeroed();
            ov.hEvent = ev.raw();
            let mut written: u32 = 0;

            let ok = WriteFile(
                self.handle.raw(),
                data.as_ptr(),
                data.len() as u32,
                &mut written,
                &mut ov,
            );
            if ok != 0 {
                return written > 0;
            }
            if GetLastError() != ERROR_IO_PENDING {
                return false;
            }
            if WaitForSingleObject(ev.raw(), timeout_ms) == WAIT_OBJECT_0 {
                GetOverlappedResult(self.handle.raw(), &ov, &mut written, 0) != 0 && written > 0
            } else {
                CancelIoEx(self.handle.raw(), &ov);
                let _ = GetOverlappedResult(self.handle.raw(), &ov, &mut written, 1);
                false
            }
        }
    }

    pub fn set_output_report(&self, data: &[u8]) -> bool {
        unsafe { HidD_SetOutputReport(self.handle.raw(), data.as_ptr() as *const c_void, data.len() as u32) != 0 }
    }

    pub fn set_feature(&self, data: &[u8]) -> bool {
        unsafe { HidD_SetFeature(self.handle.raw(), data.as_ptr() as *const c_void, data.len() as u32) != 0 }
    }

    pub fn get_feature(&self, buf: &mut [u8]) -> bool {
        unsafe { HidD_GetFeature(self.handle.raw(), buf.as_mut_ptr() as *mut c_void, buf.len() as u32) != 0 }
    }

    pub fn drain_input(&self, scratch: &mut [u8], max_rounds: usize) {
        for _ in 0..max_rounds {
            match self.read_timeout(scratch, 10, None) {
                ReadOutcome::Data(_) => continue,
                _ => break,
            }
        }
    }
}

/// Enumerates all currently present Windows HID top-level collections.
pub fn enumerate_hid() -> Vec<HidCollectionInfo> {
    let mut result = Vec::new();
    let mut hid_guid: GUID = unsafe { zeroed() };
    unsafe { HidD_GetHidGuid(&mut hid_guid) };

    let raw_info = unsafe {
        SetupDiGetClassDevsW(
            &hid_guid,
            null(),
            null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if raw_info == 0 || raw_info == -1 {
        return result;
    }
    let dev_info = DevInfoSet(raw_info);

    let mut if_data: SP_DEVICE_INTERFACE_DATA = unsafe { zeroed() };
    if_data.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;

    let mut idx = 0u32;
    loop {
        let ok = unsafe {
            SetupDiEnumDeviceInterfaces(dev_info.0, null(), &hid_guid, idx, &mut if_data)
        };
        if ok == 0 {
            break;
        }
        idx += 1;

        let mut req_bytes = 0u32;
        unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                dev_info.0,
                &if_data,
                null_mut(),
                0,
                &mut req_bytes,
                null_mut(),
            );
        }
        if req_bytes < 8 {
            continue;
        }

        let mut detail_buf = vec![0u8; req_bytes as usize];
        let detail_ptr = detail_buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
        let cb_size = if cfg!(target_pointer_width = "64") { 8u32 } else { 6u32 };
        unsafe {
            (*detail_ptr).cbSize = cb_size;
        }

        let got_detail = unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                dev_info.0,
                &if_data,
                detail_ptr,
                req_bytes,
                &mut req_bytes,
                null_mut(),
            )
        };
        if got_detail == 0 {
            continue;
        }

        let path_bytes = &detail_buf[4..];
        let mut path_u16 = Vec::with_capacity(path_bytes.len() / 2);
        for chunk in path_bytes.chunks_exact(2) {
            let ch = u16::from_le_bytes([chunk[0], chunk[1]]);
            path_u16.push(ch);
            if ch == 0 {
                break;
            }
        }
        if path_u16.last().copied() != Some(0) {
            path_u16.push(0);
        }

        let Some(probe_handle) = HidHandle::open_with(&path_u16, 0, false) else {
            continue;
        };

        let mut attr: HIDD_ATTRIBUTES = unsafe { zeroed() };
        attr.Size = size_of::<HIDD_ATTRIBUTES>() as u32;
        if unsafe { HidD_GetAttributes(probe_handle.handle.raw(), &mut attr) } == 0 {
            continue;
        }

        let mut pp_raw: PHIDP_PREPARSED_DATA = 0;
        if unsafe { HidD_GetPreparsedData(probe_handle.handle.raw(), &mut pp_raw) } == 0 {
            continue;
        }
        let pp = PreparsedData(pp_raw);

        let mut caps: HIDP_CAPS = unsafe { zeroed() };
        if unsafe { HidP_GetCaps(pp.0, &mut caps) } != HIDP_STATUS_SUCCESS {
            continue;
        }

        let mut prod_buf = [0u16; 128];
        let product = if unsafe {
            HidD_GetProductString(
                probe_handle.handle.raw(),
                prod_buf.as_mut_ptr() as *mut c_void,
                (prod_buf.len() * 2) as u32,
            )
        } != 0
        {
            let end = prod_buf.iter().position(|&c| c == 0).unwrap_or(prod_buf.len());
            String::from_utf16_lossy(&prod_buf[..end]).trim().to_string()
        } else {
            String::new()
        };

        result.push(HidCollectionInfo {
            path: path_u16,
            vid: attr.VendorID,
            pid: attr.ProductID,
            usage_page: caps.UsagePage,
            usage: caps.Usage,
            input_len: caps.InputReportByteLength as usize,
            output_len: caps.OutputReportByteLength as usize,
            feature_len: caps.FeatureReportByteLength as usize,
            product,
        });
    }

    result
}
