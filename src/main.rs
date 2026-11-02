mod config;

fn main() -> eframe::Result {
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
        Box::new(|_cc| Ok(Box::<Empty>::default())),
    )
}

#[derive(Default)]
struct Empty;

impl eframe::App for Empty {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("lechat");
        });
    }
}
