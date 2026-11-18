//! Images an agent is shown: known by their bytes, bounded in resolution and
//! encoded size for model APIs without altering files on disk.

use std::io::Cursor;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{
    DynamicImage, GenericImageView as _, ImageDecoder as _, ImageFormat, ImageReader, Limits,
    RgbImage, codecs::jpeg::JpegEncoder, imageops::FilterType, metadata::Orientation,
};
use renoa_agent::ContentBlock;

/// The longest base64 image shown to the model: Anthropic's API refuses an
/// image over 5 MiB.
const MAX_INLINE_IMAGE_BASE64_BYTES: usize = 5 * 1024 * 1024;

/// The largest image file whose base64 fits [`MAX_INLINE_IMAGE_BASE64_BYTES`].
pub(crate) const MAX_INLINE_IMAGE_BYTES: usize = MAX_INLINE_IMAGE_BASE64_BYTES / 4 * 3;

/// Largest source image read from disk or uploads for normalization before
/// falling back to file-only guidance.
pub(crate) const MAX_SOURCE_IMAGE_BYTES: usize = 32 * 1024 * 1024;

/// Cross-provider sweet-spot longest edge before APIs downscale anyway.
pub(crate) const MAX_IMAGE_LONG_EDGE: u32 = 1_536;

/// Small images within both bounds pass through untouched to preserve crispness
/// on small UI screenshots and diagrams.
const PASSTHROUGH_IMAGE_BYTES: usize = 200 * 1_024;

/// Preferred JPEG quality and step-downs if one dense image still exceeds
/// [`MAX_INLINE_IMAGE_BYTES`].
const JPEG_QUALITIES: [u8; 4] = [82, 70, 55, 40];

/// Upper bounds for decoding untrusted inputs without decompression-bomb OOMs.
const MAX_DECODE_DIMENSION: u32 = 16_384;
const MAX_DECODE_ALLOC_BYTES: u64 = 128 * 1024 * 1024;

/// One model-ready inline image normalized from raw source bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShownImage {
    pub(crate) block: ContentBlock,
    pub(crate) mime_type: &'static str,
    pub(crate) bytes: usize,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) normalized: bool,
}

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

/// Normalizes `bytes` for inline model viewing: keeps compact `<= 1536px`
/// images untouched, and downscales larger or heavier images to `<= 1536px` JPEG.
pub(crate) fn show_image(bytes: &[u8]) -> Option<ShownImage> {
    if bytes.len() > MAX_SOURCE_IMAGE_BYTES {
        return None;
    }
    let mime_type = image_type(bytes)?;
    let format = match mime_type {
        "image/jpeg" => ImageFormat::Jpeg,
        "image/png" => ImageFormat::Png,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::WebP,
        _ => return None,
    };
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::no_limits();
    limits.max_image_width = Some(MAX_DECODE_DIMENSION);
    limits.max_image_height = Some(MAX_DECODE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC_BYTES);
    reader.limits(limits);

    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut raster = DynamicImage::from_decoder(decoder).ok()?;
    raster.apply_orientation(orientation);
    let (width, height) = raster.dimensions();
    if width == 0 || height == 0 {
        return None;
    }

    let fits_edge = width <= MAX_IMAGE_LONG_EDGE && height <= MAX_IMAGE_LONG_EDGE;
    if fits_edge
        && bytes.len() <= PASSTHROUGH_IMAGE_BYTES
        && orientation == Orientation::NoTransforms
    {
        return Some(ShownImage {
            block: ContentBlock::image(STANDARD.encode(bytes), mime_type),
            mime_type,
            bytes: bytes.len(),
            width,
            height,
            normalized: false,
        });
    }

    let resized = if fits_edge {
        raster
    } else {
        raster.resize(
            MAX_IMAGE_LONG_EDGE,
            MAX_IMAGE_LONG_EDGE,
            FilterType::CatmullRom,
        )
    };
    let (out_width, out_height) = resized.dimensions();
    let rgb = flatten_to_white_rgb(resized);
    let encoded = encode_bounded_jpeg(&rgb)?;

    // Keep an un-resized, un-rotated original when recompression would actually
    // inflate it and it already respects the inline limit.
    if fits_edge
        && orientation == Orientation::NoTransforms
        && bytes.len() <= MAX_INLINE_IMAGE_BYTES
        && encoded.len() >= bytes.len()
    {
        return Some(ShownImage {
            block: ContentBlock::image(STANDARD.encode(bytes), mime_type),
            mime_type,
            bytes: bytes.len(),
            width,
            height,
            normalized: false,
        });
    }

    Some(ShownImage {
        bytes: encoded.len(),
        block: ContentBlock::image(STANDARD.encode(&encoded), "image/jpeg"),
        mime_type: "image/jpeg",
        width: out_width,
        height: out_height,
        normalized: true,
    })
}

/// `bytes` as an image block, normalized for the model.
pub(crate) fn inline_image(bytes: &[u8]) -> Option<ContentBlock> {
    show_image(bytes).map(|shown| shown.block)
}

/// Normalizes every [`ContentBlock::Image`] in tool output returned by an
/// external adapter, omitting unreadable or oversized images with a text note.
pub(crate) fn normalize_tool_content(content: Vec<ContentBlock>) -> Vec<ContentBlock> {
    content
        .into_iter()
        .map(|block| match block {
            ContentBlock::Image { data, mime_type } => {
                if let Ok(raw) = STANDARD.decode(&data)
                    && let Some(shown) = show_image(&raw)
                {
                    return shown.block;
                }
                ContentBlock::text(format!(
                    "[omitted {mime_type} image that could not be prepared within {}]",
                    shown_size(MAX_INLINE_IMAGE_BYTES)
                ))
            }
            text @ ContentBlock::Text { .. } => text,
        })
        .collect()
}

fn flatten_to_white_rgb(image: DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.into_rgb8();
    }
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    let mut rgb = RgbImage::new(width, height);
    for (src, dst) in rgba.pixels().zip(rgb.pixels_mut()) {
        let [r, g, b, a] = src.0;
        let alpha = u16::from(a);
        let inv = 255_u16.saturating_sub(alpha);
        let blend = |channel: u8| -> u8 {
            u8::try_from((u16::from(channel) * alpha + 255 * inv + 127) / 255).unwrap_or(u8::MAX)
        };
        dst.0 = [blend(r), blend(g), blend(b)];
    }
    rgb
}

fn encode_bounded_jpeg(rgb: &RgbImage) -> Option<Vec<u8>> {
    for quality in JPEG_QUALITIES {
        let mut jpeg = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut jpeg, quality);
        encoder.encode_image(rgb).ok()?;
        if jpeg.len() <= MAX_INLINE_IMAGE_BYTES {
            return Some(jpeg);
        }
    }
    None
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
pub(crate) mod test_fixtures {
    use std::io::Cursor;

    use image::{ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};

    pub(crate) fn encode_rgb(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let mut img = RgbImage::new(width, height);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            let mix = x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(2_246_822_519);
            let r = u8::try_from((mix ^ (mix >> 16)) % 256).expect("byte");
            let g = u8::try_from(((mix >> 8) ^ (mix >> 24)) % 256).expect("byte");
            let b = u8::try_from(((mix >> 4) ^ (mix >> 20)) % 256).expect("byte");
            *pixel = Rgb([r, g, b]);
        }
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), format)
            .expect("encode rgb fixture");
        out
    }

    pub(crate) fn encode_rgba_png(width: u32, height: u32) -> Vec<u8> {
        let mut img = RgbaImage::new(width, height);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            // Keep top-left quadrant transparent so JPEG flattening over white is observable.
            *pixel = if x < width / 4 && y < height / 4 {
                Rgba([0, 0, 0, 0])
            } else {
                Rgba([20, 40, 200, u8::try_from((x + y) % 256).expect("byte")])
            };
        }
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .expect("encode rgba png fixture");
        out
    }
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use image::{GenericImageView as _, ImageFormat, ImageReader};
    use renoa_agent::ContentBlock;
    use std::io::Cursor;

    use super::{
        MAX_IMAGE_LONG_EDGE, MAX_INLINE_IMAGE_BASE64_BYTES, MAX_INLINE_IMAGE_BYTES,
        MAX_SOURCE_IMAGE_BYTES, image_type, inline_image, normalize_tool_content, show_image,
        shown_size, test_fixtures,
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
    fn small_crisp_images_keep_their_original_bytes_and_format() {
        let png = test_fixtures::encode_rgb(320, 200, ImageFormat::Png);
        let shown = show_image(&png).expect("small crisp png is shown");
        assert!(!shown.normalized);
        assert_eq!(shown.mime_type, "image/png");
        assert_eq!((shown.width, shown.height), (320, 200));
        assert_eq!(shown.bytes, png.len());
    }

    #[test]
    fn oversized_dimensions_and_rgba_pngs_are_resized_and_compressed_to_jpeg() {
        let rgba_png = test_fixtures::encode_rgba_png(2_400, 1_800);
        let shown = show_image(&rgba_png).expect("oversized rgba png normalizes");
        assert!(shown.normalized);
        assert_eq!(shown.mime_type, "image/jpeg");
        assert_eq!((shown.width, shown.height), (MAX_IMAGE_LONG_EDGE, 1_152));
        let ContentBlock::Image { data, mime_type } = shown.block else {
            panic!("expected image block");
        };
        assert_eq!(mime_type, "image/jpeg");
        assert!(data.len() <= MAX_INLINE_IMAGE_BASE64_BYTES);
        let raw = STANDARD.decode(data).expect("valid base64");
        let decoded = ImageReader::with_format(Cursor::new(&raw), ImageFormat::Jpeg)
            .decode()
            .expect("valid jpeg");
        assert_eq!(decoded.dimensions(), (1_536, 1_152));
        let top_left = decoded.get_pixel(0, 0);
        assert!(
            top_left.0[0] >= 235 && top_left.0[1] >= 235 && top_left.0[2] >= 235,
            "transparent region should composite onto white, got {top_left:?}"
        );
    }

    #[test]
    fn heavy_1024px_png_above_passthrough_threshold_is_compressed() {
        let heavy_png = test_fixtures::encode_rgb(1_024, 1_024, ImageFormat::Png);
        assert!(heavy_png.len() > 200 * 1_024);
        let shown = show_image(&heavy_png).expect("heavy png normalizes");
        assert!(shown.normalized);
        assert_eq!(shown.mime_type, "image/jpeg");
        assert_eq!((shown.width, shown.height), (1_024, 1_024));
        assert!(shown.bytes < heavy_png.len());
        assert!(shown.bytes <= MAX_INLINE_IMAGE_BYTES);
    }

    #[test]
    fn corrupt_or_oversized_sources_do_not_produce_inline_blocks() {
        let mut corrupt = vec![0; 128];
        corrupt[..3].copy_from_slice(&[0xFF, 0xD8, 0xFF]);
        assert!(inline_image(&corrupt).is_none());

        let too_large = vec![0; MAX_SOURCE_IMAGE_BYTES + 1];
        assert!(inline_image(&too_large).is_none());
    }

    #[test]
    fn mcp_content_normalization_shrinks_valid_images_and_drops_corrupt_ones() {
        let big_png = test_fixtures::encode_rgb(2_000, 1_000, ImageFormat::Png);
        let out = normalize_tool_content(vec![
            ContentBlock::text("kept"),
            ContentBlock::image(STANDARD.encode(big_png), "image/png"),
            ContentBlock::image("aW1hZ2U=", "image/png"),
        ]);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], ContentBlock::text("kept"));
        assert!(matches!(
            &out[1],
            ContentBlock::Image { mime_type, .. } if mime_type == "image/jpeg"
        ));
        assert!(matches!(&out[2], ContentBlock::Text { text } if text.contains("omitted")));
    }

    #[test]
    fn sizes_read_plainly() {
        assert_eq!(shown_size(900), "900 bytes");
        assert_eq!(shown_size(312 * 1024 - 5), "312 KB");
        assert_eq!(shown_size(3 * 1_048_576 + 524_288), "3.5 MB");
    }
}
