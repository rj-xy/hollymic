# Work in progress

Status as of 2026-11-13 (first version, 0.1.0, not released). Update this file as things change: date each entry.

## Status

| Area | State | Notes |
|---|---|---|
| Protocol, device worker | 🟢 tested on a real Lark M2 (Linux) | read-only so far: start-up reads, heartbeat, version, serial, idempotent `nc strong` / `volume 6`; with Mic 2 on (2026-11-13 12:50): online, 100 %, firmware 2.1.0.36, serial read |
| Writes that change something (NC toggle, level, volume, phone speaker, restart) | 🟠 tested on the simulator only | both mics were in the case during the first session; see "Next tests" |
| KDE tray (ksni) | 🟢 registers with Plasma 6 (Wayland), title, tooltip, category | menu clicks not tried by a person yet. LED meaning per RJ (2026-11-13): green NC on, blue NC off, flashing blue = no mic on that slot; flashing not yet seen in the real panel |
| Window (egui) | 🟢 renders under Xvfb in both themes | not yet seen on the real desktop: the screen was locked |
| Single instance, CLI over IPC | 🟢 | `status`, `nc`, `volume`, `show`, `quit`, second launch opens the window |
| Firmware check | 🟢 live call works, Lark M2 gets `[null, null]` (nothing offered) | request format and signing confirmed by RJ's probe (other products get offers, a wrong signature is refused); the "update available" path is unit-tested with that live reply |
| Level meter | 🟠 PipeWire source found, stream format checked | no speech seen yet (mics in the case) |
| Notifications | 🟠 logic unit-tested | not seen on screen |
| Start on login | 🟠 writes `~/.config/autostart/hollymic.desktop` | not tried through a real login |
| Install, update, uninstall | 🟢 in a throwaway `$HOME`, without the udev step | see "Next tests" |
| macOS | 🔴 type-checks only (`cargo check --target aarch64-apple-darwin`) | reading the receiver needs root and stops its audio; waiting for RJ's decision, see [macOS](#macos) |
| GitHub repo, releases, one-liner download | ⏸ not started, on purpose | `install.sh` has a `download_latest` stub |

## Fixed

- **2026-11-13, stuck old copy after an update:** `hollymic quit` left the old copy running with its USB thread spinning at 98 % CPU, and the installer then started a second copy. Cause: a USB transfer that failed was resubmitted in a loop (`usb.rs` `drain`/`read_reply`), so the worker never saw the stop request, and the main thread waited on it forever after removing the socket. Now a failed transfer ends the session (the worker reconnects), `drain` is bounded, stopping waits at most 2 s, the socket goes last, and `install.sh` waits for every copy to exit (and stops any that hang). Checked: three quit/restart cycles, each quit in ~90 ms, interface 0 back on `usbhid`, one copy running.

## Next tests (need a person and the mics)

1. Take a mic out of the case, open the window: battery, NC, level meter move.
2. Flip NC on/off from the window and from the tray menu; watch the mic LED go green/blue and the tray LED follow.
3. Weak/Strong and volume 1–6: check the mic sounds different and the receiver keeps the volume after a re-plug.
4. Phone speaker toggle (restarts the receiver; HollyMic should reconnect by itself).
5. Unplug and replug the receiver while HollyMic runs.
6. Unlock the screen, open the window from the tray, close it, open it again (the Wayland window lifecycle).
7. Install with the udev step on a machine without RJ's hand-made `70-hollyland.rules`.

## Open questions

- **NC level per mic?** On 2026-11-13 the receiver read NC **Strong** with both mics in the case, and still Strong with only Mic 2 on (12:50), although RJ had set Weak in the web UI at 10:16 while Mic 1 was the one on. Likely the level is kept in each mic (on/off already is), and the receiver reports the connected mic's. Check: wear Mic 1 alone and run `hollymic status --short` (expect weak).
- ~~**Does Hollyland offer Lark M2 firmware through this API?**~~ Answered 2026-11-13 by [scripts/probe-firmware-api.sh](../scripts/probe-firmware-api.sh): the format is right (M2S `6302RX` V2.0.0.17, `6302TX` V2.1.0.17, A2 `6118RX`/`6118TX` V3.0.2.048 are offered), the server checks the signature (wrong one: `{"code":5000000,"msg":"验签异常"}`, "signature verification error"), and the Lark M2 (`6301RX`, `6301TX`, `6301RX_RU`) gets `null`: Hollyland offers no M2 firmware through the app. The reply carries `versionName` itself, so `getBatchDownloadUrl` is only a fallback now.
- **Icon look**: redrawn from RJ's photos on 2026-11-13 (wide receiver in the window, square body without the plug in the tray); RJ to confirm (`docs/images/tray-states.gif`, `assets/icons/receiver-wide.png`).

## macOS

RJ (2026-11-13): run as root if there's no way around it; if that's too hard, say so and he decides. Findings:

- **No way around root for reading.** The receiver answers with report ID `0xBB`, which its HID descriptor doesn't declare; IOHIDFamily drops those even with the device seized (hollyland-web-ui PROTOCOL §11.2). WebUSB and the iAP2 interface are held by macOS (`accessoryd`). `nusb` can't detach drivers on macOS; libusb can, only as root, by asking macOS to re-enumerate and *capture* the device.
- **Root costs the audio.** A capture takes the whole receiver away from macOS, including its USB audio, until it's released (hollyland-web-ui BRIDGE.md, "USB audio pauses while it runs"; seen with their bridge on a Mac on 2026-11-05/06). HollyMic polls every 3 s, so the mic would be unusable while HollyMic runs. That rules out the Linux design on a Mac.
- **What could work, best first:**
  1. **Settings only, no root:** send writes through IOKit HID (`IOHIDDeviceSetReport`, report `0xAA`). Verified on a Mac by the web UI: settings take effect, nothing can be read. The tray can't show battery or NC state, only what you last set. About a day's work plus a Mac to test.
  2. **Settings plus "read now" as root:** (1), plus a privileged helper that captures the receiver for ~2 s on demand to read battery, NC and versions; audio drops for those seconds. Needs a launchd daemon, a signed helper or a sudo prompt per read. Several days, fiddly.
  3. **Leave macOS out** and say so in the README.
- Decision pending with RJ.

## Ideas, not started

- Global shortcuts for NC on/off (KDE: run `hollymic nc on|off` from a custom shortcut today).
- Show which app is recording from the mic (PipeWire), since the meter already reads it.
- A `.deb`/AUR/Homebrew package once releases exist.
- Wake the tray faster on plug-in with hotplug events (`nusb::watch_devices`) instead of polling every 2 s.
