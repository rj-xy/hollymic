//! The tray icon on Linux: a StatusNotifierItem over D-Bus (KDE Plasma natively;
//! GNOME needs the AppIndicator extension).

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, mpsc};

use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{Icon, MenuItem, ToolTip};

use crate::app::{AppMsg, Ctx};
use crate::device::Request;
use crate::icon::{self, IconState};
use crate::protocol::{NoiseLevel, VOLUME_STEPS};
use crate::state::DeviceState;

struct HollyTray {
    ctx: Ctx,
    state: DeviceState,
    /// The icon frame showing now (flashing LEDs change it twice a second).
    icon: IconState,
}

/// Rendered icons per state, ARGB32 in network byte order as SNI wants.
fn pixmaps(st: IconState) -> Vec<Icon> {
    static CACHE: Mutex<Option<HashMap<IconState, Vec<Icon>>>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    let map = cache.get_or_insert_with(HashMap::new);
    map.entry(st)
        .or_insert_with(|| {
            [22u32, 32, 48, 64]
                .into_iter()
                .map(|size| {
                    let mut data = icon::render_rgba(size, &st);
                    for px in data.as_chunks_mut::<4>().0 {
                        px.rotate_right(1); // RGBA → ARGB
                    }
                    Icon { width: size as i32, height: size as i32, data }
                })
                .collect()
        })
        .clone()
}

impl ksni::Tray for HollyTray {
    fn id(&self) -> String {
        "hollymic".into()
    }
    fn title(&self) -> String {
        "HollyMic".into()
    }
    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }
    fn icon_pixmap(&self) -> Vec<Icon> {
        pixmaps(self.icon)
    }
    fn tool_tip(&self) -> ToolTip {
        let mut desc = self.state.summary();
        if self.state.firmware.update_available() {
            desc.push_str("\nFirmware update available");
        }
        ToolTip {
            title: self.state.model.clone().unwrap_or_else(|| "HollyMic".into()),
            description: desc,
            ..Default::default()
        }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        self.ctx.post(AppMsg::Show);
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let s = &self.state;
        let mut items: Vec<MenuItem<Self>> = Vec::new();
        let label = |text: String| -> MenuItem<Self> {
            StandardItem { label: text, enabled: false, ..Default::default() }.into()
        };
        if s.connected() {
            for (i, m) in s.mics.iter().enumerate() {
                items.push(label(match (m.online, m.battery) {
                    (true, Some(b)) => format!("Mic {}: {b} %", i + 1),
                    (true, None) => format!("Mic {}: on", i + 1),
                    (false, _) => format!("Mic {}: off", i + 1),
                }));
            }
            if s.firmware.update_available() {
                items.push(label("Firmware update available".into()));
            }
            items.push(MenuItem::Separator);
            let nc_on = s.noise.map(NoiseLevel::is_on);
            items.push(
                CheckmarkItem {
                    label: "Noise cancellation".into(),
                    checked: nc_on.unwrap_or(false),
                    enabled: nc_on.is_some() && (s.features.noise_toggle || nc_on == Some(false)),
                    activate: Box::new(|t: &mut Self| {
                        let on = !t.state.noise.is_some_and(NoiseLevel::is_on);
                        t.ctx.send(Request::SetNoise(on));
                    }),
                    ..Default::default()
                }
                .into(),
            );
            let level = s.noise_level.unwrap_or(NoiseLevel::Strong);
            items.push(
                SubMenu {
                    label: "Noise cancellation level".into(),
                    enabled: nc_on == Some(true) && s.any_mic_online(),
                    submenu: vec![
                        RadioGroup {
                            selected: if level == NoiseLevel::Low { 0 } else { 1 },
                            select: Box::new(|t: &mut Self, i| {
                                let l = if i == 0 { NoiseLevel::Low } else { NoiseLevel::Strong };
                                t.ctx.send(Request::SetNoiseLevel(l));
                            }),
                            options: vec![
                                RadioItem { label: "Weak".into(), ..Default::default() },
                                RadioItem { label: "Strong".into(), ..Default::default() },
                            ],
                        }
                        .into(),
                    ],
                    ..Default::default()
                }
                .into(),
            );
            items.push(
                SubMenu {
                    label: match s.volume_display() {
                        Some(v) => format!("Volume: {v}"),
                        None => "Volume".into(),
                    },
                    enabled: s.any_mic_online() && s.volume_locked != Some(true),
                    submenu: vec![
                        RadioGroup {
                            selected: s.volume.map(usize::from).unwrap_or(usize::MAX),
                            select: Box::new(|t: &mut Self, i| t.ctx.send(Request::SetVolume(i as u8 + 1))),
                            options: (1..=VOLUME_STEPS)
                                .map(|v| RadioItem { label: v.to_string(), ..Default::default() })
                                .collect(),
                        }
                        .into(),
                    ],
                    ..Default::default()
                }
                .into(),
            );
        } else {
            items.push(label(s.summary()));
        }
        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Open HollyMic".into(),
                icon_name: "window-new".into(),
                activate: Box::new(|t: &mut Self| t.ctx.post(AppMsg::Show)),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Settings…".into(),
                icon_name: "configure".into(),
                activate: Box::new(|t: &mut Self| t.ctx.post(AppMsg::ShowSettings)),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| {
                    t.ctx.quit.store(true, Ordering::SeqCst);
                    t.ctx.post(AppMsg::Quit);
                }),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

/// Start the tray. Sets `ctx.tray` when a tray host took it.
pub fn spawn(ctx: Ctx) {
    let flash = |c: &Ctx| c.settings.read().unwrap().flash_empty_slots;
    let state = ctx.shared.get();
    let tray = HollyTray { ctx: ctx.clone(), icon: IconState::now(&state, flash(&ctx)), state };
    let handle = match tray.spawn() {
        Ok(h) => h,
        Err(e) => {
            log::warn!("no system tray ({e}); HollyMic runs as a window only");
            return;
        }
    };
    ctx.tray.store(true, Ordering::SeqCst);
    // Push state changes to the tray from one thread, coalescing bursts.
    let (tx, rx) = mpsc::channel::<()>();
    ctx.shared.subscribe(move || {
        let _ = tx.send(());
    });
    let c = ctx.clone();
    std::thread::Builder::new()
        .name("tray".into())
        .spawn(move || {
            use std::sync::mpsc::RecvTimeoutError;
            use std::time::Duration;
            let mut shown = handle.update(|t| t.icon).unwrap_or(IconState::now(&c.shared.get(), true));
            loop {
                // Wake on state changes, and every 100 ms to step a flashing LED.
                let changed = match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(()) => {
                        std::thread::sleep(Duration::from_millis(50));
                        while rx.try_recv().is_ok() {}
                        true
                    }
                    Err(RecvTimeoutError::Timeout) => false,
                    Err(RecvTimeoutError::Disconnected) => break,
                };
                let s = c.shared.get();
                let icon = IconState::now(&s, flash(&c));
                if !changed && icon == shown {
                    continue;
                }
                shown = icon;
                if handle
                    .update(|t| {
                        t.state = s;
                        t.icon = icon;
                    })
                    .is_none()
                {
                    break;
                }
            }
        })
        .expect("spawn tray thread");
}
