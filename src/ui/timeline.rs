use std::collections::VecDeque;

use crate::model::{Entry, Message, Platform};

pub const MAX_ENTRIES: usize = 500;

pub struct Timeline {
    pub entries: VecDeque<Entry>,
    pub unread: usize,
    pub follow: bool,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            unread: 0,
            follow: true,
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
            self.entries.pop_front();
        }
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
