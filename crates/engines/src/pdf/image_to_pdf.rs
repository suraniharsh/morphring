//! Raster images to PDF, one image per page, written with krilla.
//!
//! Ported from Filee's `PdfSharpConverter` (reverse-engineering/02 section
//! 9.1). A JPEG goes in byte for byte (krilla's `Image::from_jpeg` wraps it in
//! a DCTDecode stream), everything else is decoded with the `image` crate and
//! embedded losslessly, with any real transparency kept as a soft mask.
//!
//! WEBP is not here: our `image` build has no WEBP decoder, so ImageMagick
//! keeps that edge.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::{AnimationDecoder, DynamicImage, ImageFormat};
use krilla::Document;
use krilla::geom::{Size, Transform};
use krilla::image::{BitsPerComponent, CustomImage, Image, ImageColorspace};
use krilla::page::PageSettings;
use morphring_core::{CancelToken, Edge, EngineError, OutputAllocator, PageSize, Step};

use crate::process;

/// Sources this part turns into PDF pages.
const SOURCES: [&str; 5] = ["png", "jpg", "gif", "bmp", "tiff"];

/// DPI assumed when the file does not say, the same as Filee.
const FALLBACK_DPI: f32 = 96.0;

/// The longest side a PDF page may have, in points (200 inches, the limit
/// Acrobat enforces).
const MAX_PAGE_SIDE_PT: f32 = 14_400.0;

const POINTS_PER_INCH: f32 = 72.0;
const MM_PER_INCH: f32 = 25.4;

/// A4 and Letter in points, portrait.
const A4: (f32, f32) = (595.28, 841.89);
const LETTER: (f32, f32) = (612.0, 792.0);

/// The edges this part handles.
pub(crate) fn edges() -> Vec<Edge> {
    SOURCES.iter().map(|from| Edge::new(from, "pdf")).collect()
}

/// Where one image lands: the page size and the image's box on it, in points,
/// measured from the top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Layout {
    pub(crate) page_width: f32,
    pub(crate) page_height: f32,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

/// Lays out a `width` x `height` pixel image at `dpi` on a page of `size`.
///
/// `FitImage` makes the page the image's physical size, with the long side
/// capped at [`MAX_PAGE_SIDE_PT`]: Filee keeps the 96 DPI fallback, which turns
/// a 30000 px scan into a page no PDF reader will open. A4 and Letter turn
/// landscape for wide images, keep `margin_mm` free on every side, and scale
/// the image to fit (up or down), centred.
pub(crate) fn layout(width: u32, height: u32, dpi: f32, size: PageSize, margin_mm: f32) -> Layout {
    let dpi = if dpi.is_finite() && dpi >= 1.0 {
        dpi
    } else {
        FALLBACK_DPI
    };
    let image_w = width.max(1) as f32 / dpi * POINTS_PER_INCH;
    let image_h = height.max(1) as f32 / dpi * POINTS_PER_INCH;

    let paper = match size {
        PageSize::FitImage => {
            let long = image_w.max(image_h);
            let shrink = if long > MAX_PAGE_SIDE_PT {
                MAX_PAGE_SIDE_PT / long
            } else {
                1.0
            };
            let (w, h) = (image_w * shrink, image_h * shrink);
            return Layout {
                page_width: w,
                page_height: h,
                x: 0.0,
                y: 0.0,
                width: w,
                height: h,
            };
        }
        PageSize::A4 => A4,
        PageSize::Letter => LETTER,
    };
    let (page_width, page_height) = if image_w > image_h {
        (paper.1, paper.0)
    } else {
        paper
    };

    // A margin that eats the whole page would leave a zero-sized box, and
    // krilla (rightly) refuses to draw nothing. Keep at least one point.
    let margin = (margin_mm.max(0.0) / MM_PER_INCH * POINTS_PER_INCH)
        .min((page_width.min(page_height) - 1.0) / 2.0);
    let box_w = page_width - 2.0 * margin;
    let box_h = page_height - 2.0 * margin;
    let scale = (box_w / image_w).min(box_h / image_h);
    let (draw_w, draw_h) = (image_w * scale, image_h * scale);
    Layout {
        page_width,
        page_height,
        x: margin + (box_w - draw_w) / 2.0,
        y: margin + (box_h - draw_h) / 2.0,
        width: draw_w,
        height: draw_h,
    }
}

/// Converts `step`, which is one of [`edges`].
pub(crate) fn convert(
    step: &Step,
    out: &mut dyn OutputAllocator,
    progress: &dyn Fn(f32),
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    if !SOURCES.contains(&step.from.as_str()) || step.to.as_str() != "pdf" {
        return Err(EngineError::Failed(format!(
            "cannot convert {} to {}",
            step.from, step.to
        )));
    }
    let data = std::fs::read(&step.input)?;
    if cancel.is_cancelled() {
        return Err(EngineError::Cancelled);
    }
    let pages = pages(step.from.as_str(), data, cancel)?;
    if pages.is_empty() {
        return Err(EngineError::Failed("the image has no frames".to_owned()));
    }

    let Some(target) = out.allocate("pdf", "") else {
        return Ok(Vec::new());
    };

    let options = &step.options.pdf;
    let mut document = Document::new();
    let count = pages.len();
    for (i, page) in pages.into_iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        let dpi = page.dpi;
        let image = page.image()?;
        let (width, height) = image.size();
        let spot = layout(width, height, dpi, options.page_size, options.margin_mm);
        draw(&mut document, image, &spot)?;
        progress((i + 1) as f32 / count as f32);
    }
    let bytes = document
        .finish()
        .map_err(|e| EngineError::Failed(format!("could not write the PDF: {e}")))?;
    write_into_place(&step.work_dir, &target, &bytes)?;
    Ok(vec![target])
}

/// Adds one page holding `image` at `spot`.
fn draw(document: &mut Document, image: Image, spot: &Layout) -> Result<(), EngineError> {
    let invalid = || EngineError::Failed("the image is too small to place on a page".to_owned());
    let settings = PageSettings::from_wh(spot.page_width, spot.page_height).ok_or_else(invalid)?;
    let size = Size::from_wh(spot.width, spot.height).ok_or_else(invalid)?;
    let mut page = document.start_page_with(settings);
    let mut surface = page.surface();
    surface.push_transform(&Transform::from_translate(spot.x, spot.y));
    surface.draw_image(image, size);
    surface.pop();
    surface.finish();
    page.finish();
    Ok(())
}

/// Writes `bytes` to a temp folder under `work_dir`, then moves it to `target`,
/// so a half-written PDF never shows up where the user is looking.
fn write_into_place(work_dir: &Path, target: &Path, bytes: &[u8]) -> Result<(), EngineError> {
    let tmp = tempfile::Builder::new()
        .prefix("pdf-")
        .tempdir_in(work_dir)?;
    let staged = tmp.path().join("out.pdf");
    std::fs::write(&staged, bytes)?;
    process::move_into_place(&staged, target)?;
    Ok(())
}

/// One page to be: a source image plus the DPI it claims.
struct Page {
    source: Source,
    dpi: f32,
}

/// What a page is made from.
enum Source {
    /// A JPEG file, passed through untouched.
    Jpeg(Vec<u8>),
    /// Decoded pixels.
    Pixels(DynamicImage),
}

impl Page {
    /// The krilla image for this page.
    fn image(self) -> Result<Image, EngineError> {
        match self.source {
            Source::Jpeg(data) => match Image::from_jpeg(data.clone().into(), false) {
                Ok(image) => Ok(image),
                // Some JPEGs (arithmetic coding, odd colour spaces) are too
                // exotic for krilla's sniffing; decoding them is the slow but
                // honest way round.
                Err(_) => {
                    let decoded = decode(&data, ImageFormat::Jpeg)?;
                    pixels_image(&decoded)
                }
            },
            Source::Pixels(image) => pixels_image(&image),
        }
    }
}

/// Splits the file into pages: every frame of a GIF or a multi-page TIFF,
/// otherwise just the first one.
fn pages(from: &str, data: Vec<u8>, cancel: &CancelToken) -> Result<Vec<Page>, EngineError> {
    match from {
        "jpg" => {
            let dpi = jpeg_dpi(&data).unwrap_or(FALLBACK_DPI);
            Ok(vec![Page {
                source: Source::Jpeg(data),
                dpi,
            }])
        }
        "gif" => {
            let decoder =
                image::codecs::gif::GifDecoder::new(Cursor::new(&data)).map_err(decode_error)?;
            let mut pages = Vec::new();
            for frame in decoder.into_frames() {
                if cancel.is_cancelled() {
                    return Err(EngineError::Cancelled);
                }
                let frame = frame.map_err(decode_error)?;
                pages.push(Page {
                    source: Source::Pixels(DynamicImage::ImageRgba8(frame.into_buffer())),
                    dpi: FALLBACK_DPI,
                });
            }
            Ok(pages)
        }
        "tiff" => {
            let ifds = tiff_ifds(&data);
            if ifds.len() <= 1 {
                let dpi = ifds
                    .first()
                    .and_then(|&ifd| tiff_dpi(&data, ifd))
                    .unwrap_or(FALLBACK_DPI);
                let image = decode(&data, ImageFormat::Tiff)?;
                return Ok(vec![Page {
                    source: Source::Pixels(image),
                    dpi,
                }]);
            }
            let mut pages = Vec::with_capacity(ifds.len());
            for ifd in ifds {
                if cancel.is_cancelled() {
                    return Err(EngineError::Cancelled);
                }
                let dpi = tiff_dpi(&data, ifd).unwrap_or(FALLBACK_DPI);
                let image = decode(&tiff_starting_at(&data, ifd), ImageFormat::Tiff)?;
                pages.push(Page {
                    source: Source::Pixels(image),
                    dpi,
                });
            }
            Ok(pages)
        }
        "png" => Ok(vec![Page {
            dpi: png_dpi(&data).unwrap_or(FALLBACK_DPI),
            source: Source::Pixels(decode(&data, ImageFormat::Png)?),
        }]),
        _ => Ok(vec![Page {
            dpi: bmp_dpi(&data).unwrap_or(FALLBACK_DPI),
            source: Source::Pixels(decode(&data, ImageFormat::Bmp)?),
        }]),
    }
}

fn decode(data: &[u8], format: ImageFormat) -> Result<DynamicImage, EngineError> {
    image::load_from_memory_with_format(data, format).map_err(decode_error)
}

fn decode_error(e: image::ImageError) -> EngineError {
    EngineError::Failed(format!("could not read the image: {e}"))
}

/// Decoded pixels in the shape krilla wants: 8-bit gray or RGB, plus an alpha
/// channel only when some pixel is actually see-through.
#[derive(Clone, Hash)]
struct Pixels {
    color: Arc<Vec<u8>>,
    alpha: Option<Arc<Vec<u8>>>,
    size: (u32, u32),
    gray: bool,
}

impl CustomImage for Pixels {
    fn color_channel(&self) -> &[u8] {
        &self.color
    }

    fn alpha_channel(&self) -> Option<&[u8]> {
        self.alpha.as_deref().map(Vec::as_slice)
    }

    fn bits_per_component(&self) -> BitsPerComponent {
        BitsPerComponent::Eight
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn icc_profile(&self) -> Option<&[u8]> {
        None
    }

    fn color_space(&self) -> ImageColorspace {
        if self.gray {
            ImageColorspace::Luma
        } else {
            ImageColorspace::Rgb
        }
    }
}

fn pixels_image(image: &DynamicImage) -> Result<Image, EngineError> {
    let color_type = image.color();
    let gray = !color_type.has_color();
    let size = (image.width(), image.height());
    let (color, alpha) = if color_type.has_alpha() {
        // Split the alpha off; an all-opaque alpha channel is just a slower
        // way of saying "no alpha", so it gets dropped.
        let (color, alpha) = if gray {
            let la = image.to_luma_alpha8();
            split_alpha(la.as_raw(), 1)
        } else {
            let rgba = image.to_rgba8();
            split_alpha(rgba.as_raw(), 3)
        };
        let alpha = (alpha.iter().any(|&a| a != u8::MAX)).then_some(alpha);
        (color, alpha)
    } else if gray {
        (image.to_luma8().into_raw(), None)
    } else {
        (image.to_rgb8().into_raw(), None)
    };
    let pixels = Pixels {
        color: Arc::new(color),
        alpha: alpha.map(Arc::new),
        size,
        gray,
    };
    // No interpolation: pixel art stays crisp, and photos at their own DPI
    // look the same either way.
    Image::from_custom(pixels, false)
        .map_err(|e| EngineError::Failed(format!("could not embed the image: {e}")))
}

/// Splits interleaved samples with `channels` colour components plus one
/// alpha into (colour, alpha).
fn split_alpha(raw: &[u8], channels: usize) -> (Vec<u8>, Vec<u8>) {
    let pixels = raw.len() / (channels + 1);
    let mut color = Vec::with_capacity(pixels * channels);
    let mut alpha = Vec::with_capacity(pixels);
    for px in raw.chunks_exact(channels + 1) {
        color.extend_from_slice(&px[..channels]);
        alpha.push(px[channels]);
    }
    (color, alpha)
}

// --- Density sniffing -------------------------------------------------------
//
// The image crate decodes pixels beautifully and ignores the DPI completely,
// so we read the few bytes that hold it ourselves. Every reader returns None
// when the file does not say, and the caller falls back to 96.

/// A DPI only counts when it is at least 1; zero means "nobody set it".
fn plausible(dpi: f32) -> Option<f32> {
    (dpi.is_finite() && dpi >= 1.0).then_some(dpi)
}

fn be_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn be_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// PNG: the pHYs chunk, when its unit is the metre.
fn png_dpi(data: &[u8]) -> Option<f32> {
    let mut at = 8;
    while at + 8 <= data.len() {
        let len = be_u32(data, at)? as usize;
        let kind = data.get(at + 4..at + 8)?;
        match kind {
            b"pHYs" => {
                let x = be_u32(data, at + 8)?;
                let unit = *data.get(at + 16)?;
                return if unit == 1 {
                    plausible(x as f32 * 0.0254)
                } else {
                    None
                };
            }
            // pHYs must come before the pixels; no point reading megabytes.
            b"IDAT" | b"IEND" => return None,
            _ => at = at.checked_add(12)?.checked_add(len)?,
        }
    }
    None
}

/// JPEG: the JFIF APP0 density (unit 1 is per inch, 2 per centimetre).
fn jpeg_dpi(data: &[u8]) -> Option<f32> {
    if data.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut at = 2;
    // Walk the header markers up to the start of the scan.
    while at + 4 <= data.len() {
        if data[at] != 0xFF {
            return None;
        }
        let marker = data[at + 1];
        let len = be_u16(data, at + 2)? as usize;
        if marker == 0xDA {
            return None;
        }
        if marker == 0xE0 && data.get(at + 4..at + 9) == Some(b"JFIF\0") {
            let unit = *data.get(at + 11)?;
            let x = f32::from(be_u16(data, at + 12)?);
            return match unit {
                1 => plausible(x),
                2 => plausible(x * 2.54),
                _ => None,
            };
        }
        at += 2 + len;
    }
    None
}

/// BMP: pixels per metre in the info header.
fn bmp_dpi(data: &[u8]) -> Option<f32> {
    let header = u32::from_le_bytes(data.get(14..18)?.try_into().ok()?);
    if header < 40 {
        return None;
    }
    let ppm = i32::from_le_bytes(data.get(38..42)?.try_into().ok()?);
    plausible(ppm as f32 * 0.0254)
}

/// Byte order of a classic TIFF, or None for anything else (BigTIFF included,
/// which then gets its first page only).
fn tiff_little_endian(data: &[u8]) -> Option<bool> {
    match data.get(..4)? {
        [b'I', b'I', 42, 0] => Some(true),
        [b'M', b'M', 0, 42] => Some(false),
        _ => None,
    }
}

fn tiff_u16(data: &[u8], at: usize, le: bool) -> Option<u16> {
    let b: [u8; 2] = data.get(at..at + 2)?.try_into().ok()?;
    Some(if le {
        u16::from_le_bytes(b)
    } else {
        u16::from_be_bytes(b)
    })
}

fn tiff_u32(data: &[u8], at: usize, le: bool) -> Option<u32> {
    let b: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
    Some(if le {
        u32::from_le_bytes(b)
    } else {
        u32::from_be_bytes(b)
    })
}

/// Offsets of every IFD (page) in a classic TIFF, in file order.
fn tiff_ifds(data: &[u8]) -> Vec<usize> {
    let Some(le) = tiff_little_endian(data) else {
        return Vec::new();
    };
    let mut ifds = Vec::new();
    let mut next = tiff_u32(data, 4, le).unwrap_or(0) as usize;
    // A loop in the IFD chain is a classic trick of broken writers; the cap
    // and the visited check make sure we don't take part in it.
    while next != 0 && ifds.len() < 10_000 && !ifds.contains(&next) {
        let Some(count) = tiff_u16(data, next, le) else {
            break;
        };
        ifds.push(next);
        next = tiff_u32(data, next + 2 + usize::from(count) * 12, le).unwrap_or(0) as usize;
    }
    ifds
}

/// A copy of the TIFF whose header points at `ifd`, so the decoder reads
/// that page as if it were the first. Cheaper than teaching it to seek.
fn tiff_starting_at(data: &[u8], ifd: usize) -> Vec<u8> {
    let mut copy = data.to_vec();
    let le = tiff_little_endian(data).unwrap_or(true);
    let offset = u32::try_from(ifd).unwrap_or(0);
    let bytes = if le {
        offset.to_le_bytes()
    } else {
        offset.to_be_bytes()
    };
    if let Some(slot) = copy.get_mut(4..8) {
        slot.copy_from_slice(&bytes);
    }
    copy
}

/// TIFF: XResolution (282) with ResolutionUnit (296, inch by default).
fn tiff_dpi(data: &[u8], ifd: usize) -> Option<f32> {
    let le = tiff_little_endian(data)?;
    let count = usize::from(tiff_u16(data, ifd, le)?);
    let mut x_res = None;
    let mut unit = 2;
    for i in 0..count {
        let entry = ifd + 2 + i * 12;
        match tiff_u16(data, entry, le)? {
            282 => {
                let at = tiff_u32(data, entry + 8, le)? as usize;
                let num = tiff_u32(data, at, le)?;
                let den = tiff_u32(data, at + 4, le)?;
                if den != 0 {
                    x_res = Some(num as f32 / den as f32);
                }
            }
            296 => unit = tiff_u16(data, entry + 8, le)?,
            _ => {}
        }
    }
    match unit {
        2 => plausible(x_res?),
        3 => plausible(x_res? * 2.54),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn edges_cover_the_decodable_rasters_at_cost_ten() {
        let pairs: Vec<String> = edges()
            .iter()
            .map(|e| format!("{}>{}@{}", e.from, e.to, e.cost))
            .collect();
        assert_eq!(
            pairs,
            [
                "png>pdf@10",
                "jpg>pdf@10",
                "gif>pdf@10",
                "bmp>pdf@10",
                "tiff>pdf@10"
            ]
        );
    }

    #[test]
    fn fit_image_is_the_physical_size_of_the_image() {
        let l = layout(960, 480, 96.0, PageSize::FitImage, 25.0);
        assert!(close(l.page_width, 720.0) && close(l.page_height, 360.0));
        assert_eq!((l.x, l.y), (0.0, 0.0));
        assert_eq!((l.width, l.height), (l.page_width, l.page_height));
        // 300 DPI: one inch of pixels is one inch of paper.
        let l = layout(300, 600, 300.0, PageSize::FitImage, 0.0);
        assert!(close(l.page_width, 72.0) && close(l.page_height, 144.0));
    }

    #[test]
    fn a_missing_dpi_means_96() {
        assert_eq!(
            layout(96, 96, 0.0, PageSize::FitImage, 0.0),
            layout(96, 96, 96.0, PageSize::FitImage, 0.0)
        );
        assert!(close(
            layout(96, 96, f32::NAN, PageSize::FitImage, 0.0).page_width,
            72.0
        ));
    }

    #[test]
    fn fit_image_caps_the_long_side_at_the_pdf_limit() {
        // 30000 px at 96 DPI would be 22500 pt wide; PDF readers stop at 14400.
        let l = layout(30_000, 15_000, 96.0, PageSize::FitImage, 0.0);
        assert!(close(l.page_width, 14_400.0));
        assert!(close(l.page_height, 7_200.0));
        let l = layout(1_000, 40_000, 96.0, PageSize::FitImage, 0.0);
        assert!(close(l.page_height, 14_400.0));
        assert!(close(l.page_width, 360.0));
    }

    #[test]
    fn a4_portrait_image_is_scaled_and_centred() {
        let l = layout(100, 200, 96.0, PageSize::A4, 0.0);
        assert_eq!((l.page_width, l.page_height), A4);
        // Height-bound: 841.89 tall, half that wide.
        assert!(close(l.height, 841.89));
        assert!(close(l.width, 420.945));
        assert!(close(l.x, (595.28 - 420.945) / 2.0));
        assert!(close(l.y, 0.0));
    }

    #[test]
    fn a_wide_image_turns_the_paper_landscape() {
        let l = layout(400, 300, 96.0, PageSize::Letter, 0.0);
        assert_eq!((l.page_width, l.page_height), (792.0, 612.0));
        // 300 x 225 pt, width-bound on 792 x 612.
        assert!(close(l.width, 792.0) && close(l.height, 594.0));
        assert!(close(l.x, 0.0) && close(l.y, 9.0));
    }

    #[test]
    fn margins_shrink_the_box_on_every_side() {
        // 25.4 mm is exactly 72 pt.
        let l = layout(100, 100, 96.0, PageSize::A4, 25.4);
        let side = 595.28 - 144.0;
        assert!(close(l.width, side) && close(l.height, side));
        assert!(close(l.x, 72.0));
        assert!(close(l.y, (841.89 - side) / 2.0));
    }

    #[test]
    fn a_silly_margin_still_leaves_a_drawable_box() {
        let l = layout(100, 100, 96.0, PageSize::A4, 10_000.0);
        assert!(l.width >= 1.0 - 0.01 && l.height >= 1.0 - 0.01);
    }

    #[test]
    fn png_density_comes_from_phys() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0, 0, 0, 9]);
        png.extend_from_slice(b"pHYs");
        png.extend_from_slice(&11_811u32.to_be_bytes()); // 300 DPI in px/m
        png.extend_from_slice(&11_811u32.to_be_bytes());
        png.push(1);
        png.extend_from_slice(&[0; 4]); // crc, unchecked
        assert!(close(png_dpi(&png).unwrap(), 300.0));
        // Unit 0 is an aspect ratio, not a density.
        png[24] = 0;
        assert_eq!(png_dpi(&png), None);
    }

    #[test]
    fn jpeg_density_comes_from_jfif() {
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16];
        jpg.extend_from_slice(b"JFIF\0");
        jpg.extend_from_slice(&[1, 1, 2]); // version, unit = per cm
        jpg.extend_from_slice(&118u16.to_be_bytes());
        jpg.extend_from_slice(&118u16.to_be_bytes());
        jpg.extend_from_slice(&[0, 0]);
        assert!(close(jpeg_dpi(&jpg).unwrap(), 299.72));
        jpg[13] = 0; // aspect-ratio only
        assert_eq!(jpeg_dpi(&jpg), None);
    }

    /// A little-endian TIFF with `pages` empty IFDs, the first one holding a
    /// 72/1 per-centimetre resolution.
    fn tiff(pages: usize) -> Vec<u8> {
        let mut t = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
        // IFD 0: two entries, at offset 8.
        t.extend_from_slice(&2u16.to_le_bytes());
        t.extend_from_slice(&282u16.to_le_bytes());
        t.extend_from_slice(&5u16.to_le_bytes());
        t.extend_from_slice(&1u32.to_le_bytes());
        t.extend_from_slice(&0u32.to_le_bytes()); // patched below
        t.extend_from_slice(&296u16.to_le_bytes());
        t.extend_from_slice(&3u16.to_le_bytes());
        t.extend_from_slice(&1u32.to_le_bytes());
        t.extend_from_slice(&3u32.to_le_bytes());
        let next_slot = t.len();
        t.extend_from_slice(&0u32.to_le_bytes());
        let rational = t.len() as u32;
        t[18..22].copy_from_slice(&rational.to_le_bytes());
        t.extend_from_slice(&72u32.to_le_bytes());
        t.extend_from_slice(&1u32.to_le_bytes());
        let mut slot = next_slot;
        for _ in 1..pages {
            let at = t.len() as u32;
            t[slot..slot + 4].copy_from_slice(&at.to_le_bytes());
            t.extend_from_slice(&0u16.to_le_bytes());
            slot = t.len();
            t.extend_from_slice(&0u32.to_le_bytes());
        }
        t
    }

    #[test]
    fn tiff_pages_and_density_are_found() {
        let t = tiff(3);
        let ifds = tiff_ifds(&t);
        assert_eq!(ifds.len(), 3);
        assert_eq!(ifds[0], 8);
        assert!(close(tiff_dpi(&t, 8).unwrap(), 72.0 * 2.54));
        assert_eq!(tiff_dpi(&t, ifds[1]), None);
        let moved = tiff_starting_at(&t, ifds[2]);
        assert_eq!(tiff_ifds(&moved), [ifds[2]]);
    }

    #[test]
    fn a_looping_ifd_chain_ends() {
        let mut t = tiff(2);
        let ifds = tiff_ifds(&t);
        // Point page 2 back at page 1.
        let slot = ifds[1] + 2;
        t[slot..slot + 4].copy_from_slice(&8u32.to_le_bytes());
        assert_eq!(tiff_ifds(&t).len(), 2);
    }

    #[test]
    fn bmp_density_is_pixels_per_metre() {
        let mut bmp = vec![0u8; 54];
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[38..42].copy_from_slice(&3_780i32.to_le_bytes());
        assert!(close(bmp_dpi(&bmp).unwrap(), 96.012));
        bmp[38..42].copy_from_slice(&0i32.to_le_bytes());
        assert_eq!(bmp_dpi(&bmp), None);
    }

    #[test]
    fn opaque_alpha_is_dropped_and_real_alpha_kept() {
        let (color, alpha) = split_alpha(&[1, 2, 3, 255, 4, 5, 6, 128], 3);
        assert_eq!(color, [1, 2, 3, 4, 5, 6]);
        assert_eq!(alpha, [255, 128]);
    }
}
