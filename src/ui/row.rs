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
                        && let Some(rect) = paint_zero_width(
                            ui,
                            emote,
                            previous,
                            font_size * appearance.emote_scale,
                            appearance.emote_offset,
                        )
                    {
                        hover_emote(ui, rect, &emote.name);
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
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    response.on_hover_text(emote.name.as_str());
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
) -> Option<egui::Rect> {
    let mut rect = ui::zero_width_rect(Some(previous), ui.cursor().min, height);
    rect = rect.translate(egui::vec2(0.0, offset));
    let Ok(egui::load::TexturePoll::Ready { texture }) =
        egui::Image::new(emote.uri.clone()).load_for_size(ui.ctx(), rect.size())
    else {
        return None;
    };
    ui.painter().image(
        texture.id,
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    Some(rect)
}

/// Zero-width emotes are painted on top of the previous one without their own
/// widget, so claim hover for their rect explicitly. The name is part of the id
/// to keep it stable per emote and position across frames.
fn hover_emote(ui: &mut egui::Ui, rect: egui::Rect, name: &str) {
    ui.interact(
        rect,
        ui.scope_id()
            .with(("emote_hover", name, rect.min.x.to_bits(), rect.min.y.to_bits())),
        egui::Sense::hover(),
    )
    .on_hover_text(name);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EmotePart;

    fn emote() -> EmotePart {
        EmotePart {
            name: "catKISS".into(),
            uri: "https://cdn.example/catKISS.webp".into(),
            aspect: 1.0,
            zero_width: false,
        }
    }

    fn input(time: f64) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 200.0),
            )),
            time: Some(time),
            ..Default::default()
        }
    }

    fn run(
        ctx: &egui::Context,
        input: egui::RawInput,
        add: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(input, add);
        output.textures_delta.clear();
        output
    }

    fn draw_emote(ctx: &egui::Context, input: egui::RawInput) -> (egui::Rect, egui::FullOutput) {
        let mut rect = egui::Rect::NOTHING;
        let output = run(ctx, input, |ui| {
            rect = paint_emote(ui, &emote(), egui::vec2(64.0, 64.0), 0.0);
        });
        (rect, output)
    }

    fn tooltip_painted(output: &egui::FullOutput, text: &str) -> bool {
        output.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Text(t) if t.galley.text() == text)
        })
    }

    #[test]
    fn hovering_an_emote_shows_its_name_after_a_short_delay() {
        let ctx = egui::Context::default();
        crate::settings::apply(&ctx, &crate::config::Appearance::default());
        let (rect, _) = draw_emote(&ctx, input(0.0));
        assert!(rect.is_positive(), "emote was not laid out");

        let mut moved = input(0.1);
        moved.events = vec![egui::Event::PointerMoved(rect.center())];
        draw_emote(&ctx, moved);

        // A tenth of a second after the pointer stopped: still nothing.
        let (_, output) = draw_emote(&ctx, input(0.2));
        assert!(!tooltip_painted(&output, "catKISS"));

        // Past the delay the name shows up.
        let mut shown = false;
        for frame in 7..90 {
            let (_, output) = draw_emote(&ctx, input(0.1 + f64::from(frame) * 0.016));
            if tooltip_painted(&output, "catKISS") {
                shown = true;
                break;
            }
        }
        assert!(shown, "tooltip with the emote name was not painted");
    }
}
