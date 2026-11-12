//! Single instance: a Unix socket in the runtime directory. Running `hollymic` again
//! opens the window of the copy that's already running; `hollymic status`, `nc` and
//! `volume` go to it too, because only one program can hold the receiver.
//!
//! Protocol: one line in (`show`, `settings`, `quit`, `status`, `nc on|off|weak|strong`,
//! `volume 1-6`, `refresh`), the reply text out, then the connection closes.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::app::{AppMsg, Ctx};
use crate::device::Request;
use crate::protocol::NoiseLevel;

pub fn socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("HOLLYMIC_SOCKET") {
        return PathBuf::from(p);
    }
    match dirs::runtime_dir() {
        Some(d) => d.join("hollymic.sock"),
        None => std::env::temp_dir().join(format!("hollymic-{}.sock", std::env::var("USER").unwrap_or_default())),
    }
}

/// Send a command to the running copy. `None` = no copy running.
pub fn send(cmd: &str) -> Option<String> {
    let mut s = UnixStream::connect(socket_path()).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
    s.write_all(format!("{cmd}\n").as_bytes()).ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    Some(out)
}

pub fn serve(ctx: Ctx) -> Result<()> {
    let path = socket_path();
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            anyhow::bail!("another HollyMic is running ({})", path.display());
        }
        let _ = std::fs::remove_file(&path); // stale
    }
    let listener = UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
    std::thread::Builder::new().name("ipc".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let c = ctx.clone();
            std::thread::spawn(move || handle(c, stream));
        }
    })?;
    Ok(())
}

pub fn cleanup() {
    let _ = std::fs::remove_file(socket_path());
}

fn handle(ctx: Ctx, stream: UnixStream) {
    let mut line = String::new();
    let mut reader = BufReader::new(&stream);
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let reply = command(&ctx, line.trim());
    let mut w = &stream;
    let _ = w.write_all(reply.as_bytes());
}

/// Wait up to 3 s for the state to satisfy `f` (the worker answers within a heartbeat).
fn wait_for(ctx: &Ctx, f: impl Fn(&crate::state::DeviceState) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if f(&ctx.shared.get()) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

type Check = Box<dyn Fn(&crate::state::DeviceState) -> bool>;

pub fn command(ctx: &Ctx, line: &str) -> String {
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        ["show"] => {
            ctx.post(AppMsg::Show);
            "ok\n".into()
        }
        ["settings"] => {
            ctx.post(AppMsg::ShowSettings);
            "ok\n".into()
        }
        ["quit"] => {
            ctx.post(AppMsg::Quit);
            "ok\n".into()
        }
        ["status"] => serde_json::to_string_pretty(&ctx.shared.get()).unwrap_or_default() + "\n",
        ["refresh"] => {
            ctx.send(Request::Refresh);
            "ok\n".into()
        }
        ["nc", arg] => {
            let (req, check): (Request, Check) = match *arg {
                "on" => (Request::SetNoise(true), Box::new(|s| s.noise.is_some_and(|n| n.is_on()))),
                "off" => (Request::SetNoise(false), Box::new(|s| s.noise == Some(NoiseLevel::Off))),
                "weak" | "low" => {
                    (Request::SetNoiseLevel(NoiseLevel::Low), Box::new(|s| s.noise == Some(NoiseLevel::Low)))
                }
                "strong" => {
                    (Request::SetNoiseLevel(NoiseLevel::Strong), Box::new(|s| s.noise == Some(NoiseLevel::Strong)))
                }
                _ => return "error: nc takes on, off, weak or strong\n".into(),
            };
            ctx.shared.update(|s| s.message = None);
            ctx.send(req);
            result(ctx, wait_for(ctx, |s| check(s) || s.message.is_some()))
        }
        ["volume", n] => match n.parse::<u8>() {
            Ok(v @ 1..=6) => {
                ctx.shared.update(|s| s.message = None);
                ctx.send(Request::SetVolume(v));
                result(ctx, wait_for(ctx, |s| s.volume_display() == Some(v) || s.message.is_some()))
            }
            _ => "error: volume takes 1 to 6\n".into(),
        },
        _ => format!("error: unknown command {line:?}\n"),
    }
}

fn result(ctx: &Ctx, done: bool) -> String {
    let s = ctx.shared.get();
    match (&s.message, done) {
        (Some(m), _) => format!("error: {m}\n"),
        (None, true) => format!("ok: {}\n", s.summary()),
        (None, false) => "error: no answer from the receiver\n".into(),
    }
}
