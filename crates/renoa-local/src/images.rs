//! Images an agent is shown: known by their bytes, not their names, and no
//! larger than every model API Renoa uses accepts.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use renoa_agent::ContentBlock;

/// The longest base64 image shown to the model: Anthropic's API refuses an
/// image over 5 MiB.
const MAX_INLINE_IMAGE_BASE64_BYTES: usize = 5 * 1024 * 1024;

/// The largest image file whose base64 fits [`MAX_INLINE_IMAGE_BASE64_BYTES`].
pub(crate) const MAX_INLINE_IMAGE_BYTES: usize = MAX_INLINE_IMAGE_BASE64_BYTES / 4 * 3;

/// The media type of a JPEG, PNG, GIF or WebP image, read from its first bytes.
pub(crate) fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// `bytes` as an image block, when they are an image small enough to show.
pub(crate) fn inline_image(bytes: &[u8]) -> Option<ContentBlock> {
    let mime_type = image_type(bytes)?;
    (bytes.len() <= MAX_INLINE_IMAGE_BYTES)
        .then(|| ContentBlock::image(STANDARD.encode(bytes), mime_type))
}

/// A file size as a person reads it.
pub(crate) fn shown_size(bytes: usize) -> String {
    const KIB: usize = 1024;
    match bytes {
        0..KIB => format!("{bytes} bytes"),
        KIB..1_048_576 => format!("{} KB", bytes.div_ceil(KIB)),
        _ => format!(
            "{}.{} MB",
            bytes / 1_048_576,
            bytes % 1_048_576 * 10 / 1_048_576
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_INLINE_IMAGE_BASE64_BYTES, MAX_INLINE_IMAGE_BYTES, image_type, inline_image, shown_size,
    };

    #[test]
    fn images_are_known_by_their_bytes() {
        assert_eq!(image_type(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(image_type(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(image_type(b"GIF89a.."), Some("image/gif"));
        assert_eq!(image_type(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_type(b"%PDF-1.7"), None);
    }

    #[test]
    fn the_largest_shown_image_fills_the_base64_limit_exactly() {
        assert_eq!(
            MAX_INLINE_IMAGE_BYTES.div_ceil(3) * 4,
            MAX_INLINE_IMAGE_BASE64_BYTES
        );
        let mut largest = vec![0; MAX_INLINE_IMAGE_BYTES];
        largest[..3].copy_from_slice(&[0xFF, 0xD8, 0xFF]);
        assert!(inline_image(&largest).is_some());
        largest.push(0);
        assert!(inline_image(&largest).is_none());
    }

    #[test]
    fn sizes_read_plainly() {
        assert_eq!(shown_size(900), "900 bytes");
        assert_eq!(shown_size(312 * 1024 - 5), "312 KB");
        assert_eq!(shown_size(3 * 1_048_576 + 524_288), "3.5 MB");
    }
}
