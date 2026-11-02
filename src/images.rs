use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use egui::{
    ColorImage, FrameDurations, Id,
    load::{BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint},
    mutex::Mutex,
};
use image_webp::WebPDecoder;

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
        let buffer_len = decoder
            .output_buffer_size()
            .unwrap_or((width as usize) * (height as usize) * 4);
        let mut buffer = vec![0u8; buffer_len];
        let mut frames = Vec::new();
        let mut durations = Vec::new();

        if decoder.is_animated() {
            let count = decoder.num_frames().max(1);
            for _ in 0..count {
                let delay_ms = decoder.read_frame(&mut buffer).map_err(|e| e.to_string())?;
                frames.push(Arc::new(ColorImage::from_rgba_unmultiplied(
                    [width as usize, height as usize],
                    &buffer,
                )));
                durations.push(Duration::from_millis(u64::from(delay_ms.max(20))));
            }
        } else {
            decoder
                .read_image(&mut buffer)
                .map_err(|e| e.to_string())?;
            frames.push(Arc::new(ColorImage::from_rgba_unmultiplied(
                [width as usize, height as usize],
                &buffer,
            )));
            durations.push(Duration::from_secs(1));
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

#[derive(Default)]
pub struct WebpAnimationLoader {
    cache: Mutex<std::collections::HashMap<String, Arc<AnimatedWebp>>>,
}

impl WebpAnimationLoader {
    pub const ID: &'static str = "lechat_webp_animation";
}

impl ImageLoader for WebpAnimationLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &egui::Context, frame_uri: &str, _hint: SizeHint) -> ImageLoadResult {
        let Ok((uri, index)) = egui::decode_animated_image_uri(frame_uri) else {
            return Err(LoadError::NotSupported);
        };

        if let Some(entry) = self.cache.lock().get(uri).cloned() {
            return Ok(ImagePoll::Ready {
                image: entry.frame(index),
            });
        }

        match ctx.try_load_bytes(uri) {
            Ok(BytesPoll::Ready { bytes, .. }) => {
                if !is_webp(&bytes) {
                    return Err(LoadError::NotSupported);
                }
                let decoded = AnimatedWebp::decode(&bytes).map(Arc::new);
                if let Ok(image) = &decoded {
                    let durations = image.durations.clone();
                    ctx.data_mut(|data| {
                        *data.get_temp_mut_or_default(Id::unique(uri)) = durations;
                    });
                }
                match decoded {
                    Ok(image) => {
                        self.cache.lock().insert(uri.to_string(), image.clone());
                        Ok(ImagePoll::Ready {
                            image: image.frame(index),
                        })
                    }
                    Err(err) => Err(LoadError::Loading(err)),
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
        self.cache.lock().values().map(|image| image.byte_len()).sum()
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
}
