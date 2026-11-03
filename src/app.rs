use std::sync::{Arc, RwLock};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use crate::config::{Appearance, Config, PanelCfg, PanelKind};
use crate::emotes::EmoteCatalog;
use crate::images::{ImageCache, WebpAnimationLoader};
use crate::model::{Entry, Platform};
use crate::providers::{ConnectionStatus, ProviderEvent, streamx, twitch};
use crate::ui::timeline::Timeline;
use crate::ui::{self, row, NEAR_BOTTOM_PX};
use crate::update::{self, UpdateState};

/// How often unused emote textures are looked for, in seconds.
const IMAGE_GC_INTERVAL: f64 = 1.0;

pub struct SourceState {
    pub cfg: PanelCfg,
    pub status: ConnectionStatus,
    pub error: Option<String>,
    pub catalog: Arc<RwLock<EmoteCatalog>>,
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
    pending_reconnects: Vec<bool>,
    spawn_enabled: bool,
    config: Config,
    config_path: std::path::PathBuf,
    appearance: Appearance,
    fonts: Option<Vec<String>>,
    font_query: String,
    settings_open: bool,
    update_state: UpdateState,
    update_rx: Option<UnboundedReceiver<UpdateState>>,
    webp_loader: Option<Arc<WebpAnimationLoader>>,
    image_cache: ImageCache,
    last_image_gc: f64,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: Config,
        config_error: Option<String>,
    ) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let webp_loader = Arc::new(WebpAnimationLoader::default());
        cc.egui_ctx.add_texture_loader(webp_loader.clone());
        cc.egui_ctx.set_visuals(egui::Visuals::dark());

        let mut app = Self::build(config, cc.egui_ctx.clone());
        app.config_error = config_error;
        app.webp_loader = Some(webp_loader);
        app.spawn_enabled = true;
        crate::settings::apply(&cc.egui_ctx, &app.appearance);
        if app.config.updates.check {
            let (tx, rx) = mpsc::unbounded_channel();
            app.update_rx = Some(rx);
            std::thread::spawn(move || update::check(tx));
        }
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
            })
            .collect::<Vec<_>>();
        let pending_reconnects = vec![false; sources.len()];
        let appearance = config.appearance.clone();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("lechat-net")
            .enable_all()
            .build()
            .expect("tokio runtime");

        Self {
            timeline: Timeline::new(),
            sources,
            config_error: None,
            tx,
            rx,
            runtime,
            ctx,
            handles: Vec::new(),
            pending_reconnects,
            spawn_enabled: false,
            config,
            config_path: Config::default_path(),
            appearance,
            fonts: None,
            font_query: String::new(),
            settings_open: false,
            update_state: UpdateState::Disabled,
            update_rx: None,
            webp_loader: None,
            image_cache: ImageCache::new(),
            last_image_gc: 0.0,
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
        }
        self.spawn_all();
    }

    #[cfg(test)]
    pub fn pending_reconnects(&self) -> Vec<bool> {
        self.pending_reconnects.clone()
    }

    fn start_update_check(&mut self) {
        let (tx, rx) = mpsc::unbounded_channel();
        self.update_rx = Some(rx);
        self.update_state = UpdateState::Checking;
        std::thread::spawn(move || update::check(tx));
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

    /// Drops decoded frames and textures of emotes that are no longer painted.
    /// egui keeps every loaded image until it is told otherwise, so without
    /// this the caches grow for the whole session.
    fn gc_images(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|input| input.time);
        if now - self.last_image_gc < IMAGE_GC_INTERVAL {
            return;
        }
        self.last_image_gc = now;

        let Some(loader) = self.webp_loader.as_ref() else {
            return;
        };
        let painted = loader.take_painted();
        let bytes = loader.cached_bytes();
        self.image_cache.touch(now, &painted);
        let victims = self.image_cache.evict(now, &painted, bytes);
        if victims.is_empty() {
            return;
        }
        tracing::debug!(
            count = victims.len(),
            bytes,
            "сброшены неиспользуемые эмоуты"
        );
        for uri in victims {
            ctx.forget_image(&uri);
        }
        // Decoding allocates and frees a lot of small buffers; glibc keeps
        // those pages instead of returning them, which counts as RSS.
        crate::util::release_free_memory();
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


    fn settings_window(&mut self, ctx: &egui::Context) {
        if self.fonts.is_none() {
            self.fonts = Some(crate::settings::available_fonts());
        }
        let fonts = self.fonts.clone().unwrap_or_default();

        let mut appearance = self.appearance.clone();
        let mut updates_check = self.config.updates.check;
        let mut check_now = false;
        let update_running = self.update_state == UpdateState::Checking;
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
                changed |= ui
                    .add(
                        egui::Slider::new(&mut appearance.emote_scale, 0.5..=3.0)
                            .step_by(0.05)
                            .text("размер эмоутов"),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::Slider::new(&mut appearance.emote_offset, -12.0..=12.0)
                            .step_by(0.5)
                            .text("сдвиг эмоутов по вертикали"),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::Slider::new(&mut appearance.row_gap, 0.0..=24.0)
                            .step_by(0.5)
                            .text("расстояние между строками"),
                    )
                    .changed();
                changed |= ui
                    .checkbox(&mut appearance.show_badges, "показывать значки платформ")
                    .changed();
                changed |= ui
                    .checkbox(&mut updates_check, "проверять обновления")
                    .changed();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!update_running, egui::Button::new("проверить обновления"))
                        .clicked()
                    {
                        check_now = true;
                    }
                    match &self.update_state {
                        UpdateState::Ready(version) => {
                            ui.label(format!("готово: {version}"));
                        }
                        UpdateState::Available(version) => {
                            ui.label(format!("доступно: {version}"));
                        }
                        UpdateState::Error(error) => {
                            ui.label(format!("ошибка: {error}"));
                        }
                        UpdateState::UpToDate => {
                            ui.label("обновлений нет");
                        }
                        UpdateState::Checking => {
                            ui.label("проверяю…");
                        }
                        UpdateState::Disabled => {
                            ui.label("проверка отключена");
                        }
                    }
                });
                if let Some(path) = crate::update::update_log_path() {
                    ui.label(
                        egui::RichText::new(format!("лог: {}", path.display()))
                            .size(10.0)
                            .color(egui::Color32::from_gray(120)),
                    );
                }

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
                    egui::RichText::new(format!(
                        "версия {} · настройки сохраняются в config.toml",
                        env!("CARGO_PKG_VERSION")
                    ))
                    .size(11.0)
                    .color(egui::Color32::from_gray(130)),
                );
            });

        self.settings_open = open;
        self.font_query = query;
        if self.config.updates.check != updates_check {
            self.config.updates.check = updates_check;
            changed = true;
        }
        if check_now {
            self.start_update_check();
        }
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
        self.gc_images(ui.ctx());
        if let Some(rx) = &mut self.update_rx
            && let Ok(state) = rx.try_recv()
        {
            self.update_state = state;
        }
        if ui.input(|input| input.key_pressed(egui::Key::F5)) {
            self.request_reconnect_all();
        }

        let sources = &self.sources;
        let mut settings_open = self.settings_open;
        let mut refresh = false;
        let mut restart = false;
        let update_state = self.update_state.clone();
        egui::Panel::top("lechat-status").show(ui, |ui| {
            let (gear, reload, relaunch) = status_bar(
                ui,
                sources,
                self.config_error.as_deref(),
                &update_state,
            );
            if gear {
                settings_open = !settings_open;
            }
            refresh = reload;
            restart = relaunch;
        });
        self.settings_open = settings_open;
        if refresh {
            self.request_reconnect_all();
        }
        if restart {
            restart_now();
        }

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

fn status_bar(
    ui: &mut egui::Ui,
    sources: &[SourceState],
    config_error: Option<&str>,
    update_state: &UpdateState,
) -> (bool, bool, bool) {
    let small_size = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Small)
        .map(|font| font.size)
        .unwrap_or(11.0);
    ui.add_space(4.0);
    let mut gear_clicked = false;
    let mut refresh_clicked = false;
    let mut restart_clicked = false;
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
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(rect.center(), 4.0, source.status.color());
            let hint = match &source.error {
                Some(error) => format!(
                    "{} #{} · {}: {error}",
                    source.cfg.kind.label(),
                    source.cfg.channel,
                    source.status.label()
                ),
                None => format!(
                    "{} #{} · {}",
                    source.cfg.kind.label(),
                    source.cfg.channel,
                    source.status.label()
                ),
            };
            response.on_hover_text(hint);
            ui.separator();
        }
        if let Some(error) = config_error {
            ui.colored_label(egui::Color32::from_rgb(0xE0, 0x5C, 0x5C), error);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("↻")
                .on_hover_text("переподключить всё (F5)")
                .clicked()
            {
                refresh_clicked = true;
            }
            match update_state {
                UpdateState::Ready(version) => {
                    if ui
                        .button(format!("обновление {version} — перезапустить"))
                        .clicked()
                    {
                        restart_clicked = true;
                    }
                }
                UpdateState::Available(version) => {
                    ui.label(
                        egui::RichText::new(format!(
                            "{version} — обнови через пакетный менеджер"
                        ))
                        .size(small_size)
                        .color(egui::Color32::from_gray(150)),
                    );
                }
                UpdateState::Error(error) => {
                    ui.label(
                        egui::RichText::new("⚠ обновление")
                            .size(small_size)
                            .color(egui::Color32::from_rgb(0xE0, 0xB0, 0x4C)),
                    )
                    .on_hover_text(error);
                }
                _ => {}
            }
        });
    });
    ui.add_space(4.0);
    (gear_clicked, refresh_clicked, restart_clicked)
}

fn restart_now() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
    std::process::exit(0);
}

fn feed(ui: &mut egui::Ui, timeline: &mut Timeline, appearance: &Appearance) {
    if ui.input(|input| input.smooth_scroll_delta.y > 1.0) {
        timeline.follow = false;
    }

    // Twitch/streamx render chat rows back-to-back; egui's default vertical
    // item spacing makes the feed look airy.
    ui.spacing_mut().item_spacing.y = appearance.row_gap.clamp(0.0, 24.0);

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
