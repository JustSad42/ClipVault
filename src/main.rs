//! ClipVault: a clipboard manager. Native Rust, drawn with Direct2D.
#![windows_subsystem = "windows"]
#![allow(dead_code)] // while the interface is being built

mod clip;
mod gfx;
mod images;
mod model;
mod net;
mod search;
mod settings;
mod store;
mod system;
mod textbox;
mod textops;
mod vision;
mod win;

mod app {
    pub enum Event {
        UpdateStatus,
    }
    pub fn post(_: Event) {}
}

fn main() {}
