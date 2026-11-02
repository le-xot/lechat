mod app;
mod config;
mod emotes;
mod images;
mod model;
mod providers;
mod twitch;
mod ui;

fn main() -> eframe::Result {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([720.0, 520.0])
            .with_title("lechat"),
        ..Default::default()
    };

    eframe::run_native(
        "lechat",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
