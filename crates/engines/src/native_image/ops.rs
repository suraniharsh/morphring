//! The pure half of the built-in image engine: its edges, which presets it
//! can honour, and what each preset option does to the pixels.
//!
//! Ported from the parts of Filee's `ImageEncoder.ApplyOptions` and
//! `ImageEncoder.Write` (reverse-engineering/02 section 4.6) that make sense
//! for the formats the `image` crate writes. Nothing here touches a file:
//! decoding and the output dance live in the parent module.

use image::imageops::{self, FilterType};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use morphring_core::{ConvertOptions, Edge, FormatId, ImageOptions, Resize, TiffCompression};

/// The formats this engine reads and writes, by MorphRing format id. `ppm`
/// covers the whole Netpbm family (pgm, pbm and pnm are its extensions).
pub const FORMATS: [&str; 9] = [
    "png", "jpg", "gif", "bmp", "tiff", "ico", "qoi", "ppm", "tga",
];

/// Cost of a native edge: below ImageMagick's 10, so the planner prefers the
/// engine that does not need to start a process.
pub const COST: u32 = 8;

/// Cost of a still picture into GIF, Filee's `ImageEncoder.Edge` value. A
/// photo heading for video should travel as a full-colour PNG, not a
/// 256-colour GIF, and this number is what keeps it on the right road.
pub const STILL_TO_GIF_COST: u32 = 12;

/// Longest side a JPEG may have. The format's real limit is 65,535; 65,500
/// keeps clear of encoder rounding (Filee's `MaxSide`, issue #50).
pub const JPEG_MAX_SIDE: u32 = 65_500;

/// The largest decoded picture we agree to hold, in pixels (Filee's cap for
/// rasterised SVG).
pub const MAX_PIXELS: u64 = 100_000_000;

/// The decoder memory budget handed to `image::Limits`.
pub const MAX_ALLOC: u64 = 512 * 1024 * 1024;

/// JPEG quality when neither the command line nor the preset says otherwise.
pub const DEFAULT_QUALITY: u8 = 90;

/// Every direct conversion of the engine: each format to each format, self
/// edges included (that is how a preset resizes a PNG into a PNG).
pub const EDGES: &[Edge] = &EDGE_TABLE;

const EDGE_TABLE: [Edge; FORMATS.len() * FORMATS.len()] = build_edges();

// Same 1970s double loop as magick's table: const fn still has no iterators,
// and a baked-in table is worth a little nostalgia.
const fn build_edges() -> [Edge; FORMATS.len() * FORMATS.len()] {
    let mut edges = [Edge::new("", ""); FORMATS.len() * FORMATS.len()];
    let mut n = 0;
    let mut i = 0;
    while i < FORMATS.len() {
        let mut j = 0;
        while j < FORMATS.len() {
            edges[n] = Edge::with_cost(FORMATS[i], FORMATS[j], edge_cost(FORMATS[i], FORMATS[j]));
            n += 1;
            j += 1;
        }
        i += 1;
    }
    edges
}

/// 12 for a still picture into GIF, 8 for everything else. TIFF counts as a
/// still here: this engine only ever reads its first page.
const fn edge_cost(from: &str, to: &str) -> u32 {
    if is(to, "gif") && !is(from, "gif") {
        STILL_TO_GIF_COST
    } else {
        COST
    }
}

const fn is(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Whether the engine can honour every option in `image`. A preset that asks
/// for something it does not implement is declined, so the planner routes
/// the job to ImageMagick instead of quietly ignoring the request.
pub fn accepts(image: &ImageOptions) -> bool {
    // Stripping metadata is free (the encoders write none of it), and so is
    // keeping the ICC profile with the rest. Stripping everything *except*
    // the profile is ImageMagick's party trick, not ours.
    let selective_strip = !image.keep_metadata && image.keep_color_profile;
    let dpi = image.dpi.is_some_and(|d| d > 0);
    let tiff = !matches!(
        image.tiff_compression,
        TiffCompression::Lzw | TiffCompression::None
    );
    // Background removal: nobody implements it yet, us included, and a
    // decline beats an opaque PNG labelled `_transparent`.
    !(selective_strip || dpi || tiff || image.webp_lossless || image.remove_background)
}

/// The `image` crate format for a MorphRing format id.
pub fn image_format(id: FormatId) -> Option<ImageFormat> {
    Some(match id.as_str() {
        "png" => ImageFormat::Png,
        "jpg" => ImageFormat::Jpeg,
        "gif" => ImageFormat::Gif,
        "bmp" => ImageFormat::Bmp,
        "tiff" => ImageFormat::Tiff,
        "ico" => ImageFormat::Ico,
        "qoi" => ImageFormat::Qoi,
        "ppm" => ImageFormat::Pnm,
        "tga" => ImageFormat::Tga,
        _ => return None,
    })
}

/// JPEG quality: the command line's `-q` wins, then the preset's own quality,
/// then 90. Always within 1 to 100.
pub fn quality(opts: &ConvertOptions) -> u8 {
    let q = opts.quality.unwrap_or(opts.image.quality);
    if q == 0 { DEFAULT_QUALITY } else { q.min(100) }
}

/// Targets with no alpha channel, where transparency is flattened onto the
/// preset's background. BMP could store alpha, but most readers ignore it
/// and show whatever junk the transparent pixels hide, so it is flattened too.
pub fn flattens(to: FormatId) -> bool {
    matches!(to.as_str(), "jpg" | "ppm" | "bmp")
}

/// The preset's background as RGB. `#RGB`, `#RGBA`, `#RRGGBB` and
/// `#RRGGBBAA` are understood (the alpha is ignored); anything else is white.
pub fn background(color: &str) -> [u8; 3] {
    const WHITE: [u8; 3] = [255, 255, 255];
    let Some(hex) = color.trim().strip_prefix('#') else {
        return WHITE;
    };
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return WHITE;
    }
    let digit = |i: usize| u8::from_str_radix(&hex[i..=i], 16).unwrap_or(15);
    let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(255);
    match hex.len() {
        // #abc means #aabbcc, so each digit is multiplied by 17 (0xf * 17 = 0xff).
        3 | 4 => [digit(0) * 17, digit(1) * 17, digit(2) * 17],
        6 | 8 => [pair(0), pair(2), pair(4)],
        _ => WHITE,
    }
}

/// The size the preset's resize mode asks for, or `None` to keep `w x h`.
/// Every mode only ever shrinks: a percentage of 100 or more, or a box the
/// picture already fits, leaves it alone.
pub fn resize_target(w: u32, h: u32, image: &ImageOptions) -> Option<(u32, u32)> {
    let scale = match image.resize {
        Resize::None => return None,
        Resize::Percent => {
            if image.resize_percent == 0 || image.resize_percent >= 100 {
                return None;
            }
            f64::from(image.resize_percent) / 100.0
        }
        Resize::LongEdge => {
            let long = w.max(h);
            if image.long_edge == 0 || long <= image.long_edge {
                return None;
            }
            f64::from(image.long_edge) / f64::from(long)
        }
        Resize::Width => {
            if image.width == 0 || w <= image.width {
                return None;
            }
            f64::from(image.width) / f64::from(w)
        }
    };
    Some((scaled(w, scale), scaled(h, scale)))
}

/// The size that fits `w x h` inside `max x max`, or `None` when it already fits.
pub fn fit_max_side(w: u32, h: u32, max: u32) -> Option<(u32, u32)> {
    let long = w.max(h);
    if long <= max {
        return None;
    }
    let scale = f64::from(max) / f64::from(long);
    Some((scaled(w, scale).min(max), scaled(h, scale).min(max)))
}

/// `side * scale`, rounded, and never below one pixel. A 1 x 4000 strip at
/// 10 % is still a strip, not a divide-by-zero.
fn scaled(side: u32, scale: f64) -> u32 {
    let v = (f64::from(side) * scale).round();
    // The scale is at most 1, so the result always fits back into a u32.
    if v < 1.0 { 1 } else { v as u32 }
}

/// Filee's `PrepareIcon` size list: 1 to 256 px, no duplicates, largest
/// first, with `256, 48, 32, 16` when nothing usable is left.
pub fn ico_sizes(sizes: &[u32]) -> Vec<u32> {
    let mut sizes: Vec<u32> = sizes
        .iter()
        .copied()
        .filter(|s| (1..=256).contains(s))
        .collect();
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes.dedup();
    if sizes.is_empty() {
        sizes = vec![256, 48, 32, 16];
    }
    sizes
}

/// Filee's `ApplyOptions` for one decoded (and already oriented) frame:
/// resize, fit the target's size limit, flatten alpha, grayscale.
///
/// Flattening runs before grayscale so a coloured background turns grey
/// together with the picture, instead of the picture going grey and the
/// background arriving in technicolour afterwards.
pub fn apply(img: DynamicImage, to: FormatId, opts: &ConvertOptions) -> DynamicImage {
    let image = &opts.image;
    let mut img = img;
    if let Some((w, h)) = resize_target(img.width(), img.height(), image) {
        img = img.resize_exact(w, h, FilterType::Lanczos3);
    }
    if to.as_str() == "jpg"
        && let Some((w, h)) = fit_max_side(img.width(), img.height(), JPEG_MAX_SIDE)
    {
        img = img.resize_exact(w, h, FilterType::Lanczos3);
    }
    if flattens(to) && img.color().has_alpha() {
        img = flatten(&img, background(&image.background));
    }
    if image.grayscale {
        img = img.grayscale();
    }
    img
}

/// Composites `img` over an opaque `bg` and drops the alpha channel.
pub fn flatten(img: &DynamicImage, bg: [u8; 3]) -> DynamicImage {
    let mut rgba = img.to_rgba8();
    for px in rgba.pixels_mut() {
        let a = u16::from(px[3]);
        for c in 0..3 {
            // Integer "over" with rounding: (fg * a + bg * (255 - a)) / 255.
            let mixed = u16::from(px[c]) * a + u16::from(bg[c]) * (255 - a);
            px[c] = u8::try_from((mixed + 127) / 255).unwrap_or(u8::MAX);
        }
        px[3] = 255;
    }
    DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(rgba).to_rgb8())
}

/// Pads `img` to a square with transparency, centred, the way icons like it.
pub fn square(img: &DynamicImage) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    let side = w.max(h);
    let mut canvas = RgbaImage::from_pixel(side, side, Rgba([0, 0, 0, 0]));
    imageops::overlay(
        &mut canvas,
        &img.to_rgba8(),
        i64::from((side - w) / 2),
        i64::from((side - h) / 2),
    );
    canvas
}

/// The pixel layout `img` is written in for target `to`: whatever the
/// encoder accepts, staying as close to the decoded data as it allows.
pub fn for_target(img: DynamicImage, to: FormatId) -> DynamicImage {
    use DynamicImage as D;
    let alpha = img.color().has_alpha();
    let gray = matches!(
        img,
        D::ImageLuma8(_) | D::ImageLumaA8(_) | D::ImageLuma16(_) | D::ImageLumaA16(_)
    );
    let deep = img.color().bytes_per_pixel() / img.color().channel_count() > 1;
    match to.as_str() {
        // PNG takes every 8 and 16-bit layout; only float pixels need a nudge.
        "png" => match img {
            D::ImageRgb32F(_) => D::ImageRgb16(img.to_rgb16()),
            D::ImageRgba32F(_) => D::ImageRgba16(img.to_rgba16()),
            other => other,
        },
        // TIFF has no gray + alpha layout in the encoder, so those go RGBA.
        "tiff" => match img {
            D::ImageLuma8(_) | D::ImageRgb8(_) | D::ImageRgba8(_) => img,
            D::ImageLuma16(_) | D::ImageRgb16(_) | D::ImageRgba16(_) => img,
            D::ImageLumaA8(_) => D::ImageRgba8(img.to_rgba8()),
            _ if alpha => D::ImageRgba16(img.to_rgba16()),
            _ => D::ImageRgb16(img.to_rgb16()),
        },
        "jpg" | "bmp" if gray => D::ImageLuma8(img.to_luma8()),
        "jpg" | "bmp" | "ppm" => D::ImageRgb8(img.to_rgb8()),
        "tga" if gray && alpha => D::ImageLumaA8(img.to_luma_alpha8()),
        "tga" if gray => D::ImageLuma8(img.to_luma8()),
        "tga" | "qoi" if !alpha && (deep || gray || !matches!(img, D::ImageRgb8(_))) => {
            D::ImageRgb8(img.to_rgb8())
        }
        "tga" | "qoi" if alpha && !matches!(img, D::ImageRgba8(_)) => D::ImageRgba8(img.to_rgba8()),
        "tga" | "qoi" => img,
        // gif and ico frames are always RGBA8; anything else unknown gets the same.
        _ => D::ImageRgba8(img.to_rgba8()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, LumaA, Rgb};

    fn fmt(id: &'static str) -> FormatId {
        FormatId::new(id)
    }

    #[test]
    fn every_pair_is_an_edge_and_still_to_gif_costs_more() {
        assert_eq!(EDGES.len(), 81);
        for edge in EDGES {
            let want = if edge.to.as_str() == "gif" && edge.from.as_str() != "gif" {
                12
            } else {
                8
            };
            assert_eq!(edge.cost, want, "{} -> {}", edge.from, edge.to);
            assert!(image_format(edge.from).is_some());
        }
        assert!(
            EDGES
                .iter()
                .any(|e| e.from == e.to && e.from.as_str() == "png")
        );
    }

    #[test]
    fn quality_prefers_the_command_line_then_the_preset() {
        let mut opts = ConvertOptions::default();
        assert_eq!(quality(&opts), 90);
        opts.image.quality = 70;
        assert_eq!(quality(&opts), 70);
        opts.quality = Some(40);
        assert_eq!(quality(&opts), 40);
        opts.quality = Some(250);
        assert_eq!(quality(&opts), 100);
        opts.quality = Some(0);
        assert_eq!(quality(&opts), 90);
    }

    #[test]
    fn backgrounds_parse_or_fall_back_to_white() {
        assert_eq!(background("#000000"), [0, 0, 0]);
        assert_eq!(background(" #ff8000 "), [255, 128, 0]);
        assert_eq!(background("#f80"), [255, 136, 0]);
        assert_eq!(background("#f80c"), [255, 136, 0]);
        assert_eq!(background("#11223344"), [0x11, 0x22, 0x33]);
        assert_eq!(background("blurple"), [255, 255, 255]);
        assert_eq!(background("#12345"), [255, 255, 255]);
        assert_eq!(background("#gggggg"), [255, 255, 255]);
    }

    #[test]
    fn resize_modes_only_ever_shrink() {
        let mut o = ImageOptions::default();
        assert_eq!(resize_target(400, 200, &o), None);

        o.resize = Resize::Percent;
        o.resize_percent = 50;
        assert_eq!(resize_target(400, 200, &o), Some((200, 100)));
        o.resize_percent = 100;
        assert_eq!(resize_target(400, 200, &o), None);
        o.resize_percent = 1;
        assert_eq!(resize_target(40, 20, &o), Some((1, 1)));

        o.resize = Resize::LongEdge;
        o.long_edge = 100;
        assert_eq!(resize_target(200, 400, &o), Some((50, 100)));
        assert_eq!(resize_target(80, 60, &o), None);

        o.resize = Resize::Width;
        o.width = 100;
        assert_eq!(resize_target(400, 300, &o), Some((100, 75)));
        assert_eq!(resize_target(100, 3000, &o), None);
        o.width = 0;
        assert_eq!(resize_target(400, 300, &o), None);
    }

    #[test]
    fn jpeg_max_side_is_enforced() {
        assert_eq!(
            fit_max_side(1200, 74_673, JPEG_MAX_SIDE),
            Some((1053, 65_500))
        );
        assert_eq!(fit_max_side(65_500, 10, JPEG_MAX_SIDE), None);
    }

    #[test]
    fn ico_sizes_are_capped_sorted_and_deduplicated() {
        assert_eq!(ico_sizes(&[16, 512, 32, 16, 0, 256]), [256, 32, 16]);
        assert_eq!(ico_sizes(&[1024]), [256, 48, 32, 16]);
        assert_eq!(ico_sizes(&[]), [256, 48, 32, 16]);
    }

    #[test]
    fn accepts_the_default_preset_and_declines_what_it_cannot_do() {
        let ok = ImageOptions::default();
        assert!(accepts(&ok));

        let strip_all = ImageOptions {
            keep_metadata: false,
            keep_color_profile: false,
            ..ImageOptions::default()
        };
        assert!(accepts(&strip_all));

        let declined = [
            ImageOptions {
                keep_metadata: false,
                ..ImageOptions::default()
            },
            ImageOptions {
                dpi: Some(300),
                ..ImageOptions::default()
            },
            ImageOptions {
                tiff_compression: TiffCompression::Zip,
                ..ImageOptions::default()
            },
            ImageOptions {
                webp_lossless: true,
                ..ImageOptions::default()
            },
            ImageOptions {
                remove_background: true,
                ..ImageOptions::default()
            },
        ];
        for o in declined {
            assert!(!accepts(&o), "{o:?}");
        }
        let none = ImageOptions {
            tiff_compression: TiffCompression::None,
            dpi: Some(0),
            ..ImageOptions::default()
        };
        assert!(accepts(&none));
    }

    #[test]
    fn flatten_blends_onto_the_background() {
        let mut img = RgbaImage::new(2, 1);
        img.put_pixel(0, 0, Rgba([0, 0, 0, 0]));
        img.put_pixel(1, 0, Rgba([0, 0, 0, 255]));
        let flat = flatten(&DynamicImage::ImageRgba8(img), [255, 0, 0]);
        assert!(!flat.color().has_alpha());
        assert_eq!(flat.get_pixel(0, 0), Rgba([255, 0, 0, 255]));
        assert_eq!(flat.get_pixel(1, 0), Rgba([0, 0, 0, 255]));
    }

    #[test]
    fn apply_resizes_flattens_and_grays() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(40, 20, Rgba([255, 0, 0, 128])));
        let mut opts = ConvertOptions::default();
        opts.image.resize = Resize::Percent;
        opts.image.resize_percent = 50;
        opts.image.grayscale = true;
        let out = apply(img.clone(), fmt("jpg"), &opts);
        assert_eq!((out.width(), out.height()), (20, 10));
        assert!(matches!(out, DynamicImage::ImageLuma8(_)));

        // png keeps its alpha and goes gray with it.
        let out = apply(img, fmt("png"), &opts);
        assert!(matches!(out, DynamicImage::ImageLumaA8(_)));
    }

    #[test]
    fn layouts_match_what_each_encoder_takes() {
        let la = DynamicImage::ImageLumaA8(image::ImageBuffer::from_pixel(1, 1, LumaA([9, 9])));
        let rgb16 = DynamicImage::ImageRgb16(image::ImageBuffer::from_pixel(1, 1, Rgb([1, 2, 3])));
        assert!(matches!(
            for_target(la.clone(), fmt("png")),
            DynamicImage::ImageLumaA8(_)
        ));
        assert!(matches!(
            for_target(la.clone(), fmt("tiff")),
            DynamicImage::ImageRgba8(_)
        ));
        assert!(matches!(
            for_target(la.clone(), fmt("tga")),
            DynamicImage::ImageLumaA8(_)
        ));
        assert!(matches!(
            for_target(la.clone(), fmt("qoi")),
            DynamicImage::ImageRgba8(_)
        ));
        assert!(matches!(
            for_target(la, fmt("gif")),
            DynamicImage::ImageRgba8(_)
        ));
        assert!(matches!(
            for_target(rgb16.clone(), fmt("png")),
            DynamicImage::ImageRgb16(_)
        ));
        assert!(matches!(
            for_target(rgb16.clone(), fmt("qoi")),
            DynamicImage::ImageRgb8(_)
        ));
        assert!(matches!(
            for_target(rgb16, fmt("ppm")),
            DynamicImage::ImageRgb8(_)
        ));
    }

    #[test]
    fn square_pads_and_centres() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(4, 2, Rgba([1, 2, 3, 255])));
        let sq = square(&img);
        assert_eq!(sq.dimensions(), (4, 4));
        assert_eq!(sq.get_pixel(0, 0)[3], 0);
        assert_eq!(*sq.get_pixel(0, 1), Rgba([1, 2, 3, 255]));
        assert_eq!(sq.get_pixel(0, 3)[3], 0);
    }
}
