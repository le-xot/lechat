#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    Twitch,
    Streamx,
}

impl Platform {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Twitch => "Twitch",
            Self::Streamx => "streamx",
        }
    }

    pub fn badge_letter(&self) -> &'static str {
        match self {
            Self::Twitch => "T",
            Self::Streamx => "S",
        }
    }

    pub fn badge_color(&self) -> [u8; 3] {
        match self {
            Self::Twitch => [0x91, 0x46, 0xFF],
            Self::Streamx => [0x00, 0xDC, 0x82],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Author {
    pub name: String,
    pub color: Option<[u8; 3]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmotePart {
    pub name: String,
    pub uri: String,
    pub aspect: f32,
    pub zero_width: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text(String),
    Emote(EmotePart),
    Link(String),
}

#[derive(Debug, Clone)]
pub struct Message {
    pub id: String,
    pub platform: Platform,
    pub channel: String,
    pub author: Author,
    pub parts: Vec<Part>,
    pub ts_ms: i64,
}

#[derive(Debug, Clone)]
pub struct NativeEmote {
    pub start: usize,
    pub end: usize,
    pub uri: String,
}

#[derive(Debug, Clone)]
pub enum Entry {
    Msg(Message),
    Note {
        text: String,
        ts_ms: i64,
    },
}

/// Identity of a feed row. Stable while the entry stays in the feed, which is
/// what a height cache needs: entries are evicted from the front, so indices
/// shift but identities do not.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RowKey {
    Message(Platform, String),
    Note(i64, String),
}

impl Entry {
    pub fn key(&self) -> RowKey {
        match self {
            Self::Msg(message) => RowKey::Message(message.platform, message.id.clone()),
            Self::Note { text, ts_ms } => RowKey::Note(*ts_ms, text.clone()),
        }
    }
}

impl Entry {
    pub fn ts_ms(&self) -> i64 {
        match self {
            Self::Msg(m) => m.ts_ms,
            Self::Note { ts_ms, .. } => *ts_ms,
        }
    }

    pub fn message(&self) -> Option<&Message> {
        match self {
            Self::Msg(m) => Some(m),
            Self::Note { .. } => None,
        }
    }
}
