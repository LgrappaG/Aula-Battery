#![windows_subsystem = "windows"]

mod autostart;
mod devices;
mod hid;
mod model;
mod protocol;
mod settings;
mod ui;
mod worker;

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows_sys::core::GUID;
use windows_sys::Win32::Devices::HumanInterfaceDevice::HidD_GetHidGuid;
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetWindowLongPtrW, KillTimer, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterDeviceNotificationW, RegisterWindowMessageW, SetTimer, SetWindowLongPtrW,
    TranslateMessage, UnregisterDeviceNotification, CREATESTRUCTW, DBT_DEVICEARRIVAL,
    DBT_DEVICEREMOVECOMPLETE, DBT_DEVTYP_DEVICEINTERFACE, DEVICE_NOTIFY_WINDOW_HANDLE,
    DEV_BROADCAST_DEVICEINTERFACE_W, GWLP_USERDATA, HDEVNOTIFY, MSG, WM_CLOSE, WM_COMMAND,
    WM_CREATE, WM_DESTROY, WM_DEVICECHANGE, WM_LBUTTONUP, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
};

use crate::devices::{create_providers, PollContext};
use crate::hid::bluetooth::scan_bluetooth;
use crate::hid::{enumerate_hid, WinHandle};
use crate::model::{AppState, DeviceState};
use crate::ui::menu::{show_context_menu, IDM_AUTOSTART, IDM_EXIT, IDM_LOW_ALERT, IDM_REFRESH};
use crate::ui::popup::PopupWindow;
use crate::ui::tray::TrayIcon;
use crate::ui::{WM_APP_LOW_BATTERY, WM_APP_STATE_UPDATED, WM_APP_TRAY};
use crate::worker::Workers;

const DEBOUNCE_TIMER_ID: usize = 42;
const WM_NIN_SELECT: u32 = 0x0400; // NIN_SELECT
const WM_POWERBROADCAST: u32 = 0x0218;
const PBT_APMRESUMESUSPEND: usize = 0x0007;
const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn run_probe() {
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
    println!("=== Aula Battery Diagnostic Probe ===");

    let mut providers = create_providers();
    let hid = enumerate_hid();
    let bt = scan_bluetooth();

    let relevant: Vec<_> = hid
        .iter()
        .filter(|c| providers.iter().any(|p| p.matches_hid(c)))
        .collect();

    println!("Matched HID collections ({}):", relevant.len());
    for c in &relevant {
        println!(
            "  {:04X}:{:04X} UP={:04X} U={:04X} In={} Out={} Feat={} Prod='{}'",
            c.vid, c.pid, c.usage_page, c.usage, c.input_len, c.output_len, c.feature_len, c.product
        );
    }

    let ctx = PollContext {
        hid: &hid,
        bt: &bt,
        stop_event: None,
        now: Instant::now(),
    };
    println!("Registered providers ({}):", providers.len());
    for p in &mut providers {
        println!("  {}", p.probe_details(&ctx));
    }

    let bt_with_batt: Vec<_> = bt
        .iter()
        .filter(|d| d.battery.is_some() || d.connected)
        .collect();
    println!("Bluetooth devices ({}):", bt_with_batt.len());
    if bt_with_batt.is_empty() {
        println!("  <none>");
    } else {
        for d in bt_with_batt {
            println!(
                "  '{}' connected={} battery={:?}",
                d.name, d.connected, d.battery
            );
        }
    }
}

struct AppController {
    state: Arc<Mutex<AppState>>,
    workers: Workers,
    tray: TrayIcon,
    popup: PopupWindow,
    taskbar_created_msg: u32,
    dev_notify: HDEVNOTIFY,
}

impl AppController {
    fn handle_command(&mut self, hwnd: HWND, cmd_id: usize) {
        match cmd_id {
            IDM_REFRESH => self.workers.trigger_refresh(),
            IDM_AUTOSTART => {
                let next = !autostart::is_autostart_enabled();
                let _ = autostart::set_autostart_enabled(next);
            }
            IDM_LOW_ALERT => {
                let next = {
                    let mut st = self.state.lock().unwrap();
                    st.settings.low_battery_alerts = !st.settings.low_battery_alerts;
                    st.settings.low_battery_alerts
                };
                settings::save_low_battery_alerts(next);
            }
            IDM_EXIT => unsafe {
                DestroyWindow(hwnd);
            },
            _ => {}
        }
    }
}

unsafe extern "system" fn main_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CREATE {
        let cs = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return 0;
    }

    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppController;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let ctrl = &mut *ptr;

    if ctrl.taskbar_created_msg != 0 && msg == ctrl.taskbar_created_msg {
        let snap = ctrl.state.lock().unwrap().clone();
        ctrl.tray.re_add(&snap);
        return 0;
    }

    match msg {
        WM_APP_STATE_UPDATED => {
            let snap = ctrl.state.lock().unwrap().clone();
            ctrl.tray.update(&snap);
            ctrl.popup.redraw_if_visible();
            0
        }
        WM_APP_LOW_BATTERY => {
            let name = ctrl
                .state
                .lock()
                .unwrap()
                .devices
                .get(wparam)
                .map(|d| d.name)
                .unwrap_or("Device");
            let pct = (lparam as i64).clamp(0, 100) as u8;
            ctrl.tray.show_low_battery_balloon(name, pct);
            0
        }
        WM_APP_TRAY => {
            let ev = (lparam & 0xFFFF) as u32;
            if ev == WM_LBUTTONUP || ev == WM_NIN_SELECT {
                ctrl.popup.toggle();
            } else if ev == WM_RBUTTONUP {
                let (low_alert_on, low_threshold) = {
                    let st = ctrl.state.lock().unwrap();
                    (st.settings.low_battery_alerts, st.settings.low_threshold)
                };
                let autostart_on = autostart::is_autostart_enabled();
                show_context_menu(hwnd, autostart_on, low_alert_on, low_threshold);
            }
            0
        }
        WM_COMMAND => {
            let cmd_id = wparam & 0xFFFF;
            ctrl.handle_command(hwnd, cmd_id);
            0
        }
        WM_DEVICECHANGE => {
            let ev = wparam as u32;
            if ev == DBT_DEVICEARRIVAL || ev == DBT_DEVICEREMOVECOMPLETE {
                SetTimer(hwnd, DEBOUNCE_TIMER_ID, 700, None);
            }
            0
        }
        WM_POWERBROADCAST => {
            if wparam == PBT_APMRESUMEAUTOMATIC || wparam == PBT_APMRESUMESUSPEND {
                SetTimer(hwnd, DEBOUNCE_TIMER_ID, 1200, None);
            }
            1
        }
        WM_TIMER => {
            if wparam == DEBOUNCE_TIMER_ID {
                KillTimer(hwnd, DEBOUNCE_TIMER_ID);
                ctrl.workers.trigger_refresh();
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            if !ctrl.dev_notify.is_null() {
                UnregisterDeviceNotification(ctrl.dev_notify);
                ctrl.dev_notify = null_mut();
            }
            ctrl.workers.stop();
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn main() {
    if std::env::args().any(|a| a == "--probe") {
        run_probe();
        return;
    }

    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let mutex_name = to_wide("Local\\AulaBatteryMonitor_SingleInstance_v1");
    let mutex_raw = unsafe { CreateMutexW(null(), 1, mutex_name.as_ptr()) };
    let _mutex_guard = match WinHandle::from_raw(mutex_raw) {
        Some(g) => g,
        None => return,
    };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return;
    }

    let providers = create_providers();
    let initial_devices: Vec<DeviceState> = providers
        .iter()
        .map(|p| DeviceState::new(p.id(), p.name(), p.kind()))
        .collect();
    let app_settings = settings::load_settings();
    let state = Arc::new(Mutex::new(AppState::from_devices(initial_devices, app_settings)));

    let Some(workers) = Workers::new(Arc::clone(&state), providers) else {
        return;
    };

    let hinst = unsafe { GetModuleHandleW(null()) };
    let class_name = to_wide("AulaBatteryTrayOwnerClass");
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(main_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: null_mut(),
        hCursor: null_mut(),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: class_name.as_ptr(),
    };
    unsafe { RegisterClassW(&wc) };

    // Must be a top-level hidden window (NOT HWND_MESSAGE) so TaskbarCreated and WM_DEVICECHANGE arrive.
    let title = to_wide("Aula Battery Owner");
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        )
    };
    if hwnd.is_null() {
        return;
    }

    let tray = {
        let snap = state.lock().unwrap().clone();
        TrayIcon::new(hwnd, &snap)
    };

    let owner_hwnd_val = hwnd as isize;
    let on_refresh = Box::new(move || unsafe {
        PostMessageW(owner_hwnd_val as HWND, WM_COMMAND, IDM_REFRESH, 0);
    });
    let Some(popup) = PopupWindow::create(Arc::clone(&state), on_refresh) else {
        unsafe { DestroyWindow(hwnd) };
        return;
    };

    let mut hid_guid: GUID = unsafe { zeroed() };
    unsafe { HidD_GetHidGuid(&mut hid_guid) };
    let mut filter: DEV_BROADCAST_DEVICEINTERFACE_W = unsafe { zeroed() };
    filter.dbcc_size = size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32;
    filter.dbcc_devicetype = DBT_DEVTYP_DEVICEINTERFACE;
    filter.dbcc_classguid = hid_guid;
    let dev_notify = unsafe {
        RegisterDeviceNotificationW(
            hwnd,
            &filter as *const DEV_BROADCAST_DEVICEINTERFACE_W as *const c_void,
            DEVICE_NOTIFY_WINDOW_HANDLE,
        )
    };

    let taskbar_msg_name = to_wide("TaskbarCreated");
    let taskbar_created_msg = unsafe { RegisterWindowMessageW(taskbar_msg_name.as_ptr()) };

    let ctrl = Box::new(AppController {
        state,
        workers,
        tray,
        popup,
        taskbar_created_msg,
        dev_notify,
    });
    let ctrl_ptr = Box::into_raw(ctrl);
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctrl_ptr as isize);
        // Start workers ONLY after AppController and HWND are fully installed.
        if !(*ctrl_ptr).workers.start(hwnd) {
            DestroyWindow(hwnd);
        }
    }

    let mut msg: MSG = unsafe { zeroed() };
    unsafe {
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        drop(Box::from_raw(ctrl_ptr));
    }
}
