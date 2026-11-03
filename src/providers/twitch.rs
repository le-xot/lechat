use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::emotes::{EmoteCatalog, seventv, split_parts};
use crate::model::{Author, Message, NativeEmote, Platform};
use crate::providers::{AbortOnDrop, ConnectionStatus, ProviderEvent};
use crate::twitch::irc::{IrcLine, is_ping, parse_emotes_tag, parse_line, parse_room_id};
use crate::util::parse_hex_color;

const IRC_URL: &str = "wss://irc-ws.chat.twitch.tv:443";
/// Twitch sends its own keepalive roughly every five minutes; we ping much
/// more often and treat a long silence as a dead connection.
const PING_EVERY: Duration = Duration::from_secs(60);
const SILENCE_LIMIT: Duration = Duration::from_secs(300);
const RECENT_MESSAGES_URL: &str = "https://recent-messages.robotty.de/api/v2/recent-messages";
const HISTORY_LIMIT: u32 = 100;
/// The emote catalogue is a nice-to-have: a hung CDN must not keep a source in
/// "syncing" for long.
const CATALOG_TIMEOUT: Duration = Duration::from_secs(10);
/// ROOMSTATE normally follows JOIN in the same burst; if it never arrives we
/// still backfill instead of waiting forever.
const ROOM_ID_GRACE: Duration = Duration::from_secs(3);
/// Cap for live lines held back while 7TV loads. Beyond this chat keeps flowing
/// without emotes rather than growing without bound.
const DEFERRED_LIMIT: usize = 2000;

pub fn handshake_lines(channel: &str, nick: &str) -> Vec<String> {
    vec![
        format!("PASS oauth:{nick}"),
        format!("NICK {nick}"),
        "CAP REQ :twitch.tv/tags twitch.tv/commands".to_string(),
        format!("JOIN #{}", channel.trim_start_matches('#').to_lowercase()),
    ]
}

/// URL of the public recent-messages service used by Chatterino.
pub fn recent_messages_url(channel: &str, after_ms: Option<i64>, limit: u32) -> String {
    let channel = channel.trim_start_matches('#').to_lowercase();
    let mut url = format!("{RECENT_MESSAGES_URL}/{channel}");
    match after_ms {
        Some(after) => url.push_str(&format!("?after={after}&limit={limit}")),
        None => url.push_str(&format!("?limit={limit}")),
    }
    url
}

/// Parses the JSON payload of the recent-messages service into messages.
/// `error_code` responses (e.g. `channel_not_joined`) yield nothing.
pub fn parse_recent_messages(payload: &serde_json::Value, catalog: &EmoteCatalog) -> Vec<Message> {
    if payload
        .get("error_code")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|code| !code.is_empty())
    {
        return Vec::new();
    }
    payload["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str())
        .filter_map(parse_line)
        .filter_map(|line| line_to_message(&line, catalog))
        .collect()
}

async fn fetch_recent_payload(
    http: &reqwest::Client,
    url: &str,
) -> anyhow::Result<serde_json::Value> {
    let response = http.get(url).send().await?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("recent messages HTTP {status}");
    }
    Ok(response.json().await?)
}

async fn fetch_catalog(
    http: &reqwest::Client,
    room_id: &str,
) -> anyhow::Result<EmoteCatalog> {
    let (channel, global) = tokio::join!(
        seventv::fetch_channel(http, room_id),
        seventv::fetch_global(http)
    );
    Ok(EmoteCatalog::merged(channel?, global?))
}

async fn load_catalog(
    http: &reqwest::Client,
    room_id: &str,
    catalog: &Arc<RwLock<EmoteCatalog>>,
) {
    let needed = catalog.read().unwrap().is_empty();
    if !needed || room_id.is_empty() {
        return;
    }
    let fetched = tokio::time::timeout(CATALOG_TIMEOUT, fetch_catalog(http, room_id)).await;
    match fetched {
        Ok(Ok(loaded)) => *catalog.write().unwrap() = loaded,
        Ok(Err(err)) => tracing::warn!("7TV: {err:#}"),
        Err(_) => tracing::warn!("7TV: таймаут загрузки каталога"),
    }
}

/// Backfills recent messages; `history_done` tells the IRC loop that the feed
/// can be reported as ready. Runs as a background task so the read loop keeps
/// running; `catalog_ready` tells that loop when it may parse live messages.
#[allow(clippy::too_many_arguments)]
async fn load_history(
    channel: &str,
    room_id: &str,
    catalog: &Arc<RwLock<EmoteCatalog>>,
    tx: &UnboundedSender<ProviderEvent>,
    ctx: &egui::Context,
    last_ts: &AtomicI64,
    http: &reqwest::Client,
    catalog_ready: &AtomicBool,
    history_done: &AtomicBool,
) {
    let known = last_ts.load(Ordering::Relaxed);
    let url = recent_messages_url(channel, (known > 0).then_some(known), HISTORY_LIMIT);

    // Both downloads are independent; the slower one decides how long the
    // backfill takes instead of their sum.
    let catalog_load = async {
        load_catalog(http, room_id, catalog).await;
        catalog_ready.store(true, Ordering::Relaxed);
        ctx.request_repaint();
    };
    let ((), payload) = tokio::join!(catalog_load, fetch_recent_payload(http, &url));

    match payload {
        Ok(payload) => {
            let catalog = catalog.read().unwrap();
            let mut history = parse_recent_messages(&payload, &catalog);
            let emote_count = catalog.len();
            drop(catalog);
            if let Some(newest) = history.iter().map(|message| message.ts_ms).max() {
                last_ts.fetch_max(newest, Ordering::Relaxed);
            }
            history.sort_by_key(|message| message.ts_ms);
            if !history.is_empty() {
                tracing::info!(
                    "twitch history: {} сообщений, 7TV: {}",
                    history.len(),
                    emote_count
                );
                let _ = tx.send(ProviderEvent::History(history));
                ctx.request_repaint();
            }
        }
        Err(err) => tracing::warn!("twitch history: {err:#}"),
    }

    // The session loop owns the "online" transition: setting the flag here
    // means a backfill from an old session can never report ready for a new one.
    history_done.store(true, Ordering::Relaxed);
    ctx.request_repaint();

    // A transient 7TV failure would otherwise cost every emote for the whole
    // session. Retry once in the background now that the feed is already up.
    let catalog_empty = catalog.read().unwrap().is_empty();
    if !room_id.is_empty() && catalog_empty {
        tokio::time::sleep(Duration::from_secs(15)).await;
        load_catalog(http, room_id, catalog).await;
        ctx.request_repaint();
    }
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

    let chars: Vec<char> = text.chars().collect();
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

    let channel = line
        .params
        .first()
        .map(|name| name.trim_start_matches('#').to_string())
        .unwrap_or_default();

    let _ = chars;

    Some(Message {
        id,
        platform: Platform::Twitch,
        channel,
        author: Author {
            name: author_name,
            color,
        },
        parts: split_parts(text, &native, catalog),
        ts_ms,
    })
}

fn emit_message(
    line: &IrcLine,
    catalog: &Arc<RwLock<EmoteCatalog>>,
    tx: &UnboundedSender<ProviderEvent>,
    ctx: &egui::Context,
    last_ts: &AtomicI64,
) {
    let catalog = catalog.read().unwrap();
    if let Some(message) = line_to_message(line, &catalog) {
        drop(catalog);
        last_ts.fetch_max(message.ts_ms, Ordering::Relaxed);
        let _ = tx.send(ProviderEvent::Message(message));
        ctx.request_repaint();
    }
}

pub async fn run(
    index: usize,
    channel: String,
    catalog: Arc<RwLock<EmoteCatalog>>,
    tx: UnboundedSender<ProviderEvent>,
    ctx: egui::Context,
) {
    let mut attempt = 0u32;
    let connected = Arc::new(AtomicBool::new(false));
    let last_ts = Arc::new(AtomicI64::new(0));
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default();
    loop {
        if tx.is_closed() {
            return;
        }
        let _ = tx.send(ProviderEvent::Status(index, ConnectionStatus::Connecting));
        ctx.request_repaint();
        match session(
            index,
            &channel,
            &catalog,
            &tx,
            &ctx,
            &connected,
            &last_ts,
            &http,
        )
        .await
        {
            Ok(()) => {}
            Err(err) => {
                let _ = tx.send(ProviderEvent::Error(index, format!("{err:#}")));
                if connected.swap(false, Ordering::Relaxed) {
                    let _ = tx.send(ProviderEvent::Note(
                        "соединение с Twitch потеряно, часть сообщений пропущена".to_string(),
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
        tokio::time::sleep(next_backoff(attempt)).await;
    }
}

/// Phases the IRC loop reports to the UI: JOIN, then room id (with a grace
/// period), then the backfill. Kept separate from the socket so the ordering
/// rules can be tested without a network.
#[derive(Debug, Default)]
struct SyncState {
    joined: bool,
    room_id: Option<String>,
    start_by: Option<Instant>,
    started: bool,
    ready_sent: bool,
}

impl SyncState {
    fn on_joined(&mut self, now: Instant) {
        self.joined = true;
        self.start_by = Some(now + ROOM_ID_GRACE);
    }

    fn on_room(&mut self, room: Option<String>) {
        if let Some(room) = room {
            self.room_id.get_or_insert(room);
        }
    }

    fn joined(&self) -> bool {
        self.joined
    }

    fn room_id(&self) -> Option<&str> {
        self.room_id.as_deref()
    }

    fn started(&self) -> bool {
        self.started
    }

    fn should_start_history(&self, now: Instant) -> bool {
        self.joined
            && !self.started
            && (self.room_id.is_some() || self.start_by.is_some_and(|deadline| now >= deadline))
    }

    fn mark_started(&mut self) {
        self.started = true;
    }

    fn needs_ready(&self, history_done: bool) -> bool {
        self.joined && !self.ready_sent && history_done
    }

    fn mark_ready(&mut self) {
        self.ready_sent = true;
    }

    fn waiting(&self, deferred_pending: bool) -> bool {
        deferred_pending || (self.joined && (!self.started || !self.ready_sent))
    }
}

#[allow(clippy::too_many_arguments)]
async fn session(
    index: usize,
    channel: &str,
    catalog: &Arc<RwLock<EmoteCatalog>>,
    tx: &UnboundedSender<ProviderEvent>,
    ctx: &egui::Context,
    connected: &AtomicBool,
    last_ts: &Arc<AtomicI64>,
    http: &reqwest::Client,
) -> anyhow::Result<()> {
    let (mut ws, _) = tokio::time::timeout(
        Duration::from_secs(20),
        tokio_tungstenite::connect_async(IRC_URL),
    )
    .await
    .map_err(|_| anyhow::anyhow!("таймаут подключения к Twitch"))??;

    let nick = format!("justinfan{}", std::process::id());
    for line in handshake_lines(channel, &nick) {
        ws.send(WsMessage::Text(line.into())).await?;
    }

    let mut buffer = String::new();
    let mut sync = SyncState::default();
    // Live lines wait here until 7TV answers, otherwise they would be parsed
    // without emotes.
    let mut deferred: Vec<IrcLine> = Vec::new();
    // `catalog_ready` means "the catalogue attempt is over", not "emotes are
    // available": on failure it flips too, so live lines are never stuck.
    let catalog_ready = Arc::new(AtomicBool::new(!catalog.read().unwrap().is_empty()));
    let history_done = Arc::new(AtomicBool::new(false));
    let mut history_job: Option<AbortOnDrop> = None;
    let mut _catalog_job: Option<AbortOnDrop> = None;
    let mut last_frame = Instant::now();
    let mut next_ping = Instant::now() + PING_EVERY;

    loop {
        let now = Instant::now();
        if now.duration_since(last_frame) > SILENCE_LIMIT {
            anyhow::bail!("Twitch молчит дольше {} секунд", SILENCE_LIMIT.as_secs());
        }
        if now >= next_ping {
            ws.send(WsMessage::Text("PING :tmi.twitch.tv".into()))
                .await?;
            next_ping = now + PING_EVERY;
        }

        // Start the backfill as soon as the room is known, or after the grace
        // period; this has to live in the loop head, because the timeout arm
        // below is the only thing that runs on a quiet connection.
        if sync.should_start_history(now) {
            sync.mark_started();
            let channel = channel.to_string();
            let room = sync.room_id().unwrap_or_default().to_string();
            let catalog = Arc::clone(catalog);
            let tx = tx.clone();
            let task_ctx = ctx.clone();
            let last_ts = Arc::clone(last_ts);
            let http = http.clone();
            let catalog_ready = Arc::clone(&catalog_ready);
            let history_done = Arc::clone(&history_done);
            history_job = Some(AbortOnDrop(tokio::spawn(async move {
                load_history(
                    &channel,
                    &room,
                    &catalog,
                    &tx,
                    &task_ctx,
                    &last_ts,
                    &http,
                    &catalog_ready,
                    &history_done,
                )
                .await;
            })));
            ctx.request_repaint();
        }

        let job_finished = history_job
            .as_ref()
            .is_some_and(|job| job.0.is_finished());

        // A finished job also counts as "catalogue attempt over": if the task
        // died early, buffered lines must still reach the feed.
        if (catalog_ready.load(Ordering::Relaxed) || job_finished) && !deferred.is_empty() {
            for line in deferred.drain(..) {
                emit_message(&line, catalog, tx, ctx, last_ts);
            }
        }

        // The loop reports ready, not the task, so a task that outlived its
        // session cannot turn the indicator green for the next one.
        if sync.needs_ready(history_done.load(Ordering::Relaxed) || job_finished) {
            sync.mark_ready();
            let _ = tx.send(ProviderEvent::Status(index, ConnectionStatus::Connected));
            ctx.request_repaint();
        }

        // Wake up every second while the source is still syncing: the backfill
        // runs on another task, and deferred lines must not wait for the next
        // keepalive tick.
        let wait = if sync.waiting(!deferred.is_empty()) {
            Duration::from_secs(1)
        } else {
            (next_ping - now).max(Duration::from_secs(1))
        };
        let frame = match tokio::time::timeout(wait, ws.next()).await {
            Err(_) => continue,
            Ok(None) => anyhow::bail!("Twitch закрыл соединение"),
            Ok(Some(frame)) => frame?,
        };
        last_frame = Instant::now();

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

            if line.command == "366" && !sync.joined() {
                connected.store(true, Ordering::Relaxed);
                sync.on_joined(Instant::now());
                // The socket is up, but the feed is still empty: the source is
                // not "online" until the backfill has been applied.
                let _ = tx.send(ProviderEvent::Status(index, ConnectionStatus::Syncing));
                ctx.request_repaint();
            }

            let late_room = sync.started() && sync.room_id().is_none();
            let room = parse_room_id(&line);
            if let Some(id) = &room {
                sync.on_room(Some(id.clone()));
                ctx.request_repaint();

                // The grace fallback started the backfill without a room id, so
                // the catalogue was skipped. Load it now that 7TV can be asked.
                if late_room {
                    let id = id.clone();
                    let catalog = Arc::clone(catalog);
                    let http = http.clone();
                    let task_ctx = ctx.clone();
                    _catalog_job = Some(AbortOnDrop(tokio::spawn(async move {
                        load_catalog(&http, &id, &catalog).await;
                        task_ctx.request_repaint();
                    })));
                }
            }

            if line.command == "366" || room.is_some() {
                continue;
            }

            if line.command == "RECONNECT" {
                anyhow::bail!("Twitch попросил переподключиться");
            }

            if sync.joined()
                && !catalog_ready.load(Ordering::Relaxed)
                && deferred.len() < DEFERRED_LIMIT
            {
                deferred.push(line);
                continue;
            }

            emit_message(&line, catalog, tx, ctx, last_ts);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Part;

    #[test]
    fn backfill_waits_for_room_id_or_grace_period() {
        let joined = Instant::now();
        let mut sync = SyncState::default();
        assert!(!sync.should_start_history(joined));

        sync.on_joined(joined);
        assert!(!sync.should_start_history(joined + ROOM_ID_GRACE / 2));
        assert!(sync.should_start_history(joined + ROOM_ID_GRACE));

        sync.on_room(Some("123".into()));
        assert_eq!(sync.room_id(), Some("123"));
        assert!(sync.should_start_history(joined));
    }

    #[test]
    fn ready_is_reported_once_and_only_after_history() {
        let joined = Instant::now();
        let mut sync = SyncState::default();
        assert!(!sync.waiting(false));

        sync.on_joined(joined);
        assert!(sync.waiting(false));
        assert!(!sync.needs_ready(false));

        sync.mark_started();
        assert!(sync.waiting(false));
        assert!(!sync.needs_ready(false));
        assert!(sync.needs_ready(true));

        sync.mark_ready();
        assert!(!sync.needs_ready(true));
        assert!(!sync.waiting(false));
    }

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
        let msg = line_to_message(&parse_line(raw).unwrap(), &EmoteCatalog::new()).unwrap();
        assert_eq!(msg.id, "abc");
        assert_eq!(msg.channel, "xqc");
        assert_eq!(msg.author.name, "DavidzusTV");
        assert_eq!(msg.author.color, Some([0x2e, 0x8b, 0x57]));
        assert_eq!(msg.ts_ms, 1793656722613);
        assert!(matches!(&msg.parts[0], Part::Text(t) if t == "TWEAK SHIIII"));
    }

    #[test]
    fn builds_recent_messages_url() {
        assert_eq!(
            recent_messages_url("#etonelexot", None, 100),
            "https://recent-messages.robotty.de/api/v2/recent-messages/etonelexot?limit=100"
        );
        assert_eq!(
            recent_messages_url("etonelexot", Some(1793660105830), 50),
            "https://recent-messages.robotty.de/api/v2/recent-messages/etonelexot?after=1793660105830&limit=50"
        );
    }

    #[test]
    fn parses_recent_messages_fixture() {
        let payload: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/recent_messages.json")).unwrap();
        let messages = parse_recent_messages(&payload, &EmoteCatalog::new());
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].id, "m1");
        assert_eq!(messages[0].channel, "etonelexot");
        assert_eq!(messages[1].id, "m2");
        assert!(
            messages
                .iter()
                .any(|m| matches!(&m.parts[0], Part::Text(t) if t == "тест"))
        );
    }

    #[test]
    fn error_payload_yields_no_history() {
        let payload: serde_json::Value = serde_json::from_str(
            r#"{"status":400,"error":"not joined","error_code":"channel_not_joined"}"#,
        )
        .unwrap();
        assert!(parse_recent_messages(&payload, &EmoteCatalog::new()).is_empty());
    }

    #[tokio::test]
    #[ignore = "живой запрос к recent-messages"]
    async fn live_recent_messages() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::new();
        let url = recent_messages_url("etonelexot", None, 100);
        let payload = fetch_recent_payload(&http, &url).await.unwrap();
        let messages = parse_recent_messages(&payload, &EmoteCatalog::new());
        assert!(!messages.is_empty(), "у канала нет истории: {payload}");
    }

    #[tokio::test]
    #[ignore = "живое подключение к Twitch IRC"]
    async fn live_session_reports_ready_only_after_history() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let ctx = egui::Context::default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let catalog = Arc::new(RwLock::new(EmoteCatalog::new()));
        let last_ts = Arc::new(AtomicI64::new(0));
        let connected = Arc::new(AtomicBool::new(false));
        let http = reqwest::Client::new();

        let job = tokio::spawn({
            let catalog = Arc::clone(&catalog);
            let last_ts = Arc::clone(&last_ts);
            let connected = Arc::clone(&connected);
            async move {
                let _ = session(
                    0,
                    "etonelexot",
                    &catalog,
                    &tx,
                    &ctx,
                    &connected,
                    &last_ts,
                    &http,
                )
                .await;
            }
        });

        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut statuses = Vec::new();
        let mut history_seen = false;
        loop {
            let Ok(event) = tokio::time::timeout_at(deadline, rx.recv()).await else {
                break;
            };
            match event {
                Some(ProviderEvent::Status(_, status)) => {
                    statuses.push(status);
                    if status == ConnectionStatus::Connected {
                        break;
                    }
                }
                Some(ProviderEvent::History(_)) => history_seen = true,
                Some(_) => {}
                None => break,
            }
        }
        job.abort();

        assert!(
            statuses.contains(&ConnectionStatus::Syncing),
            "статус синхронизации не пришёл: {statuses:?}"
        );
        assert!(history_seen, "история не пришла до статуса «онлайн»");
        assert_eq!(statuses.last(), Some(&ConnectionStatus::Connected));
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
