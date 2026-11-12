//! Wiring: the context every part shares, the main loop, and desktop notifications.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::Result;

use crate::device::{self, Request};
use crate::settings::Settings;
use crate::state::{Connection, DeviceState, Shared};
use crate::ui::{self, UiOptions};
use crate::{autostart, firmware, ipc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMsg {
    Show,
    ShowSettings,
    Quit,
}

/// Everything the tray, window, IPC server and notifier share.
#[derive(Clone)]
pub struct Ctx {
    pub shared: Shared,
    pub settings: Arc<RwLock<Settings>>,
    pub device: Sender<Request>,
    pub tx: Sender<AppMsg>,
    pub rx: Arc<Mutex<Receiver<AppMsg>>>,
    pub egui: Arc<Mutex<Option<egui::Context>>>,
    pub quit: Arc<AtomicBool>,
    /// A tray icon is showing (otherwise closing the window must quit).
    pub tray: Arc<AtomicBool>,
}

impl Ctx {
    /// For one-off CLI commands without the tray or window.
    pub fn headless(shared: Shared, device: Sender<Request>) -> Ctx {
        let (tx, rx) = mpsc::channel();
        Ctx {
            shared,
            settings: Arc::new(RwLock::new(Settings::load())),
            device,
            tx,
            rx: Arc::new(Mutex::new(rx)),
            egui: Arc::new(Mutex::new(None)),
            quit: Arc::new(AtomicBool::new(false)),
            tray: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn send(&self, r: Request) {
        let _ = self.device.send(r);
    }

    pub fn post(&self, m: AppMsg) {
        let _ = self.tx.send(m);
        self.repaint();
    }

    pub fn repaint(&self) {
        if let Some(c) = &*self.egui.lock().unwrap() {
            c.request_repaint();
        }
    }

    pub fn tray_ok(&self) -> bool {
        self.tray.load(Ordering::SeqCst)
    }

    /// Change, save and apply settings.
    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) -> Result<()> {
        let new = {
            let mut s = self.settings.write().unwrap();
            f(&mut s);
            s.clone()
        };
        new.save()?;
        autostart::apply(new.start_on_login)?;
        self.shared.notify();
        Ok(())
    }
}

pub struct RunOptions {
    pub demo: bool,
    /// Started by the login entry: honour "start minimised".
    pub show: Option<bool>,
    pub ui: UiOptions,
}

pub fn run(opts: RunOptions) -> Result<()> {
    let settings = Settings::load();
    if let Err(e) = autostart::apply(settings.start_on_login) {
        log::warn!("start on login: {e:#}");
    }
    let show_window = opts.show.unwrap_or(!settings.start_minimized);
    let shared = Shared::new(opts.demo);
    let dev = device::spawn(shared.clone(), opts.demo);
    let (tx, rx) = mpsc::channel();
    let ctx = Ctx {
        shared: shared.clone(),
        settings: Arc::new(RwLock::new(settings)),
        device: dev.sender(),
        tx,
        rx: Arc::new(Mutex::new(rx)),
        egui: Arc::new(Mutex::new(None)),
        quit: Arc::new(AtomicBool::new(false)),
        tray: Arc::new(AtomicBool::new(false)),
    };
    {
        let c = ctx.clone();
        shared.subscribe(move || c.repaint());
    }
    let screenshot = opts.ui.screenshot.is_some();
    if !screenshot {
        firmware::spawn(shared.clone(), ctx.settings.clone());
        spawn_notifier(ctx.clone());
        if let Err(e) = ipc::serve(ctx.clone()) {
            log::warn!("single-instance socket: {e:#}");
        }
    }

    #[cfg(target_os = "macos")]
    {
        // One window for the app's lifetime; the tray lives in its event loop.
        let r = eframe::run_native(
            "HollyMic",
            ui::native_options(show_window || screenshot),
            Box::new(move |cc| Ok(Box::new(ui::HollyApp::new(cc, ctx, opts.ui)))),
        );
        dev.stop();
        ipc::cleanup();
        return r.map_err(|e| anyhow::anyhow!("window: {e}"));
    }

    #[cfg(not(target_os = "macos"))]
    {
        if !screenshot {
            crate::tray_linux::spawn(ctx.clone());
        }
        if show_window || screenshot || !ctx.tray_ok() {
            let _ = ctx.tx.send(AppMsg::Show);
        }
        loop {
            let msg = ctx.rx.lock().unwrap().recv();
            match msg {
                Ok(AppMsg::Show) | Ok(AppMsg::ShowSettings) => {
                    let mut o = opts.ui.clone();
                    if msg == Ok(AppMsg::ShowSettings) {
                        o.page = Some(ui::Page::Settings);
                    }
                    if let Err(e) = ui::run(ctx.clone(), o) {
                        log::error!("{e:#}");
                        if !ctx.tray_ok() {
                            break;
                        }
                    }
                    *ctx.egui.lock().unwrap() = None;
                }
                Ok(AppMsg::Quit) | Err(_) => break,
            }
            if ctx.quit.load(Ordering::SeqCst) || screenshot {
                break;
            }
        }
        dev.stop();
        ipc::cleanup();
        Ok(())
    }
}

/// Show a desktop notification (D-Bus on Linux, `osascript` on macOS).
pub fn notify(title: &str, body: &str) {
    #[cfg(not(target_os = "macos"))]
    {
        let r = notify_rust::Notification::new()
            .appname("HollyMic")
            .summary(title)
            .body(body)
            .icon("hollymic")
            .timeout(notify_rust::Timeout::Milliseconds(8000))
            .show();
        if let Err(e) = r {
            log::warn!("notification: {e}");
        }
    }
    #[cfg(target_os = "macos")]
    {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let script =
            format!("display notification \"{}\" with title \"HollyMic\" subtitle \"{}\"", esc(body), esc(title));
        let _ = std::process::Command::new("osascript").args(["-e", &script]).status();
    }
}

/// Low battery, receiver plugged in or out, firmware update available.
fn spawn_notifier(ctx: Ctx) {
    let (tx, rx) = mpsc::channel::<()>();
    ctx.shared.subscribe(move || {
        let _ = tx.send(());
    });
    std::thread::Builder::new()
        .name("notifier".into())
        .spawn(move || {
            let mut prev = ctx.shared.get();
            let mut warned = [false; 2];
            let mut fw_told: Option<String> = None;
            while rx.recv().is_ok() {
                while rx.try_recv().is_ok() {}
                let s = ctx.shared.get();
                let set = ctx.settings.read().unwrap().clone();
                for (title, body) in events(&prev, &s, &set, &mut warned, &mut fw_told) {
                    notify(&title, &body);
                }
                prev = s;
            }
        })
        .expect("spawn notifier");
}

fn events(
    prev: &DeviceState,
    s: &DeviceState,
    set: &Settings,
    warned: &mut [bool; 2],
    fw_told: &mut Option<String>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if set.connection_alerts && prev.connected() != s.connected() && !matches!(s.connection, Connection::Busy(_)) {
        if s.connected() {
            out.push(("Lark M2 connected".into(), s.summary()));
        } else if prev.connected() {
            out.push(("Lark M2 unplugged".into(), "The receiver was disconnected.".into()));
        }
    }
    for (i, (m, warned)) in s.mics.iter().zip(warned.iter_mut()).enumerate() {
        match (m.online, m.battery) {
            (true, Some(b)) if b <= set.low_battery_percent => {
                if set.low_battery_alert && !*warned {
                    out.push((
                        format!("Mic {} battery low", i + 1),
                        format!("{b} % left. Put it in the case to charge."),
                    ));
                }
                *warned = true;
            }
            (true, Some(b)) if b > set.low_battery_percent + 5 => *warned = false,
            (false, _) => *warned = false,
            _ => {}
        }
    }
    let fw = &s.firmware;
    if fw.update_available() {
        let key = format!("{:?}{:?}", fw.rx.latest, fw.tx.latest);
        if fw_told.as_deref() != Some(key.as_str()) {
            let what = match (fw.rx.update, fw.tx.update) {
                (true, true) => "receiver and mics",
                (true, false) => "receiver",
                _ => "mics",
            };
            out.push((
                "Lark M2 firmware update".into(),
                format!(
                    "New firmware for the {what}. Update it with the HollyAudio phone app or Hollyland's LarkM2Upgrade."
                ),
            ));
            *fw_told = Some(key);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_battery_warns_once() {
        let set = Settings::default();
        let mut warned = [false; 2];
        let mut fw = None;
        let prev = DeviceState { connection: Connection::Connected, ..Default::default() };
        let mut s = prev.clone();
        s.mics[0].online = true;
        s.mics[0].battery = Some(14);
        assert_eq!(events(&prev, &s, &set, &mut warned, &mut fw).len(), 1);
        assert_eq!(events(&s, &s, &set, &mut warned, &mut fw).len(), 0, "only once");
        s.mics[0].battery = Some(60); // charged
        events(&s, &s, &set, &mut warned, &mut fw);
        s.mics[0].battery = Some(10);
        assert_eq!(events(&s, &s, &set, &mut warned, &mut fw).len(), 1, "again after a charge");
    }
}
