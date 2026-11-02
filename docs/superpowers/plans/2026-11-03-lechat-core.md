# lechat core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Read-only нативное окно с одной общей лентой чатов Twitch и streamx, плашкой источника у каждого сообщения, 7TV-эмоутами и устойчивым реконнектом.

**Architecture:** eframe/egui приложение в главном потоке; на каждый канал — tokio-таск, который держит соединение и шлёт `ProviderEvent` в `mpsc`. UI раз в кадр вычитывает события и вставляет их в единый `Timeline` (сортировка по времени, дедуп, лимит 500). Платформенная логика изолирована в `providers/`, отрисовка — в `ui/`.

**Tech Stack:** Rust 1.99, eframe/egui 0.37, egui_extras 0.37 (`http`, `image`, `webp`), tokio 1.54, tokio-tungstenite 0.30, reqwest 0.13, serde/toml, chrono, tracing.

**Spec:** `docs/superpowers/specs/2026-11-03-lechat-design.md`

## Global Constraints

- egui/eframe/egui_extras ровно `0.37`, edition 2024.
- Только чтение: никаких мутаций, отправки, авторизации.
- Никаких секретов и токенов в конфиге и логах.
- Лимит ленты — 500 записей всего, дедуп по `(platform, id)`.
- Минимальный размер окна 720×520.
- Плашка источника: 16px, скруглённый квадрат, `#9146FF` + `T` (Twitch), `#00DC82` + `S` (streamx), тултип `платформа + канал`.
- История streamx запрашивается с `limit = 500`.
- Высота эмоута = 1.5 × высоты строки текста, ширина по пропорции исходника.
- Порог «прилипания к низу» — 64px.
- Конфиг: `~/.config/lechat/config.toml`.
- Linux (Wayland/X11) — основная платформа.

## Review Focus

- Несколько IRC-строк в одном WS-фрейме и строка, разрезанная между фреймами: сообщения не должны теряться или склеиваться.
- Кириллица и эмодзи: смещения эмоутов в Twitch приходят в символах, а не в байтах; нарезка текста не должна паниковать и съезжать.
- streamx: протокол-относительные URL `//cdn.7tv.app/...` и неизвестные типы сегментов — не должны ломать сообщение.
- Реконнект streamx: догрузка истории не должна давать дубли, а пропущенные сообщения не должны теряться.
- Картинки: 404, огромный эмоут, битый анимированный WebP — фолбэк без паники, зависания UI и утечки памяти.
- Общая лента: сообщения двух источников с расхождением часов не должны прыгать назад/вперёд или дублироваться.

---

### Task 1: Каркас проекта и конфиг

**Files:**
- Create: `Cargo.toml`, `src/main.rs`, `src/config.rs`
- Test: `src/config.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `Config { window: WindowCfg, panel: Vec<PanelCfg> }`, `WindowCfg { width: f32, height: f32 }`, `PanelCfg { kind: PanelKind, channel: String, base_url: Option<String> }`, `PanelKind::{Twitch, Streamx}`, `Config::parse(&str) -> anyhow::Result<Config>`, `Config::load_or_create(&Path) -> anyhow::Result<Config>`, `Config::default_path() -> PathBuf`, `Config::to_toml(&self) -> String`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn parses_two_panels() {
    let cfg = Config::parse(r#"
[window]
width = 900
height = 700

[[panel]]
kind = "twitch"
channel = "etonelexot"

[[panel]]
kind = "streamx"
channel = "le-xot"
base_url = "https://streamx.satont.dev"
"#).unwrap();
    assert_eq!(cfg.panel.len(), 2);
    assert_eq!(cfg.panel[0].kind, PanelKind::Twitch);
    assert_eq!(cfg.panel[1].base_url.as_deref(), Some("https://streamx.satont.dev"));
}

#[test]
fn rejects_empty_panels() {
    assert!(Config::parse("[window]\nwidth = 900\nheight = 700\n").is_err());
}

#[test]
fn writes_default_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let cfg = Config::load_or_create(&path).unwrap();
    assert_eq!(cfg.panel.len(), 2);
    assert_eq!(cfg.panel[0].channel, "etonelexot");
    assert_eq!(cfg.panel[1].channel, "le-xot");
    assert!(path.exists());
}
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `cargo test config`
Expected: FAIL — модуль `config` не существует.

- [ ] **Step 3: Implement**

`Cargo.toml` с зависимостями из Global Constraints (`eframe = "0.37"`, `egui = "0.37"`, `egui_extras = { version = "0.37", features = ["all_loaders"] }`, `tokio`, `tokio-tungstenite`, `reqwest`, `serde`, `toml`, `anyhow`, `chrono`, `tracing`, `tracing-subscriber`, `futures-util`, `image`), `[[bin]] name = "lechat"`. `src/main.rs` запускает пустое окно 900×700 с заголовком «lechat». `src/config.rs` — serde-структуры, `default_config()` с двумя панелями, валидация: пустой `panel` — ошибка, `channel` пустой — ошибка. `load_or_create` создаёт файл и родительские папки, если его нет. `tempfile` — dev-dependency.

- [ ] **Step 4: Run tests and build**

Run: `cargo test config && cargo build`
Expected: PASS, окно собирается.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs src/config.rs
git commit -m "feat: project scaffold and panel config"
```

---

### Task 2: Модель сообщений и нарезка на части с эмоутами

**Files:**
- Create: `src/model.rs`, `src/emotes/mod.rs`
- Test: `src/emotes/mod.rs`

**Interfaces:**
- Produces: `Platform`, `Author { name, color: Option<[u8; 3]> }`, `Part::{Text, Emote, Link}`, `EmotePart { uri, aspect: f32, zero_width: bool }`, `Message { id, platform, author, parts, ts_ms: i64 }`, `NativeEmote { start: usize, end: usize, uri: String }` (индексы в символах, включительно), `EmoteInfo { name, uri, aspect, zero_width }`, `EmoteCatalog::{new, insert, get, merged}`, `split_parts(text: &str, native: &[NativeEmote], catalog: &EmoteCatalog) -> Vec<Part>`, `Entry::{Msg(Message), Note { id: String, text: String }}`.
- Consumes: ничего.

- [ ] **Step 1: Write the failing tests**

```rust
fn cat(entries: &[(&str, &str, f32)]) -> EmoteCatalog {
    let mut c = EmoteCatalog::new();
    for (name, uri, aspect) in entries {
        c.insert(EmoteInfo { name: (*name).into(), uri: (*uri).into(), aspect: *aspect, zero_width: false });
    }
    c
}

#[test]
fn matches_whole_token_only() {
    let c = cat(&[("catKISS", "https://cdn/x.webp", 1.0)]);
    assert_eq!(split_parts("catKISSx", &[], &c), vec![Part::Text("catKISSx".into())]);
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
    let native = vec![NativeEmote { start: 0, end: 4, uri: "https://cdn/twitch/1.0".into() }];
    match &split_parts("Kappa", &native, &c)[0] {
        Part::Emote(e) => assert_eq!(e.uri, "https://cdn/twitch/1.0"),
        other => panic!("expected emote, got {other:?}"),
    }
}

#[test]
fn autolinks_plain_urls_and_trims_punctuation() {
    let parts = split_parts("клип https://clips.twitch.tv/abc, смотри", &[], &EmoteCatalog::new());
    assert!(parts.iter().any(|p| matches!(p, Part::Link(u) if u == "https://clips.twitch.tv/abc")));
}

#[test]
fn cyrillic_offsets_are_char_based() {
    let native = vec![NativeEmote { start: 6, end: 10, uri: "https://cdn/twitch/2.0".into() }];
    let parts = split_parts("привет Kappa", &native, &EmoteCatalog::new());
    assert_eq!(parts[0], Part::Text("привет ".into()));
    assert!(matches!(parts[1], Part::Emote(_)));
}

#[test]
fn wide_and_zero_width_are_preserved() {
    let mut c = EmoteCatalog::new();
    c.insert(EmoteInfo { name: "privet".into(), uri: "u".into(), aspect: 3.0, zero_width: true });
    match &split_parts("privet", &[], &c)[0] {
        Part::Emote(e) => { assert_eq!(e.aspect, 3.0); assert!(e.zero_width); }
        other => panic!("expected emote, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test emotes`
Expected: FAIL — модуль не существует.

- [ ] **Step 3: Implement**

Разбить текст на токены `split_inclusive(char::is_whitespace)`; смещения считать в символах (`char_indices`), не в байтах. Если токен пересекается с `NativeEmote` — `Part::Emote` с нативным URL. Иначе `catalog.get(token)`. Остальное — `Text`. `http://`/`https://` в тексте дают `Part::Link` (хвостовые `.,!?)]` остаются текстом). `merged(channel, global)` — сначала global, поверх channel.

- [ ] **Step 4: Run tests**

Run: `cargo test emotes`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/model.rs src/emotes/mod.rs
git commit -m "feat: message model and emote-aware text splitting"
```

---

### Task 3: Парсер IRC

**Files:**
- Create: `src/twitch/irc.rs`, `src/twitch/mod.rs`
- Test: `src/twitch/irc.rs`

**Interfaces:**
- Produces: `IrcLine { tags: HashMap<String, String>, prefix: Option<String>, command: String, params: Vec<String> }`, `parse_line(&str) -> Option<IrcLine>`, `unescape_tag(&str) -> String`, `parse_emotes_tag(&str) -> Vec<(String, Vec<(usize, usize)>)>`, `parse_room_id(&IrcLine) -> Option<String>`, `is_ping(&IrcLine) -> bool`.
- Consumes: ничего.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn parses_real_privmsg_tags() {
    let raw = r#"@badge-info=subscriber/69;badges=subscriber/66,premium/1;color=#DAA520;display-name=leman0va;emotes=;id=7cdb25d2;tmi-sent-ts=1793656722692;user-id=427746412 :leman0va!leman0va@leman0va.tmi.twitch.tv PRIVMSG #xqc :LOL i can't"#;
    let line = parse_line(raw).unwrap();
    assert_eq!(line.command, "PRIVMSG");
    assert_eq!(line.params[0], "#xqc");
    assert_eq!(line.params[1], "LOL i can't");
    assert_eq!(line.tags.get("display-name").unwrap(), "leman0va");
    assert_eq!(line.tags.get("color").unwrap(), "#DAA520");
}

#[test]
fn parses_ping_and_room_state() {
    assert!(is_ping(&parse_line("PING :tmi.twitch.tv").unwrap()));
    let line = parse_line("@emote-only=0;room-id=1263747798;slow=0 :tmi.twitch.tv ROOMSTATE #etonelexot").unwrap();
    assert_eq!(parse_room_id(&line).as_deref(), Some("1263747798"));
}

#[test]
fn parses_emote_ranges_in_chars() {
    let ranges = parse_emotes_tag("25:0-4,12-16/1902:18-22");
    assert_eq!(ranges, vec![
        ("25".to_string(), vec![(0, 4), (12, 16)]),
        ("1902".to_string(), vec![(18, 22)]),
    ]);
}

#[test]
fn unescapes_tag_values() {
    assert_eq!(unescape_tag(r"a\sb\:c\\d"), "a b;c\\d");
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test twitch::irc`
Expected: FAIL.

- [ ] **Step 3: Implement**

Парсер: ведущие `@tags`, затем `:prefix`, затем `COMMAND`, затем параметры; параметр с `:` — «хвост» (может содержать пробелы). Экранирование: `\s` → пробел, `\:` → `;`, `\\` → `\`, `\r`/`\n` — вырезаются. `parse_emotes_tag` разбирает `id:start-end,start-end/...` в `usize`.

- [ ] **Step 4: Run tests**

Run: `cargo test twitch::irc`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/twitch/
git commit -m "feat: twitch IRC line parser"
```

---

### Task 4: Twitch-провайдер

**Files:**
- Create: `src/providers/mod.rs`, `src/providers/twitch.rs`
- Test: `src/providers/twitch.rs`

**Interfaces:**
- Consumes: `parse_line`, `split_parts`, `EmoteCatalog`, `ProviderEvent`.
- Produces: `ProviderEvent::{Status(ConnectionStatus), Message(usize, Message), History(usize, Vec<Message>), Note(usize, String), RoomId(usize, String), Error(usize, String)}`, `ConnectionStatus::{Connecting, Connected, Reconnecting, Failed}`, `twitch::run(panel_id: usize, channel: String, catalog: Arc<RwLock<EmoteCatalog>>, tx: UnboundedSender<ProviderEvent>, egui_ctx: egui::Context)`, `twitch::handshake_lines(channel: &str, nick: &str) -> Vec<String>`, `twitch::line_to_message(&IrcLine, &EmoteCatalog) -> Option<Message>`, `twitch::next_backoff(attempt: u32) -> Duration`. Каталог — свой на панель, `run` только читает его.

- [ ] **Step 1: Write the failing tests**

```rust
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
    assert_eq!(msg.author.name, "DavidzusTV");
    assert_eq!(msg.author.color, Some([0x2e, 0x8b, 0x57]));
    assert_eq!(msg.ts_ms, 1793656722613);
    assert!(matches!(&msg.parts[0], Part::Text(t) if t == "TWEAK SHIIII"));
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test providers::twitch`
Expected: FAIL.

- [ ] **Step 3: Implement**

`run` — tokio-таск с циклом: `connect_async("wss://irc-ws.chat.twitch.tv:443")`, handshake, `Status(Connected)` после `JOIN`/`ROOMSTATE`, чтение фреймов, нарезка по `\r\n` (буфер для разрезанных строк), `PING` → отправка `PONG`, `ROOMSTATE` → `RoomId`, `PRIVMSG` → `Message` + `egui_ctx.request_repaint()`. Разрыв → `Status(Reconnecting)`, backoff, повтор. Отмена — по закрытию канала (`tx.is_closed()`).

- [ ] **Step 4: Run tests**

Run: `cargo test providers::twitch`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/providers/
git commit -m "feat: anonymous twitch IRC provider with reconnect"
```

---

### Task 5: streamx HTTP + WebSocket

**Files:**
- Create: `src/providers/streamx.rs`
- Test: `src/providers/streamx.rs`, `src/providers/fixtures/streamx_messages.json`

**Interfaces:**
- Consumes: `Message`, `Part`, `EmotePart`, `ProviderEvent`.
- Produces: `StreamxClient { base_url: String }` с `channel_id(&self, name: &str) -> Result<String>`, `history(&self, id: &str, limit: u32) -> Result<Vec<Message>>`, `emotes(&self, id: &str) -> Result<Vec<EmoteInfo>>`; `streamx::run(panel_id, spec, tx, egui_ctx)`; `streamx::map_message(&serde_json::Value) -> Option<Message>`; `streamx::subscription_frame(channel_id: &str) -> String`; `streamx::merge_history(&mut VecDeque<Message>, Vec<Message>)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn maps_segments_and_protocol_relative_urls() {
    let v: serde_json::Value = serde_json::from_str(include_str!("fixtures/streamx_messages.json")).unwrap();
    let msgs: Vec<Message> = v.as_array().unwrap().iter().filter_map(map_message).collect();
    let m = &msgs[0];
    assert_eq!(m.author.name, "zxcpuk");
    assert!(matches!(&m.parts[0], Part::Text(t) if t == "а сам файсит"));
    match &msgs[1].parts[0] {
        Part::Emote(e) => assert!(e.uri.starts_with("https://cdn.7tv.app/")),
        other => panic!("expected emote, got {other:?}"),
    }
}

#[test]
fn subscription_frame_is_graphql_transport_ws() {
    let frame = subscription_frame("30b9ec57-9ef7-4992-b114-f0f380bc60f6");
    let v: serde_json::Value = serde_json::from_str(&frame).unwrap();
    assert_eq!(v["type"], "subscribe");
    assert_eq!(v["payload"]["variables"]["channelID"], "30b9ec57-9ef7-4992-b114-f0f380bc60f6");
}

#[test]
fn merge_history_orders_by_timestamp_and_dedupes() {
    let mut q: VecDeque<Message> = VecDeque::from(vec![msg_at("live", 200)]);
    merge_history(&mut q, vec![msg_at("old", 100), msg_at("live", 200)]);
    let ids: Vec<_> = q.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["old", "live"]);
}

#[test]
fn merge_history_caps_at_500() {
    let mut q: VecDeque<Message> = (0..500).map(|i| msg_at(&format!("old{i}"), i)).collect();
    merge_history(&mut q, vec![msg_at("new1", 501)]);
    assert_eq!(q.len(), 500);
    assert_eq!(q.back().unwrap().id, "new1");
    assert!(!q.iter().any(|m| m.id == "old0"));
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test providers::streamx`
Expected: FAIL.

- [ ] **Step 3: Implement**

HTTP-запросы — `reqwest::Client` c JSON-body `{"query": ..., "variables": ...}` на `{base}/api/query`. Запросы: `fetchUserByName`, `chatMessagesLatest` (`limit = 500`), `getEmotes`. WS: `connect_async` на `wss://{host}/api/query` (хост из `base_url`), `connection_init` → ждать `connection_ack` → `subscription_frame` → на `next` парсить `payload.data.chatMessages`. Реконнект: после `connection_ack` живые кадры копятся в буфере, запрашивается история, `merge_history` сливает её по `ts_ms` и дедупу по `id`, затем буфер доливается; если история не пересекается с последним известным `id` — `ProviderEvent::Note("часть сообщений пропущена")`. Протокол-относительный URL эмоута: `if uri.starts_with("//") { format!("https:{uri}") }`.

- [ ] **Step 4: Run tests**

Run: `cargo test providers::streamx`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/providers/streamx.rs src/providers/fixtures/
git commit -m "feat: streamx GraphQL history and chat subscription"
```

---

### Task 6: 7TV-каталог и анимированный WebP

**Files:**
- Create: `src/emotes/seventv.rs`, `src/images.rs`
- Test: `src/emotes/seventv.rs`, `src/images.rs`

**Interfaces:**
- Consumes: `EmoteInfo`, `EmoteCatalog`.
- Produces: `seventv::fetch_channel(&reqwest::Client, twitch_id: &str) -> Result<Vec<EmoteInfo>>`, `seventv::fetch_global(&reqwest::Client) -> Result<Vec<EmoteInfo>>`, `seventv::parse_set(&serde_json::Value) -> Vec<EmoteInfo>`, `seventv::pick_file_height(files: &serde_json::Value, need_px: u32) -> Option<(String, u32, u32)>`, `images::WebpAnimationLoader` (impl `egui::load::ImageLoader`), `images::is_webp(&[u8]) -> bool`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn parses_set_with_zero_width_and_aspect() {
    let v: serde_json::Value = serde_json::from_str(include_str!("fixtures/seventv_set.json")).unwrap();
    let emotes = parse_set(&v);
    let wide = emotes.iter().find(|e| e.name == "privet").unwrap();
    assert_eq!(wide.aspect, 3.0);
    let zw = emotes.iter().find(|e| e.name == "RainTime").unwrap();
    assert!(zw.zero_width);
    assert!(wide.uri.starts_with("https://cdn.7tv.app/"));
}

#[test]
fn picks_scale_closest_above_needed_height() {
    let v: serde_json::Value = serde_json::from_str(include_str!("fixtures/seventv_set.json")).unwrap();
    let files = &v["emotes"][0]["data"]["host"]["files"];
    let (url, w, h) = pick_file_height(files, 40).unwrap();
    assert!(h >= 40 && h <= 64);
    assert!(url.ends_with(".webp"));
    assert!(w > 0);
}

#[test]
fn detects_webp_header() {
    assert!(is_webp(b"RIFF\x24\x00\x00\x00WEBPVP8 "));
    assert!(!is_webp(b"\x89PNG\r\n\x1a\n"));
}

#[test]
fn channel_overrides_global_by_name() {
    let global = vec![EmoteInfo { name: "x".into(), uri: "global".into(), aspect: 1.0, zero_width: false }];
    let channel = vec![EmoteInfo { name: "x".into(), uri: "channel".into(), aspect: 1.0, zero_width: false }];
    let cat = EmoteCatalog::merged(channel, global);
    assert_eq!(cat.get("x").unwrap().uri, "channel");
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test emotes::seventv images`
Expected: FAIL.

- [ ] **Step 3: Implement**

`parse_set` читает `emotes[]`: `name`, `flags & 1` → zero_width, `data.animated`, `data.host.url` + `files`, выбирает файл по нужной высоте (32/64/96/128) и строит `https:{host}/{file}`; aspect = `width / height`. `WebpAnimationLoader` — копия структуры egui `GifLoader`: кеш `Uri -> Arc<AnimatedWebp>`, `image-webp`/`image::codecs::webp::WebPDecoder`, `read_frame` в цикле до `num_frames`, `FrameDurations` в `ctx.data`, отдаёт кадр `#N`. Если `!is_animated()` — `LoadError::NotSupported`, чтобы сработал обычный загрузчик. Фикстуры — вырезанные реальные ответы 7TV.

- [ ] **Step 4: Run tests**

Run: `cargo test emotes::seventv images`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/emotes/seventv.rs src/images.rs src/emotes/fixtures/
git commit -m "feat: 7TV emote catalog and animated webp loader"
```

---

### Task 7: Единая лента, плашка источника и строки

**Files:**
- Create: `src/ui/mod.rs`, `src/ui/timeline.rs`, `src/ui/row.rs`, `src/app.rs`
- Test: `src/ui/mod.rs`, `src/ui/timeline.rs`, `src/app.rs`

**Interfaces:**
- Consumes: `Message`, `Part`, `Entry`, `Platform`, `PanelCfg`, `ConnectionStatus`.
- Produces: `Timeline { entries: VecDeque<Entry>, unread: usize, follow: bool }` с `Timeline::push(&mut self, Entry)`, `Timeline::insert_sorted`, `Timeline::len`; `ui::emote_size(line_height: f32, aspect: f32) -> egui::Vec2`, `ui::zero_width_rect(prev: Option<egui::Rect>, cursor: egui::Pos2, line_height: f32) -> egui::Rect`, `ui::near_bottom(offset_from_end: f32) -> bool`, `ui::source_badge(ui: &mut egui::Ui, platform: Platform, tooltip: &str)`, `App { timeline: Timeline, sources: Vec<SourceState> }` + `App::apply_event(&mut self, ProviderEvent)`.

- [ ] **Step 1: Write the failing tests**

```rust
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

#[test]
fn timeline_keeps_chronological_order() {
    let mut t = Timeline::default();
    t.insert_sorted(msg_at(Platform::Streamx, "b", 200));
    t.insert_sorted(msg_at(Platform::Twitch, "a", 100));
    t.insert_sorted(msg_at(Platform::Twitch, "c", 300));
    let ids: Vec<_> = t.messages().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["a", "b", "c"]);
}

#[test]
fn timeline_dedupes_by_platform_and_id() {
    let mut t = Timeline::default();
    t.insert_sorted(msg_at(Platform::Twitch, "1", 100));
    t.insert_sorted(msg_at(Platform::Streamx, "1", 101));
    t.insert_sorted(msg_at(Platform::Twitch, "1", 102));
    assert_eq!(t.len(), 2);
}

#[test]
fn timeline_caps_at_500() {
    let mut t = Timeline::default();
    for i in 0..501 { t.insert_sorted(msg_at(Platform::Twitch, &format!("m{i}"), i)); }
    assert_eq!(t.len(), 500);
    assert!(!t.messages().any(|m| m.id == "m0"));
}

#[test]
fn apply_event_bumps_unread_when_not_following() {
    let mut app = App::new(cfg_with_two_channels());
    app.timeline.follow = false;
    app.apply_event(ProviderEvent::Message(0, msg("m1")));
    assert_eq!(app.timeline.len(), 1);
    assert_eq!(app.timeline.unread, 1);
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test ui app`
Expected: FAIL.

- [ ] **Step 3: Implement**

`Timeline` — единый `VecDeque<Entry>`: `Message` вставляется по `ts_ms` (бинарный поиск, при равенстве — после существующих), дедуп по `(platform, id)`, после вставки обрезается до 500 с начала. Шапка: для каждого источника `source_badge` + канал + точка статуса + тултип ошибки. Ниже — один `ScrollArea::vertical().stick_to_bottom(self.follow)` со строками. Строка: `source_badge` (16×16, `painter.rect_filled` + текст `T`/`S` белым, скругление 4px), `ЧЧ:ММ` (chrono, локальная зона), ник цветом, части. Эмодзи через `ui.image(uri)` с размером `emote_size`; zero-width — `ui.painter().image()` в `zero_width_rect` поверх предыдущего эмоута, в начале строки — инлайн уменьшенным. `Entry::Note` — серая курсивная строка по центру. Прокрутка: если `follow` выключен — кнопка «↓ N новых», клик возвращает `follow` и `stick_to_bottom`. Тема тёмная, шрифт 14, минимальное окно 720×520.

- [ ] **Step 4: Run tests**

Run: `cargo test ui app`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ui/ src/app.rs
git commit -m "feat: unified chat feed with source badges and emote rendering"
```

---

### Task 8: Сборка приложения, реконнект, документация

**Files:**
- Modify: `src/main.rs`, `src/app.rs`, `README.md`
- Test: `src/app.rs`

**Interfaces:**
- Consumes: всё вышеперечисленное.
- Produces: рабочий `lechat`, `App::request_reconnect_all()`, регистрация лоадеров картинок, `eframe::run_native` с persistence.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn f5_sets_reconnect_flag_for_every_source() {
    let mut app = App::new(cfg_with_two_channels());
    app.request_reconnect_all();
    assert_eq!(app.pending_reconnects(), vec![true, true]);
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test app::tests::f5`
Expected: FAIL.

- [ ] **Step 3: Implement**

`main`: `tracing_subscriber`, `egui_extras::install_image_loaders(&cc.egui_ctx)`, регистрация `WebpAnimationLoader`, `tokio::runtime::Runtime`, `eframe::run_native` с `viewport` (720×520 min, 1200×800 default), `persist_window`. `App::new`: создать источники из конфига, для каждого Twitch-канала — свой `Arc<RwLock<EmoteCatalog>>`, спавнить провайдеры с индексом источника, получить `Receiver<ProviderEvent>`; если конфиг битый — плашка с ошибкой вместо паники. `App::update`: вычитать все события, `apply_event`, `F5` → реконнект (переспавнить таски), сохранять геометрию окна. 7TV-каталог грузится отдельной таской в свой источник после `RoomId`; ошибка загрузки не блокирует чат (эмоуты текстом), повтор — реконнект, F5, отложенный через 60с.

- [ ] **Step 4: Verify build, tests, clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo build --release`
Expected: PASS без предупреждений.

- [ ] **Step 5: Manual smoke**

Run: `cargo run --release`
Expected: одно окно с общей лентой; в шапке оба источника со статусами; сообщения идут вперемешку по времени с фиолетовыми `T` и зелёными `S`; streamx загружает историю и живой поток; эмодзи анимируются и не сплющены; при обрыве сети появляется пометка и статус восстанавливается.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: wire app, reconnect and docs"
git push -u origin feat/chat-core
```

---

## Self-Review

- Покрытие спека: анонимный Twitch (Task 3–4), streamx HTTP+WS (Task 5), 7TV и анимация (Task 6), единая лента, плашки и скролл (Task 7), конфиг (Task 1), реконнект/дедуп/500 (Task 4–5, 8), документация (Task 8). Пусто: system-сообщения, бейджи — сознательные не-цели.
- Границы задач: каждая даёт работающий, отдельно тестируемый кусок; UI не зависит от сети в тестах (`apply_event`).
- Имена согласованы: `ProviderEvent`, `Message`, `Part`, `EmoteInfo`, `emote_size`, `near_bottom`, `merge_history`.
- Review Focus: чанкование IRC (Task 4), Unicode-смещения (Task 2), протокол-относительные URL (Task 5), дедуп после реконнекта (Task 5), фолбэк картинок (Task 6–7).
