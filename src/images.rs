use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use egui::{
    ColorImage, FrameDurations, Id,
    load::{BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint},
    mutex::Mutex,
};
use image_webp::WebPDecoder;

/// Animated emotes repaint the whole feed, so they are opt-in.
pub static ANIMATION_ENABLED: AtomicBool = AtomicBool::new(false);

const MAX_FRAMES: u32 = 200;

pub fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

#[derive(Clone)]
struct AnimatedWebp {
    frames: Vec<Arc<ColorImage>>,
    durations: FrameDurations,
}

impl AnimatedWebp {
    fn decode(bytes: &[u8], animate: bool) -> Result<Self, String> {
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
        let count = if animated && animate {
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
            durations.push(Duration::from_millis(u64::from(delay_ms.max(20))));
        }

        Ok(Self {
            frames,
            durations: FrameDurations::new(durations),
        })
    }

    fn frame(&self, index: usize) -> Arc<ColorImage> {
        Arc::clone(&self.frames[index % self.frames.len()])
    }

    fn byte_len(&self) -> usize {
        self.frames
            .iter()
            .map(|frame| frame.pixels.len() * std::mem::size_of::<egui::Color32>())
            .sum()
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

type Entry = Result<Arc<AnimatedWebp>, String>;

#[derive(Default)]
pub struct WebpAnimationLoader {
    cache: Mutex<std::collections::HashMap<String, Entry>>,
}

impl WebpAnimationLoader {
    pub const ID: &'static str = "lechat_webp_animation";

    fn durations_for(image: &AnimatedWebp, animate: bool) -> FrameDurations {
        if animate {
            image.durations.clone()
        } else {
            FrameDurations::new(Vec::new())
        }
    }
}

impl ImageLoader for WebpAnimationLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &egui::Context, frame_uri: &str, _hint: SizeHint) -> ImageLoadResult {
        let Ok((uri, index)) = egui::decode_animated_image_uri(frame_uri) else {
            return Err(LoadError::NotSupported);
        };
        let animate = ANIMATION_ENABLED.load(Ordering::Relaxed);

        if let Some(entry) = self.cache.lock().get(uri).cloned() {
            return match entry {
                Ok(image) => {
                    let durations = Self::durations_for(&image, animate);
                    ctx.data_mut(|data| {
                        *data.get_temp_mut_or_default(Id::unique(uri)) = durations;
                    });
                    Ok(ImagePoll::Ready {
                        image: image.frame(if animate { index } else { 0 }),
                    })
                }
                Err(err) => Err(LoadError::Loading(err)),
            };
        }

        match ctx.try_load_bytes(uri) {
            Ok(BytesPoll::Ready { bytes, .. }) => {
                if !is_webp(&bytes) {
                    return Err(LoadError::NotSupported);
                }
                let decoded = AnimatedWebp::decode(&bytes, animate).map(Arc::new);
                match &decoded {
                    Ok(image) => {
                        let durations = Self::durations_for(image, animate);
                        ctx.data_mut(|data| {
                            *data.get_temp_mut_or_default(Id::unique(uri)) = durations;
                        });
                        self.cache
                            .lock()
                            .insert(uri.to_string(), Ok(Arc::clone(image)));
                        Ok(ImagePoll::Ready {
                            image: image.frame(if animate { index } else { 0 }),
                        })
                    }
                    Err(err) => {
                        self.cache
                            .lock()
                            .insert(uri.to_string(), Err(err.clone()));
                        Err(LoadError::Loading(err.clone()))
                    }
                }
            }
            Ok(BytesPoll::Pending { size }) => Ok(ImagePoll::Pending { size }),
            Err(err) => Err(err),
        }
    }

    fn forget(&self, uri: &str) {
        self.cache.lock().remove(uri);
    }

    fn forget_all(&self) {
        self.cache.lock().clear();
    }

    fn byte_size(&self) -> usize {
        self.cache
            .lock()
            .values()
            .filter_map(|entry| entry.as_ref().ok())
            .map(|image| image.byte_len())
            .sum()
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
}
