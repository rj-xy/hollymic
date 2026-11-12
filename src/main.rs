//! HollyMic: a tray app for the Hollyland Lark M2 wireless mic, for Linux and macOS.

mod app;
mod autostart;
mod device;
mod firmware;
mod icon;
mod ipc;
mod meter;
mod protocol;
mod settings;
mod sim;
mod state;
#[cfg(not(target_os = "macos"))]
mod tray_linux;
#[cfg(target_os = "macos")]
mod tray_macos;
mod ui;
mod usb;

use std::time::{Duration, Instant};

use anyhow::{Result, bail};

const HELP: &str = "\
HollyMic: Hollyland Lark M2 mic control in the tray

Usage:
  hollymic                      start (or open the window of the running copy)
  hollymic status [--short]     receiver, mics, noise cancellation, volume (JSON)
  hollymic nc on|off|weak|strong
  hollymic volume 1-6
  hollymic show | settings | quit
  hollymic firmware             ask Hollyland's server for newer firmware
  hollymic export-icons <dir>   write the app and tray icons as PNG

Options:
  --demo                use a simulated receiver
  --show                open the window at start (ignores \"start minimised\")
  --screenshot <file>   save a PNG of the window and quit (with --page settings,
                        --theme light|dark, --details)
  -V, --version         print the version
  -h, --help            print this help

Environment: RUST_LOG=hollymic=debug for more logging.
";

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("hollymic=info")).init();
    if let Err(e) = real_main() {
        eprintln!("hollymic: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let value = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    if has("-h") || has("--help") {
        print!("{HELP}");
        return Ok(());
    }
    if has("-V") || has("--version") {
        println!("hollymic {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let demo = has("--demo");
    let mut words: Vec<&str> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if ["--screenshot", "--page", "--theme"].contains(&a.as_str()) {
            it.next(); // its value
        } else if !a.starts_with('-') {
            words.push(a.as_str());
        }
    }

    match words.first().copied() {
        None | Some("run") => {
            let screenshot = value("--screenshot").map(std::path::PathBuf::from);
            if screenshot.is_none()
                && !demo
                && let Some(r) = ipc::send("show")
            {
                eprintln!("HollyMic is already running; opened its window ({})", r.trim());
                return Ok(());
            }
            let ui = ui::UiOptions {
                screenshot: screenshot.clone(),
                page: (value("--page").as_deref() == Some("settings")).then_some(ui::Page::Settings),
                theme: match value("--theme").as_deref() {
                    Some("dark") => Some(settings::Theme::Dark),
                    Some("light") => Some(settings::Theme::Light),
                    _ => None,
                },
                details_open: has("--details"),
            };
            // --show opens the window; otherwise "start minimised" decides.
            let show = (has("--show") || screenshot.is_some()).then_some(true);
            if demo {
                // Keep demo runs away from the real settings and the running copy.
                let dir = std::env::temp_dir().join("hollymic-demo");
                unsafe {
                    std::env::set_var("HOLLYMIC_CONFIG_DIR", &dir);
                    std::env::set_var("HOLLYMIC_SOCKET", dir.join("hollymic.sock"));
                    std::env::set_var("HOLLYMIC_AUTOSTART_DIR", dir.join("autostart"));
                }
                std::fs::create_dir_all(&dir)?;
            }
            app::run(app::RunOptions { demo, show, ui })
        }
        Some("export-icons") => icon::export(std::path::Path::new(words.get(1).copied().unwrap_or("assets"))),
        Some("show") | Some("settings") | Some("quit") => match ipc::send(words[0]) {
            Some(r) => {
                print!("{r}");
                Ok(())
            }
            None => bail!("HollyMic isn't running"),
        },
        Some("status") => {
            let out = match (demo, ipc::send("status")) {
                (false, Some(r)) => r,
                _ => direct(demo, "status")?,
            };
            if has("--short") {
                let s: serde_json::Value = serde_json::from_str(&out)?;
                println!("{}", short(&s));
            } else {
                print!("{out}");
            }
            Ok(())
        }
        Some("nc") | Some("volume") => {
            let line = words.join(" ");
            let out = match (demo, ipc::send(&line)) {
                (false, Some(r)) => r,
                _ => direct(demo, &line)?,
            };
            print!("{out}");
            if out.starts_with("error") {
                std::process::exit(2);
            }
            Ok(())
        }
        Some("firmware") => {
            let shared = state::Shared::new(demo);
            let dev = device::spawn(shared.clone(), demo);
            wait_ready(&shared);
            let r = firmware::check_now(&shared);
            dev.stop();
            let fw = r?;
            println!("{}", serde_json::to_string_pretty(&fw)?);
            Ok(())
        }
        Some(other) => bail!("unknown command {other:?}; see hollymic --help"),
    }
}

fn wait_ready(shared: &state::Shared) {
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        let s = shared.get();
        if s.connected() && s.receiver.version.is_some() && s.noise.is_some() && s.volume.is_some() {
            std::thread::sleep(Duration::from_millis(400)); // first heartbeat and mic versions
            return;
        }
        if matches!(s.connection, state::Connection::NoPermission(_) | state::Connection::Busy(_)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Run one IPC-style command against the receiver without the tray app.
fn direct(demo: bool, line: &str) -> Result<String> {
    let shared = state::Shared::new(demo);
    let dev = device::spawn(shared.clone(), demo);
    wait_ready(&shared);
    let ctx = app::Ctx::headless(shared.clone(), dev.sender());
    let out = ipc::command(&ctx, line);
    dev.stop();
    Ok(out)
}

fn short(s: &serde_json::Value) -> String {
    if s["connection"]["state"] != "connected" {
        return s["connection"]["state"].as_str().unwrap_or("?").to_string();
    }
    let mics: Vec<String> = (0..2)
        .map(|i| {
            let m = &s["mics"][i];
            if m["online"] == true { format!("mic{} {}%", i + 1, m["battery"]) } else { format!("mic{} off", i + 1) }
        })
        .collect();
    format!(
        "{} · nc {} · volume {}/6",
        mics.join(" · "),
        s["noise"].as_str().unwrap_or("?").to_lowercase(),
        s["volume"].as_u64().map(|v| (v + 1).to_string()).unwrap_or("?".into())
    )
}
