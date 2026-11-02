use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use crate::config::{Config, PanelCfg, PanelKind};
use crate::emotes::{EmoteCatalog, seventv};
use crate::model::{Entry, Platform};
use crate::providers::{self, ConnectionStatus, ProviderEvent, streamx, twitch};
use crate::ui::timeline::Timeline;
use crate::ui::{self, row, NEAR_BOTTOM_PX};

pub struct SourceState {
    pub cfg: PanelCfg,
    pub status: ConnectionStatus,
    pub error: Option<String>,
    pub catalog: Arc<RwLock<EmoteCatalog>>,
    pub catalog_loading: bool,
}

pub struct App {
    pub timeline: Timeline,
    pub sources: Vec<SourceState>,
    pub config_error: Option<String>,
    tx: UnboundedSender<ProviderEvent>,
    rx: UnboundedReceiver<ProviderEvent>,
    runtime: tokio::runtime::Runtime,
    ctx: egui::Context,
    handles: Vec<Option<JoinHandle<()>>>,
    http: reqwest::Client,
    note_seq: u64,
    pending_reconnects: Vec<bool>,
    spawn_enabled: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        cc.egui_ctx
            .add_image_loader(Arc::new(crate::images::WebpAnimationLoader::default()));

        let path = Config::default_path();
        let (config, config_error) = match Config::load_or_create(&path) {
            Ok(config) => (config, None),
            Err(err) => (
                Config::default_config(),
                Some(format!("конфиг {}: {err:#}", path.display())),
            ),
        };

        let mut app = Self::build(config, cc.egui_ctx.clone());
        app.config_error = config_error;
        app.spawn_enabled = true;
        app.spawn_all();
        app
    }

    pub fn build(config: Config, ctx: egui::Context) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let sources = config
            .panel
            .iter()
            .map(|cfg| SourceState {
                cfg: cfg.clone(),
                status: ConnectionStatus::Connecting,
                error: None,
                catalog: Arc::new(RwLock::new(EmoteCatalog::new())),
                catalog_loading: false,
            })
            .collect::<Vec<_>>();
        let pending_reconnects = vec![false; sources.len()];

        Self {
            timeline: Timeline::new(),
            sources,
            config_error: None,
            tx,
            rx,
            runtime: tokio::runtime::Runtime::new().expect("tokio runtime"),
            ctx,
            handles: Vec::new(),
            http: reqwest::Client::new(),
            note_seq: 0,
            pending_reconnects,
            spawn_enabled: false,
        }
    }

    fn spawn_all(&mut self) {
        if !self.spawn_enabled {
            return;
        }
        for handle in self.handles.drain(..).flatten() {
            handle.abort();
        }
        self.handles = (0..self.sources.len()).map(|_| None).collect();
        if self.pending_reconnects.len() != self.sources.len() {
            self.pending_reconnects = vec![false; self.sources.len()];
        }
        for index in 0..self.sources.len() {
            self.spawn_source(index);
        }
    }

    fn spawn_source(&mut self, index: usize) {
        let source = &self.sources[index];
        let handle = match source.cfg.kind {
            PanelKind::Twitch => self.runtime.spawn(twitch::run(
                index,
                source.cfg.channel.clone(),
                source.catalog.clone(),
                self.tx.clone(),
                self.ctx.clone(),
            )),
            PanelKind::Streamx => self.runtime.spawn(streamx::run(
                index,
                source.cfg.clone(),
                self.tx.clone(),
                self.ctx.clone(),
            )),
        };
        if self.handles.len() <= index {
            self.handles.resize_with(index + 1, || None);
        }
        self.handles[index] = Some(handle);
    }

    pub fn request_reconnect_all(&mut self) {
        for pending in &mut self.pending_reconnects {
            *pending = true;
        }
        for source in &mut self.sources {
            source.status = ConnectionStatus::Connecting;
            source.catalog_loading = false;
        }
        self.spawn_all();
    }

    pub fn pending_reconnects(&self) -> Vec<bool> {
        self.pending_reconnects.clone()
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            self.apply_event(event);
        }
    }

    pub fn apply_event(&mut self, event: ProviderEvent) {
        match event {
            ProviderEvent::Status(index, status) => {
                if let Some(source) = self.sources.get_mut(index) {
                    source.status = status;
                    if status == ConnectionStatus::Connected {
                        source.error = None;
                    }
                }
            }
            ProviderEvent::Error(index, error) => {
                if let Some(source) = self.sources.get_mut(index) {
                    source.error = Some(error);
                    source.status = ConnectionStatus::Failed;
                }
            }
            ProviderEvent::RoomId(index, room_id) => self.spawn_catalog(index, room_id),
            ProviderEvent::Note(_, text) => {
                self.note_seq += 1;
                let ts = chrono::Utc::now().timestamp_millis();
                self.timeline.push_note(self.note_seq, text, ts);
            }
            ProviderEvent::Message(_, message) => {
                self.timeline.insert_sorted(Entry::Msg(message));
                if !self.timeline.follow {
                    self.timeline.unread += 1;
                }
            }
            ProviderEvent::History(_, messages) => {
                for message in messages {
                    self.timeline.insert_sorted(Entry::Msg(message));
                }
            }
        }
    }

    fn spawn_catalog(&mut self, index: usize, room_id: String) {
        let Some(source) = self.sources.get_mut(index) else {
            return;
        };
        if source.catalog_loading {
            return;
        }
        source.catalog_loading = true;
        let catalog = source.catalog.clone();
        let http = self.http.clone();
        let ctx = self.ctx.clone();

        self.runtime.spawn(async move {
            for attempt in 0..3 {
                let channel = seventv::fetch_channel(&http, &room_id).await;
                let global = seventv::fetch_global(&http).await;
                match (channel, global) {
                    (Ok(channel), Ok(global)) => {
                        let merged = EmoteCatalog::merged(channel, global);
                        *catalog.write().unwrap() = merged;
                        ctx.request_repaint();
                        return;
                    }
                    (channel, global) => {
                        let error = channel
                            .err()
                            .or_else(|| global.err())
                            .map(|err| format!("{err:#}"))
                            .unwrap_or_default();
                        tracing::warn!("7TV: {error}");
                        if attempt < 2 {
                            tokio::time::sleep(Duration::from_secs(60)).await;
                        }
                    }
                }
            }
        });
    }

    fn channel_for(&self, platform: Platform) -> String {
        self.sources
            .iter()
            .find(|source| match (source.cfg.kind.clone(), platform) {
                (PanelKind::Twitch, Platform::Twitch) => true,
                (PanelKind::Streamx, Platform::Streamx) => true,
                _ => false,
            })
            .map(|source| source.cfg.channel.clone())
            .unwrap_or_default()
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_events();
        if ui.input(|input| input.key_pressed(egui::Key::F5)) {
            self.request_reconnect_all();
        }

        let sources = &self.sources;
        egui::Panel::top("lechat-status").show(ui, |ui| {
            status_bar(ui, sources, self.config_error.as_deref());
        });

        let timeline = &mut self.timeline;
        let channels = self
            .sources
            .iter()
            .map(|source| match source.cfg.kind {
                PanelKind::Twitch => (Platform::Twitch, source.cfg.channel.clone()),
                PanelKind::Streamx => (Platform::Streamx, source.cfg.channel.clone()),
            })
            .collect::<Vec<_>>();
        egui::CentralPanel::default().show(ui, |ui| {
            feed(ui, timeline, &channels);
        });
    }
}

fn status_bar(ui: &mut egui::Ui, sources: &[SourceState], config_error: Option<&str>) {
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("lechat").strong());
        ui.separator();
        for source in sources {
            ui::source_badge(
                ui,
                source.cfg.kind.clone().into(),
                &format!("{} #{}", source.cfg.kind.label(), source.cfg.channel),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} #{}",
                    source.cfg.kind.label(),
                    source.cfg.channel
                ))
                .size(12.0),
            );
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(rect.center(), 4.0, source.status.color());
            let hint = match &source.error {
                Some(error) => format!("{}: {error}", source.status.label()),
                None => source.status.label().to_string(),
            };
            response.on_hover_text(hint);
            ui.label(
                egui::RichText::new(source.status.label())
                    .size(11.0)
                    .color(egui::Color32::from_gray(140)),
            );
            ui.separator();
        }
        if let Some(error) = config_error {
            ui.colored_label(egui::Color32::from_rgb(0xE0, 0x5C, 0x5C), error);
        }
    });
    ui.add_space(4.0);
}

fn feed(ui: &mut egui::Ui, timeline: &mut Timeline, channels: &[(Platform, String)]) {
    if ui.input(|input| input.smooth_scroll_delta.y > 1.0) {
        timeline.follow = false;
    }

    let scroll = egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(timeline.follow);
    let output = scroll.show(ui, |ui| {
        if timeline.entries.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("ждём сообщения…")
                        .italics()
                        .color(egui::Color32::from_gray(130)),
                );
            });
        }
        for entry in &timeline.entries {
            let channel = entry
                .message()
                .map(|message| {
                    channels
                        .iter()
                        .find(|(platform, _)| *platform == message.platform)
                        .map(|(_, channel)| channel.as_str())
                        .unwrap_or("")
                })
                .unwrap_or("");
            row::entry_ui(ui, entry, channel);
        }
    });

    let from_end = output.content_size.y - (output.state.offset().y + output.inner_rect.height());
    if ui::near_bottom(from_end) {
        timeline.follow = true;
        timeline.unread = 0;
    } else if from_end > NEAR_BOTTOM_PX {
        timeline.follow = false;
    }

    if !timeline.follow && timeline.unread > 0 {
        let button = egui::Button::new(format!("↓ {} новых", timeline.unread));
        egui::Area::new(egui::Id::unique("lechat-new-messages"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -16.0])
            .show(ui.ctx(), |ui| {
                if ui.add(button).clicked() {
                    timeline.follow = true;
                    timeline.unread = 0;
                }
            });
    }
}

impl From<PanelKind> for Platform {
    fn from(kind: PanelKind) -> Self {
        match kind {
            PanelKind::Twitch => Platform::Twitch,
            PanelKind::Streamx => Platform::Streamx,
        }
    }
}

pub fn channel_hint(sources: &[SourceState], platform: Platform) -> String {
    sources
        .iter()
        .find(|source| Platform::from(source.cfg.kind.clone()) == platform)
        .map(|source| format!("#{}", source.cfg.channel))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Author, Message, Part};

    fn test_app() -> App {
        let ctx = egui::Context::default();
        App::build(Config::default_config(), ctx)
    }

    fn msg(id: &str) -> Message {
        Message {
            id: id.to_string(),
            platform: Platform::Twitch,
            author: Author {
                name: "tester".into(),
                color: None,
            },
            parts: vec![Part::Text("hi".into())],
            ts_ms: 1,
        }
    }

    #[test]
    fn apply_event_bumps_unread_when_not_following() {
        let mut app = test_app();
        app.timeline.follow = false;
        app.apply_event(ProviderEvent::Message(0, msg("m1")));
        assert_eq!(app.timeline.len(), 1);
        assert_eq!(app.timeline.unread, 1);
    }

    #[test]
    fn f5_sets_reconnect_flag_for_every_source() {
        let mut app = test_app();
        app.request_reconnect_all();
        assert_eq!(app.pending_reconnects(), vec![true, true]);
    }

    #[test]
    fn history_does_not_bump_unread() {
        let mut app = test_app();
        app.timeline.follow = true;
        app.apply_event(ProviderEvent::History(1, vec![msg("h1"), msg("h2")]));
        assert_eq!(app.timeline.len(), 2);
        assert_eq!(app.timeline.unread, 0);
    }
}
