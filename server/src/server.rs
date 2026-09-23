//! The actual UDP server: virtual pad + BeamNG telemetry relay.
//!
//! Pulled out of `main` so the GUI launcher can start and stop it on a button,
//! feed its log into a panel, and show live status (ViGEm / phone / BeamNG).

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::motionsim;
use crate::outgauge;
use crate::protocol::{InputPacket, TelemetryPacket};
use crate::scstelemetry;
use crate::vjoy;
use vigem_client::{Client, TargetId, XButtons, XGamepad, Xbox360Wired};

/// Port the phone sends controller input to.
const INPUT_PORT: u16 = 5000;
/// Port BeamNG's OutGauge protocol targets (must match the in-game setting).
const OUTGAUGE_PORT: u16 = 4444;
/// Port BeamNG's MotionSim/OutSim protocol targets (slide + crash physics).
const MOTIONSIM_PORT: u16 = 4445;
/// How long a crash spike keeps fading after it lands.
const IMPACT_DECAY: Duration = Duration::from_millis(350);
/// Port on the phone that listens for telemetry (and for our beacon).
const PHONE_TELEMETRY_PORT: u16 = 5001;
/// How often the beacon goes out. Windows lets unicast replies to a broadcast
/// in for 3 s, so this has to stay well under that.
const BEACON_INTERVAL: Duration = Duration::from_millis(1000);

/// Optional second destination for the telemetry we already parse and enrich.
///
/// This exists because OutGauge is a single-listener protocol - the game sends
/// to exactly one address:port, and whoever binds 4444 first gets it. Rather
/// than fight over the port, anything else that wants the same telemetry can
/// ask us for a copy, and gets the enriched version (learned redline, slip and
/// impact folded in) instead of the raw packet. PicoPanel is the reason it's
/// here; see the PICOPANEL card in the launcher.
///
/// The address lives in [Shared] so the launcher can switch it on and off while
/// the server runs. `CORSACONNECT_MIRROR=127.0.0.1:5051` still seeds it at
/// startup, which is how PicoPanel's own docs describe turning this on.
fn env_mirror() -> Option<SocketAddr> {
    std::env::var("CORSACONNECT_MIRROR")
        .ok()
        .and_then(|v| v.parse::<SocketAddr>().ok())
}
/// Recenter the pad if the phone goes quiet for this long; also the poll
/// interval at which the loops notice a stop request.
const INPUT_TIMEOUT: Duration = Duration::from_millis(250);

/// Live status the GUI reads each frame.
#[derive(Default, Clone)]
pub struct Status {
    pub device_ok: bool, // the virtual controller (wheel or pad) is plugged in
    pub phone: Option<IpAddr>,
    pub beamng: bool,
    pub motion: bool, // MotionSim packets are flowing (slide + crash feedback)
    pub last: Option<(f32, f32, i8)>, // speed km/h, rpm, gear
    pub error: Option<String>,
}

/// Latest physics derived from MotionSim, shared with the telemetry relay.
struct MotionState {
    slip: f32,
    impact: f32,
    impact_at: Instant,
}

/// Shared between the server threads and the GUI.
pub struct Shared {
    logs: Mutex<VecDeque<String>>,
    pub status: Mutex<Status>,
    motion: Mutex<MotionState>,
    /// Where to send a copy of every telemetry packet, if anywhere. See
    /// [env_mirror]; the launcher can change this while the server runs.
    mirror: Mutex<Option<SocketAddr>>,
    /// How many copies have gone out, so the GUI can show the link is alive.
    mirror_sent: AtomicU64,
    /// Head tracking status/preview (runs on its own thread).
    pub head: crate::headtracker::HeadShared,
}

impl Shared {
    pub fn new() -> Arc<Self> {
        Arc::new(Shared {
            logs: Mutex::new(VecDeque::new()),
            status: Mutex::new(Status::default()),
            motion: Mutex::new(MotionState {
                slip: 0.0,
                impact: 0.0,
                impact_at: Instant::now(),
            }),
            mirror: Mutex::new(env_mirror()),
            mirror_sent: AtomicU64::new(0),
            head: crate::headtracker::HeadShared::default(),
        })
    }

    /// Feed new physics: slip is instantaneous; impact latches its peak so a
    /// brief spike between relay sends isn't lost, then fades via [IMPACT_DECAY].
    fn update_motion(&self, slip: f32, impact: f32) {
        let mut m = self.motion.lock().unwrap();
        m.slip = slip;
        let now = Instant::now();
        if impact > decay_impact(m.impact, m.impact_at, now) {
            m.impact = impact;
            m.impact_at = now;
        }
    }

    /// (slip, decayed impact) for the next telemetry packet.
    fn motion_snapshot(&self) -> (f32, f32) {
        let m = self.motion.lock().unwrap();
        (m.slip, decay_impact(m.impact, m.impact_at, Instant::now()))
    }

    pub fn log(&self, msg: impl Into<String>) {
        let mut l = self.logs.lock().unwrap();
        l.push_back(msg.into());
        while l.len() > 300 {
            l.pop_front();
        }
    }

    /// Snapshot of recent log lines for display.
    pub fn log_lines(&self) -> Vec<String> {
        self.logs.lock().unwrap().iter().cloned().collect()
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    /// Where telemetry copies go, or `None` when mirroring is off.
    pub fn mirror(&self) -> Option<SocketAddr> {
        *self.mirror.lock().unwrap()
    }

    /// Point the mirror somewhere else, or turn it off with `None`. Takes
    /// effect on the next packet - no restart, nothing to re-bind.
    pub fn set_mirror(&self, addr: Option<SocketAddr>) {
        *self.mirror.lock().unwrap() = addr;
    }

    /// Telemetry packets copied to the mirror so far.
    pub fn mirror_sent(&self) -> u64 {
        self.mirror_sent.load(Ordering::Relaxed)
    }

    /// Send one already-encoded packet to the mirror, if it's on.
    fn send_mirror(&self, tx: &UdpSocket, packet: &[u8]) {
        if let Some(a) = self.mirror() {
            if tx.send_to(packet, a).is_ok() {
                self.mirror_sent.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn set_status(&self, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock().unwrap());
    }
}

/// Linear fade of an impact peak to 0 over [IMPACT_DECAY].
fn decay_impact(impact: f32, at: Instant, now: Instant) -> f32 {
    let elapsed = now.saturating_duration_since(at).as_secs_f32();
    let span = IMPACT_DECAY.as_secs_f32();
    if elapsed >= span {
        0.0
    } else {
        impact * (1.0 - elapsed / span)
    }
}

/// The one UDP socket the phone talks to, and where the phone is.
///
/// Everything to and from the phone goes through this socket on
/// [INPUT_PORT] - input in, telemetry and beacons out - and that is what lets
/// CorsaConnect work without a firewall rule. Windows Firewall blocks
/// unsolicited inbound UDP, but it lets in replies to traffic the PC sent
/// first: unicast replies to a broadcast for 3 s, and anything coming back on
/// a flow we opened (our port 5000 <-> the phone's 5001). So the PC speaks
/// first: [beacon_loop] broadcasts from :5000 to the LAN, the phone answers
/// with input from :5001, and from then on our own telemetry and beacons to
/// the phone keep that flow open.
struct PhoneLink {
    sock: UdpSocket,
    addr: Mutex<Option<SocketAddr>>,
}

impl PhoneLink {
    fn bind() -> std::io::Result<Self> {
        let sock = UdpSocket::bind(("0.0.0.0", INPUT_PORT))?;
        sock.set_read_timeout(Some(INPUT_TIMEOUT))?;
        sock.set_broadcast(true)?;
        Ok(PhoneLink { sock, addr: Mutex::new(None) })
    }

    fn phone(&self) -> Option<SocketAddr> {
        *self.addr.lock().unwrap()
    }

    /// Send to the phone, if one has connected.
    fn send(&self, packet: &[u8]) {
        if let Some(a) = self.phone() {
            let _ = self.sock.send_to(packet, a);
        }
    }
}

/// Where the beacon goes: the limited broadcast, plus the directed broadcast
/// of our LAN (Windows sends 255.255.255.255 out of one adapter only, which is
/// the wrong one when a VPN or Hyper-V switch is up). The subnet is assumed to
/// be a /24, which is what nearly every home router hands out.
fn beacon_targets() -> Vec<SocketAddr> {
    let mut v = vec![SocketAddr::from((Ipv4Addr::BROADCAST, PHONE_TELEMETRY_PORT))];
    if let Some(ip) = local_ipv4() {
        let [a, b, c, _] = ip.octets();
        v.push(SocketAddr::from((Ipv4Addr::new(a, b, c, 255), PHONE_TELEMETRY_PORT)));
    }
    v
}

/// Announce the PC on the LAN once a second, and poke the phone directly once
/// it's connected. The broadcast is what opens the firewall for the phone's
/// first packet (and lets the app fill in the PC's IP by itself); the unicast
/// keeps the flow open while no game is sending telemetry.
fn beacon_loop(link: Arc<PhoneLink>, stop: Arc<AtomicBool>) {
    let packet = crate::protocol::beacon();
    let mut targets = beacon_targets();
    let mut ticks = 0u32;
    while !stop.load(Ordering::Relaxed) {
        // The LAN address can change under us (Wi-Fi switch, DHCP renew).
        ticks += 1;
        if ticks % 10 == 0 {
            targets = beacon_targets();
        }
        for t in &targets {
            let _ = link.sock.send_to(&packet, t);
        }
        link.send(&packet);
        // Sleep in slices so Stop doesn't wait a whole interval.
        let until = Instant::now() + BEACON_INTERVAL;
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Best-effort primary LAN IPv4. Picks the source address the OS would use to
/// reach the internet; no packets are actually sent. `None` if offline.
pub fn local_ipv4() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    sock.connect(("8.8.8.8", 80)).ok()?;
    match sock.local_addr().ok()? {
        SocketAddr::V4(a) => Some(*a.ip()),
        _ => None,
    }
}

/// The game we're feeding. The virtual controller (steering, pedals, buttons)
/// works for every game; the choice only decides where dashboard + force
/// feedback telemetry is read from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Game {
    BeamNg,
    TruckSim, // ETS2 / ATS
    Wrc10,
}

impl Game {
    pub const ALL: [Game; 3] = [Game::BeamNg, Game::TruckSim, Game::Wrc10];

    pub fn name(self) -> &'static str {
        match self {
            Game::BeamNg => "BeamNG.drive",
            Game::TruckSim => "Euro / American Truck Sim",
            Game::Wrc10 => "WRC 10",
        }
    }

    /// One-line setup hint shown under the picker.
    pub fn hint(self) -> &'static str {
        match self {
            Game::BeamNg => {
                "Options > Other > Protocols: enable OutGauge (127.0.0.1:4444) and \
                 MotionSim/OutSim (4445)."
            }
            Game::TruckSim => {
                "Drop the scs-sdk-plugin DLL into the game's bin\\win_x64\\plugins folder. \
                 (Dashboard telemetry coming soon; the wheel already works.)"
            }
            Game::Wrc10 => {
                "Dashboard telemetry coming soon; steering and pedals already work via the \
                 virtual controller."
            }
        }
    }
}

/// What the phone shows up as on the PC.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DeviceMode {
    /// A vJoy DirectInput joystick: steering on its own 15-bit axis, a separate
    /// axis per pedal. Not an XInput device, so it never collides with a real
    /// Xbox pad - both can stay bound in the game at the same time.
    Wheel,
    /// The classic virtual Xbox 360 pad through ViGEmBus, for games that only
    /// read XInput. Plugging in a real Xbox pad alongside it means fighting
    /// over controller slots, so it's no longer the default.
    Xbox360,
}

impl DeviceMode {
    pub const ALL: [DeviceMode; 2] = [DeviceMode::Wheel, DeviceMode::Xbox360];

    pub fn name(self) -> &'static str {
        match self {
            DeviceMode::Wheel => "Racing wheel (vJoy)",
            DeviceMode::Xbox360 => "Xbox 360 pad (ViGEmBus)",
        }
    }

    /// Label for the status dot.
    pub fn dot(self) -> &'static str {
        match self {
            DeviceMode::Wheel => "vJoy wheel",
            DeviceMode::Xbox360 => "ViGEmBus (virtual controller)",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            DeviceMode::Wheel => {
                "Own DirectInput device - bind it in-game as a wheel. Coexists with a real \
                 Xbox pad, so you never have to rebind. Needs the vJoy driver."
            }
            DeviceMode::Xbox360 => {
                "A second Xbox pad: works everywhere, but XInput games index pads by slot, \
                 so a real Xbox pad plugged in at the same time fights it. Needs ViGEmBus."
            }
        }
    }
}

/// The virtual device we feed, whichever backend is in use.
enum Pad {
    Wheel(vjoy::Wheel),
    Xbox(Xbox360Wired<Client>),
}

impl Pad {
    fn update(&mut self, input: &InputPacket) {
        match self {
            Pad::Wheel(w) => w.update(input),
            Pad::Xbox(p) => {
                let _ = p.update(&to_gamepad(input));
            }
        }
    }

    /// Wheel centered, pedals up - what we send when the phone goes quiet.
    fn center(&mut self) {
        match self {
            Pad::Wheel(w) => w.center(),
            Pad::Xbox(p) => {
                let _ = p.update(&XGamepad::default());
            }
        }
    }
}

/// Bring up the virtual device for `mode`, logging what the game will see.
fn open_device(shared: &Arc<Shared>, mode: DeviceMode) -> Result<Pad, String> {
    match mode {
        DeviceMode::Wheel => {
            let (wheel, notes) = vjoy::Wheel::open(vjoy::DEVICE_ID).map_err(|e| {
                format!("{e} Install vJoy from https://github.com/njz3/vJoy/releases, or switch to Xbox 360 pad mode.")
            })?;
            shared.log("Virtual racing wheel ready (vJoy).");
            for n in notes {
                shared.log(n);
            }
            Ok(Pad::Wheel(wheel))
        }
        DeviceMode::Xbox360 => {
            let client = Client::connect().map_err(|e| {
                format!("Could not connect to ViGEmBus ({e}). Install the ViGEmBus driver, then Launch again.")
            })?;
            let mut pad = Xbox360Wired::new(client, TargetId::default());
            pad.plugin()
                .and_then(|_| pad.wait_ready())
                .map_err(|e| format!("Virtual controller failed to start: {e}"))?;
            shared.log("Virtual Xbox 360 controller plugged in.");
            Ok(Pad::Xbox(pad))
        }
    }
}

/// Run the server until `stop` is set. Reports progress/errors via `shared`.
/// Returns when the input loop ends (stop requested or a fatal error).
pub fn run(shared: Arc<Shared>, stop: Arc<AtomicBool>, game: Game, device: DeviceMode) {
    shared.set_status(|s| {
        *s = Status::default();
    });
    shared.log("Starting CorsaConnect server...");

    let mut pad = match open_device(&shared, device) {
        Ok(p) => p,
        Err(msg) => {
            shared.log(&msg);
            shared.set_status(|s| s.error = Some(msg));
            return;
        }
    };
    shared.set_status(|s| s.device_ok = true);

    // The phone's socket; where to send telemetry is learned from its first
    // input packet.
    let link = match PhoneLink::bind() {
        Ok(l) => Arc::new(l),
        Err(e) => {
            let msg = format!("Could not open UDP :{INPUT_PORT} for the phone: {e}");
            shared.log(&msg);
            shared.set_status(|s| s.error = Some(msg));
            return;
        }
    };
    shared.log(format!(
        "Listening for phone input on UDP :{INPUT_PORT}, announcing the PC on the LAN (no firewall rule needed)."
    ));

    // Telemetry (dashboard + force feedback) per game. The virtual controller
    // works everywhere; only the telemetry source differs.
    let mut telem: Vec<std::thread::JoinHandle<()>> = Vec::new();
    telem.push({
        let link = Arc::clone(&link);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || beacon_loop(link, stop))
    });
    match game {
        Game::BeamNg => {
            telem.push({
                let shared = Arc::clone(&shared);
                let stop = Arc::clone(&stop);
                let link = Arc::clone(&link);
                std::thread::spawn(move || telemetry_relay(shared, stop, link))
            });
            telem.push({
                let shared = Arc::clone(&shared);
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || motion_listener(shared, stop))
            });
        }
        Game::TruckSim => {
            telem.push({
                let shared = Arc::clone(&shared);
                let stop = Arc::clone(&stop);
                let link = Arc::clone(&link);
                std::thread::spawn(move || truck_telemetry(shared, stop, link))
            });
        }
        Game::Wrc10 => {
            shared.log(format!(
                "{}: wheel, pedals and buttons are live. Dashboard telemetry isn't wired for this game yet.",
                game.name()
            ));
        }
    }

    if let Err(e) = input_loop(&shared, &stop, &mut pad, &link) {
        let msg = format!("Input listener stopped: {e}");
        shared.log(&msg);
        shared.set_status(|s| s.error = Some(msg));
    }

    for h in telem {
        let _ = h.join();
    }
    // Dropping `pad` here unplugs the virtual controller.
    drop(pad);
    shared.set_status(|s| {
        s.device_ok = false;
        s.phone = None;
        s.beamng = false;
        s.motion = false;
    });
    shared.log("Server stopped.");
}

fn input_loop(
    shared: &Arc<Shared>,
    stop: &Arc<AtomicBool>,
    pad: &mut Pad,
    link: &Arc<PhoneLink>,
) -> std::io::Result<()> {
    let mut buf = [0u8; 64];
    while !stop.load(Ordering::Relaxed) {
        match link.sock.recv_from(&mut buf) {
            Ok((n, src)) => {
                if let Some(input) = InputPacket::parse(&buf[..n]) {
                    pad.update(&input);
                    let mut slot = link.addr.lock().unwrap();
                    if slot.map(|a| a.ip()) != Some(src.ip()) {
                        // Always the phone's :5001, not the packet's source port:
                        // older apps send from a random port but listen on 5001.
                        let addr = SocketAddr::new(src.ip(), PHONE_TELEMETRY_PORT);
                        *slot = Some(addr);
                        drop(slot);
                        // Open the flow back to the phone right away rather than
                        // on the next beacon.
                        let _ = link.sock.send_to(&crate::protocol::beacon(), addr);
                        shared.log(format!("Phone connected from {}", src.ip()));
                        shared.set_status(|s| s.phone = Some(src.ip()));
                    }
                }
            }
            // Windows reports an ICMP "port unreachable" from an earlier send
            // (a beacon to a phone whose app is closed) as an error on the next
            // receive. It says nothing about this socket; keep listening.
            Err(ref e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // No input: recenter so the car doesn't run away.
                pad.center();
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn to_gamepad(input: &InputPacket) -> XGamepad {
    let clutch_axis = (input.clutch as i32 * i16::MAX as i32 / 255) as i16;
    XGamepad {
        buttons: XButtons { raw: input.buttons },
        // The free stick takes the two thumbstick axes nothing else uses: the
        // left stick's Y and the right stick's X. Not as tidy as vJoy's spare
        // axes, but an Xbox pad has nothing else left.
        thumb_lx: input.steer,
        thumb_ly: input.joy_y,
        thumb_rx: input.joy_x,
        thumb_ry: clutch_axis,
        left_trigger: input.brake,
        right_trigger: input.throttle,
    }
}

fn telemetry_relay(shared: Arc<Shared>, stop: Arc<AtomicBool>, link: Arc<PhoneLink>) {
    let sock = match UdpSocket::bind(("0.0.0.0", OUTGAUGE_PORT)) {
        Ok(s) => s,
        Err(e) => {
            shared.log(format!("Could not bind OutGauge port {OUTGAUGE_PORT}: {e}"));
            return;
        }
    };
    if sock.set_read_timeout(Some(INPUT_TIMEOUT)).is_err() {
        shared.log("Could not set OutGauge read timeout.");
        return;
    }
    let tx = match UdpSocket::bind(("0.0.0.0", 0)) {
        Ok(s) => s,
        Err(e) => {
            shared.log(format!("Could not open telemetry sender socket: {e}"));
            return;
        }
    };
    shared.log(format!("Listening for BeamNG OutGauge on UDP :{OUTGAUGE_PORT}"));

    if let Some(a) = shared.mirror() {
        shared.log(format!("Mirroring telemetry to {a}"));
    }

    let mut buf = [0u8; 128];
    let mut announced = false;
    // Redline = the rev limiter, found as the rpm where flat-out the engine stops
    // climbing (the rev cut). Until then the tach stays large with no red zone, so
    // a fresh car looks normal at idle instead of pinning the needle to a tiny
    // redline; once the limiter is learned the gauge shrinks down to it.
    // BeamNG's OutGauge has no per-car id (`car` is always "beam") and never sets
    // the shift-light bit, so a pause in the stream is our only "car changed" cue.
    let mut peak_rpm = 0.0f32;
    let mut frames_since_peak = 0u32;
    let mut limiter = 0.0f32;
    let mut idle_ticks = 0u32;
    while !stop.load(Ordering::Relaxed) {
        let (n, _) = match sock.recv_from(&mut buf) {
            Ok(v) => v,
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                idle_ticks += 1;
                continue;
            }
            Err(_) => continue,
        };
        let gap = idle_ticks;
        idle_ticks = 0;
        let Some(mut tel) = outgauge::parse(&buf[..n]) else {
            if !announced {
                shared.log(format!(
                    "Receiving UDP on :{OUTGAUGE_PORT} but {n} bytes isn't OutGauge (is MotionSim on instead of OutGauge?)"
                ));
                announced = true;
            }
            continue;
        };
        if !announced {
            shared.log("OutGauge: data flowing from BeamNG.");
            announced = true;
        }
        shared.set_status(|s| {
            s.beamng = true;
            s.last = Some((tel.speed_kmh, tel.rpm, tel.gear));
        });
        // Fold in the latest physics (0 if MotionSim isn't enabled in BeamNG).
        let (slip, impact) = shared.motion_snapshot();
        tel.slip = slip;
        tel.impact = impact;

        // A long pause in the stream (radial menu, vehicle spawn, world reload)
        // usually means a possible car change - relearn the rev range. gap counts
        // 250 ms read timeouts, so >=6 is ~1.5 s of silence.
        if gap >= 6 && (peak_rpm > 0.0 || limiter > 0.0) {
            peak_rpm = 0.0;
            limiter = 0.0;
            frames_since_peak = 0;
            shared.log("Telemetry resumed - relearning rev range.");
        }

        // Track the peak rpm and how long since it last rose.
        if tel.rpm > peak_rpm {
            peak_rpm = tel.rpm;
            frames_since_peak = 0;
            if peak_rpm > limiter + 100.0 {
                limiter = 0.0; // revved past the supposed limiter -> relearn higher
            }
        } else {
            frames_since_peak = frames_since_peak.saturating_add(1);
        }
        // Rev limiter: flat-out, high rpm, and the peak hasn't moved for a moment.
        if limiter == 0.0 && tel.throttle > 0.85 && peak_rpm > 3000.0 && frames_since_peak >= 30 {
            limiter = peak_rpm;
            shared.log(format!("Redline learned: {:.0} rpm.", limiter));
        }

        // Big gauge with no red until the limiter is known; then shrink to it.
        if limiter > 0.0 {
            tel.redline = limiter;
            tel.max_rpm = limiter * 1.08;
        } else {
            tel.max_rpm = 9000.0f32.max(peak_rpm * 1.05);
            tel.redline = tel.max_rpm;
        }
        let packet = tel.encode();
        link.send(&packet);
        // Sent whether or not a phone is connected: the mirror is a separate
        // consumer and shouldn't depend on the phone being up.
        shared.send_mirror(&tx, &packet);
    }
}

/// Reads ETS2 / ATS telemetry from the scs-sdk-plugin shared memory and relays
/// speed / rpm / gear to the phone.
fn truck_telemetry(shared: Arc<Shared>, stop: Arc<AtomicBool>, link: Arc<PhoneLink>) {
    let tx = match UdpSocket::bind(("0.0.0.0", 0)) {
        Ok(s) => s,
        Err(e) => {
            shared.log(format!("Could not open telemetry sender socket: {e}"));
            return;
        }
    };
    shared.log("Looking for ETS2/ATS telemetry (install the scs-sdk-plugin DLL)...");

    // Wait for the shared memory to appear (game + plugin running).
    let scs = loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if let Some(s) = scstelemetry::ScsShared::open() {
            break s;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    shared.log("Truck telemetry connected.");
    shared.set_status(|s| s.beamng = true);

    while !stop.load(Ordering::Relaxed) {
        let mut tel = TelemetryPacket {
            speed_kmh: scs.speed_ms() * 3.6,
            rpm: scs.rpm(),
            // SCS gear: 1 = 1st, 0 = neutral, -1 = reverse. Ours: 0 = R, 1 = N, 2 = 1st.
            gear: (scs.gear() + 1) as i8,
            fuel: scs.fuel_frac(),
            engine_temp: scs.water_temp(),
            show_lights: scs.show_lights(),
            ..Default::default()
        };
        // Sensible truck rev range until we read the real one from the plugin.
        tel.max_rpm = 3000.0;
        tel.redline = 2600.0;

        shared.set_status(|s| s.last = Some((tel.speed_kmh, tel.rpm, tel.gear)));
        let packet = tel.encode();
        link.send(&packet);
        shared.send_mirror(&tx, &packet);
        std::thread::sleep(Duration::from_millis(16));
    }
}

/// Listens for BeamNG MotionSim packets and turns them into slide + crash
/// strength for the telemetry relay to forward.
fn motion_listener(shared: Arc<Shared>, stop: Arc<AtomicBool>) {
    let sock = match UdpSocket::bind(("0.0.0.0", MOTIONSIM_PORT)) {
        Ok(s) => s,
        Err(e) => {
            shared.log(format!("Could not bind MotionSim port {MOTIONSIM_PORT}: {e}"));
            return;
        }
    };
    if sock.set_read_timeout(Some(INPUT_TIMEOUT)).is_err() {
        shared.log("Could not set MotionSim read timeout.");
        return;
    }
    shared.log(format!(
        "Listening for BeamNG MotionSim on UDP :{MOTIONSIM_PORT} (slide + crash)"
    ));

    let mut buf = [0u8; 128];
    let mut announced = false;
    let mut slip_est = motionsim::SlipEstimator::new();
    while !stop.load(Ordering::Relaxed) {
        let (n, _) = match sock.recv_from(&mut buf) {
            Ok(v) => v,
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => continue,
        };
        let Some(m) = motionsim::parse(&buf[..n]) else {
            continue;
        };
        if !announced {
            shared.log("MotionSim: data flowing from BeamNG.");
            shared.set_status(|s| s.motion = true);
            announced = true;
        }
        let slip = slip_est.update(&m);
        shared.update_motion(slip, motionsim::impact_fraction(&m));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mirror is off unless someone asks for it, follows [Shared::set_mirror]
    /// without a restart, and counts only what actually went out - that counter
    /// is what the launcher shows to prove the link is alive.
    #[test]
    fn mirror_follows_the_setting() {
        let shared = Shared::new();
        // A listener standing in for PicoPanel's "Corsa" source.
        let pico = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        pico.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let tx = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let packet = TelemetryPacket {
            speed_kmh: 88.0,
            rpm: 4200.0,
            gear: 3,
            ..Default::default()
        }
        .encode();

        // Off (whatever the environment said, this test owns the setting).
        shared.set_mirror(None);
        shared.send_mirror(&tx, &packet);
        assert_eq!(shared.mirror_sent(), 0);

        // On, mid-run.
        shared.set_mirror(Some(pico.local_addr().unwrap()));
        shared.send_mirror(&tx, &packet);
        let mut buf = [0u8; 128];
        let (n, _) = pico.recv_from(&mut buf).expect("nothing arrived at the mirror");
        assert_eq!(&buf[..n], &packet[..]);
        assert_eq!(shared.mirror_sent(), 1);

        // And off again, still mid-run.
        shared.set_mirror(None);
        shared.send_mirror(&tx, &packet);
        assert_eq!(shared.mirror_sent(), 1);
        assert!(pico.recv_from(&mut buf).is_err());
    }

    /// What the mirror carries has to be what PicoPanel's decoder expects:
    /// `<2sBb11fHI16s16s`, 86 bytes, version 7.
    #[test]
    fn packet_matches_the_shape_picopanel_unpacks() {
        let packet = TelemetryPacket::default().encode();
        assert_eq!(packet.len(), 86);
        assert_eq!(&packet[..2], b"CT");
        assert_eq!(packet[2], crate::protocol::PROTO_VERSION);
    }
}
