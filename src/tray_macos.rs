//! The menu-bar icon on macOS (`tray-icon`). Lives on the main thread inside the
//! window's event loop. UNTESTED: written without a Mac; see docs/WIP.md.

use std::sync::atomic::Ordering;

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::app::{AppMsg, Ctx};
use crate::device::Request;
use crate::icon::{self, IconState};
use crate::protocol::{NoiseLevel, VOLUME_STEPS};
use crate::state::DeviceState;

pub struct MacTray {
    ctx: Ctx,
    tray: TrayIcon,
    status: MenuItem,
    nc: CheckMenuItem,
    volume: Vec<CheckMenuItem>,
    open: MenuItem,
    settings: MenuItem,
    quit: MenuItem,
    last: Option<IconState>,
}

fn tray_icon(st: &IconState) -> Option<Icon> {
    Icon::from_rgba(icon::render_rgba(44, st), 44, 44).ok()
}

impl MacTray {
    pub fn new(ctx: Ctx) -> Option<MacTray> {
        let status = MenuItem::new("HollyMic", false, None);
        let nc = CheckMenuItem::new("Noise cancellation", false, false, None);
        let volume: Vec<CheckMenuItem> =
            (1..=VOLUME_STEPS).map(|v| CheckMenuItem::new(v.to_string(), false, false, None)).collect();
        let vol_menu = Submenu::new("Volume", true);
        for v in &volume {
            vol_menu.append(v).ok()?;
        }
        let open = MenuItem::new("Open HollyMic", true, None);
        let settings = MenuItem::new("Settings…", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &status,
            &PredefinedMenuItem::separator(),
            &nc,
            &vol_menu,
            &PredefinedMenuItem::separator(),
            &open,
            &settings,
            &quit,
        ])
        .ok()?;
        let st = IconState::now(&ctx.shared.get(), ctx.settings.read().unwrap().flash_empty_slots);
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("HollyMic")
            .with_icon(tray_icon(&st)?)
            .build()
            .map_err(|e| log::warn!("menu bar icon: {e}"))
            .ok()?;
        ctx.tray.store(true, Ordering::SeqCst);
        // Wake the event loop when the menu is used while the window is hidden.
        let c = ctx.clone();
        MenuEvent::set_event_handler(Some(move |_e| c.repaint()));
        let c = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |_e| c.repaint()));
        Some(MacTray { ctx, tray, status, nc, volume, open, settings, quit, last: Some(st) })
    }

    /// Called every frame (and while hidden, on repaint requests).
    pub fn refresh(&mut self, s: &DeviceState) {
        while let Ok(e) = MenuEvent::receiver().try_recv() {
            if e.id == self.open.id() {
                self.ctx.post(AppMsg::Show);
            } else if e.id == self.settings.id() {
                self.ctx.post(AppMsg::ShowSettings);
            } else if e.id == self.quit.id() {
                self.ctx.post(AppMsg::Quit);
            } else if e.id == self.nc.id() {
                self.ctx.send(Request::SetNoise(!s.noise.is_some_and(NoiseLevel::is_on)));
            } else if let Some(i) = self.volume.iter().position(|v| v.id() == &e.id) {
                self.ctx.send(Request::SetVolume(i as u8 + 1));
            }
        }
        while TrayIconEvent::receiver().try_recv().is_ok() {}
        let st = IconState::now(s, self.ctx.settings.read().unwrap().flash_empty_slots);
        if self.last != Some(st) {
            if let Some(i) = tray_icon(&st) {
                let _ = self.tray.set_icon(Some(i));
            }
            self.last = Some(st);
        }
        let _ = self.tray.set_tooltip(Some(s.summary()));
        self.status.set_text(s.summary());
        self.nc.set_checked(s.noise.is_some_and(NoiseLevel::is_on));
        self.nc.set_enabled(s.connected() && s.features.noise_toggle);
        for (i, v) in self.volume.iter().enumerate() {
            v.set_checked(s.volume == Some(i as u8));
            v.set_enabled(s.any_mic_online() && s.volume_locked != Some(true));
        }
    }
}
