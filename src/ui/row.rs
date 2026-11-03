use crate::config::Appearance;
use crate::model::{Entry, Message, Part};
use crate::ui;

pub fn entry_ui(ui: &mut egui::Ui, entry: &Entry, channel: &str, appearance: &Appearance) {
    match entry {
        Entry::Msg(message) => message_ui(ui, message, channel, appearance),
        Entry::Note { text, .. } => {
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
    let font_size = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Body)
        .map(|font| font.size)
        .unwrap_or(14.0);
    let small_size = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Small)
        .map(|font| font.size)
        .unwrap_or(11.0);
    let mut previous_emote: Option<egui::Rect> = None;

    ui.with_layout(
        egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
        |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(3.0, 0.0);
        if appearance.show_badges {
            ui::source_badge(
                ui,
                message.platform,
                &format!("{} · #{}", message.platform.label(), channel),
            );
        }

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
                    let size = ui::emote_size(font_size, appearance.emote_scale, emote.aspect);
                    if emote.zero_width
                        && let Some(previous) = previous_emote
                        && ui.is_rect_visible(previous)
                        && paint_zero_width(
                            ui,
                            emote,
                            previous,
                            font_size * appearance.emote_scale,
                            appearance.emote_offset,
                        )
                    {
                        continue;
                    }
                    let rect = paint_emote(ui, emote, size, appearance.emote_offset);
                    previous_emote = Some(rect);
                }
            }
        }
    });
}

/// Reserves the emote box and paints the image with a vertical nudge, so the
/// row layout stays predictable while the picture can be centred by hand.
fn paint_emote(
    ui: &mut egui::Ui,
    emote: &crate::model::EmotePart,
    size: egui::Vec2,
    offset: f32,
) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let image_rect = rect.translate(egui::vec2(0.0, offset));
    if ui.is_rect_visible(image_rect)
        && let Ok(egui::load::TexturePoll::Ready { texture }) =
            egui::Image::new(emote.uri.clone()).load_for_size(ui.ctx(), size)
    {
        ui.painter().image(
            texture.id,
            image_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        ui.ctx().request_repaint();
    }
    image_rect
}

/// Zero-width emotes layer on top of the previous one and advance no space.
fn paint_zero_width(
    ui: &mut egui::Ui,
    emote: &crate::model::EmotePart,
    previous: egui::Rect,
    height: f32,
    offset: f32,
) -> bool {
    let mut rect = ui::zero_width_rect(Some(previous), ui.cursor().min, height);
    rect = rect.translate(egui::vec2(0.0, offset));
    let Ok(egui::load::TexturePoll::Ready { texture }) =
        egui::Image::new(emote.uri.clone()).load_for_size(ui.ctx(), rect.size())
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
