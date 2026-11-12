# Lark M2 USB control protocol

What HollyMic sends to the receiver and why. The full multi-product spec (A2, M2, M2S, with Android and iOS citations) is [khromov/hollyland-web-ui `docs/PROTOCOL.md`](https://github.com/khromov/hollyland-web-ui/blob/main/docs/PROTOCOL.md); this page is the Lark M2 subset, re-checked against HollyAudio 3.1.0 for Android ([APK-NOTES.md](APK-NOTES.md)) and a real Lark M2 on Linux (2026-11-13). Code: [src/protocol.rs](../src/protocol.rs), [src/device.rs](../src/device.rs).

## Devices

| USB ID | Receiver |
|---|---|
| `3547:0007` | Lark M2, USB-C / Lightning (mobile) receiver. The one tested |
| `3547:0008` | Lark M2 camera receiver (3.5 mm out) |
| `4417:0020`, `4417:0021` | EAGLESHERO-branded Lark M2 (mobile, camera) |

HollyAudio calls this product family **6301**. Everything else (M2S 6302, A2 6118, M3 6303, speakers) is out of scope.

## Transport

- USB interface 0 is HID, with interrupt endpoints **OUT 0x03** and **IN 0x82**, 64 bytes each. Interfaces 1 and 3 are the USB audio; interface 7 is Apple's iAP2.
- Every request and reply is exactly **64 bytes**, zero-padded.
- Request: `AA DD <cmd> <dst> <lenHi> <lenLo> <payload…>`, dst `0x40` receiver, `0x41` mic 1, `0x42` mic 2.
- Reply: `BB DD <cmd> <src> <lenHi> <lenLo> <payload…>`, src `0x80` receiver, `0x81` mic 1, `0x82` mic 2.
- No checksum, no sequence numbers, no handshake. One request at a time; match the reply by command byte. Each request gets **two** IN reports: drain leftovers before the next request.
- Nothing is pushed: battery, connections and button presses only show up in the next heartbeat.

**Why libusb-style access, not hidraw:** the replies use report ID `0xBB`, which the HID descriptor doesn't declare. Linux's HID core (`__hid_input_report()`) and macOS's IOHIDFamily both drop undeclared reports, so hidraw and WebHID can write but never read. HollyMic detaches `usbhid` from interface 0 only (via usbfs, `nusb`), so the audio interfaces stay with `snd-usb-audio` and the mic keeps working. Dropping the interface re-attaches `usbhid`.

## Commands HollyMic uses

All to the receiver unless noted. R = read, W = write.

| Cmd | Name | Payload | Reply | Notes |
|---|---|---|---|---|
| `0x10` | heartbeat | – | see below | polled every 3 s, like HollyAudio's Lark M2 screen |
| `0x03` | version | – | bytes → `2.0.0.36` | to `0x40`, `0x41`, `0x42`. All zeros = not ready; ask again (4 tries per unit) |
| `0x15` | serial | – | ASCII, NULs removed | per unit |
| `0x11` | NC get | – | `0` off, `1` Weak (Low), `2` Strong | |
| `0x06` | NC level set | `1` or `2` | `[status, level]`; a 1-byte reply is a failure | only while NC is on and a mic is online |
| `0x19` | NC on/off **toggle** | `02` | status (unreliable) | always flips; HollyMic reads the heartbeat right after to see the result. Never sent twice or blind. Needs RX firmware newer than 2.0.0.31 and last part ≥ 32 |
| `0x04` | volume get | – | `0..5` | shown as 1–6. The mobile receiver (PID 7) reports volume here only, not in the heartbeat |
| `0x05` | volume set | `0..5` | `1` = ok | needs a mic online and the volume unlocked |
| `0x13` | phone speaker get | – | `0` = on | |
| `0x12` | phone speaker set | `0` on, `1` off | `1` = ok | applies after a receiver restart; HollyAudio waits 3 s, then sends `0x0C` |
| `0x35` | volume lock get | – | `1` = locked | camera receivers, firmware newer than 2.0.0.31 |
| `0x34` | volume lock set | `1` lock, `0` unlock | `1`, or the new state | |
| `0x0C` | restart receiver | – | none | the receiver drops off USB and comes back |

**Never sent:** the firmware-update commands `0x08`, `0x09`, `0x0B`, `0x0D`, `0x0E` and output report `0x55`. `protocol::encode` panics on them.

Not on the Lark M2 (HollyAudio hides them for PIDs 7 and 8): mono/stereo `0x18` and mic auto-off `0x36` (M2S only).

### Heartbeat body (`0x10`)

| Byte | Field |
|---|---|
| 0, 1 | mic 1 / mic 2 online (`1`). A mic in the case reads `0` |
| 2, 3 | mic 1 / mic 2 battery % |
| 4, 5 | signed "uv" value, unused by the app |
| 6 | NC: `0` off, `1` Weak, `2` Strong |
| 7 | volume `0..5`; HollyAudio applies it only for PID ≥ 8 |
| 8 | volume lock (`1`), camera receivers only |

The Lark M2 sends 9 bytes. There is no mute or charging state.

## Start-up sequence

HollyAudio (Android, `HLDeviceActivity`): `0x03`, `0x11`, `0x04`, `0x13`, `0x15`, `0x35` back to back, then `0x10` every 3 s; version and serial of a mic when it comes online. HollyMic does the same, but sends `0x35` only to receivers that have a volume lock.

## Live observations (RJ's Lark M2, Linux, 2026-11-13)

- Receiver firmware 2.0.0.36, so the NC switch is available; mics 2.1.0.36 (seen earlier through the web UI).
- With both mics in the case, `0x11` and the heartbeat read NC **Strong**, and still Strong with Mic 2 on (12:50), although Weak had been set through the web UI that morning while Mic 1 was on. Likely the level is stored per mic. Open: wear Mic 1 alone to confirm.
- Mic 2: serial read over `0x15` to `0x42`, firmware 2.1.0.36, battery 100 %.
- Phone speaker **on**: the receiver presents a microphone only: USB audio interface 3 has one IN endpoint (`0x81`, 2 ch), no playback, and PipeWire lists no Hollyland output. **Off** is untested; the restart it needs suggests the receiver then re-enumerates with a playback path too, so a phone (or a computer) would send its sound to the receiver. The window's tooltip warns about that.
- Receiver LEDs (RJ): one per slot; green = NC on, blue = NC off, flashing blue = no mic connected on that slot. The tray icon copies this.
- Volume read `05` (6/6); phone speaker on.

## Firmware check (cloud)

See [APK-NOTES.md](APK-NOTES.md#firmware-update-check).
