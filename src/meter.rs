//! Input level meter, like the bar on HollyAudio's Lark M2 screen (the app reads the
//! USB audio, not the receiver). On Linux it records the receiver's PipeWire source
//! with `pw-record`, which shares the mic with every other app. Runs only while the
//! window is open.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, Default)]
pub struct Level {
    /// Peak over the last ~50 ms, dBFS (-90 = silence).
    pub peak_db: f32,
    /// Slow-falling peak hold, dBFS.
    pub hold_db: f32,
    pub at: Option<Instant>,
}

pub struct Meter {
    level: Arc<Mutex<Level>>,
    stop: Arc<AtomicBool>,
    child: Option<Child>,
    pub error: Option<String>,
}

pub const FLOOR_DB: f32 = -60.0;

/// The receiver's PipeWire source name (`alsa_input.usb-Shenzhen_Hollyland_…`).
fn find_source() -> Option<String> {
    let out = Command::new("pactl").args(["list", "short", "sources"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .filter_map(|l| l.split('\t').nth(1))
        .find(|n| !n.ends_with(".monitor") && (n.contains("Hollyland") || n.contains("Wireless_microphone")))
        .map(str::to_string)
}

impl Meter {
    pub fn start(on_update: impl Fn() + Send + 'static) -> Meter {
        let level = Arc::new(Mutex::new(Level { peak_db: -90.0, hold_db: -90.0, at: None }));
        let stop = Arc::new(AtomicBool::new(false));
        if !cfg!(target_os = "linux") {
            return Meter { level, stop, child: None, error: Some("Level meter: Linux only for now".into()) };
        }
        let Some(source) = find_source() else {
            return Meter {
                level,
                stop,
                child: None,
                error: Some("Level meter: no Lark audio source in PipeWire".into()),
            };
        };
        let child = Command::new("pw-record")
            .args(["--target", &source, "--rate", "16000", "--channels", "1", "--format", "s16", "--latency", "50ms"])
            .args(["-P", "{ node.name = hollymic-meter }", "-"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => return Meter { level, stop, child: None, error: Some(format!("Level meter: pw-record: {e}")) },
        };
        let mut stdout = child.stdout.take().unwrap();
        let (lv, st) = (level.clone(), stop.clone());
        std::thread::Builder::new()
            .name("meter".into())
            .spawn(move || {
                let mut header = [0u8; 24];
                if stdout.read_exact(&mut header).is_err() {
                    return;
                }
                // pw-record writes an AU container: ".snd" = big-endian, "dns." = little-endian.
                let little = &header[0..4] == b"dns.";
                let hsize = if little {
                    u32::from_le_bytes(header[4..8].try_into().unwrap())
                } else {
                    u32::from_be_bytes(header[4..8].try_into().unwrap())
                } as usize;
                if hsize > 24 {
                    let mut skip = vec![0u8; hsize - 24];
                    if stdout.read_exact(&mut skip).is_err() {
                        return;
                    }
                }
                let mut buf = [0u8; 1600]; // 50 ms at 16 kHz mono s16
                let mut last = Instant::now();
                while !st.load(Ordering::Relaxed) {
                    if stdout.read_exact(&mut buf).is_err() {
                        break;
                    }
                    let peak = buf
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| {
                            let v = if little {
                                i16::from_le_bytes([b[0], b[1]])
                            } else {
                                i16::from_be_bytes([b[0], b[1]])
                            };
                            (v as i32).unsigned_abs()
                        })
                        .max()
                        .unwrap_or(0);
                    let db = if peak == 0 { -90.0 } else { 20.0 * (peak as f32 / 32768.0).log10() };
                    let mut l = lv.lock().unwrap();
                    let dt = last.elapsed().as_secs_f32();
                    last = Instant::now();
                    l.peak_db = db;
                    l.hold_db = if db > l.hold_db { db } else { (l.hold_db - 12.0 * dt).max(db) };
                    l.at = Some(Instant::now());
                    drop(l);
                    on_update();
                }
            })
            .expect("spawn meter thread");
        Meter { level, stop, child: Some(child), error: None }
    }

    /// Made-up speech-like levels for `--demo` and screenshots.
    pub fn demo(on_update: impl Fn() + Send + 'static) -> Meter {
        let level = Arc::new(Mutex::new(Level { peak_db: -90.0, hold_db: -90.0, at: None }));
        let stop = Arc::new(AtomicBool::new(false));
        let (lv, st) = (level.clone(), stop.clone());
        std::thread::spawn(move || {
            let t0 = Instant::now();
            while !st.load(Ordering::Relaxed) {
                let t = t0.elapsed().as_secs_f32();
                let syllables = (t * 7.0).sin().abs() * (t * 1.3).sin().abs();
                let db = -42.0 + 26.0 * syllables;
                let mut l = lv.lock().unwrap();
                l.peak_db = db;
                l.hold_db = if db > l.hold_db { db } else { (l.hold_db - 0.6).max(db) };
                l.at = Some(Instant::now());
                drop(l);
                on_update();
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        Meter { level, stop, child: None, error: None }
    }

    pub fn level(&self) -> Level {
        let l = *self.level.lock().unwrap();
        if l.at.is_some_and(|t| t.elapsed() > Duration::from_millis(500)) {
            return Level { peak_db: -90.0, hold_db: -90.0, at: l.at };
        }
        l
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}
