pub mod aula_f87;
pub mod sc900_pro;

use std::time::Instant;

use windows_sys::Win32::Foundation::HANDLE;

use crate::hid::bluetooth::BluetoothDeviceInfo;
use crate::hid::HidCollectionInfo;
use crate::model::{ConnectionMode, DeviceKind};

/// Shared per-poll snapshot passed to every registered `DeviceProvider`.
pub struct PollContext<'a> {
    pub hid: &'a [HidCollectionInfo],
    pub bt: &'a [BluetoothDeviceInfo],
    pub stop_event: Option<HANDLE>,
    pub now: Instant,
}

/// Result of polling a single device provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollOutcome {
    /// Device responded actively with an optional percentage and charging state.
    Active {
        percent: Option<u8>,
        charging: bool,
        mode: ConnectionMode,
    },
    /// Push-based device recently pushed live state on its listener thread; keep active.
    PreserveActive { mode: ConnectionMode },
    /// Dongle is present, but device is asleep or unreachable; keep last cached percentage.
    Sleeping { mode: ConnectionMode },
    /// Neither dongle, cable, nor Bluetooth connection is present.
    Disconnected,
}

/// Asynchronous push reading emitted by an optional `HidPushSpec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushUpdate {
    pub percent: u8,
    pub charging: bool,
    pub mode: ConnectionMode,
}

/// Declarative spec for devices that stream passive HID input reports.
/// The worker runs the Win32 overlapped listener & reconnect loop automatically.
pub trait HidPushSpec: Send {
    fn select_collection(
        &self,
        hid: &[HidCollectionInfo],
    ) -> Option<(HidCollectionInfo, ConnectionMode)>;
    fn parse_report(&mut self, data: &[u8], mode: ConnectionMode) -> Option<PushUpdate>;
}

/// Extensible hardware device provider contract.
/// Adding a new device requires implementing this trait and registering it in `create_providers()`.
pub trait DeviceProvider: Send {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn kind(&self) -> DeviceKind;
    fn matches_hid(&self, c: &HidCollectionInfo) -> bool;
    fn poll(&mut self, ctx: &PollContext<'_>) -> PollOutcome;

    fn push_spec(&self) -> Option<Box<dyn HidPushSpec>> {
        None
    }

    fn probe_details(&mut self, ctx: &PollContext<'_>) -> String {
        format!("{}: {:?}", self.name(), self.poll(ctx))
    }
}

/// Registry of all supported devices in display order.
pub fn create_providers() -> Vec<Box<dyn DeviceProvider>> {
    vec![
        Box::new(aula_f87::AulaF87Provider::new()),
        Box::new(sc900_pro::Sc900ProProvider::new()),
    ]
}
