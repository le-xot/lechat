use std::collections::HashMap;

pub mod seventv;

use crate::model::{EmotePart, NativeEmote, Part};

#[derive(Debug, Clone, PartialEq)]
pub struct EmoteInfo {
    pub name: String,
    pub uri: String,
    pub aspect: f32,
    pub zero_width: bool,
}

#[derive(Debug, Clone, Default)]
pub struct EmoteCatalog {
    emotes: HashMap<String, EmoteInfo>,
}

impl EmoteCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, emote: EmoteInfo) {
        self.emotes.insert(emote.name.clone(), emote);
    }

    pub fn get(&self, name: &str) -> Option<&EmoteInfo> {
        self.emotes.get(name)
    }

    /// Global set first, channel set on top (channel wins on name collisions).
    pub fn merged(channel: Vec<EmoteInfo>, global: Vec<EmoteInfo>) -> Self {
        let mut catalog = Self::new();
        for emote in global {
            catalog.insert(emote);
        }
        for emote in channel {
            catalog.insert(emote);
        }
        catalog
    }
}

/// Split message text into text / emote / link parts.
///
/// `native` carries spans (in characters, inclusive) of platform emotes whose
/// ranges were given by the platform itself; they always win.
pub fn split_parts(text: &str, native: &[NativeEmote], catalog: &EmoteCatalog) -> Vec<Part> {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();

    let mut owner: Vec<Option<usize>> = vec![None; len];
    for (idx, emote) in native.iter().enumerate() {
        if emote.start > emote.end {
            continue;
        }
        for slot in owner.iter_mut().take((emote.end + 1).min(len)).skip(emote.start) {
            *slot = Some(idx);
        }
    }

    let mut parts: Vec<Part> = Vec::new();
    let mut i = 0;
    while i < len {
        if chars[i].is_whitespace() {
            let start = i;
            while i < len && chars[i].is_whitespace() {
                i += 1;
            }
            push_text(&mut parts, chars[start..i].iter().collect());
        } else if let Some(emote_idx) = owner[i] {
            let start = i;
            while i < len && owner[i] == Some(emote_idx) {
                i += 1;
            }
            debug_assert!(i > start);
            parts.push(Part::Emote(EmotePart {
                uri: native[emote_idx].uri.clone(),
                aspect: 1.0,
                zero_width: false,
            }));
        } else {
            let start = i;
            while i < len && !chars[i].is_whitespace() && owner[i].is_none() {
                i += 1;
            }
            // `owner[i]` is set: the loop stopped on a platform emote, so take
            // that char as text to avoid an infinite loop.
            if i == start {
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            emit_token(&mut parts, &token, catalog);
        }
    }
    parts
}

fn push_text(parts: &mut Vec<Part>, text: String) {
    if text.is_empty() {
        return;
    }
    if let Some(Part::Text(prev)) = parts.last_mut() {
        prev.push_str(&text);
    } else {
        parts.push(Part::Text(text));
    }
}

fn emit_token(parts: &mut Vec<Part>, token: &str, catalog: &EmoteCatalog) {
    if let Some(emote) = catalog.get(token) {
        parts.push(Part::Emote(EmotePart {
            uri: emote.uri.clone(),
            aspect: emote.aspect,
            zero_width: emote.zero_width,
        }));
        return;
    }

    let core_len = token
        .trim_end_matches(|c: char| TRAILING_PUNCT.contains(&c))
        .chars()
        .count();
    if core_len > 0 && core_len < token.chars().count() {
        let core: String = token.chars().take(core_len).collect();
        if let Some(emote) = catalog.get(&core) {
            parts.push(Part::Emote(EmotePart {
                uri: emote.uri.clone(),
                aspect: emote.aspect,
                zero_width: emote.zero_width,
            }));
            let tail: String = token.chars().skip(core_len).collect();
            push_text(parts, tail);
            return;
        }
    }

    emit_text_with_links(parts, token);
}

fn emit_text_with_links(parts: &mut Vec<Part>, text: &str) {
    let mut rest = text;
    loop {
        let found = ["https://", "http://"]
            .iter()
            .filter_map(|scheme| rest.find(scheme))
            .min();
        let Some(pos) = found else {
            push_text(parts, rest.to_string());
            break;
        };
        push_text(parts, rest[..pos].to_string());
        let after = &rest[pos..];
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        let candidate = &after[..end];
        let trimmed = candidate.trim_end_matches(|c: char| TRAILING_PUNCT.contains(&c));
        if trimmed.len() <= "https://".len() {
            push_text(parts, candidate.to_string());
            rest = &after[end..];
            continue;
        }
        parts.push(Part::Link(trimmed.to_string()));
        rest = &after[trimmed.len()..];
    }
}

const TRAILING_PUNCT: &[char] = &[
    ',', '.', '!', '?', ';', ':', ')', ']', '}', '"', '\'', '»', '…',
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Platform;

    fn cat(entries: &[(&str, &str, f32)]) -> EmoteCatalog {
        let mut c = EmoteCatalog::new();
        for (name, uri, aspect) in entries {
            c.insert(EmoteInfo {
                name: (*name).into(),
                uri: (*uri).into(),
                aspect: *aspect,
                zero_width: false,
            });
        }
        c
    }

    #[test]
    fn matches_whole_token_only() {
        let c = cat(&[("catKISS", "https://cdn/x.webp", 1.0)]);
        assert_eq!(
            split_parts("catKISSx", &[], &c),
            vec![Part::Text("catKISSx".into())]
        );
    }

    #[test]
    fn matches_token_next_to_punctuation() {
        let c = cat(&[("catKISS", "https://cdn/x.webp", 1.0)]);
        let parts = split_parts("ну catKISS, да", &[], &c);
        assert_eq!(parts[0], Part::Text("ну ".into()));
        assert!(matches!(parts[1], Part::Emote(_)));
        assert_eq!(parts[2], Part::Text(", да".into()));
    }

    #[test]
    fn native_emote_wins_over_catalog() {
        let c = cat(&[("Kappa", "https://cdn/7tv.webp", 1.0)]);
        let native = vec![NativeEmote {
            start: 0,
            end: 4,
            uri: "https://cdn/twitch/1.0".into(),
        }];
        match &split_parts("Kappa", &native, &c)[0] {
            Part::Emote(e) => assert_eq!(e.uri, "https://cdn/twitch/1.0"),
            other => panic!("expected emote, got {other:?}"),
        }
    }

    #[test]
    fn autolinks_plain_urls_and_trims_punctuation() {
        let parts = split_parts(
            "клип https://clips.twitch.tv/abc, смотри",
            &[],
            &EmoteCatalog::new(),
        );
        assert!(
            parts
                .iter()
                .any(|p| matches!(p, Part::Link(u) if u == "https://clips.twitch.tv/abc"))
        );
    }

    #[test]
    fn cyrillic_offsets_are_char_based() {
        let native = vec![NativeEmote {
            start: 7,
            end: 11,
            uri: "https://cdn/twitch/2.0".into(),
        }];
        let parts = split_parts("привет Kappa", &native, &EmoteCatalog::new());
        assert_eq!(parts[0], Part::Text("привет ".into()));
        assert!(matches!(parts[1], Part::Emote(_)));
    }

    #[test]
    fn wide_and_zero_width_are_preserved() {
        let mut c = EmoteCatalog::new();
        c.insert(EmoteInfo {
            name: "privet".into(),
            uri: "u".into(),
            aspect: 3.0,
            zero_width: true,
        });
        match &split_parts("privet", &[], &c)[0] {
            Part::Emote(e) => {
                assert_eq!(e.aspect, 3.0);
                assert!(e.zero_width);
            }
            other => panic!("expected emote, got {other:?}"),
        }
    }

    #[test]
    fn platform_badges_have_expected_colors() {
        assert_eq!(Platform::Twitch.badge_letter(), "T");
        assert_eq!(Platform::Twitch.badge_color(), [0x91, 0x46, 0xFF]);
        assert_eq!(Platform::Streamx.badge_letter(), "S");
        assert_eq!(Platform::Streamx.badge_color(), [0x00, 0xDC, 0x82]);
    }

    #[test]
    fn emote_part_helper_is_constructible() {
        let part = EmotePart {
            uri: "u".into(),
            aspect: 1.0,
            zero_width: false,
        };
        assert_eq!(part.aspect, 1.0);
    }
}
