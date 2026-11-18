use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{GenericImageView as _, ImageFormat, ImageReader};
use renoa_agent::ContentBlock;
use std::io::Cursor;

use super::{
    ImageRefusal, MAX_IMAGE_LONG_EDGE, MAX_INLINE_IMAGE_BASE64_BYTES, MAX_INLINE_IMAGE_BYTES,
    MAX_SOURCE_IMAGE_BYTES, image_type, normalize_tool_content, show_image, shown_size,
    test_fixtures,
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
    assert_eq!(show_image(&corrupt), Err(ImageRefusal::Undecodable));

    let too_large = vec![0; MAX_SOURCE_IMAGE_BYTES + 1];
    assert_eq!(
        show_image(&too_large),
        Err(ImageRefusal::TooLarge {
            bytes: MAX_SOURCE_IMAGE_BYTES + 1
        })
    );
    assert_eq!(show_image(b"%PDF-1.7 ..."), Err(ImageRefusal::Unsupported));
}

/// A small JPEG whose header claims 16000x16000 pixels would make the
/// decoder allocate a 768 MB raster; it is refused from the header alone.
#[test]
fn a_jpeg_claiming_huge_dimensions_is_refused_before_decoding() {
    let mut jpeg = test_fixtures::encode_rgb(8, 8, ImageFormat::Jpeg);
    let frame = jpeg
        .windows(2)
        .position(|marker| marker == [0xFF, 0xC0])
        .expect("baseline frame header");
    // Length (2), precision (1), then height and width.
    jpeg[frame + 5..frame + 9].copy_from_slice(&[0x3E, 0x80, 0x3E, 0x80]);
    let refusal = show_image(&jpeg).expect_err("a huge raster is refused");
    assert_eq!(
        refusal,
        ImageRefusal::TooManyPixels {
            size: Some((16_000, 16_000))
        }
    );
    assert!(
        refusal.reason().contains("16000x16000"),
        "{}",
        refusal.reason()
    );
}

#[test]
fn a_tall_screenshot_says_it_was_scaled_down() {
    let tall = test_fixtures::encode_rgb(400, 4_000, ImageFormat::Png);
    let shown = show_image(&tall).expect("tall png normalizes");
    assert_eq!((shown.width, shown.height), (154, MAX_IMAGE_LONG_EDGE));
    assert_eq!((shown.source_width, shown.source_height), (400, 4_000));
    let note = shown.scale_note().expect("a downscaled image says so");
    assert!(note.contains("400x4000"), "{note}");

    let small = test_fixtures::encode_rgb(320, 200, ImageFormat::Png);
    assert_eq!(show_image(&small).expect("small png").scale_note(), None);
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
    assert!(
        matches!(&out[2], ContentBlock::Text { text }
            if text == "[an image (image/png) was left out because it is not a JPEG, PNG, GIF or WebP image]"),
        "{:?}",
        out[2]
    );
}

#[test]
fn sizes_read_plainly() {
    assert_eq!(shown_size(900), "900 bytes");
    assert_eq!(shown_size(312 * 1024 - 5), "312 KB");
    assert_eq!(shown_size(3 * 1_048_576 + 524_288), "3.5 MB");
}
