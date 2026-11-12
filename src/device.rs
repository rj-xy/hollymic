//! Device worker: finds the receiver, reads its state, polls the heartbeat and
//! runs the user's changes one at a time (the receiver handles one request at once).

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use crate::protocol::{self, Model, NoiseLevel, Unit, VOLUME_STEPS, cmd};
use crate::sim::SimTransport;
use crate::state::{Connection, DeviceState, Mic, Shared};
use crate::usb::{OpenError, Transport, UsbTransport};

/// The Lark M2 screen of HollyAudio polls every 3 s.
const HEARTBEAT_EVERY: Duration = Duration::from_secs(3);
const REPLY_TIMEOUT: Duration = Duration::from_millis(500);
/// Version and serial reads per unit before giving up (Android `MAX_REQUEST_RETRY_COUNT`).
const IDENTITY_TRIES: u8 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Noise cancellation on or off.
    SetNoise(bool),
    /// Weak (Low) or Strong.
    SetNoiseLevel(NoiseLevel),
    /// Volume as shown, 1..=6.
    SetVolume(u8),
    SetVolumeLock(bool),
    /// "Phone speaker". Takes effect after the receiver restarts, which the worker does.
    SetSpeaker(bool),
    RestartReceiver,
    /// Read everything again.
    Refresh,
    Stop,
}

pub struct DeviceHandle {
    tx: Sender<Request>,
    join: Option<JoinHandle<()>>,
}

impl DeviceHandle {
    pub fn sender(&self) -> Sender<Request> {
        self.tx.clone()
    }
    /// Stop the worker and wait up to 2 s for it (it releases the receiver on the way
    /// out). Never blocks longer: quitting must not hang on a stuck USB call.
    pub fn stop(mut self) {
        let _ = self.tx.send(Request::Stop);
        if let Some(j) = self.join.take() {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !j.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if j.is_finished() {
                let _ = j.join();
            } else {
                log::warn!("device thread didn't stop in 2 s; quitting anyway");
            }
        }
    }
}

pub fn spawn(shared: Shared, demo: bool) -> DeviceHandle {
    let (tx, rx) = mpsc::channel();
    let join = std::thread::Builder::new()
        .name("device".into())
        .spawn(move || worker(shared, rx, demo))
        .expect("spawn device thread");
    DeviceHandle { tx, join: Some(join) }
}

enum Exit {
    Stop,
    Lost,
}

fn worker(shared: Shared, rx: Receiver<Request>, demo: bool) {
    loop {
        let opened: std::result::Result<Box<dyn Transport>, OpenError> = if demo {
            Ok(Box::new(SimTransport::new()))
        } else {
            UsbTransport::open().map(|t| Box::new(t) as Box<dyn Transport>)
        };
        match opened {
            Ok(t) => {
                let mut s = Session::new(t, shared.clone());
                match s.run(&rx) {
                    Exit::Stop => return,
                    Exit::Lost => {
                        log::info!("receiver lost");
                        clear_device(&shared, Connection::Searching);
                    }
                }
                if wait_or_stop(&rx, &shared, Duration::from_millis(800)) {
                    return;
                }
            }
            Err(e) => {
                let conn = match &e {
                    OpenError::NotFound => Connection::Searching,
                    OpenError::Permission(m) => Connection::NoPermission(m.clone()),
                    OpenError::Busy(m) => Connection::Busy(m.clone()),
                    OpenError::Other(m) => Connection::Error(m.clone()),
                };
                if shared.get().connection != conn {
                    log::info!("{e}");
                    clear_device(&shared, conn);
                }
                let wait = if e == OpenError::NotFound { 2 } else { 5 };
                if wait_or_stop(&rx, &shared, Duration::from_secs(wait)) {
                    return;
                }
            }
        }
    }
}

fn clear_device(shared: &Shared, conn: Connection) {
    shared.update(|s| {
        let firmware = std::mem::take(&mut s.firmware);
        let demo = s.demo;
        *s = DeviceState { connection: conn, firmware, demo, ..Default::default() };
    });
}

/// Wait while no receiver is open. Answers requests with a message. True = stop.
fn wait_or_stop(rx: &Receiver<Request>, shared: &Shared, d: Duration) -> bool {
    let deadline = Instant::now() + d;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        match rx.recv_timeout(deadline - now) {
            Ok(Request::Stop) | Err(RecvTimeoutError::Disconnected) => return true,
            Ok(Request::Refresh) => return false,
            Ok(_) => shared.update(|s| s.message = Some("The receiver isn't connected.".into())),
            Err(RecvTimeoutError::Timeout) => return false,
        }
    }
}

struct Session {
    t: Box<dyn Transport>,
    model: Model,
    shared: Shared,
    /// Remaining version / serial reads for RX, TX1, TX2.
    tries: [[u8; 2]; 3],
    beats: u32,
    missed: u32,
}

fn slot(u: Unit) -> usize {
    match u {
        Unit::Rx => 0,
        Unit::Tx1 => 1,
        Unit::Tx2 => 2,
    }
}

impl Session {
    fn new(t: Box<dyn Transport>, shared: Shared) -> Session {
        let model = t.model();
        Session { t, model, shared, tries: [[IDENTITY_TRIES; 2]; 3], beats: 0, missed: 0 }
    }

    fn run(&mut self, rx: &Receiver<Request>) -> Exit {
        let model = self.model;
        self.shared.update(|s| {
            s.connection = Connection::Connected;
            s.model = Some(model.name.to_string());
            s.camera = model.camera;
            s.message = None;
        });
        if let Err(e) = self.init() {
            log::warn!("start-up reads failed: {e:#}");
            return Exit::Lost;
        }
        let mut next_beat = Instant::now();
        loop {
            let now = Instant::now();
            if now >= next_beat {
                if let Err(e) = self.heartbeat() {
                    log::warn!("heartbeat failed: {e:#}");
                    return Exit::Lost;
                }
                next_beat = Instant::now() + HEARTBEAT_EVERY;
                continue;
            }
            match rx.recv_timeout(next_beat - now) {
                Ok(Request::Stop) | Err(RecvTimeoutError::Disconnected) => return Exit::Stop,
                Ok(req) => {
                    log::info!("request {req:?}");
                    match self.handle(req) {
                        Ok(Some(exit)) => return exit,
                        Ok(None) => next_beat = Instant::now(), // read the result back now
                        Err(e) => {
                            log::warn!("request failed: {e:#}");
                            return Exit::Lost;
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    /// Read-only request, retried twice on timeout. `Ok(None)` = no reply.
    fn read(&mut self, command: u8, unit: Unit) -> Result<Option<Vec<u8>>> {
        for _ in 0..3 {
            if let Some(r) = self.t.request(command, unit, &[], REPLY_TIMEOUT)? {
                return Ok(Some(r.payload));
            }
        }
        Ok(None)
    }

    /// Write request, never retried (a toggle sent twice undoes itself).
    fn write(&mut self, command: u8, payload: &[u8]) -> Result<Option<Vec<u8>>> {
        Ok(self.t.request(command, Unit::Rx, payload, REPLY_TIMEOUT)?.map(|r| r.payload))
    }

    fn message(&self, m: impl Into<String>) {
        let m = m.into();
        log::info!("{m}");
        self.shared.update(|s| s.message = Some(m));
    }

    /// The start-up reads of HollyAudio's Lark M2 screen: 0x03, 0x11, 0x04, 0x13, 0x15, 0x35.
    fn init(&mut self) -> Result<()> {
        self.identity(Unit::Rx)?;
        if let Some(p) = self.read(cmd::NOISE_GET, Unit::Rx)?
            && let Some(n) = p.first().copied().and_then(NoiseLevel::from_byte)
        {
            self.shared.update(|s| set_noise(s, n));
        }
        self.read_volume()?;
        if let Some(p) = self.read(cmd::SPEAKER_GET, Unit::Rx)?
            && let Some(b) = p.first()
        {
            self.shared.update(|s| s.speaker = Some(*b == 0));
        }
        if self.shared.get().features.volume_lock
            && let Some(p) = self.read(cmd::VOLUME_LOCK_GET, Unit::Rx)?
        {
            self.shared.update(|s| s.volume_locked = p.first().map(|b| *b == 1));
        }
        Ok(())
    }

    fn read_volume(&mut self) -> Result<()> {
        if let Some(p) = self.read(cmd::VOLUME_GET, Unit::Rx)?
            && let Some(v) = p.first().copied().filter(|v| *v < VOLUME_STEPS)
        {
            self.shared.update(|s| s.volume = Some(v));
        }
        Ok(())
    }

    /// Version and serial of one unit, within its retry budget.
    fn identity(&mut self, unit: Unit) -> Result<()> {
        let i = slot(unit);
        let have = {
            let s = self.shared.get();
            let info = match unit {
                Unit::Rx => s.receiver,
                Unit::Tx1 => s.mics[0].info.clone(),
                Unit::Tx2 => s.mics[1].info.clone(),
            };
            [info.version.is_some(), info.serial.is_some()]
        };
        let model = self.model;
        if !have[0] && self.tries[i][0] > 0 {
            self.tries[i][0] -= 1;
            if let Some(v) = self.read(cmd::VERSION, unit)?.and_then(|p| protocol::parse_version(&p)) {
                self.shared.update(|s| match unit {
                    Unit::Rx => {
                        s.features.noise_toggle = protocol::has_noise_toggle(Some(&v));
                        s.features.volume_lock = protocol::has_volume_lock(&model, Some(&v));
                        s.receiver.version = Some(v);
                    }
                    Unit::Tx1 => s.mics[0].info.version = Some(v),
                    Unit::Tx2 => s.mics[1].info.version = Some(v),
                });
            }
        }
        if !have[1] && self.tries[i][1] > 0 {
            self.tries[i][1] -= 1;
            if let Some(v) = self.read(cmd::SERIAL, unit)?.and_then(|p| protocol::parse_serial(&p)) {
                self.shared.update(|s| match unit {
                    Unit::Rx => s.receiver.serial = Some(v),
                    Unit::Tx1 => s.mics[0].info.serial = Some(v),
                    Unit::Tx2 => s.mics[1].info.serial = Some(v),
                });
            }
        }
        Ok(())
    }

    fn heartbeat(&mut self) -> Result<()> {
        let Some(body) = self.read(cmd::HEARTBEAT, Unit::Rx)? else {
            self.missed += 1;
            if self.missed >= 3 {
                if !crate::usb::present() && !self.shared.get().demo {
                    bail!("receiver unplugged");
                }
                self.message("The receiver isn't answering.");
            }
            return Ok(());
        };
        self.missed = 0;
        self.beats += 1;
        let hb = protocol::parse_heartbeat(&body);
        let before = self.shared.get();
        let camera = self.model.camera;
        self.shared.update(|s| {
            for i in 0..2 {
                let m = &mut s.mics[i];
                if hb.tx_online[i] {
                    m.online = true;
                    m.battery = hb.tx_battery[i];
                } else if m.online || m.battery.is_some() {
                    *m = Mic::default(); // version and serial are read again next time
                }
            }
            if let Some(n) = hb.noise {
                set_noise(s, n);
            }
            // PID 7 takes its volume from 0x04 only (HLDeviceActivity cmd 16: productId >= 8).
            if camera && let Some(v) = hb.volume {
                s.volume = Some(v);
            }
            if s.features.volume_lock
                && let Some(l) = hb.volume_locked
            {
                s.volume_locked = Some(l);
            }
            if s.message.as_deref() == Some("The receiver isn't answering.") {
                s.message = None;
            }
        });
        // A mic that came online: read its version and serial (fresh budget).
        for i in 0..2 {
            let unit = Unit::tx(i);
            if hb.tx_online[i] {
                if !before.mics[i].online {
                    self.tries[slot(unit)] = [IDENTITY_TRIES; 2];
                }
                self.identity(unit)?;
            }
        }
        self.identity(Unit::Rx)?;
        if !camera && self.beats.is_multiple_of(10) {
            self.read_volume()?; // pick up a change made elsewhere (phone app)
        }
        Ok(())
    }

    fn handle(&mut self, req: Request) -> Result<Option<Exit>> {
        let s = self.shared.get();
        match req {
            Request::Stop => return Ok(Some(Exit::Stop)),
            Request::Refresh => {
                self.tries = [[IDENTITY_TRIES; 2]; 3];
                self.init()?;
            }
            Request::SetNoise(on) => {
                let Some(current) = s.noise else {
                    self.message("Noise cancellation state not read yet.");
                    return Ok(None);
                };
                if current.is_on() == on {
                    return Ok(None);
                }
                if !s.features.noise_toggle {
                    if on {
                        let level = s.noise_level.unwrap_or(NoiseLevel::Strong);
                        return self.handle(Request::SetNoiseLevel(level));
                    }
                    self.message("This receiver's firmware can't switch noise cancellation off.");
                    return Ok(None);
                }
                // 0x19 always flips; the heartbeat that follows shows the result.
                self.write(cmd::NOISE_TOGGLE, &[2])?;
                std::thread::sleep(Duration::from_millis(150));
                if let Some(body) = self.read(cmd::HEARTBEAT, Unit::Rx)?
                    && let Some(n) = protocol::parse_heartbeat(&body).noise
                {
                    self.shared.update(|s| {
                        set_noise(s, n);
                        s.message = None;
                    });
                    if n.is_on() != on {
                        self.message("Noise cancellation didn't change.");
                    }
                }
            }
            Request::SetNoiseLevel(level) => {
                if level == NoiseLevel::Off {
                    return self.handle(Request::SetNoise(false));
                }
                if s.noise == Some(level) {
                    return Ok(None);
                }
                if !s.any_mic_online() {
                    self.message("Turn a mic on first: take it out of the case.");
                    return Ok(None);
                }
                if s.noise == Some(NoiseLevel::Off) {
                    self.message("Switch noise cancellation on first.");
                    return Ok(None);
                }
                match self.write(cmd::NOISE_SET, &[level.byte()])? {
                    Some(p) if protocol::noise_set_ok(&p) => self.shared.update(|s| {
                        set_noise(s, level);
                        s.message = None;
                    }),
                    _ => self.message("The receiver didn't accept the noise cancellation level."),
                }
            }
            Request::SetVolume(shown) => {
                let v = shown.clamp(1, VOLUME_STEPS) - 1;
                if s.volume == Some(v) {
                    return Ok(None);
                }
                if !s.any_mic_online() {
                    self.message("Turn a mic on first: take it out of the case.");
                    return Ok(None);
                }
                if s.volume_locked == Some(true) {
                    self.message("The volume is locked.");
                    return Ok(None);
                }
                match self.write(cmd::VOLUME_SET, &[v])? {
                    Some(p) if protocol::ack_ok(&p) => self.shared.update(|s| {
                        s.volume = Some(v);
                        s.message = None;
                    }),
                    _ => self.message("The receiver didn't accept the volume."),
                }
                if !self.model.camera {
                    self.read_volume()?;
                }
            }
            Request::SetVolumeLock(lock) => {
                if !s.features.volume_lock {
                    self.message("This receiver has no volume lock.");
                    return Ok(None);
                }
                match self.write(cmd::VOLUME_LOCK_SET, &[lock as u8])? {
                    Some(p) if protocol::lock_set_ok(&p, lock) => self.shared.update(|s| {
                        s.volume_locked = Some(lock);
                        s.message = None;
                    }),
                    _ => self.message("The receiver didn't accept the volume lock."),
                }
            }
            Request::SetSpeaker(on) => {
                match self.write(cmd::SPEAKER_SET, &[if on { 0 } else { 1 }])? {
                    Some(p) if protocol::ack_ok(&p) => {
                        self.shared.update(|s| {
                            s.speaker = Some(on);
                            s.message = Some("Phone speaker changed: the receiver restarts in 3 s.".into());
                        });
                        // HollyAudio counts down 3 s, then restarts the receiver.
                        std::thread::sleep(Duration::from_secs(3));
                        return self.restart();
                    }
                    _ => self.message("The receiver didn't accept the phone speaker setting."),
                }
            }
            Request::RestartReceiver => return self.restart(),
        }
        Ok(None)
    }

    fn restart(&mut self) -> Result<Option<Exit>> {
        self.message("Restarting the receiver…");
        // No reply: the receiver drops off USB and comes back.
        let _ = self.t.request(cmd::RESTART_RX, Unit::Rx, &[], Duration::from_millis(200));
        std::thread::sleep(Duration::from_secs(2));
        self.shared.update(|s| s.message = None);
        Ok(Some(Exit::Lost))
    }
}

fn set_noise(s: &mut DeviceState, n: NoiseLevel) {
    s.noise = Some(n);
    if n.is_on() {
        s.noise_level = Some(n);
    }
}
