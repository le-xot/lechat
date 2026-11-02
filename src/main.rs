mod app;
mod config;
mod emotes;
mod images;
mod model;
mod providers;
mod settings;
mod twitch;
mod ui;
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

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([config.window.width.max(720.0), config.window.height.max(520.0)])
            .with_min_inner_size([720.0, 520.0])
            .with_title("lechat"),
        ..Default::default()
    };

    eframe::run_native(
        "lechat",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, config, config_error)))),
    )
}
