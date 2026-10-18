#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod pdf;
#[cfg(windows)]
mod updates;

#[cfg(windows)]
fn main() {
    app::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The desktop package targets Windows x64. The Rust service and core tests run on this platform.");
}
