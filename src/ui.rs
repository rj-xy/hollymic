//! The window: a compact status and controls page, and the settings page.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};

use egui::{Align, Align2, Color32, CornerRadius, Layout, Response, RichText, Sense, Stroke, Ui, vec2};

use crate::app::{AppMsg, Ctx};
use crate::device::Request;
use crate::icon::{self, IconState};
use crate::meter::{self, Meter};
use crate::protocol::{NoiseLevel, VOLUME_STEPS};
use crate::settings::{self, Settings, Theme};
use crate::state::{Connection, DeviceState};

const ACCENT: Color32 = Color32::from_rgb(0xf5, 0xc4, 0x00); // the receiver's yellow collar
const GREEN: Color32 = Color32::from_rgb(0x3b, 0xc2, 0x5f);
const ORANGE: Color32 = Color32::from_rgb(0xf0, 0x8c, 0x1a);
const RED: Color32 = Color32::from_rgb(0xe0, 0x45, 0x45);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Main,
    Settings,
}

#[derive(Debug, Clone, Default)]
pub struct UiOptions {
    /// Save a PNG of the window after it settles, then quit (README screenshots).
    pub screenshot: Option<PathBuf>,
    pub page: Option<Page>,
    pub theme: Option<Theme>,
    pub details_open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Confirm {
    Speaker(bool),
    Restart,
}

pub fn native_options(visible: bool) -> eframe::NativeOptions {
    let rgba = icon::render(icon::Shape::Wide, 128, 128, &icon::app_icon_state());
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("HollyMic")
            .with_app_id("hollymic")
            .with_inner_size([340.0, 590.0])
            .with_min_inner_size([300.0, 380.0])
            .with_icon(egui::IconData { rgba, width: 128, height: 128 })
            .with_visible(visible),
        run_and_return: true,
        centered: true,
        ..Default::default()
    }
}

/// Open the window and block until it closes.
#[cfg(not(target_os = "macos"))]
pub fn run(ctx: Ctx, opts: UiOptions) -> anyhow::Result<()> {
    let visible = true;
    eframe::run_native(
        "HollyMic",
        native_options(visible),
        Box::new(move |cc| Ok(Box::new(HollyApp::new(cc, ctx, opts)))),
    )
    .map_err(|e| anyhow::anyhow!("window: {e}"))
}

pub struct HollyApp {
    ctx: Ctx,
    page: Page,
    meter: Option<Meter>,
    confirm: Option<Confirm>,
    icon_tex: Option<(IconState, egui::TextureHandle)>,
    applied_theme: Option<Theme>,
    settings_error: Option<String>,
    opts: UiOptions,
    started: Instant,
    shot_requested: bool,
    shot_done: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(target_os = "macos")]
    pub tray: Option<crate::tray_macos::MacTray>,
}

impl HollyApp {
    pub fn new(cc: &eframe::CreationContext<'_>, ctx: Ctx, opts: UiOptions) -> HollyApp {
        *ctx.egui.lock().unwrap() = Some(cc.egui_ctx.clone());
        style(&cc.egui_ctx);
        HollyApp {
            page: opts.page.unwrap_or(Page::Main),
            meter: None,
            confirm: None,
            icon_tex: None,
            applied_theme: None,
            settings_error: None,
            started: Instant::now(),
            shot_requested: false,
            shot_done: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            #[cfg(target_os = "macos")]
            tray: crate::tray_macos::MacTray::new(ctx.clone()),
            ctx,
            opts,
        }
    }

    fn settings(&self) -> Settings {
        self.ctx.settings.read().unwrap().clone()
    }

    fn change_settings(&mut self, f: impl FnOnce(&mut Settings)) {
        self.settings_error = self.ctx.update_settings(f).err().map(|e| format!("{e:#}"));
    }
}

fn style(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(8.0, 3.0);
        s.spacing.interact_size.y = 22.0;
        s.visuals.selection.bg_fill = ACCENT;
        s.visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(0x1b, 0x22, 0x33));
        s.visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
        s.visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
        s.visuals.widgets.active.corner_radius = CornerRadius::same(6);
        s.visuals.widgets.noninteractive.corner_radius = CornerRadius::same(6);
    });
}

impl eframe::App for HollyApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Messages from the tray and from `hollymic` run a second time.
        let msgs: Vec<AppMsg> = {
            let rx = self.ctx.rx.lock().unwrap();
            std::iter::from_fn(|| rx.try_recv().ok()).collect()
        };
        for m in msgs {
            match m {
                AppMsg::Show | AppMsg::ShowSettings => {
                    if m == AppMsg::ShowSettings {
                        self.page = Page::Settings;
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                AppMsg::Quit => {
                    self.ctx.quit.store(true, Ordering::SeqCst);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        let settings = self.settings();
        let theme = self.opts.theme.unwrap_or(settings.theme);
        if self.applied_theme != Some(theme) {
            ctx.set_theme(match theme {
                Theme::System => egui::ThemePreference::System,
                Theme::Light => egui::ThemePreference::Light,
                Theme::Dark => egui::ThemePreference::Dark,
            });
            self.applied_theme = Some(theme);
        }

        if ctx.input(|i| i.viewport().close_requested()) && !self.ctx.quit.load(Ordering::SeqCst) {
            if !settings.close_to_tray || !self.ctx.tray_ok() {
                self.ctx.quit.store(true, Ordering::SeqCst);
            } else if cfg!(target_os = "macos") {
                // macOS keeps one window for the app's lifetime: hide it instead.
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                self.meter = None;
            }
        }

        let state = self.ctx.shared.get();
        let want_meter =
            settings.level_meter && state.connected() && !ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        if want_meter && self.meter.is_none() {
            let c = ctx.clone();
            let repaint = move || c.request_repaint_after(Duration::from_millis(30));
            self.meter = Some(if state.demo { Meter::demo(repaint) } else { Meter::start(repaint) });
        } else if !want_meter && self.meter.is_some() {
            self.meter = None;
        }

        #[cfg(target_os = "macos")]
        if let Some(t) = &mut self.tray {
            t.refresh(&state);
        }

        if let Some(path) = self.opts.screenshot.clone() {
            if self.shot_done.load(Ordering::SeqCst) {
                self.ctx.quit.store(true, Ordering::SeqCst);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if !self.shot_requested && self.started.elapsed() > shot_delay() {
                self.shot_requested = true;
                log::debug!("requesting screenshot");
                let done = self.shot_done.clone();
                let c = ctx.clone();
                ctx.request_screenshot(move |img| {
                    if let Err(e) = save_png(&path, &img) {
                        log::error!("screenshot: {e:#}");
                    }
                    done.store(true, Ordering::SeqCst);
                    c.request_repaint();
                });
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        // Step a flashing LED in the header (and the macOS menu-bar icon).
        let flashing = settings.flash_empty_slots && IconState::from_state(&state).blinks();
        ctx.request_repaint_after(if flashing { Duration::from_millis(100) } else { Duration::from_secs(1) });
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        log::trace!("ui frame");
        let state = self.ctx.shared.get();
        let tex = self.icon_texture(ui.ctx(), &state);
        egui::CentralPanel::default().show(ui, |ui| {
            header(ui, &state, &tex, &mut self.page);
            ui.add_space(4.0);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.page {
                Page::Main => self.main_page(ui, &state),
                Page::Settings => self.settings_page(ui, &state),
            });
        });
        self.confirm_dialog(ui.ctx());
    }
}

fn save_png(path: &std::path::Path, img: &egui::ColorImage) -> anyhow::Result<()> {
    let [w, h] = img.size;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for p in &img.pixels {
        rgba.extend_from_slice(&p.to_srgba_unmultiplied());
    }
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgba)?;
    log::info!("screenshot saved to {}", path.display());
    Ok(())
}

impl HollyApp {
    fn icon_texture(&mut self, ctx: &egui::Context, state: &DeviceState) -> egui::TextureHandle {
        let st = IconState::now(state, self.settings().flash_empty_slots);
        if let Some((s, t)) = &self.icon_tex
            && *s == st
        {
            return t.clone();
        }
        // The wide receiver, drawn at 3x for a crisp 64x40 header image.
        let rgba = icon::render(icon::Shape::Wide, 192, 120, &st);
        let img = egui::ColorImage::from_rgba_unmultiplied([192, 120], &rgba);
        let t = ctx.load_texture("dongle", img, egui::TextureOptions::LINEAR);
        self.icon_tex = Some((st, t.clone()));
        t
    }

    fn send(&self, r: Request) {
        self.ctx.send(r);
    }

    fn main_page(&mut self, ui: &mut Ui, s: &DeviceState) {
        if let Some(m) = &s.message {
            banner(ui, ORANGE, m, || self.ctx.shared.update(|s| s.message = None));
        }
        if !s.connected() {
            not_connected(ui, s);
            firmware_line(ui, s, &self.settings());
            return;
        }
        card(ui, |ui| {
            for i in 0..2 {
                mic_row(ui, i, s);
            }
        });

        let mic_on = s.any_mic_online();
        card(ui, |ui| {
            let nc_on = s.noise.map(NoiseLevel::is_on);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Noise cancellation").strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let mut on = nc_on.unwrap_or(false);
                    let enabled = nc_on.is_some() && (s.features.noise_toggle || !on);
                    let r = ui.add_enabled_ui(enabled, |ui| toggle(ui, &mut on)).inner;
                    if r.changed() {
                        self.send(Request::SetNoise(on));
                    }
                    if !s.features.noise_toggle {
                        r.on_hover_text("This receiver firmware has no on/off switch (needs newer than 2.0.0.31).");
                    }
                });
            });
            let level = s.noise_level.unwrap_or(NoiseLevel::Strong);
            ui.add_enabled_ui(nc_on == Some(true) && mic_on, |ui| {
                if let Some(l) =
                    segmented(ui, &[(NoiseLevel::Low, "Weak"), (NoiseLevel::Strong, "Strong")], Some(level))
                {
                    self.send(Request::SetNoiseLevel(l));
                }
            });
            if nc_on == Some(true) && !mic_on {
                ui.label(RichText::new("Take a mic out of the case to change the level.").small().weak());
            }
        });

        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Volume").strong());
                if s.volume_locked == Some(true) {
                    ui.label(RichText::new("locked").small().color(ORANGE));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(v) = s.volume_display() {
                        ui.label(RichText::new(format!("{v} / {VOLUME_STEPS}")).weak());
                    }
                });
            });
            let steps: Vec<(u8, String)> = (1..=VOLUME_STEPS).map(|v| (v, v.to_string())).collect();
            let steps: Vec<(u8, &str)> = steps.iter().map(|(v, t)| (*v, t.as_str())).collect();
            ui.add_enabled_ui(mic_on && s.volume_locked != Some(true), |ui| {
                if let Some(v) = segmented(ui, &steps, s.volume_display()) {
                    self.send(Request::SetVolume(v));
                }
            });
            if let Some(m) = &self.meter {
                ui.add_space(2.0);
                level_bar(ui, m, mic_on);
            }
        });

        firmware_line(ui, s, &self.settings());

        egui::CollapsingHeader::new("Device").default_open(self.opts.details_open).show(ui, |ui| self.details(ui, s));
    }

    fn details(&mut self, ui: &mut Ui, s: &DeviceState) {
        let dash = || "—".to_string();
        egui::Grid::new("details").num_columns(3).spacing([10.0, 4.0]).show(ui, |ui| {
            ui.label(RichText::new("").weak());
            ui.label(RichText::new("Firmware").weak());
            ui.label(RichText::new("Serial").weak());
            ui.end_row();
            ui.label("Receiver");
            ui.label(s.receiver.version.clone().unwrap_or_else(dash));
            ui.label(s.receiver.serial.clone().unwrap_or_else(dash));
            ui.end_row();
            for (i, m) in s.mics.iter().enumerate() {
                ui.label(format!("Mic {}", i + 1));
                ui.label(m.info.version.clone().unwrap_or_else(dash));
                ui.label(m.info.serial.clone().unwrap_or_else(|| if m.online { dash() } else { "off".into() }));
                ui.end_row();
            }
        });
        ui.add_space(4.0);
        if let Some(on) = s.speaker {
            ui.horizontal(|ui| {
                // Toggle right after the label, not at the far edge (RJ, 2026-11-13).
                ui.label("Phone speaker");
                info_icon(ui, PHONE_SPEAKER_HELP);
                ui.add_space(6.0);
                let mut v = on;
                if toggle(ui, &mut v).changed() {
                    self.confirm = Some(Confirm::Speaker(v));
                }
            });
        }
        if s.features.volume_lock {
            ui.horizontal(|ui| {
                ui.label("Volume lock");
                ui.add_space(6.0);
                let mut v = s.volume_locked.unwrap_or(false);
                if toggle(ui, &mut v).changed() {
                    self.send(Request::SetVolumeLock(v));
                }
            });
        }
        ui.horizontal(|ui| {
            if ui.button("Restart receiver").clicked() {
                self.confirm = Some(Confirm::Restart);
            }
            if ui.button("Refresh").clicked() {
                self.send(Request::Refresh);
            }
            if ui.button("Copy details").clicked() {
                ui.ctx().copy_text(serde_json::to_string_pretty(s).unwrap_or_default());
            }
        });
    }

    fn settings_page(&mut self, ui: &mut Ui, s: &DeviceState) {
        let cur = self.settings();
        let row = |ui: &mut Ui, label: &str, hint: &str, value: bool| -> Option<bool> {
            let mut v = value;
            let mut changed = false;
            ui.horizontal(|ui| {
                let l = ui.label(label);
                if !hint.is_empty() {
                    l.on_hover_text(hint);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| changed = toggle(ui, &mut v).changed());
            });
            changed.then_some(v)
        };
        card(ui, |ui| {
            ui.label(RichText::new("Startup").strong());
            if let Some(v) = row(ui, "Start on login", "Adds HollyMic to your desktop's autostart", cur.start_on_login)
            {
                self.change_settings(|s| s.start_on_login = v);
            }
            if let Some(v) = row(
                ui,
                "Start minimised",
                "Start as a tray icon only, without opening this window",
                cur.start_minimized,
            ) {
                self.change_settings(|s| s.start_minimized = v);
            }
            if let Some(v) = row(
                ui,
                "Close to tray",
                "Closing the window keeps HollyMic running in the tray. Off: closing the window quits.",
                cur.close_to_tray,
            ) {
                self.change_settings(|s| s.close_to_tray = v);
            }
        });
        card(ui, |ui| {
            ui.label(RichText::new("Appearance").strong());
            ui.horizontal(|ui| {
                ui.label("Theme");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // right-to-left: add in reverse
                    let opts = [(Theme::Dark, "Dark"), (Theme::Light, "Light"), (Theme::System, "System")];
                    for (t, name) in opts {
                        if ui.add(egui::Button::new(name).selected(cur.theme == t)).clicked() && cur.theme != t {
                            self.change_settings(|s| s.theme = t);
                        }
                    }
                });
            });
            if let Some(v) =
                row(ui, "Level meter", "Show the mic's input level (reads the audio through PipeWire)", cur.level_meter)
            {
                self.change_settings(|s| s.level_meter = v);
            }
        });
        card(ui, |ui| {
            ui.label(RichText::new("Notifications").strong());
            if let Some(v) = row(ui, "Low mic battery", "", cur.low_battery_alert) {
                self.change_settings(|s| s.low_battery_alert = v);
            }
            ui.add_enabled_ui(cur.low_battery_alert, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Alert at").weak());
                    let mut pct = cur.low_battery_percent;
                    if ui.add(egui::Slider::new(&mut pct, 5..=50).suffix(" %")).changed() {
                        self.change_settings(|s| s.low_battery_percent = pct);
                    }
                });
            });
            if let Some(v) = row(ui, "Receiver plugged in or out", "", cur.connection_alerts) {
                self.change_settings(|s| s.connection_alerts = v);
            }
        });
        card(ui, |ui| {
            ui.label(RichText::new("Firmware").strong());
            if let Some(v) = row(
                ui,
                "Check for updates daily",
                "Asks Hollyland's server (device-api.hollyland.com) which firmware is current. No serial numbers are sent.",
                cur.check_firmware,
            ) {
                self.change_settings(|s| s.check_firmware = v);
            }
            ui.horizontal(|ui| {
                let can = s.connected() && s.receiver.version.is_some() && !s.demo;
                if ui.add_enabled(can, egui::Button::new("Check now")).clicked() {
                    let shared = self.ctx.shared.clone();
                    std::thread::spawn(move || {
                        let _ = crate::firmware::check_now(&shared);
                    });
                }
                ui.label(RichText::new(checked_text(s)).weak().small());
            });
        });
        if let Some(e) = &self.settings_error {
            ui.colored_label(RED, e);
        }
        ui.add_space(4.0);
        ui.label(RichText::new(format!("Settings: {}", tilde(&settings::path()))).small().weak());
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("HollyMic {}", env!("CARGO_PKG_VERSION"))).small().weak());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Quit HollyMic").clicked() {
                    let _ = self.ctx.tx.send(AppMsg::Quit);
                    ui.ctx().request_repaint();
                }
            });
        });
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(c) = self.confirm else { return };
        let (title, text, action) = match c {
            Confirm::Speaker(on) => (
                if on { "Turn the phone speaker on?" } else { "Turn the phone speaker off?" },
                "The receiver restarts to apply it. The mic drops out for a few seconds.",
                "Change and restart",
            ),
            Confirm::Restart => ("Restart the receiver?", "The mic drops out for a few seconds.", "Restart"),
        };
        let mut close = false;
        egui::Modal::new(egui::Id::unique("hollymic-confirm")).show(ctx, |ui| {
            ui.set_max_width(260.0);
            ui.label(RichText::new(title).strong());
            ui.label(text);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(action).clicked() {
                    match c {
                        Confirm::Speaker(on) => self.ctx.send(Request::SetSpeaker(on)),
                        Confirm::Restart => self.ctx.send(Request::RestartReceiver),
                    }
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.confirm = None;
        }
    }
}

fn tilde(p: &std::path::Path) -> String {
    let s = p.display().to_string();
    match dirs::home_dir() {
        Some(h) => s.replacen(&h.display().to_string(), "~", 1),
        None => s,
    }
}

fn checked_text(s: &DeviceState) -> String {
    match s.firmware.checked_at {
        None => "Not checked yet".into(),
        Some(t) => {
            let ago = SystemTime::now().duration_since(t).unwrap_or_default().as_secs();
            let when = match ago {
                0..60 => "just now".to_string(),
                60..3600 => format!("{} min ago", ago / 60),
                3600..86400 => format!("{} h ago", ago / 3600),
                _ => format!("{} days ago", ago / 86400),
            };
            format!("Checked {when}")
        }
    }
}

fn header(ui: &mut Ui, s: &DeviceState, tex: &egui::TextureHandle, page: &mut Page) {
    ui.horizontal(|ui| {
        ui.add(egui::Image::new((tex.id(), vec2(64.0, 40.0))));
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let title = s.model.clone().unwrap_or_else(|| "HollyMic".into());
            ui.label(RichText::new(title).size(16.0).strong());
            let (dot, text) = match &s.connection {
                Connection::Connected if s.demo => (GREEN, "Demo receiver".to_string()),
                Connection::Connected => (GREEN, "Connected".to_string()),
                Connection::Searching => (Color32::GRAY, "Receiver not plugged in".into()),
                Connection::NoPermission(_) => (RED, "No permission".into()),
                Connection::Busy(_) => (ORANGE, "In use by another program".into()),
                Connection::Error(_) => (RED, "Error".into()),
            };
            ui.horizontal(|ui| {
                status_dot(ui, dot);
                ui.label(RichText::new(text).small().weak());
            });
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (gear, tip, next) = match page {
                Page::Main => (true, "Settings", Page::Settings),
                Page::Settings => (false, "Back", Page::Main),
            };
            if icon_button(ui, gear).on_hover_text(tip).clicked() {
                *page = next;
            }
        });
    });
}

fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let fill = ui.visuals().faint_bg_color;
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn banner(ui: &mut Ui, color: Color32, text: &str, on_close: impl FnOnce()) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(text).small());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(egui::Button::new(RichText::new("x").small()).frame(false)).clicked() {
                        on_close();
                    }
                });
            });
        });
}

fn not_connected(ui: &mut Ui, s: &DeviceState) {
    card(ui, |ui| match &s.connection {
        Connection::Searching => {
            ui.label(RichText::new("Plug in the Lark M2 receiver").strong());
            ui.label("Use a USB-C port that carries data: a charge-only port lights the receiver but the computer can't see it.");
        }
        Connection::NoPermission(e) => {
            ui.label(RichText::new("HollyMic can't open the receiver").strong());
            ui.label("Linux needs a udev rule so you can use it without root. The installer adds it; or run:");
            let cmd = "echo 'SUBSYSTEM==\"usb\", ATTR{idVendor}==\"3547\", ATTR{idProduct}==\"0007\", TAG+=\"uaccess\"' | sudo tee /etc/udev/rules.d/70-hollymic.rules && sudo udevadm control --reload && sudo udevadm trigger";
            ui.add(egui::TextEdit::multiline(&mut cmd.to_string()).code_editor().desired_rows(3));
            ui.label(RichText::new(e).small().weak());
        }
        Connection::Busy(e) => {
            ui.label(RichText::new("Another program is using the receiver").strong());
            ui.label("Close it (for example the hollyland-web-ui bridge, or a second HollyMic); HollyMic retries every few seconds.");
            ui.label(RichText::new(e).small().weak());
        }
        Connection::Error(e) => {
            ui.label(RichText::new("Something went wrong").strong());
            ui.label(e);
        }
        Connection::Connected => {}
    });
}

fn mic_row(ui: &mut Ui, i: usize, s: &DeviceState) {
    let m = &s.mics[i];
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Mic {}", i + 1)).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if m.online {
                if let Some(b) = m.battery {
                    ui.label(format!("{b} %"));
                    battery(ui, b);
                } else {
                    ui.label(RichText::new("on").weak());
                }
            } else {
                ui.label(RichText::new("off · in the case").weak());
            }
        });
    });
}

fn battery(ui: &mut Ui, pct: u8) {
    let (rect, _) = ui.allocate_exact_size(vec2(30.0, 13.0), Sense::hover());
    let p = ui.painter();
    let stroke = Stroke::new(1.2, ui.visuals().text_color().gamma_multiply(0.7));
    let body = egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x - 3.0, rect.max.y));
    p.rect_stroke(body, 3.0, stroke, egui::StrokeKind::Inside);
    let nub = egui::Rect::from_min_max(
        egui::pos2(body.max.x, rect.center().y - 2.5),
        egui::pos2(rect.max.x, rect.center().y + 2.5),
    );
    p.rect_filled(nub, 1.0, stroke.color);
    let inner = body.shrink(2.5);
    let w = inner.width() * (pct.min(100) as f32 / 100.0);
    let color = match pct {
        0..=15 => RED,
        16..=30 => ORANGE,
        _ => GREEN,
    };
    p.rect_filled(egui::Rect::from_min_size(inner.min, vec2(w, inner.height())), 1.5, color);
}

fn level_bar(ui: &mut Ui, m: &Meter, mic_on: bool) {
    if let Some(e) = &m.error {
        ui.label(RichText::new(e).small().weak());
        return;
    }
    let l = m.level();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Level").small().weak());
        let w = ui.available_width() - 56.0;
        let (rect, _) = ui.allocate_exact_size(vec2(w.max(40.0), 8.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
        let frac = |db: f32| ((db - meter::FLOOR_DB) / -meter::FLOOR_DB).clamp(0.0, 1.0);
        let f = frac(l.peak_db);
        if f > 0.0 {
            let color = if l.peak_db > -6.0 {
                RED
            } else if l.peak_db > -18.0 {
                ACCENT
            } else {
                GREEN
            };
            p.rect_filled(egui::Rect::from_min_size(rect.min, vec2(rect.width() * f, rect.height())), 4.0, color);
        }
        let h = frac(l.hold_db);
        if h > 0.0 {
            let x = rect.min.x + rect.width() * h;
            p.line_segment(
                [egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)],
                Stroke::new(2.0, ui.visuals().strong_text_color()),
            );
        }
        let text = if !mic_on {
            "—".to_string()
        } else if l.peak_db <= -89.0 {
            "silent".into()
        } else {
            format!("{:.0} dB", l.peak_db)
        };
        ui.label(RichText::new(text).small().weak().monospace());
    });
}

fn firmware_line(ui: &mut Ui, s: &DeviceState, settings: &Settings) {
    let fw = &s.firmware;
    if fw.update_available() {
        let mut parts = Vec::new();
        if fw.rx.update {
            parts.push(format!("receiver {}", fw.rx.latest.clone().unwrap_or_default()));
        }
        if fw.tx.update {
            parts.push(format!("mics {}", fw.tx.latest.clone().unwrap_or_default()));
        }
        let text = format!(
            "Firmware update available: {}. Update it with the HollyAudio phone app or Hollyland's LarkM2Upgrade (Windows, Mac).",
            parts.join(", ")
        );
        let notes =
            [&fw.rx.notes, &fw.tx.notes].iter().filter_map(|n| n.as_ref()).cloned().collect::<Vec<_>>().join("\n");
        banner(ui, ORANGE, &text, || {});
        if !notes.is_empty() {
            ui.label(RichText::new("Release notes").small().weak()).on_hover_text(notes);
        }
    } else if !settings.check_firmware || s.demo {
        // nothing
    } else if let Some(e) = &fw.error {
        ui.label(RichText::new("Firmware check failed").small().weak()).on_hover_text(e);
    } else if fw.checked_at.is_some() {
        ui.horizontal(|ui| {
            status_dot(ui, GREEN);
            ui.label(RichText::new("Firmware up to date").small().weak());
        })
        .response
        .on_hover_text(checked_text(s));
    }
}

const PHONE_SPEAKER_HELP: &str = "\
What it's for: when the receiver is plugged into a phone, this decides whether the phone keeps playing \
sound through its own speaker.

On: the receiver is just a microphone, and the phone's speaker keeps working while it's plugged in.

Off: the phone may send its sound to the receiver instead. The receiver has no speaker, so you'd hear nothing.

Changing it restarts the receiver: the mic drops out for a few seconds.

Warning: on a computer, leave it on. When it's off, your computer may see a new \"Wireless microphone\" \
sound output and switch your sound to it, so videos and calls go silent.";

/// A small drawn "i" in a circle. Its help shows at once on hover, and a click pins
/// it open until the next click anywhere.
fn info_icon(ui: &mut Ui, text: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
    let pinned_id = resp.id.with("pinned");
    let mut pinned = ui.data(|d| d.get_temp::<bool>(pinned_id)).unwrap_or(false);
    if resp.clicked() {
        pinned = !pinned;
    } else if pinned && ui.input(|i| i.pointer.any_click()) {
        pinned = false; // a click anywhere else closes it
    }
    ui.data_mut(|d| d.insert_temp(pinned_id, pinned));
    let open = pinned || resp.hovered();
    let color = if open { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
    let p = ui.painter();
    p.circle_stroke(rect.center(), 6.5, Stroke::new(1.3, color));
    p.text(rect.center() + vec2(0.0, 0.5), Align2::CENTER_CENTER, "i", egui::FontId::proportional(10.5), color);
    egui::Popup::from_response(&resp)
        .open(open)
        .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
        .align(egui::RectAlign::TOP_START)
        .align_alternatives(&[egui::RectAlign::BOTTOM_START, egui::RectAlign::TOP_END, egui::RectAlign::BOTTOM_END])
        .width(280.0)
        .show(|ui| {
            ui.set_max_width(280.0);
            ui.label(text);
        });
    resp.on_hover_cursor(egui::CursorIcon::Help)
}

fn status_dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}

/// A frameless 24 px button with a drawn gear (settings) or back chevron.
fn icon_button(ui: &mut Ui, gear: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
    let v = ui.style().interact(&resp);
    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, v.weak_bg_fill);
    }
    let c = rect.center();
    let stroke = Stroke::new(2.0, v.fg_stroke.color);
    let p = ui.painter();
    if gear {
        for k in 0..8 {
            let a = k as f32 * std::f32::consts::FRAC_PI_4;
            let (sa, ca) = a.sin_cos();
            p.line_segment([c + vec2(ca, sa) * 5.0, c + vec2(ca, sa) * 8.5], Stroke::new(2.6, v.fg_stroke.color));
        }
        p.circle_stroke(c, 5.5, stroke);
        p.circle_stroke(c, 2.0, Stroke::new(1.6, v.fg_stroke.color));
    } else {
        p.line_segment([c + vec2(3.0, -6.0), c + vec2(-3.0, 0.0)], stroke);
        p.line_segment([c + vec2(-3.0, 0.0), c + vec2(3.0, 6.0)], stroke);
    }
    resp
}

/// An on/off switch (from the egui demo).
fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let size = ui.spacing().interact_size.y * vec2(1.9, 1.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let how_on = ui.ctx().animate_bool_responsive(response.id, *on);
        let visuals = ui.style().interact_selectable(&response, *on);
        let rect = rect.expand(visuals.expansion);
        let radius = 0.5 * rect.height();
        let fill = if *on { ACCENT } else { visuals.bg_fill };
        ui.painter().rect(rect, radius, fill, visuals.bg_stroke, egui::StrokeKind::Inside);
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
        let knob = if *on { Color32::from_rgb(0x1b, 0x22, 0x33) } else { visuals.fg_stroke.color };
        ui.painter().circle_filled(egui::pos2(x, rect.center().y), 0.72 * radius, knob);
    }
    response
}

/// A row of equal buttons; returns the value clicked (if it isn't already selected).
fn segmented<T: Copy + PartialEq>(ui: &mut Ui, options: &[(T, &str)], selected: Option<T>) -> Option<T> {
    let mut out = None;
    let n = options.len() as f32;
    let gap = 4.0;
    let w = ((ui.available_width() - gap * (n - 1.0)) / n).max(24.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (v, label) in options {
            let sel = selected == Some(*v);
            let b = egui::Button::new(RichText::new(*label).strong()).selected(sel).min_size(vec2(w, 26.0));
            if ui.add(b).clicked() && !sel {
                out = Some(*v);
            }
        }
    });
    out
}

/// How long `--screenshot` waits before capturing: 1.5 s, or `HOLLYMIC_SHOT_DELAY_MS`
/// (scripts/screenshots.sh uses a longer one to capture a tooltip).
fn shot_delay() -> Duration {
    let ms = std::env::var("HOLLYMIC_SHOT_DELAY_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(1500);
    Duration::from_millis(ms)
}
