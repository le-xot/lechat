# lechat

Read-only chat viewer for streamers who broadcast to two platforms at once. One native
window, one merged feed, a platform badge in front of every message.

- **Twitch** — anonymous IRC over WebSocket (`justinfan`), no login, read-only.
- **streamx** — GraphQL over HTTP + WebSocket subscriptions, no login.
- **7TV** — channel and global emotes, wide and zero-width emotes, optional animation.
- Built with [egui](https://github.com/emilk/egui) 0.37 / eframe, one binary, no backend.

No sending, no moderation, no reactions — display only.

## Build

```bash
cargo build --release
./target/release/lechat
```

Linux (Wayland/X11) is the primary target. Requires Rust 1.85+ (edition 2024) and a
working GPU/OpenGL/Vulkan stack for eframe.

## Config

Created on first run at `~/.config/lechat/config.toml`:

```toml
[window]
width = 1100.0
height = 760.0

[[panel]]
kind = "twitch"
channel = "etonelexot"

[[panel]]
kind = "streamx"
channel = "le-xot"
base_url = "https://streamx.satont.dev"
```

`kind` is `twitch` or `streamx`. Both kinds feed the same merged timeline, sorted by
message time. Add more panels to follow more channels.

## Controls

- **F5** — reconnect every source.
- **анимация** checkbox in the header — animate 7TV emotes (off by default: animated
  emotes repaint the whole feed and cost CPU).
- Scroll up to pause auto-follow; the “↓ N новых” button jumps back to the live edge.

## Known limitations

- Twitch has no history over anonymous IRC, so messages sent while disconnected are
  gone; the feed inserts a notice when a connection was lost.
- streamx uses an unofficial internal GraphQL API; it can change without notice. The
  client reconnects with backoff and re-fetches history (limited to 500 messages).
- Badges and avatars are not rendered: Twitch only sends badge names, and resolving
  them requires a Helix client ID/secret.
- Zero-width emotes are drawn inline and slightly smaller rather than layered exactly
  like the 7TV browser extension.
