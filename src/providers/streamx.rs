use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::config::PanelCfg;
use crate::model::{Author, EmotePart, Message, Part, Platform};
use crate::providers::{ConnectionStatus, ProviderEvent};

const CHANNEL_ID_QUERY: &str = r#"
query ChannelId($channelName: String!) {
  fetchUserByName(name: $channelName) { id name }
}
"#;

const HISTORY_QUERY: &str = r#"
query ChatMessages($channelID: UUID!, $limit: Int) {
  chatMessagesLatest(channelID: $channelID, limit: $limit) {
    id
    createdAt
    senderColor
    sender { displayName }
    segments {
      type
      content
      ... on MessageSegmentEmote {
        emote { name url width height }
      }
    }
  }
}
"#;

const SUBSCRIPTION_QUERY: &str = r#"
subscription NewChatMessages($channelID: UUID!) {
  chatMessages(channelID: $channelID) {
    id
    createdAt
    senderColor
    sender { displayName }
    segments {
      type
      content
      ... on MessageSegmentEmote {
        emote { name url width height }
      }
    }
  }
}
"#;

#[derive(Clone)]
pub struct StreamxClient {
    pub base_url: String,
    http: reqwest::Client,
}

impl StreamxClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/api/query", self.base_url)
    }

    async fn query(&self, query: &str, variables: Value) -> anyhow::Result<Value> {
        let response = self
            .http
            .post(self.endpoint())
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await?;
        let status = response.status();
        let body: Value = response.json().await?;
        if let Some(errors) = body.get("errors") {
            anyhow::bail!("streamx GraphQL: {errors}");
        }
        if !status.is_success() {
            anyhow::bail!("streamx HTTP {status}");
        }
        body.get("data")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("streamx: пустой ответ"))
    }

    pub async fn channel_id(&self, name: &str) -> anyhow::Result<String> {
        let data = self
            .query(CHANNEL_ID_QUERY, json!({ "channelName": name }))
            .await?;
        data["fetchUserByName"]["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("канал {name} не найден на streamx"))
    }

    pub async fn history(&self, channel_id: &str, limit: u32) -> anyhow::Result<Vec<Message>> {
        let data = self
            .query(
                HISTORY_QUERY,
                json!({ "channelID": channel_id, "limit": limit }),
            )
            .await?;
        let items = data["chatMessagesLatest"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(items.iter().filter_map(map_message).collect())
    }

    pub fn ws_url(&self) -> anyhow::Result<String> {
        let url = reqwest::Url::parse(&self.base_url)?;
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        let host = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("некорректный base_url"))?;
        let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
        Ok(format!("{scheme}://{host}{port}/api/query"))
    }
}

pub fn normalize_emote_url(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url.to_string()
    }
}

pub fn map_message(value: &Value) -> Option<Message> {
    let id = value["id"].as_str()?.to_string();
    let sender = value["sender"]["displayName"].as_str().unwrap_or("unknown");
    let color = value["senderColor"].as_str().and_then(parse_hex_color);

    let ts_ms = value["createdAt"]
        .as_str()
        .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
        .map(|time| time.timestamp_millis())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

    let mut parts = Vec::new();
    for segment in value["segments"].as_array().into_iter().flatten() {
        let kind = segment["type"].as_str().unwrap_or_default();
        let content = segment["content"].as_str().unwrap_or_default();
        match kind {
            "EMOTE" => {
                let url = segment["emote"]["url"].as_str().unwrap_or_default();
                if url.is_empty() {
                    parts.push(Part::Text(content.to_string()));
                    continue;
                }
                let width = segment["emote"]["width"].as_f64().unwrap_or(32.0);
                let height = segment["emote"]["height"].as_f64().unwrap_or(32.0).max(1.0);
                parts.push(Part::Emote(EmotePart {
                    uri: normalize_emote_url(url),
                    aspect: (width / height) as f32,
                    zero_width: false,
                }));
            }
            "LINK" => parts.push(Part::Link(content.to_string())),
            _ => parts.push(Part::Text(content.to_string())),
        }
    }

    Some(Message {
        id,
        platform: Platform::Streamx,
        author: Author {
            name: sender.to_string(),
            color,
        },
        parts,
        ts_ms,
    })
}

fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    let hex = value.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    Some([
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ])
}

pub fn subscription_frame(channel_id: &str) -> String {
    json!({
        "id": "1",
        "type": "subscribe",
        "payload": {
            "query": SUBSCRIPTION_QUERY,
            "variables": { "channelID": channel_id }
        }
    })
    .to_string()
}

pub async fn run(
    index: usize,
    spec: PanelCfg,
    tx: UnboundedSender<ProviderEvent>,
    ctx: egui::Context,
) {
    let client = StreamxClient::new(spec.streamx_base_url());
    let mut attempt = 0u32;
    loop {
        if tx.is_closed() {
            return;
        }
        let _ = tx.send(ProviderEvent::Status(index, ConnectionStatus::Connecting));
        match session(index, &spec, &client, &tx, &ctx).await {
            Ok(()) => attempt = 0,
            Err(err) => {
                let _ = tx.send(ProviderEvent::Error(index, format!("{err:#}")));
                let _ = tx.send(ProviderEvent::Note(
                    index,
                    "соединение с streamx потеряно, часть сообщений могла пропасть".to_string(),
                ));
            }
        }
        if tx.is_closed() {
            return;
        }
        attempt += 1;
        let _ = tx.send(ProviderEvent::Status(index, ConnectionStatus::Reconnecting));
        tokio::time::sleep(crate::providers::twitch::next_backoff(attempt)).await;
    }
}

async fn session(
    index: usize,
    spec: &PanelCfg,
    client: &StreamxClient,
    tx: &UnboundedSender<ProviderEvent>,
    ctx: &egui::Context,
) -> anyhow::Result<()> {
    let channel_id = client.channel_id(&spec.channel).await?;
    let history = client.history(&channel_id, 500).await?;
    let _ = tx.send(ProviderEvent::History(index, history));
    ctx.request_repaint();

    let (mut ws, _) = tokio_tungstenite::connect_async(client.ws_url()?).await?;
    ws.send(WsMessage::Text(
        json!({ "type": "connection_init", "payload": {} }).to_string().into(),
    ))
    .await?;

    loop {
        let frame = tokio::time::timeout(Duration::from_secs(120), ws.next())
            .await
            .map_err(|_| anyhow::anyhow!("таймаут соединения со streamx"))?;
        let Some(frame) = frame else {
            anyhow::bail!("streamx закрыл соединение");
        };
        let frame = frame?;
        let text = match frame {
            WsMessage::Text(text) => text.to_string(),
            WsMessage::Binary(bytes) => String::from_utf8_lossy(&bytes).to_string(),
            WsMessage::Close(_) => anyhow::bail!("streamx закрыл соединение"),
            _ => continue,
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => continue,
        };

        match value["type"].as_str().unwrap_or_default() {
            "connection_ack" => {
                ws.send(WsMessage::Text(subscription_frame(&channel_id).into()))
                    .await?;
                let _ = tx.send(ProviderEvent::Status(index, ConnectionStatus::Connected));
                ctx.request_repaint();
            }
            "next" => {
                if let Some(message) = map_message(&value["payload"]["data"]["chatMessages"]) {
                    let _ = tx.send(ProviderEvent::Message(index, message));
                    ctx.request_repaint();
                }
            }
            "ping" => {
                ws.send(WsMessage::Text(json!({ "type": "pong" }).to_string().into()))
                    .await?;
            }
            "error" => anyhow::bail!("streamx subscription error: {}", value["payload"]),
            "complete" => anyhow::bail!("streamx закрыл подписку"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_segments_and_protocol_relative_urls() {
        let v: Value =
            serde_json::from_str(include_str!("fixtures/streamx_messages.json")).unwrap();
        let msgs: Vec<Message> = v.as_array().unwrap().iter().filter_map(map_message).collect();
        assert_eq!(msgs[0].author.name, "zxcpuk");
        assert_eq!(msgs[0].author.color, Some([0xFF, 0xB8, 0x6C]));
        assert!(matches!(&msgs[0].parts[0], Part::Text(t) if t == "а сам файсит"));
        match &msgs[1].parts[0] {
            Part::Emote(e) => {
                assert!(e.uri.starts_with("https://cdn.7tv.app/"));
                assert_eq!(e.aspect, 2.0);
            }
            other => panic!("expected emote, got {other:?}"),
        }
        assert!(matches!(&msgs[2].parts[1], Part::Text(t) if t == " смотри "));
        assert!(matches!(&msgs[2].parts[2], Part::Link(u) if u == "https://github.com/emilk/egui"));
        assert!(matches!(&msgs[2].parts[3], Part::Text(t) if t == "!"));
    }

    #[test]
    fn subscription_frame_is_graphql_transport_ws() {
        let frame = subscription_frame("30b9ec57-9ef7-4992-b114-f0f380bc60f6");
        let v: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["type"], "subscribe");
        assert_eq!(
            v["payload"]["variables"]["channelID"],
            "30b9ec57-9ef7-4992-b114-f0f380bc60f6"
        );
    }

    #[test]
    fn builds_websocket_url_from_base() {
        let client = StreamxClient::new("https://streamx.satont.dev");
        assert_eq!(client.ws_url().unwrap(), "wss://streamx.satont.dev/api/query");
        let local = StreamxClient::new("http://localhost:3000/");
        assert_eq!(local.ws_url().unwrap(), "ws://localhost:3000/api/query");
    }
}
