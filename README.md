# CorsaConnect

Turn an Android phone into a gyroscopic steering wheel **and** a live dashboard
for your racing games, and your webcam into a TrackIR-style head tracker. The
phone tilts to steer and works as a controller in any game; for games that
support telemetry it also shows live speed / RPM / gear and gives the wheel
force feedback. Telemetry is wired for
[BeamNG.drive](https://www.beamng.com/) and Euro/American Truck Simulator today,
with more games on the way.

```
┌─────────────── ANDROID (Kotlin) ───────────────┐
│  Tilt phone -> steering    Dash: speed/RPM/gear │
│  Touch buttons: gas / brake / shift / handbrake │
└──────┬───────────────────────────────▲──────────┘
       │ input UDP :5000                │ telemetry UDP :5001
       ▼                                │
┌──────────────── PC SERVER (Rust) ─────┴──────────┐
│  • vJoy: virtual racing wheel      -> any game   │
│    (or ViGEmBus: virtual Xbox 360 pad)           │
│  • Webcam head tracking -> TrackIR -> any game    │
│  • OutGauge  UDP :4444 -> parsed -> phone dash    │
│  • MotionSim UDP :4445 -> slide + crash -> wheel  │
│  • same telemetry -> PicoPanel  UDP :5051        │
└──────────────────────────────────────────────────┘
```

## Requirements

- A driver for the virtual controller, depending on the mode you pick in the
  launcher (see **Controller modes**):
  - **vJoy** (wheel mode, default): https://github.com/njz3/vJoy/releases
  - **ViGEmBus** (Xbox pad mode): https://github.com/nefarius/ViGEmBus/releases
- Phone and PC on the **same Wi-Fi network**.
- Rust toolchain (server) and Android Studio (app).

## Controller modes

The **CONTROLLER** card in the launcher picks what the phone shows up as:

- **Racing wheel (vJoy)** — a plain DirectInput joystick with steering on its
  own 15-bit axis and one axis per pedal. It isn't an XInput device, so a real
  Xbox pad plugged in at the same time doesn't collide with it: both stay bound
  in the game and you can switch between them without redoing anything.
  Install vJoy, then in **vJoyConf** give device 1 the `X`, `Y`, `RZ` and
  `Slider` axes and **14 buttons** — plus `RX` and `RY` if you want the HUD's
  joystick widget. The launcher checks that for you and says exactly what's
  missing; after Launch the log prints the resulting mapping:

  ```
  vJoy device 1: steering = X axis (32768 steps), throttle = Y, brake = RZ,
                 clutch = Slider, stick = RX/RY, 14 buttons.
  Buttons: 1=A 2=B 3=X 4=Y 5=LB 6=RB 7=Back 8=Start 9=L-stick 10=R-stick
           11=D-Up 12=D-Down 13=D-Left 14=D-Right
  ```

  The phone's HUD keeps its Xbox-style labels; the numbers above are what to
  bind in-game.

- **Xbox 360 pad (ViGEmBus)** — the original mode, for games that only read
  XInput. Works everywhere, but XInput games index pads by slot, so a real Xbox
  controller plugged in alongside it fights over which one the game listens to.

## PC launcher

Run `dist/CorsaConnect.exe` (or `cd server && cargo run --release`). A small
window shows your PC's LAN IP and a **Launch** button. Launch plugs in the
virtual controller, listens for phone input on UDP 5000, and relays BeamNG
telemetry to the phone. The status dots show the controller / phone / BeamNG,
and the log panel shows what's happening. Type the IP shown into the phone app.

The single-file `.exe` starts with no console window. Rebuild the icon from
`CorsaConnectLOGO.png` with `python tools/make_icons.py` (regenerates the
launcher icon and the Android mipmaps).

**Test it without the phone** (sweeps the wheel so you can see the controller
move in BeamNG or Windows' "Set up USB game controllers"):

```sh
cargo run --example test_client
```

## PicoPanel (sharing the telemetry)

OutGauge has exactly one listener: the game sends to one address:port, and
whoever binds 4444 first gets it. So [PicoPanel](https://github.com/Tudor077/PicoPanel) — the RP2040
dashboard panel's PC app — and CorsaConnect used to lock each other out,
whichever you started second sat silent.

The **PICOPANEL** card in the launcher settles it. CorsaConnect keeps 4444 and
sends PicoPanel a copy of every telemetry packet on UDP 5051, the same one the
phone gets — which is better than the raw OutGauge it was parsing, because the
learned redline, the slide and the crash impact are already folded in.

- **Send it a copy** turns the mirror on (and remembers it for next time). It
  takes effect immediately — no restart, no re-binding.
- **Start PicoPanel** first sets `yield_outgauge` in PicoPanel's settings, so it
  leaves 4444 alone, then starts it. That order matters: PicoPanel reads the
  flag once, at startup.
- **Make it yield 4444** writes just that flag, for a PicoPanel you start
  yourself. Restart it to pick the change up.

The dots show whether PicoPanel is running and whether it's set to yield, and
the counter under them shows packets actually copied — if it moves, the link is
live. `CORSACONNECT_MIRROR=127.0.0.1:5051` still works as an environment
variable; the card just makes it a checkbox.

## Head tracking (webcam → TrackIR)

The **HEAD TRACKING** card in the launcher turns any webcam into a head
tracker for games that support TrackIR (BeamNG, ETS2/ATS, and hundreds more) —
no opentrack or extra installs. Everything ships inside the exe: face detection
and 66-point landmarks run on ONNX Runtime, a PnP solver turns them into a 6DoF
pose, and the launcher installs its own `NPClient64.dll` + registry key that
TrackIR games discover automatically.

Start tracking **before** launching the game, switch to the interior camera,
and press **Center** with your head in its neutral position. Sliders tune
smoothing (a critically damped spring — natural easing) and rotation/position
gain; the **Axes** menu can disable or mirror any of the six axes. **Low light
boost** forces a short camera exposure so tracking stays at full frame rate in
a dark room, backing off automatically if it gets too dark to see you. The
camera preview shows exactly what the tracker sees, landmarks included.

## BeamNG setup

Options -> Other -> Protocols:
- Enable **OutGauge**, set IP `127.0.0.1`, port `4444` (dash: speed, rpm, gear).
- Enable **MotionSim / OutSim**, set IP `127.0.0.1`, port `4445` (optional, for
  the wheel to buzz on real slides and crashes). Without it, drift/collision
  feedback simply stays silent.

Then bind the virtual controller in Options -> Controls. In wheel mode it shows
up as a vJoy joystick: X axis = steering, Y = throttle, RZ = brake, Slider =
clutch, RX/RY = the joystick widget (bind it to the look/camera axes if you add
one), and the buttons in the order the launcher logs. In Xbox pad mode it's a
360 pad: left stick X = steering, right trigger = throttle, left trigger =
brake, LB/RB = shift down/up, A = handbrake.

## Android app

Open `android/` in Android Studio, build and run on a real device (the emulator
has no usable gravity sensor). Enter the PC's LAN IP, tap **Connect**, hold the
phone like a wheel and tap **Center wheel** to calibrate.

### How the steering is measured

The wheel angle is the **gravity** vector's in-plane angle: absolute, and it
cannot drift. The gyroscope is integrated on top of it for two things only —
the detail between gravity samples, and the turn count that lets the wheel go
past 180° for a 900°-style lock. A short (~80 ms) pull keeps the fused angle on
gravity, so gyro bias parks the wheel a fraction of a degree off instead of
letting it wander away and creep back.

How much gravity is believed is a 0..1 weight, not an on/off gate: it fades out
as the phone lies flatter (where its in-plane angle turns into noise) or if the
reported gravity vector isn't ~9.81 m/s² long (a device low-passing raw
acceleration, so your hand movement is leaking in). At those extremes the wheel
runs on the gyro alone and snaps back onto the nearest gravity branch — no
jump — as soon as gravity is worth listening to again.

## Wire protocol

See `server/src/protocol.rs` and `android/.../Protocol.kt` — keep them in sync.
Little-endian throughout.

- Input (phone -> server, 14 bytes, v7): `"CC"` + version + u16 buttons (raw XInput mask) + i16 steer + u8 throttle + u8 brake + u8 clutch + i16 joyX + i16 joyY, sent at ~100Hz
- Telemetry (server -> phone, 32 bytes): `"CT"` + version + i8 gear + 7×f32

## Custom HUD

Tap **✎ Edit** on the phone to enter layout mode: drag any control to move it,
drag the blue corner handle to resize, tap a button to rebind it to any XInput
button (A/B/X/Y/LB/RB/Start/…) or rename it, and use **+ Add** to drop in new
buttons or widgets (analog speedometer / tachometer, gear, speed, joystick). **⚙ Settings**
tunes steering sensitivity, dead zone, max angle, and gauge ranges. The layout is
saved per device and restored on launch; **Reset** restores the stock layout.
