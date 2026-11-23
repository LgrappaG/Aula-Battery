use std::time::Instant;

/// Category of a monitored peripheral (metadata for future icons/grouping).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Keyboard,
    Mouse,
    Headset,
    Gamepad,
    Other,
}

/// Physical transport currently supplying data for a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionMode {
    Wireless24G,
    Wireless24G8K,
    Wired,
    Bluetooth,
}

impl ConnectionMode {
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Wireless24G => "2.4G",
            Self::Wireless24G8K => "2.4G 8K",
            Self::Wired => "USB-C",
            Self::Bluetooth => "BT",
        }
    }
}

/// Lifecycle state of a peripheral.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStatusKind {
    Active,
    Sleeping,
    Disconnected,
}

/// Compact equality key representing everything visible in the tray icon and tooltip.
/// Used to skip redundant GDI icon allocations and Shell_NotifyIconW updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceVisualKey {
    pub percent: Option<u8>,
    pub charging: bool,
    pub mode: Option<ConnectionMode>,
    pub status: DeviceStatusKind,
}

/// Runtime state and cached battery reading for one peripheral.
#[derive(Debug, Clone)]
pub struct DeviceState {
    #[allow(dead_code)]
    pub id: &'static str,
    pub name: &'static str,
    #[allow(dead_code)]
    pub kind: DeviceKind,
    pub percent: Option<u8>,
    pub charging: bool,
    pub mode: Option<ConnectionMode>,
    pub status: DeviceStatusKind,
    pub last_updated: Option<Instant>,
    pub low_alert_fired: bool,
}

impl DeviceState {
    pub fn new(id: &'static str, name: &'static str, kind: DeviceKind) -> Self {
        Self {
            id,
            name,
            kind,
            percent: None,
            charging: false,
            mode: None,
            status: DeviceStatusKind::Disconnected,
            last_updated: None,
            low_alert_fired: false,
        }
    }

    pub fn visual_key(&self) -> DeviceVisualKey {
        DeviceVisualKey {
            percent: self.percent,
            charging: self.charging,
            mode: self.mode,
            status: self.status,
        }
    }

    /// Applies an active reading. `percent` of `None` preserves any cached percentage
    /// (useful when wired mode reports charging without an accurate percentage).
    pub fn apply_active_reading(
        &mut self,
        percent: Option<u8>,
        charging: bool,
        mode: ConnectionMode,
        now: Instant,
    ) {
        if let Some(p) = percent {
            self.percent = Some(p.min(100));
        }
        self.charging = charging;
        self.mode = Some(mode);
        self.status = DeviceStatusKind::Active;
        self.last_updated = Some(now);
    }

    /// Transitions to sleeping while preserving the last valid battery percentage.
    pub fn mark_sleeping(&mut self, mode: ConnectionMode) {
        self.charging = false;
        self.mode = Some(mode);
        self.status = DeviceStatusKind::Sleeping;
    }

    /// Transitions to disconnected when the receiver/cable/BT link is gone.
    pub fn mark_disconnected(&mut self) {
        self.percent = None;
        self.charging = false;
        self.mode = None;
        self.status = DeviceStatusKind::Disconnected;
        self.low_alert_fired = false;
    }

    /// Evaluates low-battery notification hysteresis. Returns `true` exactly once
    /// when transitioning into the low-battery condition while active and discharging.
    pub fn evaluate_low_battery_alert(
        &mut self,
        enabled: bool,
        low_threshold: u8,
        reset_threshold: u8,
    ) -> bool {
        if self.status == DeviceStatusKind::Disconnected || self.charging {
            self.low_alert_fired = false;
            return false;
        }
        if let Some(p) = self.percent {
            if p > reset_threshold {
                self.low_alert_fired = false;
                return false;
            }
            if enabled && self.status == DeviceStatusKind::Active && p <= low_threshold && !self.low_alert_fired {
                self.low_alert_fired = true;
                return true;
            }
        }
        false
    }
}

/// Persistent user preferences.
#[derive(Debug, Clone, Copy)]
pub struct AppSettings {
    pub low_battery_alerts: bool,
    pub low_threshold: u8,
    pub reset_threshold: u8,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            low_battery_alerts: true,
            low_threshold: 20,
            reset_threshold: 25,
        }
    }
}

/// Complete shared application state rendered by the tray and popup.
#[derive(Debug, Clone)]
pub struct AppState {
    pub devices: Vec<DeviceState>,
    pub settings: AppSettings,
    pub last_poll: Option<Instant>,
}

impl AppState {
    pub fn from_devices(devices: Vec<DeviceState>, settings: AppSettings) -> Self {
        Self {
            devices,
            settings,
            last_poll: None,
        }
    }

    pub fn visual_signature(&self) -> Vec<DeviceVisualKey> {
        self.devices.iter().map(DeviceState::visual_key).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_preserves_last_percent_and_clears_charging() {
        let mut kb = DeviceState::new("aula_f87", "Aula F87", DeviceKind::Keyboard);
        let now = Instant::now();
        kb.apply_active_reading(Some(87), true, ConnectionMode::Wireless24G, now);
        kb.mark_sleeping(ConnectionMode::Wireless24G);

        assert_eq!(kb.status, DeviceStatusKind::Sleeping);
        assert_eq!(kb.percent, Some(87));
        assert!(!kb.charging);
        assert_eq!(kb.mode, Some(ConnectionMode::Wireless24G));
    }

    #[test]
    fn wired_charging_without_percent_preserves_cached_level() {
        let mut kb = DeviceState::new("aula_f87", "Aula F87", DeviceKind::Keyboard);
        let now = Instant::now();
        kb.apply_active_reading(Some(62), false, ConnectionMode::Wireless24G, now);
        kb.apply_active_reading(None, true, ConnectionMode::Wired, now);

        assert_eq!(kb.status, DeviceStatusKind::Active);
        assert_eq!(kb.percent, Some(62));
        assert!(kb.charging);
        assert_eq!(kb.mode, Some(ConnectionMode::Wired));
    }

    #[test]
    fn visual_key_tracks_only_visible_changes() {
        let mut mouse = DeviceState::new("sc900_pro", "SC900 Pro", DeviceKind::Mouse);
        let t1 = Instant::now();
        mouse.apply_active_reading(Some(91), false, ConnectionMode::Wireless24G8K, t1);
        let k1 = mouse.visual_key();

        // Identical push 2s later only advances `last_updated`, leaving visual_key identical.
        mouse.apply_active_reading(Some(91), false, ConnectionMode::Wireless24G8K, t1);
        assert_eq!(mouse.visual_key(), k1);

        // Battery drop changes visual_key.
        mouse.apply_active_reading(Some(90), false, ConnectionMode::Wireless24G8K, t1);
        assert_ne!(mouse.visual_key(), k1);
    }

    #[test]
    fn low_battery_alert_hysteresis() {
        let mut mouse = DeviceState::new("sc900_pro", "SC900 Pro", DeviceKind::Mouse);
        let now = Instant::now();
        mouse.apply_active_reading(Some(25), false, ConnectionMode::Wireless24G8K, now);
        assert!(!mouse.evaluate_low_battery_alert(true, 20, 25));

        mouse.apply_active_reading(Some(20), false, ConnectionMode::Wireless24G8K, now);
        assert!(mouse.evaluate_low_battery_alert(true, 20, 25));
        mouse.apply_active_reading(Some(18), false, ConnectionMode::Wireless24G8K, now);
        assert!(!mouse.evaluate_low_battery_alert(true, 20, 25));

        mouse.apply_active_reading(Some(22), false, ConnectionMode::Wireless24G8K, now);
        assert!(!mouse.evaluate_low_battery_alert(true, 20, 25));
        mouse.apply_active_reading(Some(19), false, ConnectionMode::Wireless24G8K, now);
        assert!(!mouse.evaluate_low_battery_alert(true, 20, 25));

        mouse.apply_active_reading(Some(19), true, ConnectionMode::Wired, now);
        assert!(!mouse.evaluate_low_battery_alert(true, 20, 25));

        mouse.apply_active_reading(Some(19), false, ConnectionMode::Wireless24G8K, now);
        assert!(mouse.evaluate_low_battery_alert(true, 20, 25));

        mouse.apply_active_reading(Some(26), false, ConnectionMode::Wireless24G8K, now);
        assert!(!mouse.evaluate_low_battery_alert(true, 20, 25));
        mouse.apply_active_reading(Some(19), false, ConnectionMode::Wireless24G8K, now);
        assert!(mouse.evaluate_low_battery_alert(true, 20, 25));
    }
}
