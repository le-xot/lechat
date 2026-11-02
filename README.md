# lechat

Read-only chat viewer for streamers who broadcast to two platforms at once. One native
window, one merged feed, a platform badge in front of every message.

- **Twitch** — anonymous IRC over WebSocket (`justinfan`), no login, read-only. On connect
  the last messages are backfilled from the public
  [recent-messages](https://recent-messages.robotty.de) service (the same one Chatterino uses).
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

[appearance]
font_size = 14.0
# font_family = "Cantarell"   # omit for the built-in font
show_time = true
show_seconds = false

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

- **⚙** in the header — font size (11–24), any installed font with Cyrillic coverage,
  and timestamps (hide, or show with seconds). Changes apply immediately and are saved
  to the config.
- **F5** — reconnect every source.
- Scroll up to pause auto-follow; the “↓ N новых” button jumps back to the live edge.

## Known limitations

- Twitch IRC itself has no history: the backfill comes from a third-party service and
  only covers what that service recorded. Messages sent while disconnected may be
  missing; the feed inserts a notice when a connection was lost.
- Emotes animate continuously (≈16 fps cap). An animated emote in view redraws the
  feed, so idle CPU is higher than with static emotes.
- streamx uses an unofficial internal GraphQL API; it can change without notice. The
  client reconnects with backoff and re-fetches history (limited to 500 messages).
- Badges and avatars are not rendered: Twitch only sends badge names, and resolving
  them requires a Helix client ID/secret.
