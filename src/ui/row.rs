use std::time::Duration;

use crate::config::Appearance;
use crate::model::{Entry, Message, Part};
use crate::ui;

/// How soon a visible emote that is still downloading is painted again.
const PENDING_RETRY: Duration = Duration::from_millis(50);
/// Retry delay for emotes that failed to load. A broken URL must not turn the
/// whole window into a redraw loop.
const FAILED_RETRY: Duration = Duration::from_secs(1);

// Counts rows laid out by the widget pass. Only used by tests: laying out a
// row is the expensive part of a redraw, painting is culled by egui.
#[cfg(test)]
thread_local! {
    pub static ROWS_LAID_OUT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub fn entry_ui(ui: &mut egui::Ui, entry: &Entry, channel: &str, appearance: &Appearance) {
    #[cfg(test)]
    ROWS_LAID_OUT.with(|count| count.set(count.get() + 1));
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

    // `horizontal_wrapped` rather than a bare `with_layout`: the latter hands
    // the row the whole remaining window height, so the first row of the feed
    // grew to the size of the viewport.
    ui.horizontal_wrapped(|ui| {
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
    // Rows scrolled out of the feed must not keep the window awake: without
    // this early return every offscreen emote asks for a repaint on every pass,
    // and egui happily redraws the whole feed at the monitor refresh rate.
    if !ui.is_rect_visible(image_rect) {
        return image_rect;
    }
    match egui::Image::new(emote.uri.clone()).load_for_size(ui.ctx(), size) {
        Ok(egui::load::TexturePoll::Ready { texture }) => {
            ui.painter().image(
                texture.id,
                image_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        Ok(egui::load::TexturePoll::Pending { .. }) => {
            // The loaders wake the UI up when the bytes arrive; this is only a
            // safety net, so it is slow enough not to spin at the refresh rate.
            ui.ctx().request_repaint_after(PENDING_RETRY);
        }
        Err(err) => {
            tracing::debug!(emote = %emote.name, "не удалось загрузить эмоут: {err}");
            ui.ctx().request_repaint_after(FAILED_RETRY);
        }
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
    fn a_row_is_only_as_tall_as_its_content() {
        let ctx = egui::Context::default();
        crate::settings::apply(&ctx, &crate::config::Appearance::default());
        let mut input = input(0.0);
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        ));
        let mut height = 0.0;
        run(&ctx, input, |ui| {
            let top = ui.cursor().min.y;
            let message = Message {
                id: "m1".into(),
                platform: crate::model::Platform::Twitch,
                channel: "test".into(),
                author: crate::model::Author {
                    name: "tester".into(),
                    color: None,
                },
                parts: vec![Part::Text("hi".into())],
                ts_ms: 1,
            };
            message_ui(ui, &message, "test", &crate::config::Appearance::default());
            height = ui.cursor().min.y - top;
        });
        assert!(
            height.is_finite() && height < 100.0,
            "a one-line message row took {height} points"
        );
    }

    #[test]
    fn offscreen_emote_does_not_ask_for_an_immediate_repaint() {
        let ctx = egui::Context::default();
        crate::settings::apply(&ctx, &crate::config::Appearance::default());
        // Two settle passes: egui asks for its own repaint on the first pass, so
        // a later pass is the only one that reports the app's own requests.
        run(&ctx, input(0.0), |_ui| {});
        run(&ctx, input(0.1), |_ui| {});
        let output = run(&ctx, input(0.2), |ui| {
            // Push the emote below the 200px screen of `input`.
            ui.add_space(500.0);
            paint_emote(ui, &emote(), egui::vec2(64.0, 64.0), 0.0);
        });
        let delay = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .expect("root viewport output")
            .repaint_delay;
        assert!(
            delay >= Duration::from_millis(500),
            "offscreen emote asked for a repaint in {delay:?}"
        );
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
