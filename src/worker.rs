use std::ptr::null;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use windows_sys::Win32::Foundation::{HANDLE, HWND, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CreateEventW, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::devices::{DeviceProvider, HidPushSpec, PollContext, PollOutcome, PushUpdate};
use crate::hid::bluetooth::scan_bluetooth;
use crate::hid::{enumerate_hid, HidHandle, ReadOutcome, WinHandle};
use crate::model::{AppState, ConnectionMode, DeviceStatusKind};
use crate::ui::{WM_APP_LOW_BATTERY, WM_APP_STATE_UPDATED};

const POLL_ACTIVE_MS: u32 = 45_000;
const POLL_SLEEP_MS: u32 = 90_000;

struct SharedSync {
    state: Arc<Mutex<AppState>>,
    hwnd: AtomicIsize,
    stop_event: WinHandle,
    refresh_event: WinHandle,
    listener_wake: WinHandle,
}

impl SharedSync {
    fn post_ui(&self, msg: u32, wparam: usize, lparam: isize) {
        let raw = self.hwnd.load(Ordering::Acquire);
        if raw != 0 {
            unsafe {
                PostMessageW(raw as HWND, msg, wparam, lparam);
            }
        }
    }

    fn post_low_battery(&self, device_idx: usize, percent: Option<u8>, fired: bool) {
        if fired {
            if let Some(pct) = percent {
                self.post_ui(WM_APP_LOW_BATTERY, device_idx, pct as isize);
            }
        }
    }
}

pub struct Workers {
    shared: Arc<SharedSync>,
    providers: Option<Vec<Box<dyn DeviceProvider>>>,
    threads: Vec<JoinHandle<()>>,
}

impl Workers {
    /// Allocates synchronization primitives without spawning threads yet, allowing the caller
    /// to create and install `HWND` before any worker posts `WM_APP_*` messages.
    pub fn new(
        state: Arc<Mutex<AppState>>,
        providers: Vec<Box<dyn DeviceProvider>>,
    ) -> Option<Self> {
        let stop_raw = unsafe { CreateEventW(null(), 1, 0, null()) };
        let refresh_raw = unsafe { CreateEventW(null(), 0, 0, null()) };
        let wake_raw = unsafe { CreateEventW(null(), 0, 0, null()) };

        let shared = Arc::new(SharedSync {
            state,
            hwnd: AtomicIsize::new(0),
            stop_event: WinHandle::from_raw(stop_raw)?,
            refresh_event: WinHandle::from_raw(refresh_raw)?,
            listener_wake: WinHandle::from_raw(wake_raw)?,
        });

        Some(Self {
            shared,
            providers: Some(providers),
            threads: Vec::new(),
        })
    }

    /// Binds the UI target `HWND` and spawns all device push-listener threads plus the poller.
    pub fn start(&mut self, hwnd: HWND) -> bool {
        let Some(providers) = self.providers.take() else {
            return false;
        };
        self.shared.hwnd.store(hwnd as isize, Ordering::Release);

        for (idx, provider) in providers.iter().enumerate() {
            if let Some(spec) = provider.push_spec() {
                let s = Arc::clone(&self.shared);
                let name = format!("listener-{}", provider.id());
                let Ok(handle) = thread::Builder::new().name(name).spawn(move || {
                    let stop = s.stop_event.raw();
                    let wake = s.listener_wake.raw();
                    run_hid_push_listener(spec, stop, wake, &|update| {
                        let now = Instant::now();
                        let (changed, alert_fired, pct) = {
                            let mut st = s.state.lock().unwrap();
                            let settings = st.settings;
                            let dev = &mut st.devices[idx];
                            let prev_key = dev.visual_key();
                            dev.apply_active_reading(
                                Some(update.percent),
                                update.charging,
                                update.mode,
                                now,
                            );
                            let fired = dev.evaluate_low_battery_alert(
                                settings.low_battery_alerts,
                                settings.low_threshold,
                                settings.reset_threshold,
                            );
                            let changed = dev.visual_key() != prev_key;
                            let pct = dev.percent;
                            st.last_poll = Some(now);
                            (changed, fired, pct)
                        };
                        s.post_low_battery(idx, pct, alert_fired);
                        if changed {
                            s.post_ui(WM_APP_STATE_UPDATED, 0, 0);
                        }
                    });
                }) else {
                    return false;
                };
                self.threads.push(handle);
            }
        }

        let s_poll = Arc::clone(&self.shared);
        let Ok(poller) = thread::Builder::new()
            .name("device-poller".to_string())
            .spawn(move || run_poller(&s_poll, providers))
        else {
            return false;
        };
        self.threads.push(poller);
        true
    }

    pub fn trigger_refresh(&self) {
        unsafe {
            SetEvent(self.shared.refresh_event.raw());
            SetEvent(self.shared.listener_wake.raw());
        }
    }

    pub fn stop(&mut self) {
        unsafe {
            SetEvent(self.shared.stop_event.raw());
        }
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.stop();
    }
}

fn wait_stop_or_event(stop: HANDLE, other: HANDLE, ms: u32) -> bool {
    let handles = [stop, other];
    unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, ms) == WAIT_OBJECT_0 }
}

fn is_stopped(stop: HANDLE) -> bool {
    unsafe { WaitForSingleObject(stop, 0) == WAIT_OBJECT_0 }
}

/// Shared passive HID input-report listener loop for any `HidPushSpec` device.
fn run_hid_push_listener(
    mut spec: Box<dyn HidPushSpec>,
    stop: HANDLE,
    wake: HANDLE,
    on_push: &dyn Fn(PushUpdate),
) {
    while !is_stopped(stop) {
        let collections = enumerate_hid();
        let Some((coll, mode)) = spec.select_collection(&collections) else {
            if wait_stop_or_event(stop, wake, 3000) {
                break;
            }
            continue;
        };

        let Some(dev) = HidHandle::open_read_overlapped(&coll.path) else {
            if wait_stop_or_event(stop, wake, 2000) {
                break;
            }
            continue;
        };

        let mut buf = vec![0u8; coll.input_len.max(33)];
        loop {
            match dev.read_timeout(&mut buf, 4000, Some(stop)) {
                ReadOutcome::Data(n) => {
                    if let Some(update) = spec.parse_report(&buf[..n], mode) {
                        on_push(update);
                    }
                }
                ReadOutcome::Timeout => {
                    if unsafe { WaitForSingleObject(wake, 0) == WAIT_OBJECT_0 } {
                        break;
                    }
                }
                ReadOutcome::Stopped => return,
                ReadOutcome::IoError => break,
            }
        }
    }
}

fn run_poller(shared: &SharedSync, mut providers: Vec<Box<dyn DeviceProvider>>) {
    let stop = shared.stop_event.raw();
    let refresh = shared.refresh_event.raw();
    let mut first_poll = true;

    loop {
        let now = Instant::now();
        let hid = enumerate_hid();
        let bt = scan_bluetooth();
        let ctx = PollContext {
            hid: &hid,
            bt: &bt,
            stop_event: Some(stop),
            now,
        };

        let mut outcomes = Vec::with_capacity(providers.len());
        for provider in &mut providers {
            if is_stopped(stop) {
                return;
            }
            outcomes.push(provider.poll(&ctx));
        }

        let mut alerts: Vec<(usize, Option<u8>, bool)> = Vec::with_capacity(outcomes.len());
        let (visual_changed, all_idle) = {
            let mut st = shared.state.lock().unwrap();
            let prev_sig = st.visual_signature();
            let settings = st.settings;

            for (idx, outcome) in outcomes.into_iter().enumerate() {
                let dev = &mut st.devices[idx];
                match outcome {
                    PollOutcome::Active {
                        percent,
                        charging,
                        mode,
                    } => {
                        dev.apply_active_reading(percent, charging, mode, now);
                    }
                    PollOutcome::PreserveActive { mode } => {
                        if dev.status != DeviceStatusKind::Active {
                            dev.status = DeviceStatusKind::Active;
                        }
                        dev.mode = Some(mode);
                    }
                    PollOutcome::Sleeping { mode } => {
                        dev.mark_sleeping(mode);
                    }
                    PollOutcome::Disconnected => {
                        dev.mark_disconnected();
                    }
                }
                let fired = dev.evaluate_low_battery_alert(
                    settings.low_battery_alerts,
                    settings.low_threshold,
                    settings.reset_threshold,
                );
                alerts.push((idx, dev.percent, fired));
            }

            st.last_poll = Some(now);
            let new_sig = st.visual_signature();
            let all_idle = st.devices.iter().all(|d| {
                d.status != DeviceStatusKind::Active && d.mode != Some(ConnectionMode::Wired)
            });
            (first_poll || prev_sig != new_sig, all_idle)
        };

        first_poll = false;
        for (idx, pct, fired) in alerts {
            shared.post_low_battery(idx, pct, fired);
        }
        if visual_changed {
            shared.post_ui(WM_APP_STATE_UPDATED, 0, 0);
        }

        let interval = if all_idle {
            POLL_SLEEP_MS
        } else {
            POLL_ACTIVE_MS
        };
        if wait_stop_or_event(stop, refresh, interval) {
            break;
        }
    }
}
