//! SampleWright: separate songs into stems and cut loops and one-shots for your DAW.
//!
//! `samplewright [song]`

// no console window behind the app on Windows
#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod filter_view;
mod icon;
mod player;
mod theme;
mod timeline;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--write-icon") {
        // packaging helper: samplewright --write-icon out.png [size]
        let path = args.get(i + 1).map_or("samplewright.png", |s| s.as_str());
        let size = args.get(i + 2).and_then(|s| s.parse().ok()).unwrap_or(512);
        if let Err(e) = icon::write(std::path::Path::new(path), size) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let open = std::env::args_os().skip(1).find(|a| !a.to_string_lossy().starts_with('-')).map(std::path::PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 860.0])
            .with_min_inner_size([960.0, 600.0])
            .with_app_id("samplewright")
            .with_icon(icon::icon_data(256))
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("SampleWright", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, open)))))
}
