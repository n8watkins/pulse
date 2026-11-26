//! Pulse, native shell: the same panel as the web design, drawn by Slint
//! (Skia renderer), with no browser engine.

#![cfg_attr(windows, windows_subsystem = "windows")]
#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(windows)]
mod app;
mod logic;
mod record;
mod tray;
mod view;
#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("pulse-native draws a Windows tray panel; build it for Windows.");
}
