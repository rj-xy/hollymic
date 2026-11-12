//! Shared state between the device worker, the tray, the window and IPC clients.

use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use serde::Serialize;

use crate::protocol::{NoiseLevel, VOLUME_STEPS};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(tag = "state", content = "detail", rename_all = "kebab-case")]
pub enum Connection {
    /// Looking for a receiver.
    #[default]
    Searching,
    /// Found, but the USB device node isn't writable (udev rule missing).
    NoPermission(String),
    /// Found, but another program holds its control interface.
    Busy(String),
    Connected,
    Error(String),
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UnitInfo {
    pub version: Option<String>,
    pub serial: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Mic {
    pub online: bool,
    pub battery: Option<u8>,
    #[serde(flatten)]
    pub info: UnitInfo,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Features {
    /// NC on/off switch (needs RX firmware newer than 2.0.0.31).
    pub noise_toggle: bool,
    /// Volume lock (camera receivers only).
    pub volume_lock: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FirmwareUnit {
    pub latest: Option<String>,
    pub update: bool,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Firmware {
    #[serde(with = "opt_time")]
    pub checked_at: Option<SystemTime>,
    pub rx: FirmwareUnit,
    pub tx: FirmwareUnit,
    pub error: Option<String>,
}

impl Firmware {
    pub fn update_available(&self) -> bool {
        self.rx.update || self.tx.update
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DeviceState {
    pub connection: Connection,
    pub model: Option<String>,
    pub camera: bool,
    pub receiver: UnitInfo,
    pub mics: [Mic; 2],
    /// Current NC state. `None` until read.
    pub noise: Option<NoiseLevel>,
    /// Last level used while NC was on (what switching it on brings back).
    pub noise_level: Option<NoiseLevel>,
    /// Wire value 0..=5 (shown as 1..=6).
    pub volume: Option<u8>,
    pub volume_locked: Option<bool>,
    /// "Phone speaker": the receiver also plays to the phone's speaker.
    pub speaker: Option<bool>,
    pub features: Features,
    pub firmware: Firmware,
    /// Last user-visible problem (failed write, refused change).
    pub message: Option<String>,
    /// True when the state came from the demo simulator.
    pub demo: bool,
}

impl DeviceState {
    pub fn connected(&self) -> bool {
        self.connection == Connection::Connected
    }
    pub fn any_mic_online(&self) -> bool {
        self.mics.iter().any(|m| m.online)
    }
    pub fn volume_display(&self) -> Option<u8> {
        self.volume.map(|v| v.min(VOLUME_STEPS - 1) + 1)
    }
    /// One-line summary for the tray tooltip and `hollymic status`.
    pub fn summary(&self) -> String {
        match &self.connection {
            Connection::Searching => "Receiver not plugged in".into(),
            Connection::NoPermission(_) => "No permission to use the receiver (udev rule missing)".into(),
            Connection::Busy(_) => "Receiver in use by another program".into(),
            Connection::Error(e) => format!("Error: {e}"),
            Connection::Connected => {
                let mics: Vec<String> = self
                    .mics
                    .iter()
                    .enumerate()
                    .map(|(i, m)| match (m.online, m.battery) {
                        (true, Some(b)) => format!("Mic {} {b}%", i + 1),
                        (true, None) => format!("Mic {} on", i + 1),
                        (false, _) => format!("Mic {} off", i + 1),
                    })
                    .collect();
                let nc = match self.noise {
                    Some(NoiseLevel::Off) => "NC off".to_string(),
                    Some(l) => format!("NC {l}"),
                    None => "NC ?".into(),
                };
                let vol = self.volume_display().map(|v| format!("Vol {v}/6")).unwrap_or_default();
                format!("{} · {nc} · {vol}", mics.join(" · "))
            }
        }
    }
}

type Listener = Box<dyn Fn() + Send + Sync>;

/// State plus change listeners. Cheap to clone (Arc inside).
#[derive(Clone)]
pub struct Shared {
    state: Arc<RwLock<DeviceState>>,
    listeners: Arc<Mutex<Vec<Listener>>>,
}

impl Shared {
    pub fn new(demo: bool) -> Shared {
        let state = DeviceState { demo, ..Default::default() };
        Shared { state: Arc::new(RwLock::new(state)), listeners: Arc::new(Mutex::new(Vec::new())) }
    }

    pub fn get(&self) -> DeviceState {
        self.state.read().unwrap().clone()
    }

    /// Change the state and tell every listener.
    pub fn update(&self, f: impl FnOnce(&mut DeviceState)) {
        f(&mut self.state.write().unwrap());
        self.notify();
    }

    pub fn notify(&self) {
        for l in self.listeners.lock().unwrap().iter() {
            l();
        }
    }

    pub fn subscribe(&self, f: impl Fn() + Send + Sync + 'static) {
        self.listeners.lock().unwrap().push(Box::new(f));
    }
}

mod opt_time {
    use serde::Serializer;
    use std::time::{SystemTime, UNIX_EPOCH};

    pub fn serialize<S: Serializer>(t: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
        match t.and_then(|t| t.duration_since(UNIX_EPOCH).ok()) {
            Some(d) => s.serialize_u64(d.as_secs()),
            None => s.serialize_none(),
        }
    }
}
