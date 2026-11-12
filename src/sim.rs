//! A simulated Lark M2 receiver for `--demo`, screenshots and tests.
//! It answers like the real one (same frames), with made-up serial numbers.

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::protocol::{self, MODELS, Model, Reply, Unit, cmd};
use crate::usb::Transport;

pub struct SimTransport {
    online: [bool; 2],
    battery: [u8; 2],
    noise: u8,
    last_level: u8,
    volume: u8,
    speaker_off: bool,
    started: Instant,
}

impl SimTransport {
    pub fn new() -> SimTransport {
        SimTransport {
            online: [true, false],
            battery: [87, 0],
            noise: 1,
            last_level: 1,
            volume: 5,
            speaker_off: false,
            started: Instant::now(),
        }
    }

    fn reply(&self, command: u8, unit: Unit, payload: Vec<u8>) -> Option<Reply> {
        Some(Reply { command, unit, payload })
    }
}

impl Default for SimTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for SimTransport {
    fn model(&self) -> Model {
        MODELS[0]
    }

    fn request(&mut self, command: u8, unit: Unit, payload: &[u8], _timeout: Duration) -> Result<Option<Reply>> {
        let _ = protocol::encode(command, unit, payload); // same checks as the real path
        std::thread::sleep(Duration::from_millis(5));
        let p0 = payload.first().copied().unwrap_or(0);
        // Mic 1 loses 1 % every 10 minutes.
        let drain = (self.started.elapsed().as_secs() / 600) as u8;
        let r = match command {
            cmd::HEARTBEAT => {
                let b1 = if self.online[0] { self.battery[0].saturating_sub(drain) } else { 0 };
                let b2 = if self.online[1] { self.battery[1] } else { 0 };
                let body = vec![self.online[0] as u8, self.online[1] as u8, b1, b2, 0, 0, self.noise, self.volume, 0];
                self.reply(command, Unit::Rx, body)
            }
            cmd::VERSION => match unit {
                Unit::Rx => self.reply(command, unit, vec![2, 0, 0, 36]),
                Unit::Tx1 if self.online[0] => self.reply(command, unit, vec![2, 1, 0, 36]),
                Unit::Tx2 if self.online[1] => self.reply(command, unit, vec![2, 1, 0, 36]),
                _ => None,
            },
            cmd::SERIAL => {
                let s: &[u8] = match unit {
                    Unit::Rx => b"DEMORX00001",
                    Unit::Tx1 => b"DEMOTX00001",
                    Unit::Tx2 => b"DEMOTX00002",
                };
                self.reply(command, unit, s.to_vec())
            }
            cmd::NOISE_GET => self.reply(command, unit, vec![self.noise]),
            cmd::VOLUME_GET => self.reply(command, unit, vec![self.volume]),
            cmd::SPEAKER_GET => self.reply(command, unit, vec![self.speaker_off as u8]),
            cmd::VOLUME_LOCK_GET => self.reply(command, unit, vec![0]),
            cmd::NOISE_SET if (1..=2).contains(&p0) => {
                self.noise = p0;
                self.last_level = p0;
                self.reply(command, unit, vec![1, p0])
            }
            cmd::NOISE_TOGGLE => {
                self.noise = if self.noise == 0 { self.last_level } else { 0 };
                self.reply(command, unit, vec![1])
            }
            cmd::VOLUME_SET if p0 < 6 => {
                self.volume = p0;
                self.reply(command, unit, vec![1])
            }
            cmd::SPEAKER_SET => {
                self.speaker_off = p0 == 1;
                self.reply(command, unit, vec![1])
            }
            cmd::RESTART_RX => None,
            _ => self.reply(command, unit, vec![0]),
        };
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device;
    use crate::state::Shared;

    fn wait_for(shared: &Shared, f: impl Fn(&crate::state::DeviceState) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if f(&shared.get()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn worker_reads_and_writes_the_simulator() {
        let shared = Shared::new(true);
        let dev = device::spawn(shared.clone(), true);
        assert!(wait_for(&shared, |s| s.connected() && s.mics[0].battery == Some(87) && s.volume == Some(5)));
        let s = shared.get();
        assert_eq!(s.receiver.version.as_deref(), Some("2.0.0.36"));
        assert_eq!(s.mics[0].info.version.as_deref(), Some("2.1.0.36"));
        assert!(s.features.noise_toggle);
        assert_eq!(s.noise, Some(protocol::NoiseLevel::Low));

        let tx = dev.sender();
        tx.send(device::Request::SetVolume(3)).unwrap();
        assert!(wait_for(&shared, |s| s.volume_display() == Some(3)));
        tx.send(device::Request::SetNoise(false)).unwrap();
        assert!(wait_for(&shared, |s| s.noise == Some(protocol::NoiseLevel::Off)));
        tx.send(device::Request::SetNoiseLevel(protocol::NoiseLevel::Strong)).unwrap();
        assert!(wait_for(&shared, |s| s.message.as_deref() == Some("Switch noise cancellation on first.")));
        tx.send(device::Request::SetNoise(true)).unwrap();
        assert!(wait_for(&shared, |s| s.noise == Some(protocol::NoiseLevel::Low)));
        tx.send(device::Request::SetNoiseLevel(protocol::NoiseLevel::Strong)).unwrap();
        assert!(wait_for(&shared, |s| s.noise == Some(protocol::NoiseLevel::Strong)));
        dev.stop();
    }
}
