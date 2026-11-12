# AGENTS.md

HollyMic: a Rust tray app for the Hollyland **Lark M2** wireless mic (Linux first, macOS later). Read [README.md](README.md) for what it does, then [docs/WIP.md](docs/WIP.md) for where it stands. Owner: RJ (GitHub `rj-xy`).

## Layout

| Path | What |
|---|---|
| `src/protocol.rs` | frames, commands, heartbeat parsing, firmware gates. Pure, unit-tested |
| `src/usb.rs` | `nusb` transport: claims interface 0 only |
| `src/device.rs` | worker thread: connect, start-up reads, 3 s heartbeat, requests |
| `src/sim.rs` | simulated receiver (`--demo`, tests, screenshots) |
| `src/state.rs` | shared state + change listeners |
| `src/ui.rs` | egui window and settings page |
| `src/tray_linux.rs`, `src/tray_macos.rs` | tray icons |
| `src/icon.rs` | the drawn dongle icon |
| `src/firmware.rs` | Hollyland cloud firmware check (notify only) |
| `src/app.rs`, `src/ipc.rs`, `src/settings.rs`, `src/autostart.rs`, `src/meter.rs` | wiring, single instance/CLI, settings, login item, level meter |
| `install.sh`, `scripts/` | installer, packaging, screenshots |
| `docs/` | protocol, APK notes, design, WIP, releasing |

## Commands

```bash
cargo build && cargo test
cargo clippy --all-targets            # keep it at zero warnings
cargo check --target aarch64-apple-darwin   # macOS must keep compiling
cargo fmt                             # rustfmt.toml: 120 columns
./target/debug/hollymic --demo        # simulated receiver; temp settings, never the real ones
./target/debug/hollymic status        # real receiver, read-only
scripts/screenshots.sh                # regenerate docs/images (Xvfb)
scripts/package.sh                    # dist/ tarball
```

## Rules

- **Lark M2 only** (USB `3547:0007/0008`, `4417:0020/0021`). Don't add the M2S, A2, M3 or speakers.
- **Never send firmware-update commands** (`0x08`, `0x09`, `0x0B`, `0x0D`, `0x0E`, report `0x55`). `protocol::encode` panics on them; keep it that way. The app only tells the user an update exists.
- `0x19` toggles NC: send it only when the state is known, never retry it.
- **Real receiver:** RJ's mic is in daily use (dictation, an all-day recorder). Read-only commands are fine. Ask before changing his settings; set them back if a test changes them. Only one program can hold interface 0: stop HollyMic or the web UI bridge before `hollymic status` works directly.
- Use `--demo` for UI work; it never touches real settings, socket or autostart.
- **Heavy jobs** on RJ's laptop: cap them (`systemd-run --user --scope -p MemoryMax=6G …`, `-j 4`), one at a time, in the foreground. `scripts/package.sh` already does.
- **Decompiled HollyAudio** stays out of the repo (it's Hollyland's code). Notes go in `docs/APK-NOTES.md`.
- egui's bundled fonts lack most symbols (●, ⚙, emoji): draw icons with the painter.
- Docs: plain headings, dated facts (`2026-11-13`), update `docs/WIP.md` when status changes.

## Git

- Branch `main`. No remote yet; the GitHub repo comes later (`rj-xy`, never the work account).
- One logical change per commit; follow-ups as `fixup!` commits, autosquashed before pushing. RJ merges with rebase.
