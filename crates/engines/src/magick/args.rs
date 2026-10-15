//! ImageMagick edges and command-line builder.
//!
//! Ported from Filee's `src/Filee.Engines/Magick/ImageEncoder.cs` (`Edge`,
//! `SelectFrames`, `ApplyOptions`, `Write`) and the read settings in
//! `ImageReader.cs`. Filee drives Magick.NET in-process; we shell out to
//! `magick`, so every Magick.NET call becomes a CLI option here, in the same
//! order. Everything in this module is pure: no process is started.

use std::ffi::OsString;
use std::path::Path;

use morphring_core::{ConvertOptions, Edge, FormatId, QualityLevel};

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

/// Targets with no alpha channel: transparency is flattened onto white.
const NO_ALPHA: [&str; 3] = ["jpg", "ppm", "heic"];

/// Targets that take `-quality`.
const QUALITY_TARGETS: [&str; 5] = ["jpg", "webp", "avif", "heic", "jxl"];

/// Icon sizes packed into one `.ico`, largest first (Filee's default `IcoSizes`).
const ICO_SIZES: &str = "256,128,64,48,32,16";

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
    let mut push = |s: &str| args.push(OsString::from(s));

    // Animation frames can be partial deltas. Coalescing turns each into a full
    // picture, which matters when frames change container (gif <-> webp) or get
    // split into separate files (gif -> png gives one file per frame, and a
    // half-drawn delta frame is nobody's favourite PNG). gif -> gif and
    // webp -> webp keep their own optimised frames, so they skip it.
    if ANIMATED.contains(&from) && from != to {
        push("-coalesce");
    }

    push("-auto-orient");

    if let Some(side) = max_side(to) {
        // '>' means shrink only; small pictures are left alone.
        push("-resize");
        push(&format!("{side}x{side}>"));
    }

    // Filee strips metadata but keeps the colour profile, so colours don't shift.
    push("+profile");
    push("!icc,*");

    if NO_ALPHA.contains(&to) {
        push("-background");
        push("white");
        push("-alpha");
        push("remove");
        push("-alpha");
        push("off");
    }

    if QUALITY_TARGETS.contains(&to) {
        push("-quality");
        push(&quality_value(opts.quality_level()).to_string());
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
            push(&format!("icon:auto-resize={ICO_SIZES}"));
        }
        "tiff" => {
            push("-compress");
            push("LZW");
        }
        _ => {}
    }

    let mut out = OsString::from(format!("{}:", coder(to)));
    out.push(output.as_os_str());
    args.push(out);
    args
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
            &ConvertOptions { quality },
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
                "+profile",
                "!icc,*",
                "-background",
                "white",
                "-alpha",
                "remove",
                "-alpha",
                "off",
                "-quality",
                "85",
                "JPEG:/out/a.dst",
            ]
        );
    }

    #[test]
    fn quality_levels_map_to_92_85_70() {
        assert!(has_pair(&build("png", "webp", Some(90)), "-quality", "92"));
        assert!(has_pair(&build("png", "webp", None), "-quality", "85"));
        assert!(has_pair(&build("png", "avif", Some(30)), "-quality", "70"));
        assert!(!build("png", "png", Some(30)).contains(&"-quality".to_string()));
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
            assert!(has_pair(&a, "-background", "white"), "{from}");
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
