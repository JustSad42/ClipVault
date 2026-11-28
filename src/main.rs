//! ClipVault: a clipboard manager. Native Rust, drawn with Direct2D.
#![windows_subsystem = "windows"]

mod app;
mod cache;
mod clip;
mod gfx;
mod images;
mod menu;
mod model;
mod net;
mod overlay;
mod paint;
mod popup;
mod search;
mod settings;
mod settings_ui;
mod store;
mod system;
mod textbox;
mod textops;
mod theme;
mod vision;
mod win;

fn main() {
    std::panic::set_hook(Box::new(|info| win::log(&format!("crash: {info}"))));
    let args: Vec<String> = std::env::args().skip(1).collect();
    app::run(&args);
}
