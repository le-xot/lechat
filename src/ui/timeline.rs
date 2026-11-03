use std::collections::{HashMap, VecDeque};

use crate::config::Appearance;
use crate::model::{Entry, Message, Platform, RowKey};

pub const MAX_ENTRIES: usize = 500;

/// Height assumed for a row that has never been laid out. Only the scrollbar
/// and the space above the viewport depend on it.
pub const DEFAULT_ROW_HEIGHT: f32 = 26.0;

pub struct Timeline {
    pub entries: VecDeque<Entry>,
    pub unread: usize,
    pub follow: bool,
    /// Measured row heights, so a redraw only has to lay out the visible rows.
    heights: HashMap<RowKey, f32>,
    /// Typical row height, taken from the shortest measured row.
    fallback: f32,
    measured_appearance: Option<Appearance>,
    measured_width: Option<f32>,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            unread: 0,
            follow: true,
            heights: HashMap::new(),
            fallback: DEFAULT_ROW_HEIGHT,
            measured_appearance: None,
            measured_width: None,
        }
    }
}

impl Timeline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_sorted(&mut self, entry: Entry) {
        if let Entry::Msg(message) = &entry
            && self.contains(message.platform, &message.id)
        {
            return;
        }
        let ts = entry.ts_ms();
        let position = self
            .entries
            .iter()
            .position(|existing| existing.ts_ms() > ts)
            .unwrap_or(self.entries.len());
        self.entries.insert(position, entry);
        while self.entries.len() > MAX_ENTRIES {
            if let Some(dropped) = self.entries.pop_front() {
                self.heights.remove(&dropped.key());
            }
        }
    }

    /// Height of a row in points, measured or typical.
    pub fn height(&self, key: &RowKey) -> f32 {
        self.heights.get(key).copied().unwrap_or(self.fallback)
    }

    pub fn set_height(&mut self, key: RowKey, height: f32) {
        if self.heights.is_empty() {
            self.fallback = height;
        } else {
            self.fallback = self.fallback.min(height);
        }
        self.heights.insert(key, height);
    }

    #[allow(dead_code)]
    pub fn measured_rows(&self) -> usize {
        self.heights.len()
    }

    /// Height used for rows that have not been laid out yet.
    #[allow(dead_code)]
    pub fn fallback_height(&self) -> f32 {
        self.fallback
    }

    /// True while so few rows are measured that heights would be a guess, so
    /// this frame should lay out the whole feed once and fill the cache.
    pub fn needs_full_measure(&self) -> bool {
        self.entries.len() > self.heights.len() * 2
    }

    /// Drops measured heights when the layout they were measured at changes.
    pub fn apply_layout(&mut self, appearance: &Appearance, width: f32) {
        let changed = self.measured_width.is_none_or(|known| (known - width).abs() > 0.5)
            || self
                .measured_appearance
                .as_ref()
                .is_none_or(|known| known != appearance);
        if !changed {
            return;
        }
        self.measured_width = Some(width);
        self.measured_appearance = Some(appearance.clone());
        self.heights.clear();
        self.fallback = DEFAULT_ROW_HEIGHT;
    }

    pub fn push_note(&mut self, text: String, ts_ms: i64) {
        if let Some(Entry::Note {
            text: last,
            ts_ms: last_ts,
        }) = self.entries.back()
            && *last == text
            && ts_ms.saturating_sub(*last_ts) < 60_000
        {
            return;
        }
        self.insert_sorted(Entry::Note { text, ts_ms });
    }

    pub fn contains(&self, platform: Platform, id: &str) -> bool {
        self.entries.iter().any(|entry| {
            entry
                .message()
                .is_some_and(|message| message.platform == platform && message.id == id)
        })
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[allow(dead_code)]
    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.entries.iter().filter_map(Entry::message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Author, Part};

    fn msg_at(platform: Platform, id: &str, ts: i64) -> Message {
        Message {
            id: id.to_string(),
            platform,
            channel: "test".into(),
            author: Author {
                name: "tester".into(),
                color: None,
            },
            parts: vec![Part::Text("hi".into())],
            ts_ms: ts,
        }
    }

    #[test]
    fn timeline_keeps_chronological_order() {
        let mut t = Timeline::new();
        t.insert_sorted(Entry::Msg(msg_at(Platform::Streamx, "b", 200)));
        t.insert_sorted(Entry::Msg(msg_at(Platform::Twitch, "a", 100)));
        t.insert_sorted(Entry::Msg(msg_at(Platform::Twitch, "c", 300)));
        let ids: Vec<_> = t.messages().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn timeline_dedupes_by_platform_and_id() {
        let mut t = Timeline::new();
        t.insert_sorted(Entry::Msg(msg_at(Platform::Twitch, "1", 100)));
        t.insert_sorted(Entry::Msg(msg_at(Platform::Streamx, "1", 101)));
        t.insert_sorted(Entry::Msg(msg_at(Platform::Twitch, "1", 102)));
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn measured_heights_are_kept_per_row() {
        let mut t = Timeline::new();
        t.insert_sorted(Entry::Msg(msg_at(Platform::Twitch, "a", 100)));
        let key = t.entries[0].key();
        assert_eq!(t.height(&key), DEFAULT_ROW_HEIGHT, "no measurement yet");

        t.set_height(key.clone(), 42.0);
        assert_eq!(t.height(&key), 42.0);
        assert_eq!(
            t.height(&RowKey::Note(1, "нет такого".into())),
            t.fallback_height(),
            "unknown rows use the typical measured height"
        );
    }

    #[test]
    fn evicted_rows_drop_their_measured_heights() {
        let mut t = Timeline::new();
        for i in 0..MAX_ENTRIES {
            t.insert_sorted(Entry::Msg(msg_at(Platform::Twitch, &format!("m{i}"), i as i64)));
        }
        for entry in t.entries.clone() {
            t.set_height(entry.key(), 30.0);
        }
        assert_eq!(t.measured_rows(), MAX_ENTRIES);

        t.insert_sorted(Entry::Msg(msg_at(Platform::Twitch, "new", 10_000)));
        let evicted = RowKey::Message(Platform::Twitch, "m0".into());
        assert!(!t.entries.iter().any(|entry| entry.key() == evicted));
        assert_eq!(
            t.measured_rows(),
            MAX_ENTRIES - 1,
            "heights of evicted rows are forgotten, the new row is not measured yet"
        );
        assert_eq!(t.height(&evicted), t.fallback_height());
    }

    #[test]
    fn a_mostly_unmeasured_feed_asks_for_a_full_pass() {
        let mut t = Timeline::new();
        for i in 0..10 {
            t.insert_sorted(Entry::Msg(msg_at(Platform::Twitch, &format!("m{i}"), i as i64)));
        }
        assert!(t.needs_full_measure());

        for entry in t.entries.clone() {
            t.set_height(entry.key(), 20.0);
        }
        assert!(!t.needs_full_measure());
    }

    #[test]
    fn changed_appearance_or_width_resets_measured_heights() {
        let mut t = Timeline::new();
        t.insert_sorted(Entry::Msg(msg_at(Platform::Twitch, "a", 1)));
        let key = t.entries[0].key();
        let appearance = crate::config::Appearance::default();

        t.apply_layout(&appearance, 800.0);
        t.set_height(key.clone(), 30.0);
        t.apply_layout(&appearance, 900.0);
        assert_eq!(t.height(&key), DEFAULT_ROW_HEIGHT, "width changed");

        t.set_height(key.clone(), 30.0);
        t.apply_layout(&appearance, 900.0);
        assert_eq!(t.height(&key), 30.0, "same layout keeps the cache");

        let bigger = crate::config::Appearance {
            font_size: 20.0,
            ..crate::config::Appearance::default()
        };
        t.apply_layout(&bigger, 900.0);
        assert_eq!(t.height(&key), DEFAULT_ROW_HEIGHT, "font changed");
    }

    #[test]
    fn consecutive_identical_notes_collapse() {
        let mut t = Timeline::new();
        t.push_note("связь потеряна".into(), 1_000);
        t.push_note("связь потеряна".into(), 2_000);
        assert_eq!(t.len(), 1);
        t.push_note("связь потеряна".into(), 120_000);
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn timeline_caps_at_500() {
        let mut t = Timeline::new();
        for i in 0..501 {
            t.insert_sorted(Entry::Msg(msg_at(
                Platform::Twitch,
                &format!("m{i}"),
                i as i64,
            )));
        }
        assert_eq!(t.len(), MAX_ENTRIES);
        assert!(!t.messages().any(|m| m.id == "m0"));
    }
}
