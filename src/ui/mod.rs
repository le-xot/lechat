pub mod row;
pub mod timeline;

use crate::model::Platform;

pub const NEAR_BOTTOM_PX: f32 = 64.0;
pub const EMOTE_LINE_FACTOR: f32 = 1.5;

pub fn emote_size(line_height: f32, aspect: f32) -> egui::Vec2 {
    let height = line_height * EMOTE_LINE_FACTOR;
    egui::vec2(height * aspect.max(0.1), height)
}

pub fn near_bottom(offset_from_end: f32) -> bool {
    offset_from_end <= NEAR_BOTTOM_PX
}

#[allow(dead_code)]
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
    let size = egui::vec2(16.0, 16.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let [r, g, b] = platform.badge_color();
    ui.painter()
        .rect_filled(rect, 4.0, egui::Color32::from_rgb(r, g, b));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        platform.badge_letter(),
        egui::FontId::proportional(10.0),
        egui::Color32::WHITE,
    );
    response.on_hover_text(tooltip);
}

#[cfg(test)]
mod tests {
    use super::*;

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
