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

/// Only the platform's primary backend: Vulkan on Linux, DX12 on Windows,
/// Metal on macOS.
///
/// eframe adds OpenGL as well by default, and probing it loads the whole
/// gallium stack (~60 MB of resident driver code) just to never use it.
/// `WGPU_BACKEND=gl` (or any other wgpu value) still overrides this.
fn backends() -> eframe::egui_wgpu::wgpu::Backends {
    use eframe::egui_wgpu::wgpu::Backends;
    Backends::from_env().unwrap_or(Backends::PRIMARY)
}

fn run(
    config: config::Config,
    config_error: Option<String>,
    backends: eframe::egui_wgpu::wgpu::Backends,
) -> eframe::Result {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([config.window.width.max(720.0), config.window.height.max(520.0)])
            .with_min_inner_size([720.0, 520.0])
            .with_title("lechat"),
        ..Default::default()
    };
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        setup.instance_descriptor.backends = backends;
    }

    eframe::run_native(
        "lechat",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, config, config_error)))),
    )
}

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

    run(config, config_error, backends())
}
