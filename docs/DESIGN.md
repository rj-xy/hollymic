# Design

## Threads

| Thread | Does | File |
|---|---|---|
| main | Linux: waits for "show", runs the window until it closes, repeats. macOS: runs the window's event loop for the app's lifetime | [app.rs](../src/app.rs), [ui.rs](../src/ui.rs) |
| device | opens the receiver, start-up reads, heartbeat every 3 s, runs requests one at a time, reconnects | [device.rs](../src/device.rs), [usb.rs](../src/usb.rs) |
| tray (Linux) | ksni StatusNotifierItem; a helper thread pushes state changes to it | [tray_linux.rs](../src/tray_linux.rs) |
| firmware | checks Hollyland's server daily and when versions change | [firmware.rs](../src/firmware.rs) |
| notifier | low battery, plug in/out, firmware update | [app.rs](../src/app.rs) |
| ipc | Unix socket: single instance and the CLI | [ipc.rs](../src/ipc.rs) |
| meter | `pw-record` reader while the window is open | [meter.rs](../src/meter.rs) |

State lives in `state::Shared` (an `RwLock<DeviceState>` plus change listeners). The UI, tray and CLI only send `device::Request`s; only the device thread talks USB. The receiver handles one request at a time, so that's the natural shape.

## Decisions

- **Rust + egui/eframe (glow)**: one small binary, no web runtime, dark and light themes built in. `system_fonts` is off, so only glyphs in egui's bundled fonts work: icons such as the gear and status dots are drawn, not typed.
- **nusb, not libusb**: pure Rust, so no C toolchain; it detaches and re-attaches the kernel driver per interface on Linux.
- **ksni on Linux, tray-icon on macOS**: ksni speaks StatusNotifierItem over D-Bus directly (native on KDE, no GTK). tray-icon needs the main thread's event loop, which on macOS is the window's.
- **Window lifecycle**: Wayland can't hide a window, so on Linux closing the window really closes it and eframe's `run_and_return` lets the main thread open a new one later. macOS hides the window instead.
- **System `curl` for HTTPS**: avoids bundling a TLS stack (ring/rustls need a C compiler for macOS cross-checks and add ~1 MB).
- **Level meter through `pw-record`**: PipeWire already holds the ALSA device all day (dictation, mic recorder); opening it with ALSA directly would fail with "busy". `pw-record` writes an AU stream ("dns." header = little-endian).
- **Icons drawn in code** ([icon.rs](../src/icon.rs)): signed-distance shapes, 4×4 supersampled, so they're crisp at 16 to 512 px and need no artwork. Two drawings from RJ's photos of the receiver (2026-11-13): **wide** (USB-C plug from the top: a plain metal shell with square corners, yellow collar; round badge, TX1/TX2 LEDs, yellow button) for the window header and app menu, and **square without the plug** for the tray, which can't be wider than a standard icon. The badge is a plain broken ring, not Hollyland's logo. Each LED mirrors one slot.
- **Help popups** (the ⓘ next to Phone speaker): open at once on hover, and a click pins them until the next click anywhere; placed above the icon so they stay inside the window.
- **Toggle safety**: `0x19` flips NC; it's only sent when the current state is known and is never retried. Same-value requests are no-ops.
- **Demo mode** (`--demo`): a simulated receiver ([sim.rs](../src/sim.rs)) with fake serials; it uses a temp settings dir, socket and autostart dir, so it never touches the real ones.
- **Screenshots**: `--screenshot <file>` renders the window and saves it with egui's screenshot command. KDE stops painting windows while the screen is locked, so [scripts/screenshots.sh](../scripts/screenshots.sh) runs it under Xvfb.

## Settings

TOML, kebab-case keys, every key optional (missing keys keep their defaults). Written atomically (temp file + rename). Start on login writes `~/.config/autostart/hollymic.desktop` (Linux) or `~/Library/LaunchAgents/io.github.rj_xy.hollymic.plist` (macOS), pointing at the current binary, and refreshes it at every start in case the binary moved.
