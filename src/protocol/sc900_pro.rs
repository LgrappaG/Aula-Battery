//! PixArt 093A:522C (8K wireless dongle) / 093A:622C (wired) HID protocol for Aula SC900 Pro.

pub const VID_PIXART: u16 = 0x093A;
pub const PID_WIRELESS: u16 = 0x522C;
pub const PID_WIRED: u16 = 0x622C;

pub const USAGE_PAGE: u16 = 0xFF05;
pub const USAGE: u16 = 0x0001;

pub const REPORT_ID: u8 = 0x09;
pub const FEATURE_LEN: usize = 9;
pub const CMD_STATUS: u8 = 0x89;
pub const CMD_DEVICE_INFO: u8 = 0x8F;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushLayout {
    Unknown,
    Packed,
    Old,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiverKind {
    Unknown,
    Standard,
    EightK,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseReading {
    pub percent: u8,
    pub charging: bool,
}

pub fn build_feature_query(cmd: u8) -> [u8; FEATURE_LEN] {
    let mut buf = [0u8; FEATURE_LEN];
    buf[0] = REPORT_ID;
    buf[1] = cmd;
    buf
}

/// Parses `09 8F` device info reply into `(model_code, receiver_kind)`.
pub fn parse_device_info(buf: &[u8]) -> Option<(u8, ReceiverKind)> {
    if buf.len() < 5 || buf[0] != REPORT_ID || buf[1] != CMD_DEVICE_INFO {
        return None;
    }
    if buf[2] == 0 && buf[3] == 0 && buf[4] == 0 {
        return None;
    }
    let kind = if buf[4] == 2 {
        ReceiverKind::EightK
    } else {
        ReceiverKind::Standard
    };
    Some((buf[3], kind))
}

/// Parses an interrupt Input Report `0x09` pushed by the mouse while awake.
pub fn parse_status_push(buf: &[u8], layout: &mut PushLayout) -> Option<MouseReading> {
    if buf.len() < 8 || buf[0] != REPORT_ID {
        return None;
    }
    let d = &buf[1..8];
    if d.iter().all(|&b| b == 0) {
        return None;
    }

    let packed = (d[0] & 0x7F) <= 100
        && (d[1] & 0x0F) <= 6
        && (d[1] >> 4) <= 5
        && (d[2] & 0x3F) <= 30;
    let old = d[0] <= 1 && d[1] <= 100 && d[2] <= 5 && d[3] <= 6 && d[5] == 0 && d[6] == 0;

    if !packed && !old {
        return None;
    }

    let use_layout = if packed != old {
        let settled = if packed {
            PushLayout::Packed
        } else {
            PushLayout::Old
        };
        *layout = settled;
        settled
    } else if *layout != PushLayout::Unknown {
        *layout
    } else {
        PushLayout::Old
    };

    let (percent, charging) = match use_layout {
        PushLayout::Old => (d[1], d[0] != 0),
        PushLayout::Packed | PushLayout::Unknown => (d[0] & 0x7F, (d[0] & 0x80) != 0),
    };

    Some(MouseReading { percent, charging })
}

/// Parses a `09 89` Feature Report response (valid only on wired USB-C or non-8K receivers).
pub fn parse_status_feature(buf: &[u8]) -> Option<MouseReading> {
    if buf.len() < 4 || buf[0] != REPORT_ID || buf[1] != CMD_STATUS {
        return None;
    }
    // Receiver "no data yet" placeholder has bytes 2 and 3 both zero.
    if (buf[2] | buf[3]) == 0 {
        return None;
    }
    let percent = buf[2] & 0x7F;
    if percent > 100 {
        return None;
    }
    Some(MouseReading {
        percent,
        charging: (buf[2] & 0x80) != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_live_sc900_8k_device_info() {
        let info = [0x09, 0x8F, 0x03, 0x0C, 0x02, 0xF0, 0xF0, 0x00, 0x00];
        assert_eq!(parse_device_info(&info), Some((0x0C, ReceiverKind::EightK)));
    }

    #[test]
    fn parses_packed_push_discharging_and_charging() {
        let mut layout = PushLayout::Unknown;
        let discharging = [0x09, 0x57, 0x13, 0x08, 0x00, 0x0F, 0x0F, 0x11, 0x00];
        assert_eq!(
            parse_status_push(&discharging, &mut layout),
            Some(MouseReading {
                percent: 87,
                charging: false,
            })
        );
        assert_eq!(layout, PushLayout::Packed);

        let charging = [0x09, 0xD7, 0x13, 0x08, 0x00, 0x0F, 0x0F, 0x11, 0x00];
        assert_eq!(
            parse_status_push(&charging, &mut layout),
            Some(MouseReading {
                percent: 87,
                charging: true,
            })
        );
    }

    #[test]
    fn parses_old_push_layout() {
        let mut layout = PushLayout::Unknown;
        // Ambiguous (0x40 fits packed rate/dpi too): decodes as Old without locking layout yet.
        let ambiguous_old = [0x09, 0x01, 64, 0x02, 0x01, 0x00, 0x00, 0x00];
        assert_eq!(
            parse_status_push(&ambiguous_old, &mut layout),
            Some(MouseReading {
                percent: 64,
                charging: true,
            })
        );
        assert_eq!(layout, PushLayout::Unknown);

        // Unambiguous Old (89 = 0x59 -> low nibble 9 > 6 cannot be packed polling rate index).
        let unambiguous_old = [0x09, 0x01, 89, 0x02, 0x01, 0x00, 0x00, 0x00];
        assert_eq!(
            parse_status_push(&unambiguous_old, &mut layout),
            Some(MouseReading {
                percent: 89,
                charging: true,
            })
        );
        assert_eq!(layout, PushLayout::Old);
    }

    #[test]
    fn rejects_stale_8k_placeholder_09_89() {
        // Observed live response from sleeping SC900PRO 8K dongle: 09-89-00-08-00-0F-0F-01-00
        let stale_zero_pct = [0x09, 0x89, 0x00, 0x08, 0x00, 0x0F, 0x0F, 0x01, 0x00];
        assert_eq!(
            parse_status_feature(&stale_zero_pct),
            Some(MouseReading {
                percent: 0,
                charging: false,
            })
        );
        // Note: receiver-side 09 89 is never used when ReceiverKind::EightK;
        // placeholder with bytes 2 and 3 both zero is rejected:
        let empty = [0x09, 0x89, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(parse_status_feature(&empty), None);
    }
}
