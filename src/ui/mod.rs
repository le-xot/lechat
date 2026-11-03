pub mod row;
pub mod timeline;

use crate::model::Platform;

pub const NEAR_BOTTOM_PX: f32 = 64.0;
pub const EMOTE_LINE_FACTOR: f32 = 1.5;

/// Relative luminance of an sRGB colour (0..1).
pub fn luminance([r, g, b]: [u8; 3]) -> f32 {
    fn linear(channel: u8) -> f32 {
        let value = channel as f32 / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// White or near-black text that stays readable on the given background.
pub fn contrasting_text([r, g, b]: [u8; 3]) -> egui::Color32 {
    if luminance([r, g, b]) > 0.4 {
        egui::Color32::from_rgb(0x10, 0x12, 0x14)
    } else {
        egui::Color32::WHITE
    }
}

pub fn format_time(ts_ms: i64, with_seconds: bool) -> String {
    let pattern = if with_seconds { "%H:%M:%S" } else { "%H:%M" };
    chrono::DateTime::from_timestamp_millis(ts_ms)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format(pattern)
                .to_string()
        })
        .unwrap_or_else(|| "--:--".to_string())
}

pub fn emote_size(line_height: f32, aspect: f32) -> egui::Vec2 {
    let height = line_height * EMOTE_LINE_FACTOR;
    egui::vec2(height * aspect.clamp(0.1, 8.0), height)
}

pub fn near_bottom(offset_from_end: f32) -> bool {
    offset_from_end <= NEAR_BOTTOM_PX
}

pub fn zero_width_rect(
    previous: Option<egui::Rect>,
    cursor: egui::Pos2,
    line_height: f32,
) -> egui::Rect {
    match previous {
        Some(prev) => {
            let size = prev.size() * 0.6;
            egui::Rect::from_center_size(prev.center(), size)
        }
        None => {
            let side = line_height * EMOTE_LINE_FACTOR * 0.75;
            egui::Rect::from_min_size(cursor, egui::vec2(side, side))
        }
    }
}

pub fn source_badge(ui: &mut egui::Ui, platform: Platform, tooltip: &str) {
    let size = egui::vec2(18.0, 18.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let [r, g, b] = platform.badge_color();
    let background = egui::Color32::from_rgb(r, g, b);
    ui.painter().rect_filled(rect, 5.0, background);

    let text_color = contrasting_text(platform.badge_color());
    let galley = ui.painter().layout_no_wrap(
        platform.badge_letter().to_string(),
        egui::FontId::proportional(11.0),
        text_color,
    );
    let pos = rect.center() - galley.size() / 2.0 + egui::vec2(0.0, -0.5);
    ui.painter().galley(pos, galley, text_color);

    response.on_hover_text(tooltip);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_text_contrasts_with_background() {
        assert_eq!(
            contrasting_text([0x91, 0x46, 0xFF]),
            egui::Color32::WHITE
        );
        assert_eq!(
            contrasting_text([0x00, 0xDC, 0x82]),
            egui::Color32::from_rgb(0x10, 0x12, 0x14)
        );
        assert!(luminance([0, 0, 0]) < 0.01);
        assert!(luminance([255, 255, 255]) > 0.99);
    }

    #[test]
    fn formats_time_with_and_without_seconds() {
        let minutes = format_time(1_793_656_722_613, false);
        let seconds = format_time(1_793_656_722_613, true);
        assert_eq!(minutes.len(), 5);
        assert_eq!(seconds.len(), 8);
        assert_eq!(minutes.matches(':').count(), 1);
        assert_eq!(seconds.matches(':').count(), 2);
    }

    #[test]
    fn emote_keeps_aspect_ratio() {
        let s = emote_size(20.0, 3.0);
        assert_eq!(s.y, 30.0);
        assert_eq!(s.x, 90.0);
    }

    #[test]
    fn near_bottom_uses_64px_threshold() {
        assert!(near_bottom(0.0));
        assert!(near_bottom(63.0));
        assert!(!near_bottom(65.0));
    }

    #[test]
    fn zero_width_overlays_previous_emote() {
        let prev = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(90.0, 30.0));
        let r = zero_width_rect(Some(prev), egui::pos2(100.0, 0.0), 30.0);
        assert_eq!(r.center(), prev.center());
        assert!(r.width() < prev.width());
    }
}
