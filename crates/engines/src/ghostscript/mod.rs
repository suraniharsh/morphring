//! The Ghostscript engine: PDF, EPS, PostScript and Illustrator files rendered
//! to PNG, JPEG or TIFF through `gs`, and EPS, PostScript and Illustrator
//! files turned into vector PDF through `pdfwrite`.
//!
//! Ported from Filee's `GhostscriptConverter` (reverse-engineering/02, section
//! 8.2). EPS, PS and AI render through `pngalpha`, so the background stays
//! transparent; PDF renders on white paper, and only gets here when Poppler
//! is missing, because its edges cost a little more. Every page lands as its
//! own file with a `_p{n}` suffix, exactly like Poppler, so nobody can tell
//! which engine did the work. (Ghostscript would like the credit, though.)
//!
//! PDF output is the exception: `pdfwrite` keeps the vectors and the text and
//! writes the whole document as one file, because a two-page PostScript file
//! deserves a two-page PDF, not two blurry one-page ones.

mod args;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use morphring_core::{
    CancelToken, Converter, Edge, EngineError, EngineStatus, OutputAllocator, Step, page_range,
};

use crate::discovery::{self, Tool};
use crate::process::{self, RunError, RunOptions};
use crate::progress::Estimate;

use args::{AiKind, Device, RenderSpec};

/// How long one gs run (one contiguous run of pages) may take.
const TIMEOUT: Duration = Duration::from_secs(120);

/// How long gs gets to count a PDF's pages.
const COUNT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a gs run usually takes, for the estimated progress.
const TYPICAL: Duration = Duration::from_secs(2);

/// Environment variables gs would otherwise read from the user's setup
/// (extra options, resource and font paths). Filee blanks them so results are
/// the same on every machine; we remove them, which gs takes the same way.
const DIRTY_ENV: [&str; 3] = ["GS_OPTIONS", "GS_LIB", "GS_FONTPATH"];

/// Filee's message for an `.ai` file that is neither PDF nor PostScript.
const NOT_AN_AI_FILE: &str =
    "The file is not an Adobe Illustrator document (neither PDF nor PostScript inside).";

/// The edges: vector sources at the default 10, PDF at 12 so Poppler (10)
/// wins whenever it's installed and gs only steps in when it isn't. The
/// vector-to-PDF edges cost 10 too, which beats rendering a PNG and wrapping
/// it in a PDF (20) and keeps the text where `pdftotext` can find it.
static EDGES: [Edge; 15] = [
    Edge::with_cost("eps", "png", 10),
    Edge::with_cost("eps", "jpg", 10),
    Edge::with_cost("eps", "tiff", 10),
    Edge::with_cost("ps", "png", 10),
    Edge::with_cost("ps", "jpg", 10),
    Edge::with_cost("ps", "tiff", 10),
    Edge::with_cost("ai", "png", 10),
    Edge::with_cost("ai", "jpg", 10),
    Edge::with_cost("ai", "tiff", 10),
    Edge::with_cost("eps", "pdf", 10),
    Edge::with_cost("ps", "pdf", 10),
    Edge::with_cost("ai", "pdf", 10),
    Edge::with_cost("pdf", "png", 12),
    Edge::with_cost("pdf", "jpg", 12),
    Edge::with_cost("pdf", "tiff", 12),
];

/// The Ghostscript engine.
#[derive(Clone, Copy, Debug, Default)]
pub struct GhostscriptEngine;

impl GhostscriptEngine {
    /// A new engine.
    pub fn new() -> Self {
        Self
    }
}

impl Converter for GhostscriptEngine {
    fn id(&self) -> &'static str {
        Tool::Ghostscript.id()
    }

    fn display_name(&self) -> &'static str {
        Tool::Ghostscript.display_name()
    }

    fn edges(&self) -> &[Edge] {
        &EDGES
    }

    fn status(&self) -> EngineStatus {
        discovery::status(Tool::Ghostscript)
    }

    fn convert(
        &self,
        step: &Step,
        out: &mut dyn OutputAllocator,
        progress: &dyn Fn(f32),
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError> {
        let refuse = || {
            EngineError::Failed(format!(
                "Ghostscript cannot convert {} to {}",
                step.from, step.to
            ))
        };
        let Some(device) = Device::for_pair(step.from.as_str(), step.to.as_str()) else {
            return Err(refuse());
        };
        if !EDGES.iter().any(|e| e.from == step.from && e.to == step.to) {
            return Err(refuse());
        }

        // An .ai file is a PDF or a PostScript file wearing a costume; decide
        // which before gs has to guess.
        let ai = if step.from.as_str() == "ai" {
            match args::ai_kind(&read_head(&step.input)?) {
                AiKind::Unknown => return Err(EngineError::Failed(NOT_AN_AI_FILE.to_owned())),
                kind => Some(kind),
            }
        } else {
            None
        };
        let paged = step.from.as_str() == "pdf" || ai == Some(AiKind::Pdf);
        let eps_crop = args::wants_eps_crop(step.from.as_str(), ai);

        let Some(found) = discovery::find(Tool::Ghostscript) else {
            return Err(EngineError::NotInstalled {
                tool: Tool::Ghostscript.display_name(),
                hint: discovery::install_hint_here(Tool::Ghostscript),
            });
        };
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }

        // gs runs inside the temp folder (see args::output_pattern), so the
        // input has to be absolute or it would go looking in the wrong place.
        let input = std::path::absolute(&step.input)?;
        let run = Runner {
            program: &found.program,
            input: &input,
            spec: RenderSpec {
                device,
                dpi: args::clamp_dpi(step.options.pdf.render_dpi),
                quality: step.options.image.quality.clamp(1, 100),
                eps_crop,
                pages: None,
            },
            ext: step.to.as_str(),
            postscript_ai: ai == Some(AiKind::PostScript),
            cancel,
        };

        fs::create_dir_all(&step.work_dir)?;
        let tmp = tempfile::Builder::new()
            .prefix("gs-")
            .tempdir_in(&step.work_dir)?;

        let count = if paged {
            page_count(run.program, &input, cancel)?
        } else {
            None
        };
        if !device.is_raster() {
            let range = count.map(|count| {
                let pages: Vec<usize> = page_range::parse(&step.options.pdf.page_range, count)
                    .into_iter()
                    .map(|index| index + 1)
                    .collect();
                (pages, count)
            });
            let written = run.document(tmp.path(), range, out, progress)?;
            progress(1.0);
            return Ok(written);
        }
        let written = match count {
            Some(count) => run.pages(
                tmp.path(),
                count,
                &step.options.pdf.page_range,
                out,
                progress,
            )?,
            None => {
                // EPS and PS are rendered whole, like Filee does; a PDF lands
                // here only when gs couldn't count its pages, and then the range
                // is applied to whatever came out.
                let range = if paged {
                    step.options.pdf.page_range.as_str()
                } else {
                    ""
                };
                run.everything(tmp.path(), range, out, progress)?
            }
        };
        progress(1.0);
        Ok(written)
    }
}

/// Everything one conversion's gs runs have in common.
struct Runner<'a> {
    program: &'a Path,
    input: &'a Path,
    /// The render settings; `pages` is filled in per run.
    spec: RenderSpec,
    /// The target extension handed to the output allocator.
    ext: &'a str,
    /// A PostScript Illustrator file, which earns a hint when gs chokes on it.
    postscript_ai: bool,
    cancel: &'a CancelToken,
}

impl Runner<'_> {
    /// Renders the selected pages of a document with `count` pages, one gs
    /// run per contiguous run of pages.
    fn pages(
        &self,
        dir: &Path,
        count: usize,
        range: &str,
        out: &mut dyn OutputAllocator,
        progress: &dyn Fn(f32),
    ) -> Result<Vec<PathBuf>, EngineError> {
        let selected: Vec<usize> = page_range::parse(range, count)
            .into_iter()
            .map(|index| index + 1)
            .collect();
        let total = selected.len();

        // Ask for every name up front: a page the skip policy refuses is a
        // page gs never has to render.
        let mut targets: BTreeMap<usize, PathBuf> = BTreeMap::new();
        for &page in &selected {
            if let Some(target) = out.allocate(self.ext, &args::suffix_for(page, total)) {
                targets.insert(page, target);
            }
        }
        if targets.is_empty() {
            return Ok(Vec::new());
        }
        let wanted: Vec<usize> = targets.keys().map(|page| page - 1).collect();
        let runs = page_range::runs(&wanted);
        let to_render = wanted.len() as f32;

        let reported = Cell::new(0.0f32);
        let mut written = Vec::new();
        let mut done = 0usize;
        for (first, last) in runs {
            // Each run's estimate covers its own slice of the bar, so the
            // ring never sees progress go backwards between runs.
            let base = done as f32 / to_render;
            let span = (last + 1 - first) as f32 / to_render;
            let slice = |p: f32| {
                let value = base + p * span;
                if value > reported.get() {
                    reported.set(value);
                    progress(value);
                }
            };
            let rendered = self.render(dir, Some((first, last)), &slice)?;
            for page in first..=last {
                let Some(from) = rendered.get(&(page + 1 - first)) else {
                    return Err(EngineError::Failed(format!(
                        "Ghostscript did not render page {page}"
                    )));
                };
                place(from, &targets[&page], page)?;
                written.push(targets[&page].clone());
            }
            clear(dir)?;
            done += last + 1 - first;
            slice(1.0);
        }
        Ok(written)
    }

    /// Renders every page in one run, then keeps the ones `range` selects.
    fn everything(
        &self,
        dir: &Path,
        range: &str,
        out: &mut dyn OutputAllocator,
        progress: &dyn Fn(f32),
    ) -> Result<Vec<PathBuf>, EngineError> {
        // Nobody knows the page count yet, so names can only be asked for
        // after the fact; the skip policy then just costs a little rendering.
        let rendered = self.render(dir, None, progress)?;
        let pages: Vec<usize> = rendered.keys().copied().collect();
        let selected = page_range::parse(range, pages.len());
        let total = selected.len();
        let mut written = Vec::new();
        for index in selected {
            let page = pages[index];
            if let Some(target) = out.allocate(self.ext, &args::suffix_for(page, total)) {
                place(&rendered[&page], &target, page)?;
                written.push(target);
            }
        }
        Ok(written)
    }

    /// Writes the whole document as one vector PDF through `pdfwrite`.
    /// `range` is the selected 1-based pages out of the page count, known
    /// only for a PDF-based `.ai`; PostScript goes through whole, like it
    /// does when rendering.
    fn document(
        &self,
        dir: &Path,
        range: Option<(Vec<usize>, usize)>,
        out: &mut dyn OutputAllocator,
        progress: &dyn Fn(f32),
    ) -> Result<Vec<PathBuf>, EngineError> {
        let page_list = match &range {
            Some((pages, _)) if pages.is_empty() => return Ok(Vec::new()),
            Some((pages, count)) => args::page_list(pages, *count),
            None => None,
        };
        // One document, one name: no _p suffixes for a file that keeps its
        // pages together.
        let Some(target) = out.allocate(self.ext, "") else {
            return Ok(Vec::new());
        };
        let argv = args::pdfwrite_args(self.input, self.spec.eps_crop, page_list.as_deref());
        self.run_gs(dir, &argv, progress)?;
        let written = dir.join(args::PDF_OUTPUT);
        match fs::metadata(&written) {
            Ok(meta) if meta.len() > 0 => {}
            _ => return Err(self.failure("Ghostscript wrote no PDF".to_owned())),
        }
        process::move_into_place(&written, &target)?;
        Ok(vec![target])
    }

    /// One gs run over `pages` (or all of them) into `dir`; returns the files
    /// it wrote by run-relative index, starting at 1.
    fn render(
        &self,
        dir: &Path,
        pages: Option<(usize, usize)>,
        progress: &dyn Fn(f32),
    ) -> Result<BTreeMap<usize, PathBuf>, EngineError> {
        let spec = RenderSpec {
            pages,
            ..self.spec.clone()
        };
        let argv = args::render_args(&spec, self.input);
        self.run_gs(dir, &argv, progress)?;
        let rendered = collect(dir, spec.device.ext())?;
        if rendered.is_empty() {
            return Err(self.failure("Ghostscript wrote no image".to_owned()));
        }
        Ok(rendered)
    }

    /// Runs gs with `argv` inside `dir`, turning its many ways of failing
    /// into engine errors.
    fn run_gs(
        &self,
        dir: &Path,
        argv: &[OsString],
        progress: &dyn Fn(f32),
    ) -> Result<(), EngineError> {
        if self.cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        // gs works in silence, so the ring gets an estimate meanwhile.
        let result = Estimate::during(progress, TYPICAL, || {
            process::run(
                self.program,
                argv,
                RunOptions {
                    timeout: Some(TIMEOUT),
                    cwd: Some(dir.to_path_buf()),
                    env_remove: DIRTY_ENV.to_vec(),
                    ..RunOptions::default()
                },
                self.cancel,
            )
        });
        match result {
            Ok(_) => {}
            Err(RunError::Cancelled) => return Err(EngineError::Cancelled),
            Err(RunError::TimedOut) => {
                return Err(EngineError::Failed(format!(
                    "Ghostscript did not finish within {} seconds",
                    TIMEOUT.as_secs()
                )));
            }
            Err(RunError::Failed { code, tail }) => {
                let code = code.map_or_else(|| "a signal".to_owned(), |c| format!("exit {c}"));
                return Err(self.failure(format!(
                    "Ghostscript failed ({code}). {}",
                    args::failure_detail(&tail)
                )));
            }
            Err(err) => return Err(EngineError::Failed(format!("Ghostscript {err}"))),
        }
        Ok(())
    }

    /// A gs failure, with Filee's advice attached for PostScript `.ai` files:
    /// those often lean on Illustrator's own procsets, which gs has never met.
    fn failure(&self, message: String) -> EngineError {
        let message = message.trim().to_owned();
        if self.postscript_ai {
            EngineError::Failed(format!(
                "This Illustrator file was saved without PDF compatibility, so it is \
                 PostScript, and Ghostscript could not read it. Save it in Illustrator with \
                 \"Create PDF compatible file\". {message}"
            ))
        } else {
            EngineError::Failed(message)
        }
    }
}

/// The page count of a PDF according to gs, or `None` when gs can't say.
fn page_count(
    program: &Path,
    input: &Path,
    cancel: &CancelToken,
) -> Result<Option<usize>, EngineError> {
    let opts = RunOptions {
        timeout: Some(COUNT_TIMEOUT),
        env_remove: DIRTY_ENV.to_vec(),
        ..RunOptions::default()
    };
    let argv: Vec<OsString> = args::page_count_args(input);
    match process::run(program, &argv, opts, cancel) {
        Ok(output) => Ok(args::parse_page_count(&output.stdout)),
        Err(RunError::Cancelled) => Err(EngineError::Cancelled),
        Err(err) => {
            tracing::debug!(error = %err, "gs could not count the pages; rendering them all");
            Ok(None)
        }
    }
}

/// The first kilobyte of `path`, or less if the file is shorter.
fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(1024);
    fs::File::open(path)?.take(1024).read_to_end(&mut head)?;
    Ok(head)
}

/// Every page file gs wrote into `dir`, by run-relative index.
fn collect(dir: &Path, ext: &str) -> std::io::Result<BTreeMap<usize, PathBuf>> {
    let mut pages = BTreeMap::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if let Some(index) = args::index_of(&entry.file_name().to_string_lossy(), ext) {
            pages.insert(index, entry.path());
        }
    }
    Ok(pages)
}

/// Empties `dir` between runs: gs numbers every run from 1, so a leftover
/// `p1.png` would otherwise pose as the next run's first page.
fn clear(dir: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_file() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

/// Moves a rendered page to its final name, refusing empty files.
fn place(from: &Path, to: &Path, page: usize) -> Result<(), EngineError> {
    if fs::metadata(from)?.len() == 0 {
        return Err(EngineError::Failed(format!(
            "Ghostscript produced an empty image for page {page}"
        )));
    }
    process::move_into_place(from, to)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::test_support::{NoOutputs, step};

    fn pairs() -> Vec<(String, String, u32)> {
        GhostscriptEngine::new()
            .edges()
            .iter()
            .map(|e| (e.from.to_string(), e.to.to_string(), e.cost))
            .collect()
    }

    #[test]
    fn the_engine_has_an_id_and_a_name() {
        let engine = GhostscriptEngine::new();
        assert_eq!(engine.id(), "ghostscript");
        assert_eq!(engine.display_name(), "Ghostscript");
    }

    #[test]
    fn vector_edges_cost_ten_and_pdf_edges_twelve() {
        let pairs = pairs();
        assert_eq!(pairs.len(), 15);
        for from in ["eps", "ps", "ai"] {
            assert!(
                pairs.contains(&(from.into(), "pdf".into(), 10)),
                "{from} -> pdf"
            );
        }
        for from in ["eps", "ps", "ai", "pdf"] {
            let cost = if from == "pdf" { 12 } else { 10 };
            for to in ["png", "jpg", "tiff"] {
                assert!(
                    pairs.contains(&(from.into(), to.into(), cost)),
                    "{from} -> {to} at {cost}"
                );
            }
        }
    }

    #[test]
    fn every_edge_has_a_device() {
        for edge in GhostscriptEngine::new().edges() {
            assert!(
                Device::for_pair(edge.from.as_str(), edge.to.as_str()).is_some(),
                "{} -> {}",
                edge.from,
                edge.to
            );
        }
    }

    #[test]
    fn a_pair_without_an_edge_is_refused() {
        let err = GhostscriptEngine::new()
            .convert(
                &step("pdf", "webp"),
                &mut NoOutputs,
                &|_| {},
                &CancelToken::new(),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "Ghostscript cannot convert pdf to webp");
    }

    #[test]
    fn an_ai_file_that_is_neither_pdf_nor_postscript_is_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let input = dir.path().join("fake.ai");
        fs::write(&input, b"definitely not illustrator").expect("write");
        let mut s = step("ai", "png");
        s.input = input;
        let err = GhostscriptEngine::new()
            .convert(&s, &mut NoOutputs, &|_| {}, &CancelToken::new())
            .unwrap_err();
        assert_eq!(err.to_string(), NOT_AN_AI_FILE);
    }

    #[test]
    fn status_comes_from_discovery() {
        assert_eq!(
            GhostscriptEngine::new().status(),
            discovery::status(Tool::Ghostscript)
        );
    }

    #[test]
    fn collect_and_clear_handle_run_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        for name in ["p10.png", "p2.png", "p1.png", "stray.png", "p3.jpg"] {
            fs::write(dir.path().join(name), b"x").expect("write");
        }
        let pages: Vec<usize> = collect(dir.path(), "png")
            .expect("list")
            .into_keys()
            .collect();
        assert_eq!(pages, [1, 2, 10]);
        clear(dir.path()).expect("clear");
        assert_eq!(fs::read_dir(dir.path()).expect("list").count(), 0);
    }

    #[test]
    fn empty_outputs_are_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let from = dir.path().join("p1.png");
        fs::write(&from, b"").expect("write");
        let err = place(&from, &dir.path().join("out.png"), 1).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Ghostscript produced an empty image for page 1"
        );
    }
}
