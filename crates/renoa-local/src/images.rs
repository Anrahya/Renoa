//! Images an agent is shown: known by their bytes, bounded in resolution and
//! encoded size for model APIs without altering files on disk.

use std::{
    io::Cursor,
    sync::{Mutex, PoisonError},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{
    DynamicImage, GenericImageView as _, ImageDecoder as _, ImageError, ImageFormat, ImageReader,
    Limits, RgbImage, codecs::jpeg::JpegEncoder, imageops::FilterType, metadata::Orientation,
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

/// The widest or tallest image decoded.
const MAX_DECODE_DIMENSION: u32 = 16_384;

/// The largest decoded raster, checked against the decoder's own size before
/// any pixel buffer is allocated: a 64-megapixel RGB photo fits. The decoder
/// limit also bounds what the PNG decoder allocates while decoding.
const MAX_DECODE_ALLOC_BYTES: u64 = 192 * 1024 * 1024;

/// One image is decoded at a time, so concurrent reads, uploads and tool
/// results never hold more than one raster of [`MAX_DECODE_ALLOC_BYTES`].
static DECODING: Mutex<()> = Mutex::new(());

/// One model-ready inline image normalized from raw source bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShownImage {
    pub(crate) block: ContentBlock,
    pub(crate) mime_type: &'static str,
    pub(crate) bytes: usize,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// The upright size of the source, before any downscale.
    pub(crate) source_width: u32,
    pub(crate) source_height: u32,
    pub(crate) normalized: bool,
}

impl ShownImage {
    /// Whether the image was made smaller than its source, so fine detail
    /// such as small text may not be legible.
    pub(crate) fn downscaled(&self) -> bool {
        (self.width, self.height) != (self.source_width, self.source_height)
    }

    /// A line that says the image was made smaller, or `None` at full size.
    pub(crate) fn scale_note(&self) -> Option<String> {
        self.downscaled().then(|| {
            format!(
                "It was scaled down from {}x{}, so small text or fine detail may be unreadable; \
                 crop the part you need with bash and look at that.",
                self.source_width, self.source_height
            )
        })
    }
}

/// Why an image cannot be shown inline, worded for the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageRefusal {
    /// The source file is larger than [`MAX_SOURCE_IMAGE_BYTES`].
    TooLarge { bytes: usize },
    /// Not a JPEG, PNG, GIF or WebP image.
    Unsupported,
    /// The bytes do not decode as the image they claim to be.
    Undecodable,
    /// Decoding would allocate more than [`MAX_DECODE_ALLOC_BYTES`].
    TooManyPixels { size: Option<(u32, u32)> },
    /// Still over [`MAX_INLINE_IMAGE_BYTES`] at the lowest JPEG quality.
    TooDense,
    /// The JPEG encoder refused the decoded raster.
    Unencodable,
}

impl ImageRefusal {
    /// Why, as the end of a sentence.
    pub(crate) fn reason(self) -> String {
        match self {
            Self::TooLarge { bytes } => format!(
                "the file is {}, over the {} an image read takes",
                shown_size(bytes),
                shown_size(MAX_SOURCE_IMAGE_BYTES)
            ),
            Self::Unsupported => "it is not a JPEG, PNG, GIF or WebP image".to_owned(),
            Self::Undecodable => {
                "its data does not decode as an image, so the file may be damaged".to_owned()
            }
            Self::TooManyPixels {
                size: Some((width, height)),
            } => format!("it is {width}x{height} pixels, more than can be decoded at once"),
            Self::TooManyPixels { size: None } => {
                "it has more pixels than can be decoded at once".to_owned()
            }
            Self::Unencodable => "it could not be re-encoded as a JPEG".to_owned(),
            Self::TooDense => format!(
                "it does not compress under {} even at low quality",
                shown_size(MAX_INLINE_IMAGE_BYTES)
            ),
        }
    }
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
///
/// # Errors
///
/// Returns why the image cannot be shown.
pub(crate) fn show_image(bytes: &[u8]) -> Result<ShownImage, ImageRefusal> {
    if bytes.len() > MAX_SOURCE_IMAGE_BYTES {
        return Err(ImageRefusal::TooLarge { bytes: bytes.len() });
    }
    let mime_type = image_type(bytes).ok_or(ImageRefusal::Unsupported)?;
    let format = match mime_type {
        "image/jpeg" => ImageFormat::Jpeg,
        "image/png" => ImageFormat::Png,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::WebP,
        _ => return Err(ImageRefusal::Unsupported),
    };
    let _decoding = DECODING.lock().unwrap_or_else(PoisonError::into_inner);
    let (raster, orientation) = decode(bytes, format)?;
    let (width, height) = raster.dimensions();
    if width == 0 || height == 0 {
        return Err(ImageRefusal::Undecodable);
    }

    let fits_edge = width <= MAX_IMAGE_LONG_EDGE && height <= MAX_IMAGE_LONG_EDGE;
    let original = || ShownImage {
        block: ContentBlock::image(STANDARD.encode(bytes), mime_type),
        mime_type,
        bytes: bytes.len(),
        width,
        height,
        source_width: width,
        source_height: height,
        normalized: false,
    };
    if fits_edge
        && bytes.len() <= PASSTHROUGH_IMAGE_BYTES
        && orientation == Orientation::NoTransforms
    {
        return Ok(original());
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
        return Ok(original());
    }

    Ok(ShownImage {
        bytes: encoded.len(),
        block: ContentBlock::image(STANDARD.encode(&encoded), "image/jpeg"),
        mime_type: "image/jpeg",
        width: out_width,
        height: out_height,
        source_width: width,
        source_height: height,
        normalized: true,
    })
}

/// Decodes the first frame upright. The decoder reports the size of the
/// raster it would produce before allocating it, and an image over the
/// bounds is refused there: not every codec enforces [`Limits::max_alloc`].
fn decode(bytes: &[u8], format: ImageFormat) -> Result<(DynamicImage, Orientation), ImageRefusal> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::no_limits();
    limits.max_alloc = Some(MAX_DECODE_ALLOC_BYTES);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|error| refusal(&error))?;
    let (width, height) = decoder.dimensions();
    if width > MAX_DECODE_DIMENSION
        || height > MAX_DECODE_DIMENSION
        || decoder.total_bytes() > MAX_DECODE_ALLOC_BYTES
    {
        return Err(ImageRefusal::TooManyPixels {
            size: Some((width, height)),
        });
    }
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut raster = DynamicImage::from_decoder(decoder).map_err(|error| refusal(&error))?;
    raster.apply_orientation(orientation);
    Ok((raster, orientation))
}

fn refusal(error: &ImageError) -> ImageRefusal {
    match error {
        ImageError::Limits(_) => ImageRefusal::TooManyPixels { size: None },
        _ => ImageRefusal::Undecodable,
    }
}

/// Normalizes every [`ContentBlock::Image`] in tool output returned by an
/// external adapter, replacing one that cannot be shown with a note that says why.
pub(crate) fn normalize_tool_content(content: Vec<ContentBlock>) -> Vec<ContentBlock> {
    content
        .into_iter()
        .map(|block| match block {
            ContentBlock::Image { data, mime_type } => {
                let shown = STANDARD
                    .decode(&data)
                    .map_err(|_| ImageRefusal::Undecodable)
                    .and_then(|raw| show_image(&raw));
                match shown {
                    Ok(shown) => shown.block,
                    Err(refusal) => ContentBlock::text(format!(
                        "[an image ({mime_type}) was left out because {}]",
                        refusal.reason()
                    )),
                }
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

fn encode_bounded_jpeg(rgb: &RgbImage) -> Result<Vec<u8>, ImageRefusal> {
    for quality in JPEG_QUALITIES {
        let mut jpeg = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut jpeg, quality);
        encoder
            .encode_image(rgb)
            .map_err(|_| ImageRefusal::Unencodable)?;
        if jpeg.len() <= MAX_INLINE_IMAGE_BYTES {
            return Ok(jpeg);
        }
    }
    Err(ImageRefusal::TooDense)
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
mod tests;
