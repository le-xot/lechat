#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod emotes;
mod images;
mod model;
mod providers;
mod settings;
mod twitch;
mod ui;
mod update;
mod util;

fn main() -> eframe::Result {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let path = config::Config::default_path();
    let (config, config_error) = match config::Config::load_or_create(&path) {
        Ok(config) => (config, None),
        Err(err) => (
            config::Config::default_config(),
            Some(format!("конфиг {}: {err:#}", path.display())),
        ),
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([config.window.width.max(720.0), config.window.height.max(520.0)])
        .with_min_inner_size([720.0, 520.0])
        .with_title("lechat");
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "lechat",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, config, config_error)))),
    )
}

/// Window icon used by the title bar, task bar and alt-tab switcher.
///
/// Explorer reads the icon embedded into the executable (see `build.rs`),
/// but Windows also lets a window override it at runtime.
fn window_icon() -> Option<egui::IconData> {
    let image = image::load_from_memory(include_bytes!("../assets/lechat.png"))
        .ok()?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Some(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_icon_decodes() {
        let icon = window_icon().expect("bundled icon must decode");
        assert_eq!((icon.width, icon.height), (512, 512));
        assert_eq!(icon.rgba.len(), 512 * 512 * 4);
        assert!(icon.rgba.iter().skip(3).step_by(4).any(|&alpha| alpha > 0));
    }

    #[test]
    fn windows_icon_has_small_and_large_sizes() {
        let bytes = include_bytes!("../assets/lechat.ico");
        assert!(bytes.len() >= 6);
        let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        let sizes: Vec<u32> = (0..count)
            .map(|i| {
                let at = 6 + i * 16;
                match bytes[at] {
                    0 => 256,
                    width => u32::from(width),
                }
            })
            .collect();
        for expected in [16, 32, 48, 256] {
            assert!(sizes.contains(&expected), "missing {expected}px icon: {sizes:?}");
        }
    }
}
