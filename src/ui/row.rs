use crate::config::Appearance;
use crate::model::{Entry, Message, Part};
use crate::ui;

pub fn entry_ui(ui: &mut egui::Ui, entry: &Entry, channel: &str, appearance: &Appearance) {
    match entry {
        Entry::Msg(message) => message_ui(ui, message, channel, appearance),
        Entry::Note { text, .. } => {
            ui.add_space(2.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(text)
                        .italics()
                        .size(ui.style().text_styles.get(&egui::TextStyle::Small).map(|f| f.size).unwrap_or(12.0))
                        .color(egui::Color32::from_gray(150)),
                );
            });
            ui.add_space(2.0);
        }
    }
}

pub fn message_ui(
    ui: &mut egui::Ui,
    message: &Message,
    channel: &str,
    appearance: &Appearance,
) {
    let line_height = ui.text_style_height(&egui::TextStyle::Body);
    let small_size = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Small)
        .map(|font| font.size)
        .unwrap_or(11.0);
    let mut previous_emote: Option<egui::Rect> = None;

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui::source_badge(
            ui,
            message.platform,
            &format!("{} · #{}", message.platform.label(), channel),
        );

        if appearance.show_time {
            let time = ui::format_time(message.ts_ms, appearance.show_seconds);
            ui.label(
                egui::RichText::new(time)
                    .size(small_size)
                    .color(egui::Color32::from_gray(150)),
            );
        }

        let color = message
            .author
            .color
            .map(|[r, g, b]| egui::Color32::from_rgb(r, g, b))
            .unwrap_or(egui::Color32::from_rgb(0xC4, 0xC0, 0xCC));
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
                        if let Some(previous) = previous_emote
                            && ui.is_rect_visible(previous)
                            && paint_zero_width(ui, emote, previous, line_height)
                        {
                            continue;
                        }
                        ui.add(
                            egui::Image::new(emote.uri.clone())
                                .fit_to_exact_size(size * 0.7)
                                .alt_text(emote.name.clone()),
                        );
                    } else {
                        // Only paint emotes inside the viewport: an animated
                        // image schedules repaints, and off-screen rows would
                        // otherwise keep redrawing the whole feed.
                        let rect =
                            egui::Rect::from_min_size(ui.cursor().min, size);
                        if ui.is_rect_visible(rect) {
                            let response = ui.add(
                                egui::Image::new(emote.uri.clone())
                                    .fit_to_exact_size(size)
                                    .alt_text(emote.name.clone()),
                            );
                            previous_emote = Some(response.rect);
                        } else {
                            ui.allocate_exact_size(size, egui::Sense::hover());
                        }
                    }
                }
            }
        }
    });
}

/// Zero-width emotes layer on top of the previous one and advance no space.
fn paint_zero_width(
    ui: &mut egui::Ui,
    emote: &crate::model::EmotePart,
    previous: egui::Rect,
    line_height: f32,
) -> bool {
    let rect = ui::zero_width_rect(Some(previous), ui.cursor().min, line_height);
    let image = egui::Image::new(emote.uri.clone());
    let Ok(egui::load::TexturePoll::Ready { texture }) =
        image.load_for_size(ui.ctx(), rect.size())
    else {
        return false;
    };
    ui.painter().image(
        texture.id,
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    true
}
