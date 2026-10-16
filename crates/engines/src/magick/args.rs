//! ImageMagick edges and command-line builder.
//!
//! Ported from Filee's `src/Filee.Engines/Magick/ImageEncoder.cs` (`Edge`,
//! `SelectFrames`, `ApplyOptions`, `Write`) and the read settings in
//! `ImageReader.cs`. Filee drives Magick.NET in-process; we shell out to
//! `magick`, so every Magick.NET call becomes a CLI option here, in the same
//! order. Everything in this module is pure: no process is started.

use std::ffi::OsString;
use std::path::Path;

use morphring_core::{
    ConvertOptions, Edge, FormatId, ImageOptions, QualityLevel, Resize, TiffCompression,
};

/// Raster formats `magick` reads in v0.1.0. No PDF, PS or EPS: reading those
/// needs Ghostscript and a permissive policy (D4).
pub const READABLE: [&str; 14] = [
    "png", "jpg", "webp", "gif", "bmp", "tiff", "ico", "avif", "heic", "jxl", "tga", "ppm", "psd",
    "raw",
];

/// Raster formats `magick` writes. Unlike Filee's bundled Magick.NET, the
/// system `magick` writes HEIC through libheif, so HEIC is writable here.
pub const WRITABLE: [&str; 12] = [
    "png", "jpg", "webp", "gif", "bmp", "tiff", "ico", "avif", "heic", "jxl", "tga", "ppm",
];

/// Formats that hold several frames or pages in one file.
const MULTI_FRAME: [&str; 2] = ["tiff", "gif"];

/// Formats whose frames are an animation (timing included).
const ANIMATED: [&str; 2] = ["gif", "webp"];

/// Targets with no alpha channel: transparency is flattened onto the preset's
/// background (white by default).
const NO_ALPHA: [&str; 3] = ["jpg", "ppm", "heic"];

/// Targets that take `-quality`.
const QUALITY_TARGETS: [&str; 5] = ["jpg", "webp", "avif", "heic", "jxl"];

/// Every direct conversion the ImageMagick engine offers: each readable raster
/// to each writable raster (self edges included, for re-encoding), and each
/// readable raster to PDF.
pub const EDGES: &[Edge] = &EDGE_TABLE;

const EDGE_COUNT: usize = READABLE.len() * (WRITABLE.len() + 1);
const EDGE_TABLE: [Edge; EDGE_COUNT] = build_edges();

// const fn has no iterators and no str ==, so this is a 1970s-style double loop.
// The upside: the table is baked into the binary and the planner gets a plain &[Edge].
const fn build_edges() -> [Edge; EDGE_COUNT] {
    let mut edges = [Edge::new("", ""); EDGE_COUNT];
    let mut n = 0;
    let mut i = 0;
    while i < READABLE.len() {
        let from = READABLE[i];
        let mut j = 0;
        while j < WRITABLE.len() {
            let to = WRITABLE[j];
            edges[n] = Edge::with_cost(from, to, edge_cost(from, to));
            n += 1;
            j += 1;
        }
        edges[n] = Edge::with_cost(from, "pdf", Edge::DEFAULT_COST);
        n += 1;
        i += 1;
    }
    edges
}

/// Filee's `ImageEncoder.Edge`: a still picture into GIF costs 12, so a photo
/// heading for video goes through a full-colour PNG instead of a 256-colour GIF.
/// Animations and multi-page files keep the plain cost.
const fn edge_cost(from: &str, to: &str) -> u32 {
    if str_eq(to, "gif") && !contains(&MULTI_FRAME, from) && !contains(&ANIMATED, from) {
        12
    } else {
        Edge::DEFAULT_COST
    }
}

const fn str_eq(a: &str, b: &str) -> bool {
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

const fn contains(set: &[&str], id: &str) -> bool {
    let mut i = 0;
    while i < set.len() {
        if str_eq(set[i], id) {
            return true;
        }
        i += 1;
    }
    false
}

/// How many files one conversion writes when the source has several frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FramePolicy {
    /// Exactly one output file: the source is read as one picture, or every
    /// frame goes into one multi-frame file (TIFF, GIF, an animation, a PDF).
    Single,
    /// One output file per frame, named with the suffix `_p{n}` (1-based) when
    /// there is more than one frame. A one-frame source still writes one file
    /// with no suffix.
    PerFrame,
}

/// Whether a `from -> to` conversion writes one file or one file per frame.
pub fn frame_policy(from: FormatId, to: FormatId) -> FramePolicy {
    let (from, to) = (from.as_str(), to.as_str());
    // psd/raw are read as one picture (the composite / the decoded sensor),
    // ico as its largest icon, so there is only ever one frame to write.
    let one_picture = matches!(from, "psd" | "raw" | "ico");
    let one_container = MULTI_FRAME.contains(&to) || to == "pdf" || keeps_animation(from, to);
    if one_picture || one_container || !can_have_frames(from) {
        FramePolicy::Single
    } else {
        FramePolicy::PerFrame
    }
}

/// True when the frames are an animation in both formats (GIF and WebP), so
/// they are written as one animated file with their timing.
fn keeps_animation(from: &str, to: &str) -> bool {
    ANIMATED.contains(&from) && ANIMATED.contains(&to)
}

fn can_have_frames(from: &str) -> bool {
    MULTI_FRAME.contains(&from) || ANIMATED.contains(&from)
}

/// The longest side a target can store (Filee's `MaxSide`, issue #50: a
/// 1200 x 74673 px JPEG failed to write). JPEG's real limit is 65535; 65500
/// keeps clear of encoder rounding.
fn max_side(to: &str) -> Option<u32> {
    match to {
        "jpg" => Some(65_500),
        "webp" => Some(16_383),
        "avif" => Some(16_384),
        _ => None,
    }
}

/// The `-quality` value for a quality bucket.
pub fn quality_value(level: QualityLevel) -> u8 {
    match level {
        QualityLevel::High => 92,
        QualityLevel::Normal => 85,
        QualityLevel::Small => 70,
    }
}

/// The ImageMagick coder name used as the explicit `FMT:` output prefix.
fn coder(id: &str) -> &str {
    match id {
        "png" => "PNG",
        "jpg" => "JPEG",
        "webp" => "WEBP",
        "gif" => "GIF",
        "bmp" => "BMP",
        "tiff" => "TIFF",
        "ico" => "ICO",
        "avif" => "AVIF",
        "heic" => "HEIC",
        "jxl" => "JXL",
        "tga" => "TGA",
        "ppm" => "PPM",
        "pdf" => "PDF",
        // Not a v0.1.0 target, but upper-casing the id is what IM accepts anyway.
        other => other,
    }
}

/// Builds the `magick` argument list (without the program name) that converts
/// `input` (format `from`) into `output` (format `to`).
///
/// - `frame` picks one frame of the input (`IN[n]`). Use it for ICO sources,
///   where Filee keeps the largest icon (see [`identify_args`] and
///   [`largest_frame`]). PSD sources always read frame 0, the composite.
/// - The output is written as `FMT:path`, so the extension never decides the
///   format. For [`FramePolicy::PerFrame`] with several frames, `output` may
///   contain `%d`, which `magick` replaces with the 0-based frame number.
pub fn args(
    input: &Path,
    output: &Path,
    from: FormatId,
    to: FormatId,
    opts: &ConvertOptions,
    frame: Option<usize>,
) -> Vec<OsString> {
    let (from, to) = (from.as_str(), to.as_str());
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |s: &str| args.push(OsString::from(s));

    if from == "raw" {
        // Read settings go before the input. These mirror Filee's DngReadDefines:
        // camera white balance yes, auto WB no, sRGB out (1), AHD demosaic (3),
        // auto-brightness left on.
        for define in [
            "dng:use-camera-wb=true",
            "dng:use-auto-wb=false",
            "dng:output-color=1",
            "dng:interpolation-quality=3",
            "dng:no-auto-bright=false",
        ] {
            push("-define");
            push(define);
        }
    }
    args.push(input_arg(input, from, frame));

    // Animation frames can be partial deltas. Coalescing turns each into a full
    // picture, which matters when frames change container (gif <-> webp) or get
    // split into separate files (gif -> png gives one file per frame, and a
    // half-drawn delta frame is nobody's favourite PNG). gif -> gif and
    // webp -> webp keep their own optimised frames, so they skip it.
    if ANIMATED.contains(&from) && from != to {
        args.push(OsString::from("-coalesce"));
    }

    apply_options(&mut args, to, opts);

    let mut out = OsString::from(format!("{}:", coder(to)));
    out.push(output.as_os_str());
    args.push(out);
    args
}

/// Filee's `ApplyOptions`, step for step and in the same order: orient,
/// resize, fit the format's size limit, grayscale, DPI, metadata, flatten,
/// then the per-format settings. Order matters to `magick` as much as it did
/// to Magick.NET: operators run left to right, so a `-colorspace` before the
/// `-resize` would make the resize slower and nobody happier.
fn apply_options(args: &mut Vec<OsString>, to: &str, opts: &ConvertOptions) {
    let image = &opts.image;
    let mut push = |s: &str| args.push(OsString::from(s));

    push("-auto-orient");

    if let Some(geometry) = resize_geometry(image) {
        push("-resize");
        push(&geometry);
    }

    if let Some(side) = max_side(to) {
        // '>' means shrink only; small pictures are left alone.
        push("-resize");
        push(&format!("{side}x{side}>"));
    }

    if image.grayscale {
        push("-colorspace");
        push("Gray");
    }

    if let Some(dpi) = image.dpi.filter(|&d| d > 0) {
        push("-units");
        push("PixelsPerInch");
        push("-density");
        push(&dpi.to_string());
    }

    if !image.keep_metadata {
        if image.keep_color_profile {
            // Everything but the ICC profile goes, so colours don't shift.
            push("+profile");
            push("!icc,*");
        } else {
            push("-strip");
        }
    }

    if NO_ALPHA.contains(&to) {
        // A no-op for opaque pictures, so there is no need to ask first whether
        // the image has alpha the way Filee does.
        push("-background");
        push(background(&image.background));
        push("-alpha");
        push("remove");
        push("-alpha");
        push("off");
    }

    if QUALITY_TARGETS.contains(&to) {
        push("-quality");
        push(&quality_for(opts).to_string());
    }
    if to == "webp" && image.webp_lossless {
        push("-define");
        push("webp:lossless=true");
    }

    match to {
        "ico" => {
            // Icons want a square: pad the short side with transparency (centred),
            // cap at 256 px, then let the ICO coder render every size into one file.
            for a in [
                "-background",
                "none",
                "-gravity",
                "center",
                "-extent",
                "%[fx:max(w,h)]x%[fx:max(w,h)]",
                "-resize",
                "256x256>",
                "-define",
            ] {
                push(a);
            }
            push(&format!("icon:auto-resize={}", ico_sizes(&image.ico_sizes)));
        }
        "tiff" => {
            push("-compress");
            push(match image.tiff_compression {
                TiffCompression::None => "None",
                TiffCompression::Lzw => "LZW",
                TiffCompression::Zip => "Zip",
            });
        }
        _ => {}
    }
}

/// The `-resize` geometry for the preset's resize mode, or `None` to keep the
/// size. Every mode only ever shrinks: a percentage of 100 or more is ignored
/// and the box modes carry IM's '>' flag (shrink only if larger).
fn resize_geometry(image: &ImageOptions) -> Option<String> {
    match image.resize {
        Resize::None => None,
        Resize::Percent => (image.resize_percent > 0 && image.resize_percent < 100)
            .then(|| format!("{}%", image.resize_percent)),
        Resize::LongEdge => (image.long_edge > 0).then(|| format!("{0}x{0}>", image.long_edge)),
        // "W>" with no height keeps the aspect ratio, same as Filee's hand-rolled
        // round(h * W / w), minus the hand-rolling.
        Resize::Width => (image.width > 0).then(|| format!("{}>", image.width)),
    }
}

/// The `-quality` value: the explicit `-q` wins, then the preset's own
/// quality when it differs from the default, then the engine default (85).
pub fn quality_for(opts: &ConvertOptions) -> u8 {
    let preset = opts.image.quality;
    opts.quality
        .or_else(|| (preset != ImageOptions::default().quality).then_some(preset))
        .map_or_else(|| quality_value(QualityLevel::Normal), |q| q.clamp(1, 100))
}

/// The preset's background as a colour `magick` will parse, or white. Only
/// `#RGB`, `#RGBA`, `#RRGGBB` and `#RRGGBBAA` get through; anything else
/// falls back to white like Filee's `ParseColor`, instead of finding out what
/// IM thinks "blurple" is.
fn background(color: &str) -> &str {
    let color = color.trim();
    let valid = color.strip_prefix('#').is_some_and(|hex| {
        matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit())
    });
    if valid { color } else { "#FFFFFF" }
}

/// Filee's `PrepareIcon` size list: 1 to 256 px, no duplicates, largest
/// first, with `256,48,32,16` when nothing usable is left.
fn ico_sizes(sizes: &[u32]) -> String {
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
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn input_arg(input: &Path, from: &str, frame: Option<usize>) -> OsString {
    let mut arg = OsString::new();
    if from == "raw" {
        // Force LibRaw for every camera extension: .srw is unknown to IM and a
        // bare .raw would otherwise be read as headerless pixel soup.
        arg.push("DNG:");
    } else if input.is_relative() && input.as_os_str().as_encoded_bytes().starts_with(b"-") {
        // A file called "-resize" is a legal file name and a terrible argument.
        arg.push("./");
    }
    arg.push(input.as_os_str());
    let frame = if from == "psd" { Some(0) } else { frame };
    if let Some(n) = frame {
        arg.push(format!("[{n}]"));
    }
    arg
}

/// `magick identify` arguments that print one `width height` line per frame;
/// feed the output to [`largest_frame`].
pub fn identify_args(input: &Path) -> Vec<OsString> {
    vec![
        OsString::from("identify"),
        OsString::from("-format"),
        OsString::from("%w %h\n"),
        input.as_os_str().to_owned(),
    ]
}

/// The index of the frame with the largest area in [`identify_args`] output,
/// or `None` when no line parses. Ties go to the first frame.
pub fn largest_frame(identify_output: &str) -> Option<usize> {
    let mut best: Option<(usize, u64)> = None;
    for (index, line) in identify_output.lines().enumerate() {
        let mut parts = line.split_whitespace().map(str::parse::<u64>);
        let (Some(Ok(w)), Some(Ok(h))) = (parts.next(), parts.next()) else {
            continue;
        };
        let area = w * h;
        if best.is_none_or(|(_, a)| area > a) {
            best = Some((index, area));
        }
    }
    best.map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn id(s: &'static str) -> FormatId {
        FormatId::new(s)
    }

    fn build(from: &'static str, to: &'static str, quality: Option<u8>) -> Vec<String> {
        args(
            Path::new("/in/a.src"),
            Path::new("/out/a.dst"),
            id(from),
            id(to),
            &ConvertOptions {
                quality,
                ..Default::default()
            },
            None,
        )
        .into_iter()
        .map(|a| a.into_string().unwrap())
        .collect()
    }

    fn build_with(from: &'static str, to: &'static str, opts: &ConvertOptions) -> Vec<String> {
        args(
            Path::new("/in/a.src"),
            Path::new("/out/a.dst"),
            id(from),
            id(to),
            opts,
            None,
        )
        .into_iter()
        .map(|a| a.into_string().unwrap())
        .collect()
    }

    fn has_pair(args: &[String], a: &str, b: &str) -> bool {
        args.windows(2).any(|w| w[0] == a && w[1] == b)
    }

    fn edge(from: &str, to: &str) -> Option<&'static Edge> {
        EDGES
            .iter()
            .find(|e| e.from.as_str() == from && e.to.as_str() == to)
    }

    #[test]
    fn png_to_jpg_at_quality_80_exact() {
        assert_eq!(
            build("png", "jpg", Some(80)),
            [
                "/in/a.src",
                "-auto-orient",
                "-resize",
                "65500x65500>",
                "-background",
                "#FFFFFF",
                "-alpha",
                "remove",
                "-alpha",
                "off",
                "-quality",
                "80",
                "JPEG:/out/a.dst",
            ]
        );
    }

    #[test]
    fn quality_cli_wins_then_preset_then_default() {
        assert!(has_pair(&build("png", "webp", Some(90)), "-quality", "90"));
        assert!(has_pair(&build("png", "webp", None), "-quality", "85"));
        assert!(has_pair(&build("png", "avif", Some(30)), "-quality", "30"));
        assert!(!build("png", "png", Some(30)).contains(&"-quality".to_string()));

        let mut opts = ConvertOptions::default();
        opts.image.quality = 70;
        assert!(has_pair(&build_with("png", "jpg", &opts), "-quality", "70"));
        // The preset default (90) means "no opinion": the engine default stays.
        opts.image.quality = ImageOptions::default().quality;
        assert!(has_pair(&build_with("png", "jpg", &opts), "-quality", "85"));
        opts.image.quality = 70;
        opts.quality = Some(40);
        assert!(has_pair(&build_with("png", "jpg", &opts), "-quality", "40"));
        opts.quality = Some(0);
        assert_eq!(quality_for(&opts), 1);
        assert_eq!(quality_value(QualityLevel::High), 92);
        assert_eq!(quality_value(QualityLevel::Small), 70);
    }

    fn with_image(f: impl FnOnce(&mut ImageOptions)) -> ConvertOptions {
        let mut opts = ConvertOptions::default();
        f(&mut opts.image);
        opts
    }

    #[test]
    fn default_options_add_nothing_beyond_the_format_needs() {
        assert_eq!(
            build("png", "png", None),
            ["/in/a.src", "-auto-orient", "PNG:/out/a.dst"]
        );
    }

    #[test]
    fn resize_modes_only_shrink() {
        let percent = |p| {
            build_with(
                "png",
                "png",
                &with_image(|i| {
                    i.resize = Resize::Percent;
                    i.resize_percent = p;
                }),
            )
        };
        assert!(has_pair(&percent(50), "-resize", "50%"));
        assert!(!percent(100).contains(&"-resize".to_string()));
        assert!(!percent(250).contains(&"-resize".to_string()));
        assert!(!percent(0).contains(&"-resize".to_string()));

        let long = build_with(
            "png",
            "png",
            &with_image(|i| {
                i.resize = Resize::LongEdge;
                i.long_edge = 1920;
            }),
        );
        assert!(has_pair(&long, "-resize", "1920x1920>"));

        let width = build_with(
            "png",
            "png",
            &with_image(|i| {
                i.resize = Resize::Width;
                i.width = 1200;
            }),
        );
        assert!(has_pair(&width, "-resize", "1200>"));

        // The user's resize runs before the format's size cap.
        let jpg = build_with("png", "jpg", &with_image(|i| i.resize = Resize::Percent));
        let first = jpg.iter().position(|a| a == "50%").unwrap();
        let cap = jpg.iter().position(|a| a == "65500x65500>").unwrap();
        assert!(first < cap);
    }

    #[test]
    fn grayscale_and_dpi() {
        let a = build_with(
            "png",
            "png",
            &with_image(|i| {
                i.grayscale = true;
                i.dpi = Some(300);
            }),
        );
        assert!(has_pair(&a, "-colorspace", "Gray"));
        assert!(has_pair(&a, "-units", "PixelsPerInch"));
        assert!(has_pair(&a, "-density", "300"));
        let gray = a.iter().position(|x| x == "-colorspace").unwrap();
        let dpi = a.iter().position(|x| x == "-units").unwrap();
        assert!(gray < dpi);
        let zero = build_with("png", "png", &with_image(|i| i.dpi = Some(0)));
        assert!(!zero.contains(&"-density".to_string()));
    }

    #[test]
    fn metadata_strip_keeps_icc_when_asked() {
        let keep_icc = build_with("png", "png", &with_image(|i| i.keep_metadata = false));
        assert!(has_pair(&keep_icc, "+profile", "!icc,*"));
        assert!(!keep_icc.contains(&"-strip".to_string()));

        let all = build_with(
            "png",
            "png",
            &with_image(|i| {
                i.keep_metadata = false;
                i.keep_color_profile = false;
            }),
        );
        assert!(all.contains(&"-strip".to_string()));
        assert!(!all.contains(&"+profile".to_string()));

        let kept = build("png", "png", None);
        assert!(!kept.contains(&"-strip".to_string()));
        assert!(!kept.contains(&"+profile".to_string()));
    }

    #[test]
    fn background_colour_is_used_or_falls_back_to_white() {
        let colour =
            |c: &str| build_with("png", "jpg", &with_image(|i| i.background = c.to_owned()));
        assert!(has_pair(&colour("#123abc"), "-background", "#123abc"));
        assert!(has_pair(&colour(" #000 "), "-background", "#000"));
        assert!(has_pair(&colour("blurple"), "-background", "#FFFFFF"));
        assert!(has_pair(&colour("#12345"), "-background", "#FFFFFF"));
        assert!(has_pair(&colour(""), "-background", "#FFFFFF"));
        // Targets with alpha keep it, background or not.
        let png = build_with("png", "png", &with_image(|i| i.background = "#000".into()));
        assert!(!png.contains(&"-background".to_string()));
    }

    #[test]
    fn tiff_compression_modes() {
        for (mode, arg) in [
            (TiffCompression::None, "None"),
            (TiffCompression::Lzw, "LZW"),
            (TiffCompression::Zip, "Zip"),
        ] {
            let a = build_with("png", "tiff", &with_image(|i| i.tiff_compression = mode));
            assert!(has_pair(&a, "-compress", arg), "{mode:?}");
        }
    }

    #[test]
    fn webp_lossless_define_only_for_webp() {
        let lossless = with_image(|i| i.webp_lossless = true);
        assert!(has_pair(
            &build_with("png", "webp", &lossless),
            "-define",
            "webp:lossless=true"
        ));
        assert!(!build_with("png", "avif", &lossless).contains(&"-define".to_string()));
        assert!(!build("png", "webp", None).contains(&"-define".to_string()));
    }

    #[test]
    fn ico_sizes_are_filtered_sorted_and_deduplicated() {
        let ico = |sizes: Vec<u32>| build_with("png", "ico", &with_image(|i| i.ico_sizes = sizes));
        assert!(has_pair(
            &ico(vec![16, 512, 32, 32, 0, 64]),
            "-define",
            "icon:auto-resize=64,32,16"
        ));
        assert!(has_pair(
            &ico(vec![]),
            "-define",
            "icon:auto-resize=256,48,32,16"
        ));
        assert!(has_pair(
            &ico(vec![1000]),
            "-define",
            "icon:auto-resize=256,48,32,16"
        ));
    }

    #[test]
    fn ico_args_include_auto_resize() {
        let a = build("png", "ico", None);
        assert!(has_pair(
            &a,
            "-define",
            "icon:auto-resize=256,128,64,48,32,16"
        ));
        assert!(has_pair(&a, "-resize", "256x256>"));
        assert_eq!(a.last().unwrap(), "ICO:/out/a.dst");
    }

    #[test]
    fn jpg_target_always_flattens_onto_white() {
        for from in READABLE {
            let a = build(from, "jpg", None);
            assert!(has_pair(&a, "-background", "#FFFFFF"), "{from}");
            assert!(has_pair(&a, "-alpha", "remove"), "{from}");
        }
        assert!(has_pair(&build("png", "ppm", None), "-alpha", "remove"));
        assert!(has_pair(&build("png", "heic", None), "-alpha", "remove"));
        assert!(!build("png", "png", None).contains(&"-alpha".to_string()));
    }

    #[test]
    fn max_side_per_format() {
        assert!(has_pair(
            &build("png", "webp", None),
            "-resize",
            "16383x16383>"
        ));
        assert!(has_pair(
            &build("png", "avif", None),
            "-resize",
            "16384x16384>"
        ));
        assert!(!build("png", "png", None).contains(&"-resize".to_string()));
    }

    #[test]
    fn tiff_uses_lzw() {
        assert!(has_pair(&build("png", "tiff", None), "-compress", "LZW"));
    }

    #[test]
    fn psd_reads_composite_and_raw_forces_dng() {
        assert_eq!(build("psd", "png", None)[0], "/in/a.src[0]");
        let raw = build("raw", "jpg", None);
        let input = raw.iter().position(|a| a == "DNG:/in/a.src").unwrap();
        assert!(has_pair(&raw[..input], "-define", "dng:use-camera-wb=true"));
    }

    #[test]
    fn frame_index_selects_one_input_frame() {
        let a = args(
            Path::new("/in/a.ico"),
            Path::new("/out/a.png"),
            id("ico"),
            id("png"),
            &ConvertOptions::default(),
            Some(2),
        );
        assert_eq!(a[0], "/in/a.ico[2]");
    }

    #[test]
    fn coalesce_only_when_animation_frames_are_rebuilt() {
        let has = |f, t| build(f, t, None).contains(&"-coalesce".to_string());
        assert!(has("gif", "webp"));
        assert!(has("webp", "gif"));
        assert!(has("gif", "png"));
        assert!(!has("gif", "gif"));
        assert!(!has("png", "webp"));
        assert!(!has("tiff", "png"));
    }

    #[test]
    fn output_names_the_format_explicitly() {
        let a = args(
            Path::new("in.png"),
            &PathBuf::from("/out/weird name.jpg"),
            id("png"),
            id("webp"),
            &ConvertOptions::default(),
            None,
        );
        assert_eq!(a.last().unwrap(), "WEBP:/out/weird name.jpg");
        assert_eq!(build("png", "pdf", None).last().unwrap(), "PDF:/out/a.dst");
    }

    #[test]
    fn dash_leading_relative_input_is_not_an_option() {
        let a = args(
            Path::new("-resize.png"),
            Path::new("/o.png"),
            id("png"),
            id("png"),
            &ConvertOptions::default(),
            None,
        );
        assert_eq!(a[0], "./-resize.png");
    }

    #[test]
    fn edges_cover_self_gif_pdf_and_no_pdf_input() {
        assert_eq!(edge("png", "png").unwrap().cost, 10);
        assert_eq!(edge("png", "gif").unwrap().cost, 12);
        assert_eq!(edge("jpg", "gif").unwrap().cost, 12);
        assert_eq!(edge("gif", "gif").unwrap().cost, 10);
        assert_eq!(edge("tiff", "gif").unwrap().cost, 10);
        assert_eq!(edge("webp", "gif").unwrap().cost, 10);
        assert_eq!(edge("png", "pdf").unwrap().cost, 10);
        assert_eq!(edge("png", "jpg").unwrap().cost, 10);
        assert!(
            EDGES
                .iter()
                .all(|e| !matches!(e.from.as_str(), "pdf" | "eps" | "ps"))
        );
        // psd and raw are read-only.
        assert!(edge("png", "psd").is_none());
        assert!(edge("png", "raw").is_none());
        assert!(edge("raw", "jpg").is_some());
        for w in WRITABLE {
            assert!(edge(w, w).is_some(), "missing self edge {w}");
        }
        assert_eq!(EDGES.len(), READABLE.len() * (WRITABLE.len() + 1));
    }

    #[test]
    fn frame_policies() {
        assert_eq!(frame_policy(id("gif"), id("png")), FramePolicy::PerFrame);
        assert_eq!(frame_policy(id("gif"), id("webp")), FramePolicy::Single);
        assert_eq!(frame_policy(id("webp"), id("gif")), FramePolicy::Single);
        assert_eq!(frame_policy(id("gif"), id("tiff")), FramePolicy::Single);
        assert_eq!(frame_policy(id("tiff"), id("png")), FramePolicy::PerFrame);
        assert_eq!(frame_policy(id("tiff"), id("pdf")), FramePolicy::Single);
        assert_eq!(frame_policy(id("png"), id("jpg")), FramePolicy::Single);
        assert_eq!(frame_policy(id("psd"), id("png")), FramePolicy::Single);
        assert_eq!(frame_policy(id("ico"), id("png")), FramePolicy::Single);
    }

    #[test]
    fn largest_frame_picks_biggest_area() {
        assert_eq!(largest_frame("16 16\n256 256\n48 48\n"), Some(1));
        assert_eq!(largest_frame("32 32\n32 32\n"), Some(0));
        assert_eq!(largest_frame("garbage\n"), None);
        assert_eq!(identify_args(Path::new("a.ico"))[0], "identify");
    }
}
