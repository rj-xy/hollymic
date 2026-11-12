# HollyAudio APK notes

How HollyAudio (the Hollyland phone app for the Lark M2) was looked at, and what HollyMic took from it.

## Getting and decompiling it (2026-11-13)

- HollyAudio **3.1.0** for Android, package `com.hollyland.larkc1`, version code `3101020`, 184 MB, from APKPure: `https://d.apkpure.com/b/APK/com.hollyland.larkc1?version=latest`. Hollyland publishes no APK of its own (Google Play only).
- **Is it the official build?** Hollyland publishes no APK; this copy came from APKPure, so its signature was checked (2026-11-13). It carries only an APK Signature Scheme v2 block, signed by a Google Play App Signing certificate: `CN=Android, OU=Android, O=Google Inc.`, valid from 2022-09-23 (the year HollyAudio launched), SHA-256 `D8:35:6A:F2:76:E8:BE:15:02:A7:86:5D:F5:2A:0F:46:5F:BE:AB:9F:AB:32:12:06:E0:45:7F:3A:98:BE:57:D6`. That's what Play-distributed apps are signed with, so APKPure passed on the Play build unmodified (a repacked APK couldn't carry that signature). File SHA-256 `ead3a1474bdf852b2f1715872c31b8332e3b86c46360620fc57f905274b517e2`. A copy straight from Play (with a Google account and a tool such as apkeep) would remove the last doubt.
- [REA](https://rea.tools/) ([morluto/rea](https://github.com/morluto/rea), suggested by RJ 2026-11-13) is an MCP toolkit that drives JADX, Ghidra and friends for coding agents. Not used: for an APK it runs the same JADX that was run by hand here.
- Decompiled with jadx 1.5.6 (Homebrew), capped so it can't take the laptop's memory:

```bash
JAVA_OPTS='-Xmx4g' systemd-run --user --scope -p MemoryMax=6G jadx -j 2 --no-res -d jadx-out HollyAudio_3.1.0.apk
```

- 22,099 classes, 449 jadx errors (normal for Kotlin coroutines), 432 MB of sources. Kept outside the repo (`/tmp/hollyaudio-apk/`); don't commit it, it's Hollyland's code.

## Where things are

Paths under `jadx-out/sources/com/hollyland/`:

| What | Where |
|---|---|
| Lark M2 screen (6301) | `larkc1/HLDeviceActivity.java` |
| M2 command classes | `usbmic/protocol/Pro_*.java` |
| Product IDs and names | `application/common/protocol/Product.java` (`PRODUCT_6301_MOBILE = 7`, `PRODUCT_6301_CAMERA = 8`, EAGLES 32/33) |
| Firmware package codes | `application/httpcommon/entities/HlDevice.getDeviceCode` |
| Cloud API | `application/httpcommon/http/HttpService.java`, `HttpRepository.java`, `HttpConst.java` |
| Upgrade check flow | `application/httpcommon/update/HUpdater.java` |

## Lark M2 screen features

`HLDeviceActivity` view IDs: `battery1/2`, `mic1/2_battery`, `noise_level_switch`, `low_level`, `strong_level`, `voice_view_group` (volume 1–6), `voice_lock`, `voice_outgoing_switch` (phone speaker), `uv_progress` (level meter), `device_upgrade`. `voice_mode` and `shutdown_time` are shown only for PIDs 1029/1030 (M2S), so HollyMic leaves them out.

- The NC switch shows only when the receiver firmware is newer than `SPEC_VERSION = "2.0.0.31"`; otherwise only the level buttons.
- The level meter (`uv_progress`) is fed by the phone's own audio capture (`AudioRecordUtils`), not by the receiver. HollyMic does the same with PipeWire.
- Volume lock shows only for the new camera receiver (`isNew6301Camera`: PID 8/33 and firmware newer than 2.0.0.31).

## Firmware update check

What HollyAudio does (and HollyMic copies, minus the download and flash):

1. Base URL `https://device-api.hollyland.com` (overseas build; China uses `device-api.szmoma-tech.com`). `HttpConst.OVERSEAS_BASE_URL`.
2. `GET /api/v1/sign/getSignKey` → `{"code":200,"data":"hollyland-sign-key","success":true}` (2026-11-13).
3. `POST /api/v1/upgrade/checkBatchUpgrade`, headers `APP-PKG-CODE: com.hollyland.larkc1` and `APP-VERSION-VALUE: 3101020`, body:

   ```json
   {"sign":"<md5>","timestamp":1794535765769,"body":[{"packageCode":"6301RX","versionValue":1000},{"packageCode":"6301TX","versionValue":1000}]}
   ```

   `sign = lowercase hex md5(key + timestamp + json(body) + key)` (`HttpRepository.makeSignStr`). The app also sends `deviceSn` per unit (`UpgradeSnResolver`); HollyMic leaves it out. `versionValue` is always `1000` (`HttpRepository.createRequest`).
4. `data` has one entry per package: `null` = nothing offered, otherwise `{packageCode, versionValue, upgradeType, advised, packageDescription(En), …}`.
5. For offered packages, `POST /api/v1/upgrade/getBatchDownloadUrl` (same signing) gives `versionName`, `fileUrl`, `fileMd5`, notes. `HLDeviceActivity.checkDeviceUpgradeStatus` compares `versionName` (lowercased, `v` removed) with each unit's version and offers the update when the cloud one is newer.

Package codes (`HlDevice.getDeviceCode`): `6301RX`, `6301TX`; EAGLESHERO units `6301RX_RU`, `6301TX_RU`.

**Results on 2026-11-13** ([scripts/probe-firmware-api.sh](../scripts/probe-firmware-api.sh), run by RJ):

| Package | Answer |
|---|---|
| `6301RX`, `6301TX`, `6301RX_RU` (Lark M2) | `null`: nothing offered. Matches the app's "Latest RX Version: None" on RJ's phone (2026-10-09) |
| `6302RX` / `6302TX` (M2S) | `V2.0.0.17` / `V2.1.0.17`, "1. Fixed known bugs", 732,672 / 731,648 bytes |
| `6118RX` / `6118TX` (A2) | `V3.0.2.048` (`HLD_A6118_S3.0.2.048_RX`/`_TX`) |
| `6118CAMRX` | `null` |
| wrong signature | `{"code":5000000,"msg":"验签异常","success":false}` ("signature verification error") |

So the request format and signing are right, and the server checks the signature. Each offer already carries `versionName`, `packageDescription(En)`, `packageSize`, `upgradeType` and `advised` (with `fileUrl: null`); HollyMic reads the version from there and only calls `getBatchDownloadUrl` if it's missing.

## Not taken

- Firmware download and flashing (`0x08`, `0x09`/`0x0D`, report `0x55` packets, `0x0E` acks, `0x0B` result): the two apps disagree on the ack rules, and a bad flash can brick a mic.
- Accounts, analytics (Sensors Data, Firebase), presets and the music/voice features: they belong to other product families (6201, 6303).
