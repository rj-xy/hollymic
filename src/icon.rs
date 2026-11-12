//! The icons: the Lark M2 USB-C receiver, drawn in code (no Hollyland artwork).
//! Wide with the plug for the window and app menu; square without the plug for the tray.
//!
//! Navy body, silver USB-C plug with the yellow collar, the end button as a plain pad,
//! and the receiver's two LEDs, one per mic (TX1, TX2):
//! (the same as the receiver's own LEDs, RJ 2026-11-13):
//! - green: that mic is connected, noise cancellation on
//! - blue: that mic is connected, noise cancellation off
//! - flashing blue: no mic connected on that slot
//! - off, dongle faded: no receiver

use crate::protocol::NoiseLevel;
use crate::state::DeviceState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Led {
    Off,
    Green,
    Blue,
    /// Flashing blue: no mic on this slot. Resolved to `Blue`/`Off` per frame.
    Blink,
}

/// Flash phase shared by the tray and the window: on for 500 ms, off for 500 ms.
pub fn blink_on() -> bool {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    (ms / 500).is_multiple_of(2)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IconState {
    pub leds: [Led; 2],
    /// No receiver: draw the dongle faded.
    pub faded: bool,
}

impl IconState {
    pub fn from_state(s: &DeviceState) -> IconState {
        let lit = match s.noise {
            Some(NoiseLevel::Off) => Led::Blue,
            Some(_) => Led::Green,
            None => Led::Blue,
        };
        if !s.connected() {
            return IconState { leds: [Led::Off, Led::Off], faded: true };
        }
        let leds = [0, 1].map(|i| if s.mics[i].online { lit } else { Led::Blink });
        IconState { leds, faded: false }
    }

    pub fn blinks(&self) -> bool {
        self.leds.contains(&Led::Blink)
    }

    /// What to draw now: flashing LEDs on or off for this phase, or always off
    /// when the user turned flashing off.
    pub fn frame(self, flash: bool, on: bool) -> IconState {
        let leds = self.leds.map(|l| match l {
            Led::Blink if flash && on => Led::Blue,
            Led::Blink => Led::Off,
            other => other,
        });
        IconState { leds, ..self }
    }

    /// The state to draw right now.
    pub fn now(s: &DeviceState, flash: bool) -> IconState {
        IconState::from_state(s).frame(flash, blink_on())
    }
}

#[derive(Clone, Copy)]
struct Rgba(f32, f32, f32, f32);

fn hex(c: u32, a: f32) -> Rgba {
    Rgba(((c >> 16) & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, (c & 0xff) as f32 / 255.0, a)
}

/// Signed distance to a rounded rectangle (negative inside).
fn sd_round_rect(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> f32 {
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (hx, hy) = ((x1 - x0) / 2.0 - r, (y1 - y0) / 2.0 - r);
    let (dx, dy) = ((px - cx).abs() - hx, (py - cy).abs() - hy);
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    outside + dx.max(dy).min(0.0) - r
}

fn sd_circle(px: f32, py: f32, cx: f32, cy: f32, r: f32) -> f32 {
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - r
}

fn over(dst: Rgba, src: Rgba) -> Rgba {
    let a = src.3 + dst.3 * (1.0 - src.3);
    if a <= 0.0 {
        return Rgba(0.0, 0.0, 0.0, 0.0);
    }
    let mix = |s: f32, d: f32| (s * src.3 + d * dst.3 * (1.0 - src.3)) / a;
    Rgba(mix(src.0, dst.0), mix(src.1, dst.1), mix(src.2, dst.2), a)
}

/// The two drawings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    /// Square, no plug: the tray (64×64 design units).
    Tray,
    /// The receiver as it looks, plug on top: window header and app icon (128×80 units).
    Wide,
}

impl Shape {
    fn design(self) -> (f32, f32) {
        match self {
            Shape::Tray => (64.0, 64.0),
            Shape::Wide => (128.0, 80.0),
        }
    }
}

const NAVY: u32 = 0x1d2433;
const RIM: u32 = 0x7d879f;
const YELLOW: u32 = 0xf5c400;
const SILVER: u32 = 0xc9ced6;

fn fill(c: &mut Rgba, d: f32, col: Rgba) {
    if d <= 0.0 {
        *c = over(*c, col);
    }
}

fn led(c: &mut Rgba, x: f32, y: f32, cx: f32, cy: f32, r: f32, led: Led) {
    let d = sd_circle(x, y, cx, cy, r);
    let col = match led {
        Led::Off => hex(0x3a4357, 1.0),
        Led::Green => hex(0x3be26b, 1.0),
        Led::Blue | Led::Blink => hex(0x3d8bff, 1.0),
    };
    let lit = led != Led::Off;
    if lit && d > 0.0 && d < r {
        let glow = 0.55 * (1.0 - d / r).powi(2);
        *c = over(*c, Rgba(col.0, col.1, col.2, glow));
    }
    fill(c, d, col);
    if lit {
        fill(c, sd_circle(x, y, cx - r * 0.3, cy - r * 0.3, r * 0.3), Rgba(1.0, 1.0, 1.0, 0.65));
    }
}

/// Tray: the receiver's body seen square-on, its two LEDs large, the yellow button on the edge.
fn sample_tray(x: f32, y: f32, st: &IconState) -> Rgba {
    let mut c = Rgba(0.0, 0.0, 0.0, 0.0);
    // Yellow button peeking out of the left edge (photo: left end of the receiver).
    fill(&mut c, sd_round_rect(x, y, 1.0, 22.0, 8.0, 42.0, 2.5), hex(YELLOW, 1.0));
    // Body with a light rim so it shows on dark panels.
    fill(&mut c, sd_round_rect(x, y, 4.0, 6.0, 63.0, 58.0, 12.0), hex(RIM, 1.0));
    fill(&mut c, sd_round_rect(x, y, 6.0, 8.0, 61.0, 56.0, 10.5), hex(NAVY, 1.0));
    led(&mut c, x, y, 22.5, 32.0, 7.5, st.leds[0]);
    led(&mut c, x, y, 44.5, 32.0, 7.5, st.leds[1]);
    if st.faded {
        c.3 *= 0.45;
    }
    c
}

/// Wide: the USB-C receiver from RJ's photos (2026-11-13): plug and yellow collar on top,
/// round badge in the middle (a plain ring, not Hollyland's logo), TX1/TX2 LEDs on the right.
fn sample_wide(x: f32, y: f32, st: &IconState) -> Rgba {
    let mut c = Rgba(0.0, 0.0, 0.0, 0.0);
    // USB-C plug seen from the top: a plain metal shell with square corners (the tip is
    // only rounded seen edge-on), nothing visible
    // on it (RJ, 2026-11-13: pins or a slot make it look like Lightning).
    fill(&mut c, sd_round_rect(x, y, 54.0, 2.0, 74.0, 24.0, 1.2), hex(SILVER, 1.0));
    fill(&mut c, sd_round_rect(x, y, 56.5, 4.5, 59.0, 17.0, 1.2), Rgba(1.0, 1.0, 1.0, 0.35)); // sheen
    fill(&mut c, sd_round_rect(x, y, 46.0, 17.0, 82.0, 26.0, 2.5), hex(YELLOW, 1.0));
    // Yellow button on the bottom edge, right.
    fill(&mut c, sd_round_rect(x, y, 98.0, 72.0, 118.0, 79.0, 2.5), hex(YELLOW, 1.0));
    // Body.
    fill(&mut c, sd_round_rect(x, y, 2.0, 22.0, 126.0, 77.0, 15.0), hex(RIM, 1.0));
    fill(&mut c, sd_round_rect(x, y, 4.0, 24.0, 124.0, 75.0, 13.5), hex(NAVY, 1.0));
    // Badge: grey ring, black face, a white ring broken on the diagonal (a nod to
    // the logo without copying it).
    let (bx, by) = (64.0, 49.5);
    fill(&mut c, sd_circle(x, y, bx, by, 20.0), hex(0x6a707b, 1.0));
    fill(&mut c, sd_circle(x, y, bx, by, 15.0), hex(0x15181d, 1.0));
    let ang = (y - by).atan2(x - bx).to_degrees();
    let gap = |a: f32| ((ang - a + 540.0) % 360.0 - 180.0).abs() < 22.0;
    if !gap(-45.0) && !gap(135.0) {
        fill(&mut c, sd_circle(x, y, bx, by, 9.0).abs() - 2.8, hex(0xe6e8ec, 1.0));
    }
    // "TX1 TX2" labels as short bars, the LEDs under them.
    for cx in [98.0, 113.0] {
        fill(&mut c, sd_round_rect(x, y, cx - 5.5, 39.0, cx + 5.5, 42.5, 1.2), hex(0x9aa3b5, 1.0));
    }
    led(&mut c, x, y, 98.0, 54.0, 5.5, st.leds[0]);
    led(&mut c, x, y, 113.0, 54.0, 5.5, st.leds[1]);
    if st.faded {
        c.3 *= 0.45;
    }
    c
}

fn sample(shape: Shape, x: f32, y: f32, st: &IconState) -> Rgba {
    match shape {
        Shape::Tray => sample_tray(x, y, st),
        Shape::Wide => sample_wide(x, y, st),
    }
}

/// Draw `shape` into a `w`×`h` canvas as RGBA8 (straight alpha), 4×4 supersampled,
/// scaled to fit and centred (the rest stays transparent).
pub fn render(shape: Shape, w: u32, h: u32, st: &IconState) -> Vec<u8> {
    const SS: u32 = 4;
    let (dw, dh) = shape.design();
    let scale = (dw / w as f32).max(dh / h as f32); // design units per pixel
    let (ox, oy) = ((w as f32 * scale - dw) / 2.0, (h as f32 * scale - dh) / 2.0);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for py in 0..h {
        for px in 0..w {
            let (mut r, mut g, mut b, mut a) = (0.0, 0.0, 0.0, 0.0);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) * scale - ox;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) * scale - oy;
                    let c = sample(shape, x, y, st);
                    r += c.0 * c.3;
                    g += c.1 * c.3;
                    b += c.2 * c.3;
                    a += c.3;
                }
            }
            let alpha = a / (SS * SS) as f32;
            let un = |v: f32| if a > 0.0 { (v / a * 255.0).round().clamp(0.0, 255.0) as u8 } else { 0 };
            out.extend_from_slice(&[un(r), un(g), un(b), (alpha * 255.0).round() as u8]);
        }
    }
    out
}

/// The tray icon at `size`×`size`.
pub fn render_rgba(size: u32, st: &IconState) -> Vec<u8> {
    render(Shape::Tray, size, size, st)
}

/// The app icon (both mics on, noise cancellation on).
pub fn app_icon_state() -> IconState {
    IconState { leds: [Led::Green, Led::Green], faded: false }
}

pub fn write_png(path: &std::path::Path, shape: Shape, w: u32, h: u32, st: &IconState) -> anyhow::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&render(shape, w, h, st))?;
    Ok(())
}

/// `hollymic export-icons <dir>`: the app icon and every tray state, for packaging and docs.
pub fn export(dir: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    for size in [16, 22, 24, 32, 48, 64, 128, 256, 512] {
        // App menu icon: the wide receiver, centred in a square.
        write_png(&dir.join(format!("hollymic-{size}.png")), Shape::Wide, size, size, &app_icon_state())?;
    }
    write_png(&dir.join("receiver-wide.png"), Shape::Wide, 256, 160, &app_icon_state())?;
    // Each tray state; flashing ones get an -on and an -off frame (docs GIF).
    let f = false;
    let states = [
        ("tray-no-receiver", IconState { leds: [Led::Off, Led::Off], faded: true }),
        ("tray-no-mics", IconState { leds: [Led::Blink, Led::Blink], faded: f }),
        ("tray-nc-on", IconState { leds: [Led::Green, Led::Blink], faded: f }),
        ("tray-nc-on-both", IconState { leds: [Led::Green, Led::Green], faded: f }),
        ("tray-nc-off", IconState { leds: [Led::Blue, Led::Blink], faded: f }),
        ("tray-nc-off-both", IconState { leds: [Led::Blue, Led::Blue], faded: f }),
    ];
    for (name, st) in states {
        if st.blinks() {
            write_png(&dir.join(format!("{name}-on.png")), Shape::Tray, 64, 64, &st.frame(true, true))?;
            write_png(&dir.join(format!("{name}-off.png")), Shape::Tray, 64, 64, &st.frame(true, false))?;
        } else {
            write_png(&dir.join(format!("{name}.png")), Shape::Tray, 64, 64, &st)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leds_follow_mics_and_noise() {
        let mut s = DeviceState { connection: crate::state::Connection::Connected, ..Default::default() };
        s.mics[0].online = true;
        s.noise = Some(NoiseLevel::Low);
        assert_eq!(IconState::from_state(&s).leds, [Led::Green, Led::Blink]);
        assert_eq!(IconState::from_state(&s).frame(true, true).leds, [Led::Green, Led::Blue]);
        assert_eq!(IconState::from_state(&s).frame(true, false).leds, [Led::Green, Led::Off]);
        assert_eq!(IconState::from_state(&s).frame(false, true).leds, [Led::Green, Led::Off]);
        s.noise = Some(NoiseLevel::Off);
        s.mics[1].online = true;
        assert_eq!(IconState::from_state(&s).leds, [Led::Blue, Led::Blue]);
        s.connection = crate::state::Connection::Searching;
        let st = IconState::from_state(&s);
        assert!(st.faded);
        assert_eq!(st.leds, [Led::Off, Led::Off]);
    }

    #[test]
    fn renders_every_size() {
        for size in [16, 22, 64] {
            let px = render_rgba(size, &app_icon_state());
            assert_eq!(px.len(), (size * size * 4) as usize);
            assert!(px.chunks(4).any(|p| p[3] == 255), "some opaque pixels");
            assert_eq!(px[3], 0, "transparent corner");
        }
    }
}
