//! The built-in SVG engine: SVG and SVGZ rendered to PNG with resvg, plus the
//! svg <-> svgz repack.
//!
//! Ported from Filee's `VectorConverter` / `SvgRenderer` (reverse-engineering
//! 02, section 8.1). Only PNG comes out of here; every other raster target
//! chains through the image engine or ImageMagick, which already know how to
//! write them and apply the image options.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use morphring_core::{
    CancelToken, Converter, Edge, EngineError, EngineStatus, OutputAllocator, Resize, Step,
};
use usvg::fontdb;

use crate::discovery;

/// Every pair this engine converts. Rendering costs the usual 10; the repack
/// is just gzip, so it is cheaper than any route through a raster.
const EDGES: &[Edge] = &[
    Edge::new("svg", "png"),
    Edge::new("svgz", "png"),
    Edge::with_cost("svg", "svgz", 5),
    Edge::with_cost("svgz", "svg", 5),
];

/// DPI used when the options leave it unset (Filee's `RenderDpi` default).
pub const DEFAULT_DPI: u32 = 150;
/// The DPI option is clamped to this range before it becomes a scale.
pub const DPI_RANGE: (u32, u32) = (36, 1200);
/// Small icons are rendered at least this big on the long edge.
pub const MIN_LONG_EDGE: f64 = 1024.0;
/// No side of a rendered image is longer than this.
pub const MAX_SIDE: f64 = 16_384.0;
/// No rendered image has more pixels than this.
pub const MAX_PIXELS: f64 = 100_000_000.0;

/// How much of an SVG is searched for the `<svg` tag before the repack
/// declares it something else.
const SNIFF_LEN: usize = 4096;
/// Gunzip stops here. A 1 KB svgz that inflates to 40 GB is a zip bomb with a
/// vector-graphics costume, and we're not falling for it.
const MAX_INFLATED: u64 = 512 * 1024 * 1024;

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// The Built-in SVG engine.
#[derive(Clone, Copy, Debug, Default)]
pub struct SvgEngine;

impl SvgEngine {
    /// A new engine.
    pub fn new() -> Self {
        Self
    }
}

impl Converter for SvgEngine {
    fn id(&self) -> &'static str {
        "resvg"
    }

    fn display_name(&self) -> &'static str {
        "Built-in SVG"
    }

    fn edges(&self) -> &[Edge] {
        EDGES
    }

    fn status(&self) -> EngineStatus {
        discovery::builtin_status()
    }

    fn convert(
        &self,
        step: &Step,
        out: &mut dyn OutputAllocator,
        progress: &dyn Fn(f32),
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError> {
        let (from, to) = (step.from.as_str(), step.to.as_str());
        if !EDGES.iter().any(|e| e.from == step.from && e.to == step.to) {
            return Err(EngineError::Failed(format!(
                "resvg cannot convert {from} to {to}"
            )));
        }
        check_cancel(cancel)?;
        let raw = fs::read(&step.input)?;
        progress(0.1);

        // The skip policy can say no to the output; that's not an error, just
        // nothing to do.
        let Some(target) = out.allocate(to, "") else {
            return Ok(Vec::new());
        };

        let written = if to == "png" {
            render_png(step, &raw, progress, cancel)
        } else if to == "svgz" {
            to_svgz(&raw)
        } else {
            to_svg(&raw)
        };
        let bytes = written?;
        check_cancel(cancel)?;
        write_output(&target, &bytes)?;
        progress(1.0);
        Ok(vec![target])
    }
}

fn check_cancel(cancel: &CancelToken) -> Result<(), EngineError> {
    if cancel.is_cancelled() {
        Err(EngineError::Cancelled)
    } else {
        Ok(())
    }
}

/// Writes the whole output, and removes the half-written file if that fails.
fn write_output(target: &Path, bytes: &[u8]) -> Result<(), EngineError> {
    fs::write(target, bytes).map_err(|e| {
        let _ = fs::remove_file(target);
        EngineError::Io(e)
    })
}

/// True when `data` is gzip-compressed.
fn is_gzip(data: &[u8]) -> bool {
    data.starts_with(&GZIP_MAGIC)
}

/// Inflates gzip data, refusing anything that grows past [`MAX_INFLATED`].
pub fn gunzip(data: &[u8]) -> Result<Vec<u8>, EngineError> {
    let mut text = Vec::new();
    GzDecoder::new(data)
        .take(MAX_INFLATED + 1)
        .read_to_end(&mut text)
        .map_err(|e| EngineError::Failed(format!("the SVGZ file is not valid gzip: {e}")))?;
    if text.len() as u64 > MAX_INFLATED {
        return Err(EngineError::Failed(
            "the SVGZ file is too large once decompressed".to_owned(),
        ));
    }
    Ok(text)
}

/// The input as SVG text: gunzipped when it starts with `1F 8B`, as-is otherwise.
fn svg_text(raw: &[u8]) -> Result<std::borrow::Cow<'_, [u8]>, EngineError> {
    if is_gzip(raw) {
        Ok(std::borrow::Cow::Owned(gunzip(raw)?))
    } else {
        Ok(std::borrow::Cow::Borrowed(raw))
    }
}

/// Fails unless `<svg` shows up in the first 4 KB, Filee's cheap sniff for
/// "this really is an SVG and not a renamed shopping list".
fn require_svg_tag(text: &[u8]) -> Result<(), EngineError> {
    let head = &text[..text.len().min(SNIFF_LEN)];
    if head.windows(4).any(|w| w == b"<svg") {
        Ok(())
    } else {
        Err(EngineError::Failed(
            "the file is not an SVG image".to_owned(),
        ))
    }
}

/// svg -> svgz: gzip at the smallest size. An input that is already gzip is
/// copied untouched once its contents pass the sniff.
fn to_svgz(raw: &[u8]) -> Result<Vec<u8>, EngineError> {
    let text = svg_text(raw)?;
    require_svg_tag(&text)?;
    if is_gzip(raw) {
        return Ok(raw.to_vec());
    }
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&text)?;
    Ok(encoder.finish()?)
}

/// svgz -> svg: gunzip, then the same sniff. A plain SVG wearing an .svgz
/// name passes straight through.
fn to_svg(raw: &[u8]) -> Result<Vec<u8>, EngineError> {
    let text = svg_text(raw)?;
    require_svg_tag(&text)?;
    Ok(text.into_owned())
}

/// The system fonts, loaded once per process. Scanning every font on the
/// machine takes long enough that doing it per file would be rude. The SVG to
/// PDF engine shares this database.
pub(crate) fn fonts() -> Arc<fontdb::Database> {
    static FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            pick_generic_families(&mut db);
            Arc::new(db)
        })
        .clone()
}

/// Candidates for the generic families, best first. fontdb maps them to the
/// Windows fonts (Arial, Times New Roman, Courier New) and does not ask
/// fontconfig, so on a Linux box without Microsoft fonts every
/// `font-family="sans-serif"` quietly rendered as no text at all.
const SANS: &[&str] = &[
    "Arial",
    "Helvetica",
    "Liberation Sans",
    "DejaVu Sans",
    "Noto Sans",
    "Cantarell",
    "Ubuntu",
];
const SERIF: &[&str] = &[
    "Times New Roman",
    "Times",
    "Liberation Serif",
    "DejaVu Serif",
    "Noto Serif",
];
const MONO: &[&str] = &[
    "Courier New",
    "Courier",
    "Liberation Mono",
    "DejaVu Sans Mono",
    "Noto Sans Mono",
];

/// Points serif, sans-serif and monospace at fonts this machine really has.
/// When none of the candidates is installed, any font beats a blank.
fn pick_generic_families(db: &mut fontdb::Database) {
    let installed: Vec<String> = db
        .faces()
        .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
        .collect();
    let pick = |candidates: &[&str]| {
        candidates
            .iter()
            .find(|c| installed.iter().any(|f| f.eq_ignore_ascii_case(c)))
            .map(|c| (*c).to_owned())
            .or_else(|| installed.first().cloned())
    };
    if let Some(family) = pick(SANS) {
        db.set_sans_serif_family(family);
    }
    if let Some(family) = pick(SERIF) {
        db.set_serif_family(family);
    }
    if let Some(family) = pick(MONO) {
        db.set_monospace_family(family);
    }
}

/// True for an href that would leave the machine: URLs and UNC/network paths.
fn is_remote(href: &str) -> bool {
    href.contains("://") || href.starts_with("//") || href.starts_with("\\\\")
}

/// Parse options: resources come from the SVG's own folder, and only from the
/// local disk. usvg never fetches URLs and never expands external entities,
/// but it would happily open `\\server\share\x.png` on Windows, which is the
/// network wearing a filesystem moustache, so those are refused here.
fn parse_options(input: &Path) -> usvg::Options<'static> {
    let mut opt = usvg::Options {
        resources_dir: input.parent().map(Path::to_path_buf),
        fontdb: fonts(),
        ..usvg::Options::default()
    };
    let local = usvg::ImageHrefResolver::default_string_resolver();
    opt.image_href_resolver.resolve_string = Box::new(move |href, opts| {
        if is_remote(href) {
            None
        } else {
            local(href, opts)
        }
    });
    opt
}

/// The render scale for an SVG of `width` x `height` user units (96 per inch).
///
/// `max(dpi / 96, 1024 / long edge)`, so small icons still come out at least
/// 1024 px. A long-edge or width resize sets the scale exactly instead, and a
/// percent resize multiplies it. Whatever comes out is then capped at 16,384 px
/// per side and 100 MP.
pub fn raster_scale(width: f64, height: f64, options: &morphring_core::ImageOptions) -> f64 {
    let long = width.max(height);
    let dpi = f64::from(
        options
            .dpi
            .unwrap_or(DEFAULT_DPI)
            .clamp(DPI_RANGE.0, DPI_RANGE.1),
    );
    let mut scale = match options.resize {
        Resize::LongEdge if options.long_edge > 0 => f64::from(options.long_edge) / long,
        Resize::Width if options.width > 0 => f64::from(options.width) / width,
        _ => (dpi / 96.0).max(MIN_LONG_EDGE / long),
    };
    if options.resize == Resize::Percent && options.resize_percent > 0 {
        scale *= f64::from(options.resize_percent) / 100.0;
    }
    scale = scale.min(MAX_SIDE / long);
    let pixels = width * height * scale * scale;
    if pixels > MAX_PIXELS {
        scale *= (MAX_PIXELS / pixels).sqrt();
    }
    scale
}

/// Pixel size of the render: the scaled size rounded, never below 1 px and
/// never past the caps (rounding up must not sneak over them).
fn pixel_size(width: f64, height: f64, scale: f64) -> (u32, u32) {
    let side = |v: f64| (v * scale).round().clamp(1.0, MAX_SIDE) as u32;
    let (mut w, mut h) = (side(width), side(height));
    while f64::from(w) * f64::from(h) > MAX_PIXELS {
        // Off by one row after rounding; shave it rather than recompute.
        if w >= h {
            w -= 1;
        } else {
            h -= 1;
        }
    }
    (w, h)
}

/// svg/svgz -> png on a transparent canvas.
fn render_png(
    step: &Step,
    raw: &[u8],
    progress: &dyn Fn(f32),
    cancel: &CancelToken,
) -> Result<Vec<u8>, EngineError> {
    let text = svg_text(raw)?;
    let opt = parse_options(&step.input);
    let tree = usvg::Tree::from_data(&text, &opt)
        .map_err(|e| EngineError::Failed(format!("cannot read the SVG file: {e}")))?;
    progress(0.4);
    check_cancel(cancel)?;

    let size = tree.size();
    let (width, height) = (f64::from(size.width()), f64::from(size.height()));
    let drawable = tree.root().has_children()
        && width.is_finite()
        && height.is_finite()
        && width > 0.0
        && height > 0.0;
    if !drawable {
        return Err(EngineError::Failed(
            "the SVG has no drawable content or no size".to_owned(),
        ));
    }

    let scale = raster_scale(width, height, &step.options.image);
    let (w, h) = pixel_size(width, height, scale);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)
        .ok_or_else(|| EngineError::Failed("cannot allocate the image".to_owned()))?;
    // Stretch to the rounded pixel size per axis, so the edges land on pixels
    // instead of leaving a transparent half-pixel sliver on the right.
    let transform = resvg::tiny_skia::Transform::from_scale(
        (f64::from(w) / width) as f32,
        (f64::from(h) / height) as f32,
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    progress(0.8);
    check_cancel(cancel)?;

    // encode_png un-premultiplies the alpha on the way out, so the straight
    // alpha Filee converts to by hand comes for free.
    pixmap
        .encode_png()
        .map_err(|e| EngineError::Failed(format!("cannot encode the PNG: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::test_support::{NoOutputs, step};
    use morphring_core::ImageOptions;

    #[test]
    fn has_an_id_and_the_four_edges() {
        let engine = SvgEngine::new();
        assert_eq!(engine.id(), "resvg");
        assert_eq!(engine.display_name(), "Built-in SVG");
        let pairs: Vec<(String, String, u32)> = engine
            .edges()
            .iter()
            .map(|e| (e.from.to_string(), e.to.to_string(), e.cost))
            .collect();
        let expected: Vec<(String, String, u32)> = [
            ("svg", "png", 10),
            ("svgz", "png", 10),
            ("svg", "svgz", 5),
            ("svgz", "svg", 5),
        ]
        .iter()
        .map(|(a, b, c)| ((*a).to_owned(), (*b).to_owned(), *c))
        .collect();
        assert_eq!(pairs, expected);
    }

    #[test]
    fn refuses_a_pair_it_has_no_edge_for() {
        let err = SvgEngine::new()
            .convert(
                &step("pdf", "png"),
                &mut NoOutputs,
                &|_| {},
                &CancelToken::new(),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "resvg cannot convert pdf to png");
    }

    #[test]
    fn a_builtin_engine_is_always_available() {
        assert_eq!(
            SvgEngine::new().status(),
            EngineStatus::Available {
                path: PathBuf::new(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            }
        );
    }

    #[test]
    fn small_icons_render_at_least_1024_on_the_long_edge() {
        let scale = raster_scale(48.0, 24.0, &ImageOptions::default());
        assert_eq!(pixel_size(48.0, 24.0, scale), (1024, 512));
    }

    #[test]
    fn big_drawings_use_the_dpi() {
        // 150 dpi on a 2000-unit drawing: 2000 * 150 / 96 = 3125.
        let scale = raster_scale(2000.0, 1000.0, &ImageOptions::default());
        assert_eq!(pixel_size(2000.0, 1000.0, scale), (3125, 1563));
        let options = ImageOptions {
            dpi: Some(5000),
            ..ImageOptions::default()
        };
        // Clamped to 1200 dpi: 12.5x.
        let scale = raster_scale(1000.0, 100.0, &options);
        assert!((scale - 12.5).abs() < 1e-9);
    }

    #[test]
    fn the_scale_is_capped_by_side_and_by_pixels() {
        let side = raster_scale(100_000.0, 10.0, &ImageOptions::default());
        assert_eq!(pixel_size(100_000.0, 10.0, side).0, 16_384);
        let (w, h) = pixel_size(
            8000.0,
            8000.0,
            raster_scale(8000.0, 8000.0, &ImageOptions::default()),
        );
        assert!(u64::from(w) * u64::from(h) <= 100_000_000);
        assert!(w <= 16_384 && h <= 16_384);
    }

    #[test]
    fn resize_options_set_the_scale() {
        let long = ImageOptions {
            resize: Resize::LongEdge,
            long_edge: 300,
            ..ImageOptions::default()
        };
        assert_eq!(
            pixel_size(48.0, 24.0, raster_scale(48.0, 24.0, &long)),
            (300, 150)
        );
        let width = ImageOptions {
            resize: Resize::Width,
            width: 200,
            ..ImageOptions::default()
        };
        assert_eq!(
            pixel_size(48.0, 24.0, raster_scale(48.0, 24.0, &width)),
            (200, 100)
        );
        let percent = ImageOptions {
            resize: Resize::Percent,
            resize_percent: 50,
            ..ImageOptions::default()
        };
        assert_eq!(
            pixel_size(48.0, 24.0, raster_scale(48.0, 24.0, &percent)),
            (512, 256)
        );
    }

    #[test]
    fn the_sniff_wants_an_svg_tag_in_the_first_4kb() {
        assert!(require_svg_tag(b"<?xml version=\"1.0\"?><svg/>").is_ok());
        assert!(require_svg_tag(b"hello, I am a text file").is_err());
        let mut late = vec![b' '; SNIFF_LEN];
        late.extend_from_slice(b"<svg/>");
        assert!(require_svg_tag(&late).is_err());
    }

    #[test]
    fn gzip_round_trips_and_plain_svg_passes_through() {
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec();
        let packed = to_svgz(&svg).unwrap();
        assert!(is_gzip(&packed));
        assert_eq!(to_svgz(&packed).unwrap(), packed);
        assert_eq!(to_svg(&packed).unwrap(), svg);
        assert_eq!(to_svg(&svg).unwrap(), svg);
    }

    #[test]
    fn broken_gzip_is_an_error_not_a_panic() {
        let err = to_svg(&[0x1f, 0x8b, 0, 1, 2, 3]).unwrap_err();
        assert!(err.to_string().contains("not valid gzip"), "{err}");
    }

    #[test]
    fn remote_hrefs_are_refused() {
        assert!(is_remote("https://example.com/a.png"));
        assert!(is_remote("//example.com/a.png"));
        assert!(is_remote("\\\\server\\share\\a.png"));
        assert!(!is_remote("pics/a.png"));
        assert!(!is_remote("/home/me/a.png"));
    }
}
