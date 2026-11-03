use std::sync::Arc;

use crate::config::Appearance;

/// Text styles derived from one base size, so timestamps and status lines
/// scale together with the message text.
pub fn style_sizes(base: f32) -> Vec<(egui::TextStyle, f32)> {
    let base = base.clamp(11.0, 24.0);
    vec![
        (egui::TextStyle::Small, (base - 3.0).max(8.0)),
        (egui::TextStyle::Body, base),
        (egui::TextStyle::Monospace, (base - 1.0).max(8.0)),
        (egui::TextStyle::Button, base),
        (egui::TextStyle::Heading, base + 4.0),
    ]
}

pub fn apply(ctx: &egui::Context, appearance: &Appearance) {
    let mut definitions = egui::FontDefinitions::default();
    if let Some(family) = &appearance.font_family
        && let Some((bytes, index)) = load_family(family)
    {
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        definitions
            .font_data
            .insert("lechat-user".to_string(), Arc::new(data));
        definitions
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "lechat-user".to_string());
    }
    ctx.set_fonts(definitions);

    ctx.set_theme(egui::Theme::Dark);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals.override_text_color = Some(egui::Color32::from_rgb(0xE4, 0xE6, 0xEA));
    // Полсекунды неподвижного курсора — и подсказка с именем эмоута появляется.
    style.interaction.tooltip_delay = 0.5;
    for (text_style, size) in style_sizes(appearance.font_size) {
        let family = if text_style == egui::TextStyle::Monospace {
            egui::FontFamily::Monospace
        } else {
            egui::FontFamily::Proportional
        };
        style
            .text_styles
            .insert(text_style, egui::FontId::new(size, family));
    }
    ctx.set_style_of(egui::Theme::Dark, style);
}

/// Family names of installed fonts that have Cyrillic glyphs and usable
/// outlines, sorted case-insensitively.
pub fn available_fonts() -> Vec<String> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();

    let mut names = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for face in db.faces() {
        let Some((family, _)) = face.families.first() else {
            continue;
        };
        if !seen.insert(family.clone()) {
            continue;
        }
        let usable = db
            .with_face_data(face.id, supports_cyrillic)
            .unwrap_or(false);
        if usable {
            names.push(family.clone());
        }
    }
    names.sort_by_key(|name| name.to_lowercase());
    names
}

pub fn load_family(name: &str) -> Option<(Vec<u8>, u32)> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let query = fontdb::Query {
        families: &[fontdb::Family::Name(name)],
        ..Default::default()
    };
    let id = db.query(&query)?;
    db.with_face_data(id, |data, index| (data.to_vec(), index))
}

fn supports_cyrillic(data: &[u8], index: u32) -> bool {
    let Ok(face) = ttf_parser::Face::parse(data, index) else {
        return false;
    };
    face.glyph_index('Я').is_some() && face.glyph_index('ё').is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_scale_from_base() {
        let sizes = style_sizes(18.0);
        let get = |style: egui::TextStyle| {
            sizes
                .iter()
                .find(|(candidate, _)| *candidate == style)
                .map(|(_, size)| *size)
                .unwrap()
        };
        assert_eq!(get(egui::TextStyle::Body), 18.0);
        assert_eq!(get(egui::TextStyle::Small), 15.0);
        assert!(get(egui::TextStyle::Heading) > get(egui::TextStyle::Body));
        assert!(get(egui::TextStyle::Small) < get(egui::TextStyle::Body));
    }

    #[test]
    fn sizes_are_clamped() {
        let sizes = style_sizes(3.0);
        let body = sizes
            .iter()
            .find(|(style, _)| *style == egui::TextStyle::Body)
            .unwrap()
            .1;
        assert_eq!(body, 11.0);
    }

    #[test]
    fn system_font_list_is_usable() {
        let fonts = available_fonts();
        assert!(!fonts.is_empty(), "не найдено ни одного шрифта с кириллицей");
        assert!(
            fonts
                .windows(2)
                .all(|pair| pair[0].to_lowercase() <= pair[1].to_lowercase())
        );
        if let Some(first) = fonts.first() {
            let loaded = load_family(first);
            assert!(loaded.is_some(), "не удалось загрузить {first}");
            let (bytes, _) = loaded.unwrap();
            assert!(!bytes.is_empty());
        }
    }

    #[test]
    fn apply_works_for_builtin_real_and_missing_font() {
        let ctx = egui::Context::default();
        apply(&ctx, &Appearance::default());
        apply(
            &ctx,
            &Appearance {
                font_size: 18.0,
                font_family: Some("DejaVu Sans".into()),
                ..Appearance::default()
            },
        );
        apply(
            &ctx,
            &Appearance {
                font_size: 18.0,
                font_family: Some("нет-такого-шрифта".into()),
                ..Appearance::default()
            },
        );
    }

    #[test]
    fn missing_family_falls_back() {
        assert!(load_family("Определённо-нет-такого-шрифта").is_none());
    }
}
