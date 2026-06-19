# CorsaConnect

Turn an Android phone into a gyroscopic steering wheel **and** a live dashboard
for [BeamNG.drive](https://www.beamng.com/). The phone tilts to steer and shows
speed / RPM / gear streamed from the game.

```
┌─────────────── ANDROID (Kotlin) ───────────────┐
│  Tilt phone -> steering    Dash: speed/RPM/gear │
│  Touch buttons: gas / brake / shift / handbrake │
└──────┬───────────────────────────────▲──────────┘
       │ input UDP :5000                │ telemetry UDP :5001
       ▼                                │
┌──────────────── PC SERVER (Rust) ─────┴──────────┐
│  • ViGEmBus: virtual Xbox 360 pad  -> BeamNG     │
│  • OutGauge UDP :4444  -> parsed -> phone dash    │
└──────────────────────────────────────────────────┘
```

## Requirements

- **ViGEmBus driver** on the PC: https://github.com/nefarius/ViGEmBus/releases
- Phone and PC on the **same Wi-Fi network**.
- Rust toolchain (server) and Android Studio (app).

## PC launcher

Run `dist/CorsaConnect.exe` (or `cd server && cargo run --release`). A small
window shows your PC's LAN IP and a **Launch** button. Launch plugs in a virtual
Xbox 360 controller, listens for phone input on UDP 5000, and relays BeamNG
telemetry to the phone. The status dots show ViGEmBus / phone / BeamNG, and the
log panel shows what's happening. Type the IP shown into the phone app.

The single-file `.exe` starts with no console window. Rebuild the icon from
`CorsaConnectLOGO.png` with `python tools/make_icons.py` (regenerates the
launcher icon and the Android mipmaps).

**Test it without the phone** (sweeps the wheel so you can see the controller
move in BeamNG or Windows' "Set up USB game controllers"):

```sh
cargo run --example test_client
```

## BeamNG setup

Options -> Other -> Protocols:
- Enable **OutGauge**, set IP `127.0.0.1`, port `4444`.

Then bind the virtual controller in Options -> Controls (it appears as an Xbox
360 pad): left stick X = steering, right trigger = throttle, left trigger =
brake, LB/RB = shift down/up, A = handbrake.

## Android app

Open `android/` in Android Studio, build and run on a real device (the emulator
has no usable gravity sensor). Enter the PC's LAN IP, tap **Connect**, hold the
phone like a wheel and tap **Center wheel** to calibrate.

## Wire protocol

See `server/src/protocol.rs` and `android/.../Protocol.kt` — keep them in sync.
Little-endian throughout.

- Input (phone -> server, 9 bytes, v2): `"CC"` + version + u16 buttons (raw XInput mask) + i16 steer + u8 throttle + u8 brake
- Telemetry (server -> phone, 32 bytes): `"CT"` + version + i8 gear + 7×f32

## Custom HUD

Tap **✎ Edit** on the phone to enter layout mode: drag any control to move it,
drag the blue corner handle to resize, tap a button to rebind it to any XInput
button (A/B/X/Y/LB/RB/Start/…) or rename it, and use **+ Add** to drop in new
buttons or widgets (analog speedometer / tachometer, gear, speed). **⚙ Settings**
tunes steering sensitivity, dead zone, max angle, and gauge ranges. The layout is
saved per device and restored on launch; **Reset** restores the stock layout.
