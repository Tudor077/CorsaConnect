//! CorsaConnect launcher.
//!
//! A small window that shows the PC's LAN IP (to type on the phone) and a
//! Launch button that starts the server: a virtual Xbox 360 pad fed by phone
//! input over UDP, plus a BeamNG OutGauge -> phone telemetry relay.
//!
//! In release the console window is hidden so it looks like a normal app.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gui;
mod motionsim;
mod outgauge;
mod protocol;
mod server;

fn main() -> eframe::Result<()> {
    gui::run()
}
