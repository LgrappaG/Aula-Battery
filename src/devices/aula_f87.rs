use std::time::{Duration, Instant};

use crate::devices::{DeviceProvider, PollContext, PollOutcome};
use crate::hid::bluetooth::find_connected_battery;
use crate::hid::{HidCollectionInfo, HidHandle, ReadOutcome};
use crate::model::{ConnectionMode, DeviceKind};
use crate::protocol::aula_f87::{
    build_frame, parse_response, BatteryReading, CMD_BATTERY, FRAME_LEN, PID_COMPX_FA09,
    PID_WIRED, USAGE, USAGE_PAGE, VID_COMPX, VID_WIRED,
};

pub struct AulaF87Provider;

impl AulaF87Provider {
    pub fn new() -> Self {
        Self
    }

    fn poll_wireless(
        collections: &[HidCollectionInfo],
        wired_present: bool,
        stop_event: Option<windows_sys::Win32::Foundation::HANDLE>,
    ) -> Option<(BatteryReading, ConnectionMode)> {
        let coll = collections.iter().find(|c| {
            c.vid == VID_COMPX
                && c.pid == PID_COMPX_FA09
                && c.usage_page == USAGE_PAGE
                && c.usage == USAGE
                && c.input_len >= FRAME_LEN
                && c.output_len >= FRAME_LEN
        })?;

        let dev = HidHandle::open_rw_overlapped(&coll.path)?;
        let mut buf = vec![0u8; coll.input_len.max(FRAME_LEN)];
        dev.drain_input(&mut buf, 16);

        // Alternate arg 0x01 and 0x00 across two attempts for sleepy links.
        for (attempt, arg) in [0x01u8, 0x00u8].iter().enumerate() {
            let req = build_frame(CMD_BATTERY, *arg);
            let sent =
                dev.set_output_report(&req) || (attempt == 0 && dev.write_timeout(&req, 200));
            if !sent {
                continue;
            }

            let deadline = Instant::now() + Duration::from_millis(1200);
            while Instant::now() < deadline {
                let rem = deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis()
                    .clamp(10, 250) as u32;
                match dev.read_timeout(&mut buf, rem, stop_event) {
                    ReadOutcome::Data(n) => {
                        if let Some(reading) = parse_response(&buf[..n]) {
                            let mode = if wired_present && reading.charging {
                                ConnectionMode::Wired
                            } else {
                                ConnectionMode::Wireless24G
                            };
                            return Some((reading, mode));
                        }
                    }
                    ReadOutcome::Timeout => continue,
                    ReadOutcome::Stopped | ReadOutcome::IoError => break,
                }
            }
        }
        None
    }
}

pub(crate) fn resolve_f87_outcome(
    wireless_reading: Option<(BatteryReading, ConnectionMode)>,
    wired_present: bool,
    dongle_present: bool,
    bt_percent: Option<u8>,
) -> PollOutcome {
    if let Some((reading, mode)) = wireless_reading {
        return PollOutcome::Active {
            percent: reading.percent,
            charging: reading.charging,
            mode,
        };
    }

    if wired_present {
        return PollOutcome::Active {
            percent: None,
            charging: true,
            mode: ConnectionMode::Wired,
        };
    }

    if let Some(bt_pct) = bt_percent {
        return PollOutcome::Active {
            percent: Some(bt_pct),
            charging: false,
            mode: ConnectionMode::Bluetooth,
        };
    }

    if dongle_present {
        PollOutcome::Sleeping {
            mode: ConnectionMode::Wireless24G,
        }
    } else {
        PollOutcome::Disconnected
    }
}

impl DeviceProvider for AulaF87Provider {
    fn id(&self) -> &'static str {
        "aula_f87"
    }

    fn name(&self) -> &'static str {
        "Aula F87"
    }

    fn kind(&self) -> DeviceKind {
        DeviceKind::Keyboard
    }

    fn matches_hid(&self, c: &HidCollectionInfo) -> bool {
        (c.vid == VID_COMPX && c.pid == PID_COMPX_FA09)
            || (c.vid == VID_WIRED && c.pid == PID_WIRED)
    }

    fn poll(&mut self, ctx: &PollContext<'_>) -> PollOutcome {
        let wired_present = ctx
            .hid
            .iter()
            .any(|c| c.vid == VID_WIRED && c.pid == PID_WIRED);
        let dongle_present = ctx.hid.iter().any(|c| {
            c.vid == VID_COMPX
                && c.pid == PID_COMPX_FA09
                && c.usage_page == USAGE_PAGE
                && c.usage == USAGE
        });
        let wireless = Self::poll_wireless(ctx.hid, wired_present, ctx.stop_event);
        let bt_pct = find_connected_battery(ctx.bt, "f87");

        resolve_f87_outcome(wireless, wired_present, dongle_present, bt_pct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_wireless_beats_bt_and_dongle_sleep() {
        let reading = BatteryReading {
            percent: Some(88),
            charging: false,
        };
        assert_eq!(
            resolve_f87_outcome(
                Some((reading, ConnectionMode::Wireless24G)),
                false,
                true,
                Some(70),
            ),
            PollOutcome::Active {
                percent: Some(88),
                charging: false,
                mode: ConnectionMode::Wireless24G,
            }
        );
    }

    #[test]
    fn wired_beats_bt_and_sleeping_when_wireless_silent() {
        assert_eq!(
            resolve_f87_outcome(None, true, true, Some(75)),
            PollOutcome::Active {
                percent: None,
                charging: true,
                mode: ConnectionMode::Wired,
            }
        );
    }

    #[test]
    fn bt_beats_dongle_sleep_when_switched_to_bluetooth() {
        assert_eq!(
            resolve_f87_outcome(None, false, true, Some(64)),
            PollOutcome::Active {
                percent: Some(64),
                charging: false,
                mode: ConnectionMode::Bluetooth,
            }
        );
    }

    #[test]
    fn dongle_without_reply_or_bt_reports_sleeping() {
        assert_eq!(
            resolve_f87_outcome(None, false, true, None),
            PollOutcome::Sleeping {
                mode: ConnectionMode::Wireless24G,
            }
        );
    }

    #[test]
    fn absent_hardware_reports_disconnected() {
        assert_eq!(
            resolve_f87_outcome(None, false, false, None),
            PollOutcome::Disconnected
        );
    }
}
