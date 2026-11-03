use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use egui::{
    ColorImage, TextureHandle, TextureOptions,
    load::{
        BytesPoll, LoadError, SizeHint, SizedTexture, TextureLoadResult, TextureLoader,
        TexturePoll,
    },
    mutex::Mutex,
};
use image_webp::WebPDecoder;

const MAX_FRAMES: u32 = 120;

/// Animation rate shared by every emote in the feed, in frames per second.
///
/// A global tick keeps the redraw rate independent of how many animated emotes
/// are on screen: with a per-emote timer the feed is redrawn on the earliest
/// deadline of them all, which is every display frame once enough emotes are
/// visible.
pub const ANIMATION_FPS: f32 = 15.0;
pub const MIN_FPS: f32 = 5.0;
pub const MAX_FPS: f32 = 30.0;

/// Frame shown at `seconds` of the global animation clock.
pub fn frame_index(seconds: f64, frames: usize, fps: f32) -> usize {
    if frames <= 1 {
        return 0;
    }
    let fps = f64::from(fps.clamp(MIN_FPS, MAX_FPS));
    let ticks = (seconds.max(0.0) * fps) as u64;
    (ticks % frames as u64) as usize
}

/// Time left until the next animation tick.
pub fn next_tick(seconds: f64, fps: f32) -> Duration {
    let fps = f64::from(fps.clamp(MIN_FPS, MAX_FPS));
    let period = 1.0 / fps;
    let phase = seconds.max(0.0) % period;
    Duration::from_secs_f64((period - phase).max(0.001))
}

pub fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

#[derive(Clone)]
struct AnimatedWebp {
    frames: Vec<Arc<ColorImage>>,
}

impl AnimatedWebp {
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        let mut decoder = WebPDecoder::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
        let (width, height) = decoder.dimensions();
        let (width, height) = (width as usize, height as usize);
        let buffer_len = decoder
            .output_buffer_size()
            .unwrap_or(width * height * 4);
        let mut buffer = vec![0u8; buffer_len];
        let mut frames = Vec::new();

        let animated = decoder.is_animated();
        let count = if animated {
            decoder.num_frames().clamp(1, MAX_FRAMES)
        } else {
            1
        };

        for _ in 0..count {
            if animated {
                decoder.read_frame(&mut buffer).map_err(|e| e.to_string())?;
            } else {
                decoder
                    .read_image(&mut buffer)
                    .map_err(|e| e.to_string())?;
            }
            frames.push(Arc::new(to_color_image(&buffer, width, height)?));
        }

        Ok(Self { frames })
    }

    fn size(&self) -> egui::Vec2 {
        let [width, height] = self.frames[0].size;
        egui::vec2(width as f32, height as f32)
    }

    fn byte_len(&self) -> usize {
        self.frames
            .iter()
            .map(|frame| frame.pixels.len() * std::mem::size_of::<egui::Color32>())
            .sum()
    }
}

/// One texture and all decoded frames of a single emote.
///
/// egui's animated-image path asks for `uri#frame` and would otherwise upload a
/// separate texture for every frame of every emote ever shown; we keep one
/// texture and rewrite it whenever the frame changes.
struct Animated {
    webp: Arc<AnimatedWebp>,
    handle: TextureHandle,
    shown: usize,
}

impl Animated {
    fn show(&mut self, index: usize, options: TextureOptions) {
        let index = index % self.webp.frames.len();
        if index != self.shown {
            self.handle
                .set(self.webp.frames[index].as_ref().clone(), options);
            self.shown = index;
        }
    }

    fn texture(&self) -> SizedTexture {
        SizedTexture::new(self.handle.id(), self.webp.size())
    }
}

fn to_color_image(buffer: &[u8], width: usize, height: usize) -> Result<ColorImage, String> {
    let pixels = width * height;
    if buffer.len() >= pixels * 4 {
        return Ok(ColorImage::from_rgba_unmultiplied(
            [width, height],
            &buffer[..pixels * 4],
        ));
    }
    if buffer.len() >= pixels * 3 {
        let mut rgba = Vec::with_capacity(pixels * 4);
        let (chunks, _) = buffer[..pixels * 3].as_chunks::<3>();
        for chunk in chunks {
            rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
        }
        return Ok(ColorImage::from_rgba_unmultiplied([width, height], &rgba));
    }
    Err(format!(
        "webp: неожиданный размер буфера {} для {}x{}",
        buffer.len(),
        width,
        height
    ))
}

type Entry = Result<Animated, String>;

pub struct WebpAnimationLoader {
    cache: Mutex<HashMap<String, Entry>>,
    painted: Mutex<HashSet<String>>,
    animate: AtomicBool,
    fps: AtomicU32,
}

impl Default for WebpAnimationLoader {
    fn default() -> Self {
        Self {
            cache: Mutex::default(),
            painted: Mutex::default(),
            animate: AtomicBool::new(true),
            fps: AtomicU32::new(ANIMATION_FPS.to_bits()),
        }
    }
}

impl WebpAnimationLoader {
    pub const ID: &'static str = "lechat_webp_animation";

    /// Emote animation settings come from the config and can change while the
    /// app runs, so they live behind atomics that the UI thread writes and the
    /// paint path reads.
    pub fn set_animation(&self, animate: bool, fps: f32) {
        self.animate.store(animate, Ordering::Relaxed);
        self.fps
            .store(fps.clamp(MIN_FPS, MAX_FPS).to_bits(), Ordering::Relaxed);
    }

    pub fn animation_enabled(&self) -> bool {
        self.animate.load(Ordering::Relaxed)
    }

    pub fn fps(&self) -> f32 {
        let fps = f32::from_bits(self.fps.load(Ordering::Relaxed));
        if fps.is_finite() {
            fps.clamp(MIN_FPS, MAX_FPS)
        } else {
            ANIMATION_FPS
        }
    }

    /// Bytes of decoded animation frames currently held.
    pub fn cached_bytes(&self) -> usize {
        self.cache
            .lock()
            .values()
            .filter_map(|entry| entry.as_ref().ok())
            .map(|animated| animated.webp.byte_len())
            .sum()
    }

    /// Emotes that were painted since the last call. Only painted emotes need
    /// their pixels; everything else in the feed is invisible.
    pub fn take_painted(&self) -> HashSet<String> {
        std::mem::take(&mut *self.painted.lock())
    }

    /// Returns the texture for the frame that is due at this moment.
    ///
    /// The frame index is derived from the global animation clock instead of a
    /// timer per emote, so a feed full of animated emotes still costs one
    /// redraw per tick rather than one per emote.
    fn cached(&self, ctx: &egui::Context, uri: &str, options: TextureOptions) -> TextureLoadResult {
        self.painted.lock().insert(uri.to_string());
        let now = ctx.input(|input| input.time);
        let fps = self.fps();
        let mut cache = self.cache.lock();
        match cache.get_mut(uri) {
            Some(Ok(animated)) => {
                if self.animation_enabled() {
                    animated.show(frame_index(now, animated.webp.frames.len(), fps), options);
                    if animated.webp.frames.len() > 1 {
                        ctx.request_repaint_after(next_tick(now, fps));
                    }
                }
                Ok(TexturePoll::Ready {
                    texture: animated.texture(),
                })
            }
            Some(Err(err)) => Err(LoadError::Loading(err.clone())),
            None => Err(LoadError::NotSupported),
        }
    }

    fn insert(
        &self,
        ctx: &egui::Context,
        uri: &str,
        options: TextureOptions,
        decoded: Result<AnimatedWebp, String>,
    ) -> TextureLoadResult {
        match decoded {
            Ok(webp) => {
                let handle = ctx.load_texture(uri, webp.frames[0].as_ref().clone(), options);
                let animated = Animated {
                    webp: Arc::new(webp),
                    handle,
                    shown: 0,
                };
                self.cache.lock().insert(uri.to_string(), Ok(animated));
                self.cached(ctx, uri, options)
            }
            Err(err) => {
                self.cache
                    .lock()
                    .insert(uri.to_string(), Err(err.clone()));
                Err(LoadError::Loading(err))
            }
        }
    }
}

impl TextureLoader for WebpAnimationLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(
        &self,
        ctx: &egui::Context,
        frame_uri: &str,
        options: TextureOptions,
        _hint: SizeHint,
    ) -> TextureLoadResult {
        let Ok((uri, _index)) = egui::decode_animated_image_uri(frame_uri) else {
            return Err(LoadError::NotSupported);
        };

        if self.cache.lock().contains_key(uri) {
            return self.cached(ctx, uri, options);
        }

        match ctx.try_load_bytes(uri) {
            Ok(BytesPoll::Ready { bytes, .. }) => {
                if !is_webp(&bytes) {
                    return Err(LoadError::NotSupported);
                }
                self.insert(ctx, uri, options, AnimatedWebp::decode(&bytes))
            }
            Ok(BytesPoll::Pending { size }) => Ok(TexturePoll::Pending { size }),
            Err(err) => Err(err),
        }
    }

    fn forget(&self, uri: &str) {
        let base = egui::decode_animated_image_uri(uri).map_or(uri, |(base, _)| base);
        self.cache.lock().remove(base);
    }

    fn forget_all(&self) {
        self.cache.lock().clear();
    }

    fn byte_size(&self) -> usize {
        self.cached_bytes()
    }
}

/// Bounds the decoded frames and textures that egui never releases on its own:
/// emotes leave the feed after `MAX_ENTRIES` messages, but their pixels stay
/// cached forever.
///
/// Tracks when every URI was last on screen and reports the ones that are safe
/// to forget.
pub struct ImageCache {
    last_seen: HashMap<String, f64>,
    grace_secs: f64,
    budget_bytes: usize,
}

impl ImageCache {
    /// Long enough for a message to come back into view after a scroll.
    pub const GRACE_SECS: f64 = 180.0;
    pub const BUDGET_BYTES: usize = 48 * 1024 * 1024;

    pub fn new() -> Self {
        Self::with_limits(Self::GRACE_SECS, Self::BUDGET_BYTES)
    }

    pub fn with_limits(grace_secs: f64, budget_bytes: usize) -> Self {
        Self {
            last_seen: HashMap::new(),
            grace_secs,
            budget_bytes,
        }
    }

    /// Marks every `used` URI as seen at `now` (seconds).
    pub fn touch(&mut self, now: f64, used: &HashSet<String>) {
        for uri in used {
            self.last_seen.insert(uri.clone(), now);
        }
    }

    /// URIs to forget: everything that left the feed more than `grace_secs`
    /// ago, or — once `cached_bytes` is over budget — everything that is not on
    /// screen right now. Forgetting is reported once.
    pub fn evict(&mut self, now: f64, used: &HashSet<String>, cached_bytes: usize) -> Vec<String> {
        let urgent = cached_bytes > self.budget_bytes;
        let mut victims = Vec::new();
        self.last_seen.retain(|uri, seen| {
            let keep = used.contains(uri) || (!urgent && now - *seen < self.grace_secs);
            if !keep {
                victims.push(uri.clone());
            }
            keep
        });
        victims.sort();
        victims
    }

    #[allow(dead_code)]
    pub fn tracked(&self) -> usize {
        self.last_seen.len()
    }
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_advances_one_frame_per_tick() {
        assert_eq!(frame_index(0.0, 4, 10.0), 0);
        assert_eq!(frame_index(0.099, 4, 10.0), 0);
        assert_eq!(frame_index(0.1, 4, 10.0), 1);
        assert_eq!(frame_index(0.35, 4, 10.0), 3);
        // Loops around after the last frame.
        assert_eq!(frame_index(0.4, 4, 10.0), 0);
    }

    #[test]
    fn static_image_never_advances() {
        assert_eq!(frame_index(1234.5, 1, 10.0), 0);
    }

    #[test]
    fn next_tick_is_within_one_period() {
        assert_eq!(next_tick(0.0, 10.0), Duration::from_millis(100));
        let delay = next_tick(0.08, 10.0);
        assert!(delay > Duration::ZERO);
        assert!(delay <= Duration::from_millis(20));
    }

    #[test]
    fn animation_settings_are_shared_with_the_loader() {
        let loader = WebpAnimationLoader::default();
        assert!(loader.animation_enabled());
        assert_eq!(loader.fps(), ANIMATION_FPS);
        loader.set_animation(false, 8.0);
        assert!(!loader.animation_enabled());
        assert_eq!(loader.fps(), 8.0);
        // Junk from a hand-edited config must not produce a zero-length tick.
        loader.set_animation(true, 0.0);
        assert!(loader.fps() >= MIN_FPS);
        loader.set_animation(true, 1000.0);
        assert!(loader.fps() <= MAX_FPS);
    }

    #[test]
    fn detects_webp_header() {
        assert!(is_webp(b"RIFF\x24\x00\x00\x00WEBPVP8 "));
        assert!(!is_webp(b"\x89PNG\r\n\x1a\n"));
        assert!(!is_webp(b"RIFF"));
    }

    #[test]
    fn ignores_non_webp_for_animation() {
        assert!(!is_webp(b"GIF89a"));
    }

    #[test]
    fn converts_rgb_buffer_without_panicking() {
        let image = to_color_image(&[1, 2, 3, 4, 5, 6], 2, 1).unwrap();
        assert_eq!(image.size, [2, 1]);
        assert_eq!(image.pixels[0], egui::Color32::from_rgb(1, 2, 3));
        assert_eq!(image.pixels[1], egui::Color32::from_rgb(4, 5, 6));
    }

    #[test]
    fn rejects_short_buffer() {
        assert!(to_color_image(&[1, 2, 3], 4, 4).is_err());
    }

    fn uris(items: &[&str]) -> HashSet<String> {
        items.iter().map(|uri| (*uri).to_string()).collect()
    }

    #[test]
    fn keeps_images_that_stay_on_screen() {
        let mut cache = ImageCache::with_limits(60.0, 1024);
        cache.touch(0.0, &uris(&["a"]));
        assert!(cache.evict(10_000.0, &uris(&["a"]), 0).is_empty());
        assert_eq!(cache.tracked(), 1);
    }

    #[test]
    fn forgets_images_after_grace() {
        let mut cache = ImageCache::with_limits(60.0, 1024);
        cache.touch(100.0, &uris(&["a", "b"]));
        assert!(cache.evict(130.0, &uris(&[]), 0).is_empty());
        assert_eq!(cache.evict(200.0, &uris(&[]), 0), vec!["a", "b"]);
        assert_eq!(cache.tracked(), 0);
    }

    #[test]
    fn reports_each_forgotten_image_once() {
        let mut cache = ImageCache::with_limits(60.0, 1024);
        cache.touch(0.0, &uris(&["a"]));
        assert_eq!(cache.evict(120.0, &uris(&[]), 0), vec!["a"]);
        assert!(cache.evict(120.0, &uris(&[]), 0).is_empty());
    }

    #[test]
    fn budget_forgets_hidden_images_immediately() {
        let mut cache = ImageCache::with_limits(600.0, 1024);
        cache.touch(0.0, &uris(&["a", "b"]));
        assert_eq!(cache.evict(1.0, &uris(&["a"]), 2048), vec!["b"]);
    }

    #[test]
    fn budget_never_forgets_visible_images() {
        let mut cache = ImageCache::with_limits(600.0, 0);
        cache.touch(0.0, &uris(&["a"]));
        assert!(cache.evict(1.0, &uris(&["a"]), 1_000_000).is_empty());
    }

    #[test]
    fn forgotten_image_can_be_tracked_again() {
        let mut cache = ImageCache::with_limits(60.0, 1024);
        cache.touch(0.0, &uris(&["a"]));
        assert_eq!(cache.evict(120.0, &uris(&[]), 0), vec!["a"]);
        cache.touch(120.0, &uris(&["a"]));
        assert!(cache.evict(150.0, &uris(&["a"]), 0).is_empty());
    }
}
