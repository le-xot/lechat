use crate::model::{Entry, Message, Part};
use crate::ui;

pub fn entry_ui(ui: &mut egui::Ui, entry: &Entry, channel: &str) {
    match entry {
        Entry::Msg(message) => message_ui(ui, message, channel),
        Entry::Note { text, .. } => {
            ui.add_space(2.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(text)
                        .italics()
                        .size(12.0)
                        .color(egui::Color32::from_gray(150)),
                );
            });
            ui.add_space(2.0);
        }
    }
}

pub fn message_ui(ui: &mut egui::Ui, message: &Message, channel: &str) {
    let line_height = ui.text_style_height(&egui::TextStyle::Body);

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui::source_badge(
            ui,
            message.platform,
            &format!("{} · #{}", message.platform.label(), channel),
        );

        let time = chrono::DateTime::from_timestamp_millis(message.ts_ms)
            .map(|time| {
                time.with_timezone(&chrono::Local)
                    .format("%H:%M")
                    .to_string()
            })
            .unwrap_or_else(|| "--:--".to_string());
        ui.label(
            egui::RichText::new(time)
                .size(11.0)
                .color(egui::Color32::from_gray(120)),
        );

        let color = message
            .author
            .color
            .map(|[r, g, b]| egui::Color32::from_rgb(r, g, b))
            .unwrap_or(egui::Color32::from_rgb(0xAD, 0xA9, 0xB4));
        ui.label(
            egui::RichText::new(format!("{}:", message.author.name))
                .strong()
                .color(color),
        );

        for part in &message.parts {
            match part {
                Part::Text(text) => {
                    ui.label(egui::RichText::new(text.clone()));
                }
                Part::Link(url) => {
                    ui.hyperlink_to(url.clone(), url.clone());
                }
                Part::Emote(emote) => {
                    let size = ui::emote_size(line_height, emote.aspect);
                    if emote.zero_width {
                        ui.add(egui::Image::new(emote.uri.clone()).fit_to_exact_size(size * 0.7));
                        ui.add_space(-size.x * 0.45);
                    } else {
                        ui.add(egui::Image::new(emote.uri.clone()).fit_to_exact_size(size));
                    }
                }
            }
        }
    });
}
