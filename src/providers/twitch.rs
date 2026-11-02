use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::emotes::{EmoteCatalog, split_parts};
use crate::model::{Author, Message, NativeEmote, Platform};
use crate::providers::{ConnectionStatus, ProviderEvent};
use crate::twitch::irc::{IrcLine, is_ping, parse_emotes_tag, parse_line, parse_room_id};

const IRC_URL: &str = "wss://irc-ws.chat.twitch.tv:443";

pub fn handshake_lines(channel: &str, nick: &str) -> Vec<String> {
    vec![
        format!("PASS oauth:{nick}"),
        format!("NICK {nick}"),
        "CAP REQ :twitch.tv/tags twitch.tv/commands".to_string(),
        format!("JOIN #{}", channel.trim_start_matches('#').to_lowercase()),
    ]
}

pub fn next_backoff(attempt: u32) -> Duration {
    let secs = 1u64 << attempt.min(5);
    Duration::from_secs(secs.min(30))
}

pub fn line_to_message(line: &IrcLine, catalog: &EmoteCatalog) -> Option<Message> {
    if !line.command.eq_ignore_ascii_case("PRIVMSG") {
        return None;
    }
    let text = line.params.get(1)?.trim_end_matches('\u{1}');
    if text.is_empty() {
        return None;
    }

    let native: Vec<NativeEmote> = parse_emotes_tag(
        line.tags
            .get("emotes")
            .map(String::as_str)
            .unwrap_or_default(),
    )
    .into_iter()
    .flat_map(|(emote_id, ranges)| {
        ranges.into_iter().map(move |(start, end)| NativeEmote {
            start,
            end,
            uri: format!("https://static-cdn.jtvnw.net/emoticons/v2/{emote_id}/default/dark/1.0"),
        })
    })
    .collect();

    let ts_ms = line
        .tags
        .get("tmi-sent-ts")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

    let author_name = line
        .tags
        .get("display-name")
        .filter(|value| !value.is_empty())
        .cloned()
        .or_else(|| {
            line.prefix
                .as_deref()
                .and_then(|prefix| prefix.split('!').next())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string());

    let id = line
        .tags
        .get("id")
        .cloned()
        .unwrap_or_else(|| format!("tw-{ts_ms}-{author_name}"));

    let color = line
        .tags
        .get("color")
        .and_then(|value| parse_hex_color(value));

    Some(Message {
        id,
        platform: Platform::Twitch,
        author: Author {
            name: author_name,
            color,
        },
        parts: split_parts(text, &native, catalog),
        ts_ms,
    })
}

pub fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    let hex = value.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some([r, g, b])
}

pub async fn run(
    index: usize,
    channel: String,
    catalog: Arc<RwLock<EmoteCatalog>>,
    tx: UnboundedSender<ProviderEvent>,
    ctx: egui::Context,
) {
    let mut attempt = 0u32;
    loop {
        if tx.is_closed() {
            return;
        }
        let _ = tx.send(ProviderEvent::Status(index, ConnectionStatus::Connecting));
        match session(index, &channel, &catalog, &tx, &ctx).await {
            Ok(()) => attempt = 0,
            Err(err) => {
                let _ = tx.send(ProviderEvent::Error(index, format!("{err:#}")));
            }
        }
        if tx.is_closed() {
            return;
        }
        attempt += 1;
        let _ = tx.send(ProviderEvent::Status(index, ConnectionStatus::Reconnecting));
        tokio::time::sleep(next_backoff(attempt)).await;
    }
}

async fn session(
    index: usize,
    channel: &str,
    catalog: &Arc<RwLock<EmoteCatalog>>,
    tx: &UnboundedSender<ProviderEvent>,
    ctx: &egui::Context,
) -> anyhow::Result<()> {
    let (mut ws, _) = tokio_tungstenite::connect_async(IRC_URL).await?;
    let nick = format!("justinfan{}", std::process::id());
    for line in handshake_lines(channel, &nick) {
        ws.send(WsMessage::Text(line.into())).await?;
    }

    let mut buffer = String::new();
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(150), ws.next())
            .await
            .map_err(|_| anyhow::anyhow!("таймаут соединения с Twitch"))?;
        let Some(frame) = frame else {
            anyhow::bail!("Twitch закрыл соединение");
        };
        let frame = frame?;
        let text = match frame {
            WsMessage::Text(text) => text.to_string(),
            WsMessage::Binary(bytes) => String::from_utf8_lossy(&bytes).to_string(),
            WsMessage::Close(_) => anyhow::bail!("Twitch закрыл соединение"),
            _ => continue,
        };

        buffer.push_str(&text);
        while let Some(pos) = buffer.find("\r\n") {
            let raw: String = buffer.drain(..pos + 2).collect();
            let Some(line) = parse_line(&raw) else {
                continue;
            };

            if is_ping(&line) {
                ws.send(WsMessage::Text("PONG :tmi.twitch.tv".into()))
                    .await?;
                continue;
            }

            if line.command == "366" {
                let _ = tx.send(ProviderEvent::Status(index, ConnectionStatus::Connected));
                ctx.request_repaint();
                continue;
            }

            if let Some(room_id) = parse_room_id(&line) {
                let _ = tx.send(ProviderEvent::RoomId(index, room_id));
                ctx.request_repaint();
                continue;
            }

            if line.command == "RECONNECT" {
                anyhow::bail!("Twitch попросил переподключиться");
            }

            let catalog = catalog.read().unwrap();
            if let Some(message) = line_to_message(&line, &catalog) {
                drop(catalog);
                let _ = tx.send(ProviderEvent::Message(message));
                ctx.request_repaint();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Part;

    #[test]
    fn handshake_is_anonymous_read_only() {
        let lines = handshake_lines("etonelexot", "justinfan12345");
        assert_eq!(lines[0], "PASS oauth:justinfan12345");
        assert_eq!(lines[1], "NICK justinfan12345");
        assert_eq!(lines[2], "CAP REQ :twitch.tv/tags twitch.tv/commands");
        assert_eq!(lines[3], "JOIN #etonelexot");
    }

    #[test]
    fn backoff_is_capped() {
        assert_eq!(next_backoff(0), Duration::from_secs(1));
        assert_eq!(next_backoff(3), Duration::from_secs(8));
        assert_eq!(next_backoff(20), Duration::from_secs(30));
    }

    #[test]
    fn converts_privmsg_to_message() {
        let raw = "@color=#2E8B57;display-name=DavidzusTV;id=abc;tmi-sent-ts=1793656722613 :davidzustv!davidzustv@davidzustv.tmi.twitch.tv PRIVMSG #xqc :TWEAK SHIIII";
        let msg =
            line_to_message(&parse_line(raw).unwrap(), &EmoteCatalog::new()).unwrap();
        assert_eq!(msg.id, "abc");
        assert_eq!(msg.author.name, "DavidzusTV");
        assert_eq!(msg.author.color, Some([0x2e, 0x8b, 0x57]));
        assert_eq!(msg.ts_ms, 1793656722613);
        assert!(matches!(&msg.parts[0], Part::Text(t) if t == "TWEAK SHIIII"));
    }

    #[tokio::test]
    #[ignore = "живое подключение к Twitch IRC"]
    async fn live_handshake_yields_room_id() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (mut ws, _) = tokio_tungstenite::connect_async(IRC_URL).await.unwrap();
        for line in handshake_lines("etonelexot", "justinfan424242") {
            ws.send(WsMessage::Text(line.into())).await.unwrap();
        }

        let mut room_id = None;
        for _ in 0..40 {
            let frame = tokio::time::timeout(Duration::from_secs(3), ws.next())
                .await
                .expect("тишина от Twitch");
            let Some(Ok(WsMessage::Text(text))) = frame else {
                continue;
            };
            for raw in text.lines() {
                if let Some(line) = parse_line(raw)
                    && let Some(id) = parse_room_id(&line)
                {
                    room_id = Some(id);
                }
            }
            if room_id.is_some() {
                break;
            }
        }
        assert_eq!(room_id.as_deref(), Some("1263747798"));
    }
}
