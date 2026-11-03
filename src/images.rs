use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use egui::{
    ColorImage, FrameDurations, Id, TextureHandle, TextureOptions,
    load::{
        BytesPoll, LoadError, SizeHint, SizedTexture, TextureLoadResult, TextureLoader,
        TexturePoll,
    },
    mutex::Mutex,
};
use image_webp::WebPDecoder;

const MAX_FRAMES: u32 = 120;

pub fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

#[derive(Clone)]
struct AnimatedWebp {
    frames: Vec<Arc<ColorImage>>,
    durations: FrameDurations,
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
        let mut durations = Vec::new();

        let animated = decoder.is_animated();
        let count = if animated {
            decoder.num_frames().clamp(1, MAX_FRAMES)
        } else {
            1
        };

        for _ in 0..count {
            let delay_ms = if animated {
                decoder.read_frame(&mut buffer).map_err(|e| e.to_string())?
            } else {
                decoder
                    .read_image(&mut buffer)
                    .map_err(|e| e.to_string())?;
                1000
            };
            frames.push(Arc::new(to_color_image(&buffer, width, height)?));
            // Cap the redraw rate: egui relayouts the whole feed per animation frame.
            durations.push(Duration::from_millis(u64::from(delay_ms.max(60))));
        }

        Ok(Self {
            frames,
            durations: FrameDurations::new(durations),
        })
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

#[derive(Default)]
pub struct WebpAnimationLoader {
    cache: Mutex<HashMap<String, Entry>>,
    painted: Mutex<HashSet<String>>,
}

impl WebpAnimationLoader {
    pub const ID: &'static str = "lechat_webp_animation";

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

    fn cached(&self, uri: &str, index: usize, options: TextureOptions) -> TextureLoadResult {
        self.painted.lock().insert(uri.to_string());
        let mut cache = self.cache.lock();
        match cache.get_mut(uri) {
            Some(Ok(animated)) => {
                animated.show(index, options);
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
        index: usize,
        options: TextureOptions,
        decoded: Result<AnimatedWebp, String>,
    ) -> TextureLoadResult {
        match decoded {
            Ok(webp) => {
                let webp = Arc::new(webp);
                ctx.data_mut(|data| {
                    *data.get_temp_mut_or_default(Id::unique(uri)) = webp.durations.clone();
                });
                let handle = ctx.load_texture(uri, webp.frames[0].as_ref().clone(), options);
                let mut animated = Animated {
                    webp,
                    handle,
                    shown: 0,
                };
                animated.show(index, options);
                let texture = animated.texture();
                self.cache.lock().insert(uri.to_string(), Ok(animated));
                self.painted.lock().insert(uri.to_string());
                Ok(TexturePoll::Ready { texture })
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
        let Ok((uri, index)) = egui::decode_animated_image_uri(frame_uri) else {
            return Err(LoadError::NotSupported);
        };

        if self.cache.lock().contains_key(uri) {
            return self.cached(uri, index, options);
        }

        match ctx.try_load_bytes(uri) {
            Ok(BytesPoll::Ready { bytes, .. }) => {
                if !is_webp(&bytes) {
                    return Err(LoadError::NotSupported);
                }
                self.insert(ctx, uri, index, options, AnimatedWebp::decode(&bytes))
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
