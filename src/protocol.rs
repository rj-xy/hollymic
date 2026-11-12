//! Lark M2 USB control protocol (Hollyland product family 6301).
//!
//! Every request and reply is one 64-byte interrupt transfer on USB interface 0
//! (HID): OUT endpoint 0x03, IN endpoint 0x82. Sources: HollyAudio 3.1.0 for
//! Android (`com.hollyland.usbmic.protocol.Pro_*`, `HLDeviceActivity`) and
//! khromov/hollyland-web-ui `docs/PROTOCOL.md`. See `docs/PROTOCOL.md` here.

use std::fmt;

pub const PACKET_LEN: usize = 64;
pub const MAX_PAYLOAD: usize = PACKET_LEN - 6;

/// Hollyland's USB vendor ID, and the EAGLESHERO OEM one.
pub const VID_HOLLYLAND: u16 = 0x3547;
pub const VID_EAGLES: u16 = 0x4417;

/// A Lark M2 receiver the app supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    pub vid: u16,
    pub pid: u16,
    pub name: &'static str,
    /// Camera receiver (3.5 mm out) rather than the USB-C / Lightning one.
    pub camera: bool,
}

/// Only the Lark M2 (family 6301). The M2S, A2, M3 and the speakers are out of scope.
pub const MODELS: &[Model] = &[
    Model { vid: VID_HOLLYLAND, pid: 0x0007, name: "Lark M2", camera: false },
    Model { vid: VID_HOLLYLAND, pid: 0x0008, name: "Lark M2 (camera)", camera: true },
    Model { vid: VID_EAGLES, pid: 0x0020, name: "Lark M2 (EAGLESHERO)", camera: false },
    Model { vid: VID_EAGLES, pid: 0x0021, name: "Lark M2 (EAGLESHERO camera)", camera: true },
];

pub fn model_for(vid: u16, pid: u16) -> Option<Model> {
    MODELS.iter().copied().find(|m| m.vid == vid && m.pid == pid)
}

/// Which unit a command goes to (request byte 3) or came from (reply byte 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum Unit {
    Rx,
    Tx1,
    Tx2,
}

impl Unit {
    pub fn dst(self) -> u8 {
        match self {
            Unit::Rx => 0x40,
            Unit::Tx1 => 0x41,
            Unit::Tx2 => 0x42,
        }
    }
    /// Android's `mapResponseDeviceType`: 0x81 → TX1, 0x82 → TX2, anything else → RX.
    pub fn from_src(src: u8) -> Unit {
        match src {
            0x81 => Unit::Tx1,
            0x82 => Unit::Tx2,
            _ => Unit::Rx,
        }
    }
    pub fn tx(index: usize) -> Unit {
        if index == 0 { Unit::Tx1 } else { Unit::Tx2 }
    }
}

/// Command IDs used by the Lark M2 screen of HollyAudio (`Protocol.HIP_6501_*`).
pub mod cmd {
    pub const VERSION: u8 = 0x03;
    pub const VOLUME_GET: u8 = 0x04;
    pub const VOLUME_SET: u8 = 0x05;
    pub const NOISE_SET: u8 = 0x06;
    pub const RESTART_RX: u8 = 0x0C;
    pub const HEARTBEAT: u8 = 0x10;
    pub const NOISE_GET: u8 = 0x11;
    pub const SPEAKER_SET: u8 = 0x12;
    pub const SPEAKER_GET: u8 = 0x13;
    pub const SERIAL: u8 = 0x15;
    pub const NOISE_TOGGLE: u8 = 0x19;
    pub const VOLUME_LOCK_SET: u8 = 0x34;
    pub const VOLUME_LOCK_GET: u8 = 0x35;

    /// Commands that can brick a unit (firmware upload). Never sent by this app.
    pub const OTA_FORBIDDEN: &[u8] = &[0x08, 0x09, 0x0B, 0x0D, 0x0E];
}

/// Noise cancellation level on the Lark M2: 0 off, 1 Low ("Weak" in the app), 2 Strong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NoiseLevel {
    Off,
    Low,
    Strong,
}

impl NoiseLevel {
    pub fn from_byte(b: u8) -> Option<NoiseLevel> {
        match b as i8 {
            0 => Some(NoiseLevel::Off),
            1 => Some(NoiseLevel::Low),
            2 => Some(NoiseLevel::Strong),
            _ => None,
        }
    }
    pub fn byte(self) -> u8 {
        match self {
            NoiseLevel::Off => 0,
            NoiseLevel::Low => 1,
            NoiseLevel::Strong => 2,
        }
    }
    pub fn is_on(self) -> bool {
        self != NoiseLevel::Off
    }
}

impl fmt::Display for NoiseLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            NoiseLevel::Off => "Off",
            NoiseLevel::Low => "Weak",
            NoiseLevel::Strong => "Strong",
        })
    }
}

/// Number of volume steps on the Lark M2 (wire values 0..=5, shown as 1..=6).
pub const VOLUME_STEPS: u8 = 6;

/// Build a 64-byte request: `AA DD cmd dst lenHi lenLo payload… 00…`.
pub fn encode(command: u8, unit: Unit, payload: &[u8]) -> [u8; PACKET_LEN] {
    assert!(payload.len() <= MAX_PAYLOAD, "payload too long");
    assert!(!cmd::OTA_FORBIDDEN.contains(&command), "firmware commands are not supported");
    let mut p = [0u8; PACKET_LEN];
    p[0] = 0xAA;
    p[1] = 0xDD;
    p[2] = command;
    p[3] = unit.dst();
    p[4] = (payload.len() >> 8) as u8;
    p[5] = payload.len() as u8;
    p[6..6 + payload.len()].copy_from_slice(payload);
    p
}

/// A decoded reply frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub command: u8,
    pub unit: Unit,
    pub payload: Vec<u8>,
}

/// Decode `BB DD cmd src lenHi lenLo payload…`. Returns `None` for anything else.
pub fn decode(data: &[u8]) -> Option<Reply> {
    if data.len() < 6 || data[0] != 0xBB || data[1] != 0xDD {
        return None;
    }
    let len = ((data[4] as usize) << 8) | data[5] as usize;
    let end = (6 + len).min(data.len());
    Some(Reply { command: data[2], unit: Unit::from_src(data[3]), payload: data[6..end].to_vec() })
}

/// Heartbeat (0x10) reply for the 6301 family (`Pro_get_heart.receiveData`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Heartbeat {
    pub tx_online: [bool; 2],
    pub tx_battery: [Option<u8>; 2],
    pub noise: Option<NoiseLevel>,
    /// Wire value 0..=5. The mobile Lark M2 (PID 7) ignores this byte; see `Device`.
    pub volume: Option<u8>,
    pub volume_locked: Option<bool>,
}

pub fn parse_heartbeat(body: &[u8]) -> Heartbeat {
    let b = |i: usize| body.get(i).copied();
    Heartbeat {
        tx_online: [b(0) == Some(1), b(1) == Some(1)],
        tx_battery: [b(2).filter(|v| *v <= 100), b(3).filter(|v| *v <= 100)],
        noise: b(6).and_then(NoiseLevel::from_byte),
        volume: b(7).filter(|v| *v < VOLUME_STEPS),
        volume_locked: b(8).map(|v| v == 1),
    }
}

/// Version reply (0x03): decimals joined with dots, e.g. `02 00 00 24` → `2.0.0.36`.
pub fn parse_version(payload: &[u8]) -> Option<String> {
    if payload.is_empty() || payload.iter().all(|b| *b == 0) {
        return None; // "0.0.0.0" counts as missing: the apps ask again
    }
    Some(payload.iter().map(|b| b.to_string()).collect::<Vec<_>>().join("."))
}

/// Serial reply (0x15): UTF-8 with every NUL removed (as iOS does), trimmed.
pub fn parse_serial(payload: &[u8]) -> Option<String> {
    let s: String = String::from_utf8_lossy(payload).chars().filter(|c| *c != '\0').collect();
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Set acknowledgement: `payload[0] == 1` means success.
pub fn ack_ok(payload: &[u8]) -> bool {
    payload.first() == Some(&1)
}

/// Noise-level set ack on the M2 needs two bytes `[status, level]`; one byte is a failure.
pub fn noise_set_ok(payload: &[u8]) -> bool {
    payload.len() >= 2 && payload[0] == 1
}

/// Volume-lock set ack on the Lark M2: Android wants `1`, iOS reads the new state.
pub fn lock_set_ok(payload: &[u8], requested: bool) -> bool {
    match payload.first() {
        Some(1) => true,
        Some(b) => (*b != 0) == requested,
        None => false,
    }
}

/// Parse a dotted version into numbers (non-digits ignored, like the apps).
pub fn version_parts(v: &str) -> Vec<u32> {
    v.trim()
        .trim_start_matches(['v', 'V'])
        .split('.')
        .map(|p| p.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0))
        .collect()
}

/// Compare dotted versions part by part (missing parts count as 0).
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let (a, b) = (version_parts(a), version_parts(b));
    let n = a.len().max(b.len());
    for i in 0..n {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x.cmp(&y);
        }
    }
    std::cmp::Ordering::Equal
}

/// Android `Util.isNewVersion`: needs at least four parts in both; strictly newer.
pub fn is_newer_android(version: &str, than: &str) -> bool {
    let (a, b) = (version_parts(version), version_parts(than));
    if a.len() < 4 || b.len() < 4 {
        return false;
    }
    a[..4] > b[..4]
}

/// The receiver firmware the Lark M2 features were gated on (`HLDeviceActivity.SPEC_VERSION`).
pub const SPEC_VERSION: &str = "2.0.0.31";

/// NC on/off switch (0x19): Android needs RX firmware newer than 2.0.0.31, iOS the last
/// part ≥ 32. Both must hold.
pub fn has_noise_toggle(rx_version: Option<&str>) -> bool {
    let Some(v) = rx_version else { return false };
    let last = version_parts(v).last().copied().unwrap_or(0);
    is_newer_android(v, SPEC_VERSION) && last >= 32
}

/// Volume lock (0x34/0x35): camera receivers with RX firmware newer than 2.0.0.31.
pub fn has_volume_lock(model: &Model, rx_version: Option<&str>) -> bool {
    model.camera && rx_version.is_some_and(|v| is_newer_android(v, SPEC_VERSION))
}

/// Hollyland cloud firmware package codes for a unit (`HlDevice.getDeviceCode`).
pub fn package_code(model: &Model, unit: Unit) -> String {
    let suffix = match (model.vid == VID_EAGLES, unit) {
        (true, Unit::Rx) => "RX_RU",
        (true, _) => "TX_RU",
        (false, Unit::Rx) => "RX",
        (false, _) => "TX",
    };
    format!("6301{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_heartbeat() {
        let p = encode(cmd::HEARTBEAT, Unit::Rx, &[]);
        assert_eq!(&p[..6], &[0xAA, 0xDD, 0x10, 0x40, 0, 0]);
        assert!(p[6..].iter().all(|b| *b == 0));
    }

    #[test]
    fn encodes_payload() {
        let p = encode(cmd::NOISE_SET, Unit::Rx, &[2]);
        assert_eq!(&p[..7], &[0xAA, 0xDD, 0x06, 0x40, 0, 1, 2]);
        let p = encode(cmd::VERSION, Unit::Tx2, &[]);
        assert_eq!(p[3], 0x42);
    }

    #[test]
    #[should_panic]
    fn refuses_ota() {
        encode(0x0D, Unit::Rx, &[]);
    }

    #[test]
    fn decodes_live_m2_heartbeat() {
        // Lark M2, macOS bridge capture 2026-11-06 (khromov/hollyland-web-ui §11.8).
        let mut f = vec![0xBB, 0xDD, 0x10, 0x80, 0x00, 0x09, 1, 1, 0x62, 0x64, 0, 0, 0, 5, 0];
        f.resize(64, 0);
        let r = decode(&f).unwrap();
        assert_eq!(r.command, 0x10);
        assert_eq!(r.unit, Unit::Rx);
        assert_eq!(r.payload.len(), 9);
        let h = parse_heartbeat(&r.payload);
        assert_eq!(h.tx_online, [true, true]);
        assert_eq!(h.tx_battery, [Some(98), Some(100)]);
        assert_eq!(h.noise, Some(NoiseLevel::Off));
        assert_eq!(h.volume, Some(5));
        assert_eq!(h.volume_locked, Some(false));
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(&[0xAA, 0xDD, 0x10, 0x40, 0, 0]).is_none());
        assert!(decode(&[0xBB]).is_none());
    }

    #[test]
    fn short_heartbeat_has_no_bogus_values() {
        let h = parse_heartbeat(&[1, 0, 50]);
        assert_eq!(h.tx_online, [true, false]);
        assert_eq!(h.tx_battery, [Some(50), None]);
        assert_eq!(h.noise, None);
        assert_eq!(h.volume, None);
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version(&[2, 0, 0, 0x24]).as_deref(), Some("2.0.0.36"));
        assert_eq!(parse_version(&[0, 0, 0, 0]), None);
        assert!(has_noise_toggle(Some("2.0.0.36")));
        assert!(has_noise_toggle(Some("2.0.0.33")));
        assert!(!has_noise_toggle(Some("2.0.0.31")));
        assert!(!has_noise_toggle(None));
        assert_eq!(compare_versions("2.0.0.40", "2.0.0.36"), std::cmp::Ordering::Greater);
        assert_eq!(compare_versions("v2.1.0.36", "2.1.0.36"), std::cmp::Ordering::Equal);
        assert_eq!(compare_versions("2.0.0", "2.0.0.1"), std::cmp::Ordering::Less);
    }

    #[test]
    fn serials() {
        assert_eq!(parse_serial(b"C74X226Z9ZM\0\0").as_deref(), Some("C74X226Z9ZM"));
        assert_eq!(parse_serial(b"\0\0"), None);
    }

    #[test]
    fn acks() {
        assert!(ack_ok(&[1]));
        assert!(!ack_ok(&[0]));
        assert!(!noise_set_ok(&[1]));
        assert!(noise_set_ok(&[1, 2]));
        assert!(lock_set_ok(&[1], false));
        assert!(lock_set_ok(&[0], false));
        assert!(!lock_set_ok(&[0], true));
    }

    #[test]
    fn models_and_codes() {
        let m = model_for(0x3547, 7).unwrap();
        assert!(!m.camera);
        assert!(model_for(0x3547, 0x0405).is_none(), "M2S is out of scope");
        assert_eq!(package_code(&m, Unit::Rx), "6301RX");
        assert_eq!(package_code(&m, Unit::Tx2), "6301TX");
        assert!(has_volume_lock(&model_for(0x3547, 8).unwrap(), Some("2.0.0.36")));
        assert!(!has_volume_lock(&m, Some("2.0.0.36")));
    }
}
