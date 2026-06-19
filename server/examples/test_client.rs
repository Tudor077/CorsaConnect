//! Fake phone: sends oscillating steering + throttle to the server so you can
//! confirm the virtual controller shows up and moves in BeamNG (or in Windows'
//! "Set up USB game controllers" dialog) before the Android app exists.
//!
//! Run the server in one terminal, then:  cargo run --example test_client

use std::net::UdpSocket;
use std::time::{Duration, Instant};

const SERVER: &str = "127.0.0.1:5000";

fn main() {
    let sock = UdpSocket::bind(("0.0.0.0", 0)).unwrap();
    println!("Sending test input to {SERVER}. Ctrl+C to stop.");

    let start = Instant::now();
    loop {
        let t = start.elapsed().as_secs_f32();
        // Sweep steering left<->right and pulse the throttle.
        let steer = (t.sin() * i16::MAX as f32) as i16;
        let throttle = ((t * 0.5).sin() * 0.5 + 0.5) as f32 * 255.0;

        let mut pkt = Vec::with_capacity(10);
        pkt.extend_from_slice(b"CC"); // magic
        pkt.push(3); // version
        pkt.extend_from_slice(&0u16.to_le_bytes()); // buttons (XInput mask)
        pkt.extend_from_slice(&steer.to_le_bytes());
        pkt.push(throttle as u8); // throttle
        pkt.push(0); // brake
        pkt.push(0); // clutch

        sock.send_to(&pkt, SERVER).unwrap();
        std::thread::sleep(Duration::from_millis(16)); // ~60Hz
    }
}
