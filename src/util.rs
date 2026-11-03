/// Parses `#RRGGBB`. Rejects non-ASCII input instead of slicing bytes.
pub fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    let hex = value.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    Some([
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ])
}

/// Returns free heap pages to the OS.
///
/// Decoding emotes allocates and frees many small buffers; glibc keeps the
/// freed pages on its free lists, so the high-water mark sticks around as RSS
/// even though nothing uses it.
pub fn release_free_memory() {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rgb_hex() {
        assert_eq!(parse_hex_color("#2E8B57"), Some([0x2E, 0x8B, 0x57]));
        assert_eq!(parse_hex_color("2e8b57"), None);
    }

    #[test]
    fn rejects_non_ascii_without_panicking() {
        assert_eq!(parse_hex_color("#aé€xyz"), None);
        assert_eq!(parse_hex_color("#日本語"), None);
    }
}
