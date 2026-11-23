use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::devices::{DeviceProvider, HidPushSpec, PollContext, PollOutcome, PushUpdate};
use crate::hid::bluetooth::find_connected_battery;
use crate::hid::{HidCollectionInfo, HidHandle, ReadOutcome};
use crate::model::{ConnectionMode, DeviceKind};
use crate::protocol::sc900_pro::{
    build_feature_query, parse_device_info, parse_status_feature, parse_status_push,
    MouseReading, PushLayout, ReceiverKind, CMD_DEVICE_INFO, CMD_STATUS, FEATURE_LEN, PID_WIRED,
    PID_WIRELESS, USAGE, USAGE_PAGE, VID_PIXART,
};

const SC900_SLEEP_TIMEOUT: Duration = Duration::from_secs(180);

pub struct Sc900ProProvider {
    cached_receiver_kind: ReceiverKind,
    last_push: Arc<Mutex<Option<Instant>>>,
}

impl Sc900ProProvider {
    pub fn new() -> Self {
        Self {
            cached_receiver_kind: ReceiverKind::Unknown,
            last_push: Arc::new(Mutex::new(None)),
        }
    }

    fn has_pid(collections: &[HidCollectionInfo], pid: u16) -> bool {
        collections
            .iter()
            .any(|c| c.vid == VID_PIXART && c.pid == pid && c.usage_page == USAGE_PAGE)
    }

    /// Queries `09 8F` device info on the 2.4GHz dongle to identify standard vs 8K receiver.
    fn probe_receiver_kind(collections: &[HidCollectionInfo]) -> ReceiverKind {
        let candidates: Vec<&HidCollectionInfo> = collections
            .iter()
            .filter(|c| {
                c.vid == VID_PIXART
                    && c.pid == PID_WIRELESS
                    && c.usage_page == USAGE_PAGE
                    && c.feature_len >= FEATURE_LEN
            })
            .collect();

        for coll in candidates.into_iter().rev() {
            let Some(dev) = HidHandle::open_feature_only(&coll.path) else {
                continue;
            };
            let req = build_feature_query(CMD_DEVICE_INFO);
            if !dev.set_feature(&req) {
                continue;
            }
            let mut resp = vec![0u8; coll.feature_len];
            for _ in 0..8 {
                thread::sleep(Duration::from_millis(45));
                resp[0] = req[0];
                if !dev.get_feature(&mut resp) {
                    break;
                }
                if let Some((_model, kind)) = parse_device_info(&resp) {
                    return kind;
                }
            }
        }
        ReceiverKind::Unknown
    }

    /// Polls `09 89` feature report when wired (`093A:622C`) or on a non-8K dongle.
    /// Never called on an 8K wireless receiver (`ReceiverKind::EightK`).
    fn poll_feature_status(collections: &[HidCollectionInfo], pid: u16) -> Option<MouseReading> {
        let candidates: Vec<&HidCollectionInfo> = collections
            .iter()
            .filter(|c| {
                c.vid == VID_PIXART
                    && c.pid == pid
                    && c.usage_page == USAGE_PAGE
                    && c.feature_len >= FEATURE_LEN
            })
            .collect();

        // Try in reverse order: on USB-C cable the first FF05 collection rejects SET_FEATURE.
        for coll in candidates.into_iter().rev() {
            let Some(dev) = HidHandle::open_feature_only(&coll.path) else {
                continue;
            };
            let req = build_feature_query(CMD_STATUS);
            if !dev.set_feature(&req) {
                continue;
            }
            let mut resp = vec![0u8; coll.feature_len];
            for _ in 0..10 {
                thread::sleep(Duration::from_millis(50));
                resp[0] = req[0];
                if !dev.get_feature(&mut resp) {
                    break;
                }
                if let Some(reading) = parse_status_feature(&resp) {
                    return Some(reading);
                }
            }
        }
        None
    }

    fn find_input_collection(collections: &[HidCollectionInfo]) -> Option<(HidCollectionInfo, bool)> {
        for (pid, is_wired) in [(PID_WIRED, true), (PID_WIRELESS, false)] {
            if let Some(coll) = collections.iter().find(|c| {
                c.vid == VID_PIXART
                    && c.pid == pid
                    && c.usage_page == USAGE_PAGE
                    && c.usage == USAGE
                    && c.input_len >= 8
            }) {
                return Some((coll.clone(), is_wired));
            }
            if let Some(coll) = collections.iter().find(|c| {
                c.vid == VID_PIXART
                    && c.pid == pid
                    && c.usage_page == USAGE_PAGE
                    && c.input_len >= 8
            }) {
                return Some((coll.clone(), is_wired));
            }
        }
        None
    }
}

pub(crate) fn resolve_sc900_outcome(
    wired_present: bool,
    wired_reading: Option<MouseReading>,
    wireless_present: bool,
    receiver_kind: ReceiverKind,
    std_wireless_reading: Option<MouseReading>,
    recent_push: bool,
    bt_percent: Option<u8>,
) -> PollOutcome {
    if wired_present {
        if let Some(r) = wired_reading {
            return PollOutcome::Active {
                percent: Some(r.percent),
                charging: r.charging,
                mode: ConnectionMode::Wired,
            };
        }
        return PollOutcome::Active {
            percent: None,
            charging: true,
            mode: ConnectionMode::Wired,
        };
    }

    if wireless_present {
        let mode = if receiver_kind == ReceiverKind::Standard {
            ConnectionMode::Wireless24G
        } else {
            ConnectionMode::Wireless24G8K
        };

        if receiver_kind == ReceiverKind::Standard {
            if let Some(r) = std_wireless_reading {
                return PollOutcome::Active {
                    percent: Some(r.percent),
                    charging: r.charging,
                    mode,
                };
            }
        }

        if recent_push {
            return PollOutcome::PreserveActive { mode };
        }

        if let Some(bt_pct) = bt_percent {
            return PollOutcome::Active {
                percent: Some(bt_pct),
                charging: false,
                mode: ConnectionMode::Bluetooth,
            };
        }

        return PollOutcome::Sleeping { mode };
    }

    if let Some(bt_pct) = bt_percent {
        return PollOutcome::Active {
            percent: Some(bt_pct),
            charging: false,
            mode: ConnectionMode::Bluetooth,
        };
    }

    PollOutcome::Disconnected
}

impl DeviceProvider for Sc900ProProvider {
    fn id(&self) -> &'static str {
        "aula_sc900_pro"
    }

    fn name(&self) -> &'static str {
        "SC900 Pro"
    }

    fn kind(&self) -> DeviceKind {
        DeviceKind::Mouse
    }

    fn matches_hid(&self, c: &HidCollectionInfo) -> bool {
        c.vid == VID_PIXART && (c.pid == PID_WIRELESS || c.pid == PID_WIRED)
    }

    fn push_spec(&self) -> Option<Box<dyn HidPushSpec>> {
        Some(Box::new(Sc900PushSpec {
            last_push: Arc::clone(&self.last_push),
            layout: PushLayout::Unknown,
        }))
    }

    fn poll(&mut self, ctx: &PollContext<'_>) -> PollOutcome {
        let wired = Self::has_pid(ctx.hid, PID_WIRED);
        let wireless = Self::has_pid(ctx.hid, PID_WIRELESS);

        if !wireless {
            self.cached_receiver_kind = ReceiverKind::Unknown;
        } else if self.cached_receiver_kind == ReceiverKind::Unknown {
            self.cached_receiver_kind = Self::probe_receiver_kind(ctx.hid);
        }

        let wired_reading = if wired {
            Self::poll_feature_status(ctx.hid, PID_WIRED)
        } else {
            None
        };

        let std_wireless_reading =
            if !wired && wireless && self.cached_receiver_kind == ReceiverKind::Standard {
                Self::poll_feature_status(ctx.hid, PID_WIRELESS)
            } else {
                None
            };

        let recent_push = (*self.last_push.lock().unwrap())
            .map(|t| ctx.now.saturating_duration_since(t) <= SC900_SLEEP_TIMEOUT)
            .unwrap_or(false);
        let bt_pct = find_connected_battery(ctx.bt, "sc900");

        let outcome = resolve_sc900_outcome(
            wired,
            wired_reading,
            wireless,
            self.cached_receiver_kind,
            std_wireless_reading,
            recent_push,
            bt_pct,
        );
        if outcome == PollOutcome::Disconnected {
            *self.last_push.lock().unwrap() = None;
        }
        outcome
    }

    fn probe_details(&mut self, ctx: &PollContext<'_>) -> String {
        let wired = Self::has_pid(ctx.hid, PID_WIRED);
        let wireless = Self::has_pid(ctx.hid, PID_WIRELESS);
        if !wired && !wireless {
            let bt = find_connected_battery(ctx.bt, "sc900");
            return format!("SC900 Pro: HID absent, Bluetooth={:?}", bt);
        }

        let mut parts = Vec::new();
        if wireless {
            let kind = Self::probe_receiver_kind(ctx.hid);
            parts.push(format!("2.4G dongle receiver_kind={:?}", kind));
        }
        if wired {
            let feat = Self::poll_feature_status(ctx.hid, PID_WIRED);
            parts.push(format!("USB-C feature={:?}", feat));
        }
        if let Some((coll, is_wired)) = Self::find_input_collection(ctx.hid) {
            if let Some(dev) = HidHandle::open_read_overlapped(&coll.path) {
                let mut buf = vec![0u8; coll.input_len.max(33)];
                let mut layout = PushLayout::Unknown;
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut got = None;
                while Instant::now() < deadline {
                    if let ReadOutcome::Data(n) = dev.read_timeout(&mut buf, 500, None) {
                        if let Some(r) = parse_status_push(&buf[..n], &mut layout) {
                            got = Some((r, buf[..n.min(9)].to_vec()));
                            break;
                        }
                    }
                }
                match got {
                    Some((r, raw)) => {
                        parts.push(format!("live push(wired={}): {:?}, raw={:02X?}", is_wired, r, raw));
                    }
                    None => parts.push("FF05:0001 ready; no push in 3s (mouse asleep/idle)".to_string()),
                }
            } else {
                parts.push("ERROR opening FF05:0001 listener".to_string());
            }
        }
        format!("SC900 Pro: {}", parts.join(" | "))
    }
}

struct Sc900PushSpec {
    last_push: Arc<Mutex<Option<Instant>>>,
    layout: PushLayout,
}

impl HidPushSpec for Sc900PushSpec {
    fn select_collection(
        &self,
        hid: &[HidCollectionInfo],
    ) -> Option<(HidCollectionInfo, ConnectionMode)> {
        let (coll, is_wired) = Sc900ProProvider::find_input_collection(hid)?;
        let mode = if is_wired {
            ConnectionMode::Wired
        } else {
            ConnectionMode::Wireless24G8K
        };
        Some((coll, mode))
    }

    fn parse_report(&mut self, data: &[u8], mode: ConnectionMode) -> Option<PushUpdate> {
        let reading = parse_status_push(data, &mut self.layout)?;
        *self.last_push.lock().unwrap() = Some(Instant::now());
        Some(PushUpdate {
            percent: reading.percent,
            charging: reading.charging,
            mode,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wired_overrides_wireless_dongle() {
        let wired_r = MouseReading {
            percent: 72,
            charging: true,
        };
        assert_eq!(
            resolve_sc900_outcome(
                true,
                Some(wired_r),
                true,
                ReceiverKind::EightK,
                None,
                true,
                Some(50),
            ),
            PollOutcome::Active {
                percent: Some(72),
                charging: true,
                mode: ConnectionMode::Wired,
            }
        );

        // Wired fallback without feature reading still reports charging on USB-C.
        assert_eq!(
            resolve_sc900_outcome(true, None, true, ReceiverKind::EightK, None, false, None),
            PollOutcome::Active {
                percent: None,
                charging: true,
                mode: ConnectionMode::Wired,
            }
        );
    }

    #[test]
    fn eight_k_dongle_uses_push_then_bt_then_sleeping() {
        assert_eq!(
            resolve_sc900_outcome(false, None, true, ReceiverKind::EightK, None, true, Some(60)),
            PollOutcome::PreserveActive {
                mode: ConnectionMode::Wireless24G8K,
            }
        );

        assert_eq!(
            resolve_sc900_outcome(
                false,
                None,
                true,
                ReceiverKind::EightK,
                None,
                false,
                Some(60),
            ),
            PollOutcome::Active {
                percent: Some(60),
                charging: false,
                mode: ConnectionMode::Bluetooth,
            }
        );

        assert_eq!(
            resolve_sc900_outcome(false, None, true, ReceiverKind::EightK, None, false, None),
            PollOutcome::Sleeping {
                mode: ConnectionMode::Wireless24G8K,
            }
        );
    }

    #[test]
    fn standard_receiver_uses_feature_reading_when_available() {
        let std_r = MouseReading {
            percent: 84,
            charging: false,
        };
        assert_eq!(
            resolve_sc900_outcome(
                false,
                None,
                true,
                ReceiverKind::Standard,
                Some(std_r),
                false,
                None,
            ),
            PollOutcome::Active {
                percent: Some(84),
                charging: false,
                mode: ConnectionMode::Wireless24G,
            }
        );
    }

    #[test]
    fn disconnected_when_no_transport_present() {
        assert_eq!(
            resolve_sc900_outcome(false, None, false, ReceiverKind::Unknown, None, false, None),
            PollOutcome::Disconnected
        );
    }
}
