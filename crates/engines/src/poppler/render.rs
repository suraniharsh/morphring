//! PDF pages to images with `pdftoppm`.
//!
//! Ported in spirit from Filee's `PdfiumConverter` (reverse-engineering/02,
//! section 9.2): every selected page becomes its own file with a `_p{n}`
//! suffix, or no suffix at all when exactly one page is selected. Filee drives
//! PDFium in-process; we hand the job to `pdftoppm`, which renders on white
//! paper just like Filee's "white background" and never shares a thread with
//! anyone. Multi-page TIFF and the combine-pages stacking are a later task.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use morphring_core::{CancelToken, Edge, EngineError, OutputAllocator, Step, page_range};

use crate::discovery::{self, Tool};
use crate::process::{self, RunError, RunOptions};

/// How long one `pdftoppm` run (one contiguous run of pages) may take.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(120);

/// How long `pdfinfo` gets to count the pages.
const PDFINFO_TIMEOUT: Duration = Duration::from_secs(30);

/// The resolution range Filee clamps `RenderDpi` to.
const MIN_DPI: u32 = 36;
const MAX_DPI: u32 = 1200;

/// The file name prefix handed to pdftoppm inside the private temp folder.
const PREFIX: &str = "p";

/// The edges this part handles.
pub(crate) fn edges() -> Vec<Edge> {
    vec![
        Edge::with_cost("pdf", "png", 10),
        Edge::with_cost("pdf", "jpg", 10),
        Edge::with_cost("pdf", "tiff", 10),
    ]
}

/// An image format pdftoppm can write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageKind {
    Png,
    Jpeg,
    Tiff,
}

impl ImageKind {
    /// The kind for a target format id, if pdftoppm writes it.
    pub(crate) fn for_target(to: &str) -> Option<Self> {
        match to {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "tiff" | "tif" => Some(Self::Tiff),
            _ => None,
        }
    }

    /// The switch that picks this format.
    fn flag(self) -> &'static str {
        match self {
            Self::Png => "-png",
            Self::Jpeg => "-jpeg",
            Self::Tiff => "-tiff",
        }
    }

    /// The extension pdftoppm puts on its files, without the dot. Note the
    /// three-letter `tif`: pdftoppm grew up in the 8.3 era and never moved out.
    pub(crate) fn produced_ext(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Tiff => "tif",
        }
    }
}

/// One pdftoppm invocation: which pages, in which format, at which quality.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunSpec {
    pub(crate) kind: ImageKind,
    /// Already clamped to Filee's 36..=1200.
    pub(crate) dpi: u32,
    /// JPEG quality, 1..=100; ignored by the other formats.
    pub(crate) quality: u8,
    /// First page, 1-based.
    pub(crate) first: usize,
    /// Last page, 1-based; `None` means "to the end" (no `-l`).
    pub(crate) last: Option<usize>,
    /// Write one file named exactly after the prefix, no page number.
    pub(crate) single_file: bool,
}

/// Clamps a requested resolution to what Filee allows.
pub(crate) fn clamp_dpi(dpi: u32) -> u32 {
    dpi.clamp(MIN_DPI, MAX_DPI)
}

/// The pdftoppm arguments for `spec`, reading `input` and writing next to
/// `prefix` (pdftoppm appends `-N.ext` itself, or just `.ext` with
/// `-singlefile`).
pub(crate) fn args(spec: &RunSpec, input: &Path, prefix: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "-r".into(),
        spec.dpi.to_string().into(),
        spec.kind.flag().into(),
    ];
    if spec.kind == ImageKind::Jpeg {
        args.push("-jpegopt".into());
        args.push(format!("quality={}", spec.quality.clamp(1, 100)).into());
    }
    args.push("-f".into());
    args.push(spec.first.to_string().into());
    if let Some(last) = spec.last {
        args.push("-l".into());
        args.push(last.to_string().into());
    }
    if spec.single_file {
        args.push("-singlefile".into());
    }
    args.push(input.into());
    args.push(prefix.into());
    args
}

/// The page a pdftoppm output file belongs to.
///
/// pdftoppm writes `p-7.png`, `p-07.png` or `p-007.png` depending on how many
/// pages the document has, so any run of digits is fine. With `-singlefile`
/// the file is just `p.png` and `single_page` says which page that was.
pub(crate) fn page_of(file_name: &str, ext: &str, single_page: Option<usize>) -> Option<usize> {
    let stem = file_name.strip_suffix(ext)?.strip_suffix('.')?;
    if stem == PREFIX {
        return single_page;
    }
    let digits = stem.strip_prefix(PREFIX)?.strip_prefix('-')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Leading zeros are pdftoppm being tidy, not a different page.
    digits.parse().ok().filter(|&n| n > 0)
}

/// The output suffix for 1-based `page` when `selected` pages were asked for:
/// `_p{page}`, or nothing when there is only the one.
pub(crate) fn suffix_for(page: usize, selected: usize) -> String {
    if selected == 1 {
        String::new()
    } else {
        format!("_p{page}")
    }
}

/// Reads `Pages:` out of pdfinfo's report.
pub(crate) fn parse_page_count(stdout: &str) -> Option<usize> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("Pages:"))
        .and_then(|n| n.trim().parse().ok())
}

/// Converts `step`, which is one of [`edges`].
pub(crate) fn convert(
    step: &Step,
    out: &mut dyn OutputAllocator,
    progress: &dyn Fn(f32),
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    let Some(kind) = ImageKind::for_target(step.to.as_str()) else {
        return Err(EngineError::Failed(format!(
            "Poppler cannot convert {} to {}",
            step.from, step.to
        )));
    };
    let Some(found) = discovery::find(Tool::Pdftoppm) else {
        return Err(EngineError::NotInstalled {
            tool: Tool::Pdftoppm.display_name(),
            hint: discovery::install_hint_here(Tool::Pdftoppm),
        });
    };
    if cancel.is_cancelled() {
        return Err(EngineError::Cancelled);
    }

    let dpi = clamp_dpi(step.options.pdf.render_dpi);
    let quality = step.options.image.quality.clamp(1, 100);
    let spec = |first, last, single_file| RunSpec {
        kind,
        dpi,
        quality,
        first,
        last,
        single_file,
    };

    fs::create_dir_all(&step.work_dir)?;
    let tmp = tempfile::Builder::new()
        .prefix("poppler-")
        .tempdir_in(&step.work_dir)?;
    let prefix = tmp.path().join(PREFIX);
    let produced_ext = kind.produced_ext();
    let ext = step.to.as_str();

    let pdfinfo = found.companion("pdfinfo");
    let Some(count) = page_count(&pdfinfo, &step.input, cancel)? else {
        // pdfinfo is sulking (missing companion, odd PDF it can't summarise).
        // pdftoppm may still cope, so render everything and sort it out after.
        let pass = spec(1, None, false);
        run_pdftoppm(&found.program, &args(&pass, &step.input, &prefix), cancel)?;
        let rendered = collect(tmp.path(), produced_ext, None)?;
        let selected = page_range::parse(&step.options.pdf.page_range, rendered.len());
        let pages: Vec<usize> = rendered.keys().copied().collect();
        let total = selected.len();
        let mut written = Vec::new();
        for index in selected {
            let page = pages[index];
            if let Some(target) = out.allocate(ext, &suffix_for(page, total)) {
                place(&rendered[&page], &target, page)?;
                written.push(target);
            }
        }
        progress(1.0);
        return Ok(written);
    };

    let selected: Vec<usize> = page_range::parse(&step.options.pdf.page_range, count)
        .into_iter()
        .map(|index| index + 1)
        .collect();
    if selected.is_empty() {
        return Err(EngineError::Failed("the PDF has no pages".to_string()));
    }
    let total = selected.len();

    // Ask for every name up front: a page the skip policy refuses is a page we
    // don't have to render, and the runs are rebuilt around the gaps.
    let mut targets: BTreeMap<usize, PathBuf> = BTreeMap::new();
    for &page in &selected {
        if let Some(target) = out.allocate(ext, &suffix_for(page, total)) {
            targets.insert(page, target);
        }
    }
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let wanted: Vec<usize> = targets.keys().map(|page| page - 1).collect();
    let runs = page_range::runs(&wanted);
    let to_render = wanted.len();

    progress(0.0);
    let mut written = Vec::new();
    let mut done = 0usize;
    for (first, last) in runs {
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        // -singlefile for a lone page spares us guessing the zero padding;
        // pdftoppm only honours it for the first page anyway.
        let single = first == last;
        let pass = spec(first, Some(last), single);
        run_pdftoppm(&found.program, &args(&pass, &step.input, &prefix), cancel)?;
        let rendered = collect(tmp.path(), produced_ext, single.then_some(first))?;
        for page in first..=last {
            let Some(from) = rendered.get(&page) else {
                return Err(EngineError::Failed(format!(
                    "Poppler did not render page {page}"
                )));
            };
            place(from, &targets[&page], page)?;
            written.push(targets[&page].clone());
        }
        // Whatever else turned up (it shouldn't) goes, so the next run starts
        // with a clean folder and no stale page can sneak in.
        for (page, path) in rendered {
            if !(first..=last).contains(&page) {
                let _ = fs::remove_file(path);
            }
        }
        done += last + 1 - first;
        progress(done as f32 / to_render as f32);
    }
    Ok(written)
}

/// The document's page count from `pdfinfo`, or `None` when pdfinfo can't say.
fn page_count(
    pdfinfo: &Path,
    input: &Path,
    cancel: &CancelToken,
) -> Result<Option<usize>, EngineError> {
    let opts = RunOptions {
        timeout: Some(PDFINFO_TIMEOUT),
        ..RunOptions::default()
    };
    match process::run(pdfinfo, &[input.into()], opts, cancel) {
        Ok(output) => Ok(parse_page_count(&output.stdout)),
        Err(RunError::Cancelled) => Err(EngineError::Cancelled),
        // pdftoppm would hit the very same lock, so say so now instead of
        // shrugging and letting it fail with poppler's wording.
        Err(err) => match super::password_error(&err) {
            Some(locked) => Err(locked),
            None => {
                tracing::debug!(error = %err, "pdfinfo failed; rendering every page");
                Ok(None)
            }
        },
    }
}

/// Runs pdftoppm once and turns its failures into engine errors.
fn run_pdftoppm(
    program: &Path,
    args: &[OsString],
    cancel: &CancelToken,
) -> Result<(), EngineError> {
    let opts = RunOptions {
        timeout: Some(TIMEOUT),
        ..RunOptions::default()
    };
    match process::run(program, args, opts, cancel) {
        Ok(_) => Ok(()),
        Err(RunError::Cancelled) => Err(EngineError::Cancelled),
        Err(RunError::TimedOut) => Err(EngineError::Failed(format!(
            "Poppler did not finish within {} seconds",
            TIMEOUT.as_secs()
        ))),
        Err(err) => Err(super::password_error(&err)
            .unwrap_or_else(|| EngineError::Failed(format!("pdftoppm {err}")))),
    }
}

/// Every pdftoppm output in `dir`, by page.
fn collect(
    dir: &Path,
    ext: &str,
    single_page: Option<usize>,
) -> std::io::Result<BTreeMap<usize, PathBuf>> {
    let mut pages = BTreeMap::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if let Some(page) = page_of(&name.to_string_lossy(), ext, single_page) {
            pages.insert(page, entry.path());
        }
    }
    Ok(pages)
}

/// Moves a rendered page to its final name, refusing empty files.
fn place(from: &Path, to: &Path, page: usize) -> Result<(), EngineError> {
    if fs::metadata(from)?.len() == 0 {
        return Err(EngineError::Failed(format!(
            "Poppler produced an empty image for page {page}"
        )));
    }
    process::move_into_place(from, to)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: ImageKind, first: usize, last: Option<usize>, single: bool) -> RunSpec {
        RunSpec {
            kind,
            dpi: 150,
            quality: 90,
            first,
            last,
            single_file: single,
        }
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn edges_are_png_jpg_tiff_at_cost_ten() {
        let pairs: Vec<(String, String, u32)> = edges()
            .iter()
            .map(|e| (e.from.to_string(), e.to.to_string(), e.cost))
            .collect();
        assert_eq!(
            pairs,
            [
                ("pdf".into(), "png".into(), 10),
                ("pdf".into(), "jpg".into(), 10),
                ("pdf".into(), "tiff".into(), 10),
            ]
        );
    }

    #[test]
    fn dpi_is_clamped_like_filee() {
        assert_eq!(clamp_dpi(0), 36);
        assert_eq!(clamp_dpi(36), 36);
        assert_eq!(clamp_dpi(150), 150);
        assert_eq!(clamp_dpi(1200), 1200);
        assert_eq!(clamp_dpi(9000), 1200);
    }

    #[test]
    fn png_run_has_range_input_and_prefix() {
        let a = args(
            &spec(ImageKind::Png, 2, Some(5), false),
            Path::new("/in.pdf"),
            Path::new("/tmp/p"),
        );
        assert_eq!(
            strings(&a),
            [
                "-r", "150", "-png", "-f", "2", "-l", "5", "/in.pdf", "/tmp/p"
            ]
        );
    }

    #[test]
    fn jpeg_run_carries_the_quality() {
        let mut s = spec(ImageKind::Jpeg, 1, Some(1), true);
        s.quality = 72;
        let a = strings(&args(&s, Path::new("in.pdf"), Path::new("p")));
        assert_eq!(
            a,
            [
                "-r",
                "150",
                "-jpeg",
                "-jpegopt",
                "quality=72",
                "-f",
                "1",
                "-l",
                "1",
                "-singlefile",
                "in.pdf",
                "p"
            ]
        );
    }

    #[test]
    fn unknown_page_count_renders_from_one_without_a_last_page() {
        let a = strings(&args(
            &spec(ImageKind::Tiff, 1, None, false),
            Path::new("in.pdf"),
            Path::new("p"),
        ));
        assert_eq!(a, ["-r", "150", "-tiff", "-f", "1", "in.pdf", "p"]);
        assert!(!a.iter().any(|s| s == "-l" || s == "-jpegopt"));
    }

    #[test]
    fn targets_map_to_pdftoppm_kinds_and_extensions() {
        assert_eq!(ImageKind::for_target("png"), Some(ImageKind::Png));
        assert_eq!(ImageKind::for_target("jpg"), Some(ImageKind::Jpeg));
        assert_eq!(ImageKind::for_target("tiff"), Some(ImageKind::Tiff));
        assert_eq!(ImageKind::for_target("webp"), None);
        assert_eq!(ImageKind::Jpeg.produced_ext(), "jpg");
        assert_eq!(ImageKind::Tiff.produced_ext(), "tif");
    }

    #[test]
    fn zero_padded_names_map_to_pages() {
        assert_eq!(page_of("p-1.png", "png", None), Some(1));
        assert_eq!(page_of("p-07.png", "png", None), Some(7));
        assert_eq!(page_of("p-012.tif", "tif", None), Some(12));
        assert_eq!(page_of("p-0.png", "png", None), None);
        assert_eq!(page_of("p-1.jpg", "png", None), None);
        assert_eq!(page_of("p-x.png", "png", None), None);
        assert_eq!(page_of("p-.png", "png", None), None);
        assert_eq!(page_of("q-1.png", "png", None), None);
        assert_eq!(page_of("p-1png", "png", None), None);
    }

    #[test]
    fn single_file_output_is_the_requested_page() {
        assert_eq!(page_of("p.png", "png", Some(4)), Some(4));
        assert_eq!(page_of("p.png", "png", None), None);
    }

    #[test]
    fn suffix_is_dropped_for_a_lone_page() {
        assert_eq!(suffix_for(2, 1), "");
        assert_eq!(suffix_for(1, 3), "_p1");
        assert_eq!(suffix_for(12, 20), "_p12");
    }

    #[test]
    fn page_count_comes_from_the_pages_line() {
        let report = "Title:  x\nPages:          3\nEncrypted: no\n";
        assert_eq!(parse_page_count(report), Some(3));
        assert_eq!(parse_page_count("Encrypted: no\n"), None);
        assert_eq!(parse_page_count("Pages: lots\n"), None);
    }

    #[test]
    fn collect_sorts_pages_numerically_and_ignores_strays() {
        let dir = tempfile::tempdir().expect("temp dir");
        for name in ["p-10.png", "p-02.png", "p-1.png", "stray.png", "p-3.jpg"] {
            fs::write(dir.path().join(name), b"x").expect("write");
        }
        let pages: Vec<usize> = collect(dir.path(), "png", None)
            .expect("list")
            .into_keys()
            .collect();
        assert_eq!(pages, [1, 2, 10]);
    }

    #[test]
    fn empty_outputs_are_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let from = dir.path().join("p-1.png");
        fs::write(&from, b"").expect("write");
        let err = place(&from, &dir.path().join("out.png"), 1).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Poppler produced an empty image for page 1"
        );
    }
}
