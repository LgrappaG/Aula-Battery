//! Compx 2.4GHz HID protocol (VID 0x3554 / PID 0xFA09) for Aula F87 / F87 Pro.

pub const VID_COMPX: u16 = 0x3554;
pub const PID_COMPX_FA09: u16 = 0xFA09;
pub const VID_WIRED: u16 = 0x258A;
pub const PID_WIRED: u16 = 0x010C;

pub const USAGE_PAGE: u16 = 0xFF02;
pub const USAGE: u16 = 0x0002;

pub const REPORT_ID: u8 = 0x13;
pub const FRAME_LEN: usize = 20;
pub const CMD_BATTERY: u8 = 0x4A;
pub const CMD_PUSH_STATUS: u8 = 0x0A;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryReading {
    pub percent: Option<u8>,
    pub charging: bool,
}

/// Computes the 8-bit modular checksum over the first 19 bytes.
pub fn checksum(frame: &[u8]) -> u8 {
    let len = frame.len().min(FRAME_LEN - 1);
    frame[..len].iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
}

/// Builds a 20-byte Compx output report with trailing checksum.
pub fn build_frame(cmd: u8, arg: u8) -> [u8; FRAME_LEN] {
    let mut frame = [0u8; FRAME_LEN];
    frame[0] = REPORT_ID;
    frame[1] = cmd;
    frame[2] = arg;
    frame[FRAME_LEN - 1] = checksum(&frame);
    frame
}

/// Parses a 20-byte Compx input report into a battery reading.
/// Rejects request echoes, unrelated commands, and corrupt checksums.
pub fn parse_response(frame: &[u8]) -> Option<BatteryReading> {
    if frame.len() < FRAME_LEN || frame[0] != REPORT_ID {
        return None;
    }

    // Reject our own echoed request (payload bytes 4..19 all zero).
    if frame[4..FRAME_LEN - 1].iter().all(|&b| b == 0) {
        return None;
    }

    let valid_checksum = frame[FRAME_LEN - 1] == checksum(frame);
    let valid_zero_trailer = frame[FRAME_LEN - 1] == 0 && frame[2] == 1 && frame[3] == 0;
    if !valid_checksum && !valid_zero_trailer {
        return None;
    }

    let (raw_level, flags) = if (frame[1] & 0x7F) == CMD_BATTERY
        && frame[2] == 0x01
        && frame[3] == 0x00
        && frame[4] == 0x02
    {
        (frame[5], frame[6])
    } else if frame[1] == CMD_PUSH_STATUS
        && frame[2] == 0x01
        && frame[4] == 0x04
        && frame[5] == 0x05
    {
        (frame[6], frame[7])
    } else {
        return None;
    };

    let charging = (flags & 0x10) != 0;
    let percent = if (1..=100).contains(&raw_level) {
        Some(raw_level)
    } else if charging {
        None
    } else {
        return None;
    };

    Some(BatteryReading { percent, charging })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_query_checksums_match_spec() {
        let q1 = build_frame(CMD_BATTERY, 0x01);
        assert_eq!(q1[0], 0x13);
        assert_eq!(q1[1], 0x4A);
        assert_eq!(q1[2], 0x01);
        assert_eq!(q1[19], 0x5E);

        let q0 = build_frame(CMD_BATTERY, 0x00);
        assert_eq!(q0[19], 0x5D);
    }

    #[test]
    fn parses_verified_live_capture_100_percent() {
        let frame: [u8; 20] = [
            0x13, 0x4A, 0x01, 0x00, 0x02, 0x64, 0x01, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC5,
        ];
        assert_eq!(
            parse_response(&frame),
            Some(BatteryReading {
                percent: Some(100),
                charging: false,
            })
        );
    }

    #[test]
    fn parses_charging_flag_and_high_bit_command() {
        let mut frame = [0u8; 20];
        frame[0] = 0x13;
        frame[1] = 0xCA; // 0x4A | 0x80
        frame[2] = 0x01;
        frame[3] = 0x00;
        frame[4] = 0x02;
        frame[5] = 78;
        frame[6] = 0x10;
        frame[19] = checksum(&frame);

        assert_eq!(
            parse_response(&frame),
            Some(BatteryReading {
                percent: Some(78),
                charging: true,
            })
        );
    }

    #[test]
    fn parses_push_0a_format() {
        let mut frame = [0u8; 20];
        frame[0] = 0x13;
        frame[1] = 0x0A;
        frame[2] = 0x01;
        frame[4] = 0x04;
        frame[5] = 0x05;
        frame[6] = 63;
        frame[7] = 0x01;
        frame[19] = checksum(&frame);

        assert_eq!(
            parse_response(&frame),
            Some(BatteryReading {
                percent: Some(63),
                charging: false,
            })
        );
    }

    #[test]
    fn rejects_echo_and_bad_checksum() {
        let echo = build_frame(CMD_BATTERY, 0x01);
        assert_eq!(parse_response(&echo), None);

        let mut bad = [
            0x13, 0x4A, 0x01, 0x00, 0x02, 0x64, 0x01, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC6,
        ];
        assert_eq!(parse_response(&bad), None);
        bad[19] = 0xC5;
        assert!(parse_response(&bad).is_some());
    }
}
