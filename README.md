# lechat

Read-only chat viewer that shows Twitch and [streamx](https://streamx.satont.dev) chat side by side in a single native window.

- Twitch: anonymous IRC over WebSocket (`justinfan`), no login, read-only
- streamx: GraphQL over HTTP + WebSocket subscriptions, no login
- 7TV emotes (channel + global), animated WebP, wide and zero-width emotes
- Built with [egui](https://github.com/emilk/egui) 0.37 / eframe

No sending, no moderation, no reactions — display only.
