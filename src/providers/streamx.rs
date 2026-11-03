use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::config::PanelCfg;
use crate::model::{Author, EmotePart, Message, Part, Platform};
use crate::providers::{AbortOnDrop, ConnectionStatus, ProviderEvent};
use crate::util::parse_hex_color;

const PING_EVERY: Duration = Duration::from_secs(25);
const SILENCE_LIMIT: Duration = Duration::from_secs(90);

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
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .connect_timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
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
        let channel = channel_id.to_string();
        Ok(items
            .iter()
            .filter_map(|value| map_message(value, &channel))
            .collect())
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

pub fn map_message(value: &Value, channel: &str) -> Option<Message> {
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
                let aspect = (width / height) as f32;
                let aspect = if aspect.is_finite() {
                    aspect.clamp(0.1, 8.0)
                } else {
                    1.0
                };
                parts.push(Part::Emote(EmotePart {
                    name: segment["emote"]["name"]
                        .as_str()
                        .unwrap_or(content)
                        .to_string(),
                    uri: normalize_emote_url(url),
                    aspect,
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
        channel: channel.to_string(),
        author: Author {
            name: sender.to_string(),
            color,
        },
        parts,
        ts_ms,
    })
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
    let connected = Arc::new(AtomicBool::new(false));
    loop {
        if tx.is_closed() {
            return;
        }
        let _ = tx.send(ProviderEvent::Status(index, ConnectionStatus::Connecting));
        ctx.request_repaint();
        match session(index, &spec, &client, &tx, &ctx, &connected).await {
            Ok(()) => {}
            Err(err) => {
                tracing::warn!("streamx: {err:#}");
                let _ = tx.send(ProviderEvent::Error(index, format!("{err:#}")));
                if connected.swap(false, Ordering::Relaxed) {
                    let _ = tx.send(ProviderEvent::Note(
                        "соединение с streamx потеряно, часть сообщений могла пропасть"
                            .to_string(),
                    ));
                }
            }
        }
        if tx.is_closed() {
            return;
        }
        if !connected.load(Ordering::Relaxed) {
            attempt += 1;
        } else {
            attempt = 0;
            connected.store(false, Ordering::Relaxed);
        }
        let _ = tx.send(ProviderEvent::Status(index, ConnectionStatus::Reconnecting));
        ctx.request_repaint();
        tokio::time::sleep(crate::providers::twitch::next_backoff(attempt)).await;
    }
}

async fn session(
    index: usize,
    spec: &PanelCfg,
    client: &StreamxClient,
    tx: &UnboundedSender<ProviderEvent>,
    ctx: &egui::Context,
    connected: &AtomicBool,
) -> anyhow::Result<()> {
    let channel_id = client.channel_id(&spec.channel).await?;

    let ws_url = client.ws_url()?;
    let mut request = ws_url.into_client_request()?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        tokio_tungstenite::tungstenite::http::HeaderValue::from_static("graphql-transport-ws"),
    );
    let (mut ws, _) = tokio::time::timeout(
        Duration::from_secs(20),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|_| anyhow::anyhow!("таймаут подключения к streamx"))??;

    ws.send(WsMessage::Text(
        json!({ "type": "connection_init", "payload": {} })
            .to_string()
            .into(),
    ))
    .await?;

    // Wait for the protocol handshake before subscribing.
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(20), ws.next())
            .await
            .map_err(|_| anyhow::anyhow!("streamx не ответил на connection_init"))?;
        let Some(frame) = frame else {
            anyhow::bail!("streamx закрыл соединение");
        };
        let text = text_frame(frame?)?;
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => continue,
        };
        match value["type"].as_str().unwrap_or_default() {
            "connection_ack" => break,
            "ping" => {
                ws.send(WsMessage::Text(json!({ "type": "pong" }).to_string().into()))
                    .await?;
            }
            "error" => anyhow::bail!("streamx handshake error: {}", value["payload"]),
            _ => {}
        }
    }

    ws.send(WsMessage::Text(subscription_frame(&channel_id).into()))
        .await?;
    connected.store(true, Ordering::Relaxed);
    let _ = tx.send(ProviderEvent::Status(index, ConnectionStatus::Syncing));
    ctx.request_repaint();

    // Subscribe first, then backfill in the background. Frames that arrive
    // while history is in flight are handled by the read loop below and merged
    // by timestamp.
    let history_done = Arc::new(AtomicBool::new(false));
    let history_job = AbortOnDrop(tokio::spawn({
        let client = client.clone();
        let channel_id = channel_id.clone();
        let tx = tx.clone();
        let ctx = ctx.clone();
        let history_done = Arc::clone(&history_done);
        async move {
            match client.history(&channel_id, 500).await {
                Ok(history) => {
                    let _ = tx.send(ProviderEvent::History(history));
                    ctx.request_repaint();
                }
                Err(err) => tracing::warn!("streamx history: {err:#}"),
            }
            history_done.store(true, Ordering::Relaxed);
            ctx.request_repaint();
        }
    }));
    let mut ready_sent = false;

    let _ = &spec;
    let mut last_frame = Instant::now();
    let mut next_ping = Instant::now() + PING_EVERY;
    loop {
        let now = Instant::now();
        if now.duration_since(last_frame) > SILENCE_LIMIT {
            anyhow::bail!("streamx молчит дольше {} секунд", SILENCE_LIMIT.as_secs());
        }
        if now >= next_ping {
            ws.send(WsMessage::Text(json!({ "type": "ping" }).to_string().into()))
                .await?;
            next_ping = now + PING_EVERY;
        }

        // The read loop owns the ready transition, so a backfill that outlived
        // its session cannot report ready for a newer one.
        if !ready_sent && (history_done.load(Ordering::Relaxed) || history_job.0.is_finished()) {
            ready_sent = true;
            let _ = tx.send(ProviderEvent::Status(index, ConnectionStatus::Connected));
            ctx.request_repaint();
        }

        let wait = if ready_sent {
            (next_ping - now).max(Duration::from_secs(1))
        } else {
            Duration::from_secs(1)
        };
        let frame = match tokio::time::timeout(wait, ws.next()).await {
            Err(_) => continue,
            Ok(None) => anyhow::bail!("streamx закрыл соединение"),
            Ok(Some(frame)) => frame?,
        };
        last_frame = Instant::now();

        let text = text_frame(frame)?;
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => continue,
        };

        match value["type"].as_str().unwrap_or_default() {
            "next" => {
                if let Some(message) = map_message(
                    &value["payload"]["data"]["chatMessages"],
                    &channel_id,
                ) {
                    let _ = tx.send(ProviderEvent::Message(message));
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

fn text_frame(frame: WsMessage) -> anyhow::Result<String> {
    match frame {
        WsMessage::Text(text) => Ok(text.to_string()),
        WsMessage::Binary(bytes) => Ok(String::from_utf8_lossy(&bytes).to_string()),
        WsMessage::Close(_) => anyhow::bail!("streamx закрыл соединение"),
        _ => Ok(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("fixtures/streamx_messages.json")).unwrap()
    }

    #[test]
    fn maps_segments_and_protocol_relative_urls() {
        let v = fixture();
        let msgs: Vec<Message> = v
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|value| map_message(value, "le-xot"))
            .collect();
        assert_eq!(msgs[0].author.name, "zxcpuk");
        assert_eq!(msgs[0].author.color, Some([0xFF, 0xB8, 0x6C]));
        assert_eq!(msgs[0].channel, "le-xot");
        assert!(matches!(&msgs[0].parts[0], Part::Text(t) if t == "а сам файсит"));
        match &msgs[1].parts[0] {
            Part::Emote(e) => {
                assert!(e.uri.starts_with("https://cdn.7tv.app/"));
                assert_eq!(e.name, "aRolf");
                assert_eq!(e.aspect, 2.0);
            }
            other => panic!("expected emote, got {other:?}"),
        }
        assert!(matches!(&msgs[2].parts[1], Part::Text(t) if t == " смотри "));
        assert!(
            matches!(&msgs[2].parts[2], Part::Link(u) if u == "https://github.com/emilk/egui")
        );
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

    #[tokio::test]
    #[ignore = "живой запрос к streamx"]
    async fn live_channel_and_history() {
        let client = StreamxClient::new("https://streamx.satont.dev");
        let id = client.channel_id("le-xot").await.unwrap();
        assert_eq!(id, "30b9ec57-9ef7-4992-b114-f0f380bc60f6");
        let history = client.history(&id, 5).await.unwrap();
        assert!(!history.is_empty());
        assert!(
            history
                .iter()
                .all(|message| message.platform == Platform::Streamx)
        );
    }
}
