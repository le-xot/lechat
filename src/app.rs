use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use crate::config::{Appearance, Config, PanelCfg, PanelKind};
use crate::emotes::{EmoteCatalog, seventv};
use crate::model::{Entry, Platform};
use crate::providers::{ConnectionStatus, ProviderEvent, streamx, twitch};
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
    pending_reconnects: Vec<bool>,
    spawn_enabled: bool,
    config: Config,
    config_path: std::path::PathBuf,
    appearance: Appearance,
    fonts: Option<Vec<String>>,
    font_query: String,
    settings_open: bool,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: Config,
        config_error: Option<String>,
    ) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        cc.egui_ctx
            .add_image_loader(Arc::new(crate::images::WebpAnimationLoader::default()));
        cc.egui_ctx.set_visuals(egui::Visuals::dark());

        let mut app = Self::build(config, cc.egui_ctx.clone());
        app.config_error = config_error;
        app.spawn_enabled = true;
        crate::settings::apply(&cc.egui_ctx, &app.appearance);
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
        let appearance = config.appearance.clone();

        Self {
            timeline: Timeline::new(),
            sources,
            config_error: None,
            tx,
            rx,
            runtime: tokio::runtime::Runtime::new().expect("tokio runtime"),
            ctx,
            handles: Vec::new(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            pending_reconnects,
            spawn_enabled: false,
            config,
            config_path: Config::default_path(),
            appearance,
            fonts: None,
            font_query: String::new(),
            settings_open: false,
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
        self.pending_reconnects.fill(true);
        for source in &mut self.sources {
            source.status = ConnectionStatus::Connecting;
            source.catalog_loading = false;
        }
        self.spawn_all();
    }

    #[cfg(test)]
    pub fn pending_reconnects(&self) -> Vec<bool> {
        self.pending_reconnects.clone()
    }

    fn drain_events(&mut self) {
        let mut applied = false;
        while let Ok(event) = self.rx.try_recv() {
            self.apply_event(event);
            applied = true;
        }
        if applied {
            self.ctx.request_repaint();
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
            ProviderEvent::Note(text) => {
                let ts = chrono::Utc::now().timestamp_millis();
                self.timeline.push_note(text, ts);
            }
            ProviderEvent::Message(message) => {
                self.timeline.insert_sorted(Entry::Msg(message));
                if !self.timeline.follow {
                    self.timeline.unread += 1;
                }
            }
            ProviderEvent::History(messages) => {
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
    fn settings_window(&mut self, ctx: &egui::Context) {
        if self.fonts.is_none() {
            self.fonts = Some(crate::settings::available_fonts());
        }
        let fonts = self.fonts.clone().unwrap_or_default();

        let mut appearance = self.appearance.clone();
        let mut query = self.font_query.clone();
        let mut open = self.settings_open;
        let mut changed = false;

        egui::Window::new("Настройки")
            .open(&mut open)
            .resizable(false)
            .default_width(340.0)
            .show(ctx, |ui| {
                let size = ui.add(
                    egui::Slider::new(&mut appearance.font_size, 11.0..=24.0)
                        .step_by(0.5)
                        .text("размер шрифта"),
                );
                changed |= size.changed();

                changed |= ui
                    .checkbox(&mut appearance.show_time, "показывать время")
                    .changed();
                ui.add_enabled_ui(appearance.show_time, |ui| {
                    changed |= ui
                        .checkbox(&mut appearance.show_seconds, "с секундами")
                        .changed();
                });

                ui.separator();
                ui.label("шрифт");
                ui.add(
                    egui::TextEdit::singleline(&mut query)
                        .hint_text("поиск шрифта")
                        .desired_width(f32::INFINITY),
                );
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if ui
                            .selectable_label(
                                appearance.font_family.is_none(),
                                "по умолчанию (Ubuntu)",
                            )
                            .clicked()
                        {
                            appearance.font_family = None;
                            changed = true;
                        }
                        let needle = query.to_lowercase();
                        for family in &fonts {
                            if !needle.is_empty() && !family.to_lowercase().contains(&needle) {
                                continue;
                            }
                            let selected =
                                appearance.font_family.as_deref() == Some(family.as_str());
                            if ui.selectable_label(selected, family).clicked() {
                                appearance.font_family = Some(family.clone());
                                changed = true;
                            }
                        }
                    });

                ui.separator();
                if ui.button("сбросить").clicked() {
                    appearance = Appearance::default();
                    query.clear();
                    changed = true;
                }
                ui.label(
                    egui::RichText::new("размер и шрифт сохраняются в config.toml")
                        .size(11.0)
                        .color(egui::Color32::from_gray(130)),
                );
            });

        self.settings_open = open;
        self.font_query = query;
        if changed {
            self.appearance = appearance;
            self.config.appearance = self.appearance.clone();
            crate::settings::apply(ctx, &self.appearance);
            if let Err(err) = self.config.save(&self.config_path) {
                tracing::warn!("не удалось сохранить конфиг: {err:#}");
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_events();
        if ui.input(|input| input.key_pressed(egui::Key::F5)) {
            self.request_reconnect_all();
        }

        let sources = &self.sources;
        let mut settings_open = self.settings_open;
        egui::Panel::top("lechat-status").show(ui, |ui| {
            if status_bar(ui, sources, self.config_error.as_deref()) {
                settings_open = !settings_open;
            }
        });
        self.settings_open = settings_open;

        if self.settings_open {
            let ctx = ui.ctx().clone();
            self.settings_window(&ctx);
        }

        let timeline = &mut self.timeline;
        let appearance = &self.appearance;
        egui::CentralPanel::default().show(ui, |ui| {
            feed(ui, timeline, appearance);
        });
    }
}

fn status_bar(ui: &mut egui::Ui, sources: &[SourceState], config_error: Option<&str>) -> bool {
    let small_size = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Small)
        .map(|font| font.size)
        .unwrap_or(11.0);
    ui.add_space(4.0);
    let mut gear_clicked = false;
    ui.horizontal_wrapped(|ui| {
        if ui.button("⚙").on_hover_text("настройки").clicked() {
            gear_clicked = true;
        }
        ui.label(egui::RichText::new("lechat").strong());
        ui.separator();
        for source in sources {
            let platform = Platform::from(source.cfg.kind.clone());
            ui::source_badge(
                ui,
                platform,
                &format!("{} #{}", source.cfg.kind.label(), source.cfg.channel),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} #{}",
                    source.cfg.kind.label(),
                    source.cfg.channel
                ))
                .size(small_size),
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
                    .size(small_size)
                    .color(egui::Color32::from_gray(140)),
            );
            ui.separator();
        }
        if let Some(error) = config_error {
            ui.colored_label(egui::Color32::from_rgb(0xE0, 0x5C, 0x5C), error);
        }
    });
    ui.add_space(4.0);
    gear_clicked
}

fn feed(ui: &mut egui::Ui, timeline: &mut Timeline, appearance: &Appearance) {
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
                .map(|message| message.channel.as_str())
                .unwrap_or("");
            row::entry_ui(ui, entry, channel, appearance);
        }
    });

    let from_end = output.content_size.y - (output.state.unclamped_offset().y + output.inner_rect.height());
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
            channel: "test".into(),
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
        app.apply_event(ProviderEvent::Message(msg("m1")));
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
        app.apply_event(ProviderEvent::History(vec![msg("h1"), msg("h2")]));
        assert_eq!(app.timeline.len(), 2);
        assert_eq!(app.timeline.unread, 0);
    }
}
