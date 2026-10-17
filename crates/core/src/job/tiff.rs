//! Multi-page TIFF mode: every file (and every page of every file) ends up in
//! one TIFF. Ported from Filee's `src/Filee.Core/Conversion/JobQueue.cs`,
//! `RunMultiPageTiffAsync` (lines 160-193).
//!
//! The recipe is merge mode's, with pages instead of PDFs: every input goes
//! through the planner to PNG in the job's work dir (a PDF fans out into one
//! PNG per page), and the whole stack, in input order, goes to the first
//! engine with a [`TiffMerger`]. One TIFF comes out, named after the first
//! file of the job. One file is enough: a 10-page PDF becomes one 10-page TIFF.
//!
//! Only a TIFF skips the trip, as in Filee (JobQueue.cs:172-174): it is a
//! stack of pages already. Everything else, PNG and JPG included, takes the
//! engine detour. That is not busywork: the detour is where resize,
//! grayscale, quality and friends get applied (the merger only flattens and
//! compresses), and it is what keeps a camera raw or a HEIC the merger cannot
//! read from taking the whole job down with it. One bad file fails alone.
//!
//! Departures from Filee, all on purpose:
//! - Filee converts the pages to TIFF first; here they become PNG, which
//!   every merger reads and which loses nothing on the way. A source with no
//!   road to PNG but one to TIFF takes that one instead.
//! - like merge mode, the output goes to the first file that made it rather
//!   than to a failed file 1, and a merger error fails the files still in the
//!   running instead of escaping the job.
//!
//! Filee also sends a TIFF through the engine when the preset turns or
//! mirrors it. Our options cannot turn anything yet; when rotation and flip
//! arrive, the pass-through check in `convert_one` has to learn about them.

use std::cell::{Cell, RefCell};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use super::{FinalAllocator, Job, JobEvent, Outcome, Target, TempAllocator, panic_message};
use crate::contract::{Converter, FileResult, FileState, FormatId, Step, TiffMerger};
use crate::error::{EngineError, ErrorKey};
use crate::formats;
use crate::planner::Route;

/// The share of a file's progress bar spent turning it into pages; the merge
/// gets the rest.
const CONVERT_SHARE: f32 = 0.8;

const TIFF: FormatId = FormatId::new("tiff");
const PNG: FormatId = FormatId::new("png");

/// Runs the whole job as one multi-page TIFF when its options ask for it and
/// an engine can merge TIFFs. `None` means "not a multi-page TIFF job"; the
/// caller converts file by file.
pub(super) fn try_run(job: &Job<'_>, files: &[PathBuf]) -> Option<Vec<FileResult>> {
    if !job.options.image.multi_page_tiff || job.target != Target::Format(TIFF) {
        return None;
    }
    let (engine, merger) = find_tiff_merger(job)?;
    Some(
        Stack {
            job,
            files,
            engine,
            merger,
        }
        .run(),
    )
}

/// The first installed engine, willing to take this job's options, that can
/// merge TIFF pages.
fn find_tiff_merger<'a>(job: &Job<'a>) -> Option<(&'a dyn Converter, &'a dyn TiffMerger)> {
    job.engines.iter().find_map(|&engine| {
        if !engine.status().is_available() || !engine.accepts(job.options) {
            return None;
        }
        engine.tiff_merger().map(|merger| (engine, merger))
    })
}

/// Where one input stands once the page phase is over.
enum Part {
    /// The frames this input contributes to the TIFF, in order.
    Ready(Vec<PathBuf>),
    /// The input is out of the TIFF, for this reason.
    Out(FileState),
}

impl From<Outcome> for Part {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Finished(frames) => Self::Ready(frames),
            Outcome::Cancelled => Self::Out(FileState::Cancelled),
            Outcome::Failed(key, detail) => Self::Out(FileState::Failed { key, detail }),
        }
    }
}

/// One multi-page TIFF job in flight. A stack of pages, hence the name; the
/// TIFF spec calls them IFDs, which is less fun to say.
struct Stack<'j, 'a> {
    job: &'j Job<'a>,
    files: &'j [PathBuf],
    engine: &'a dyn Converter,
    merger: &'a dyn TiffMerger,
}

impl Stack<'_, '_> {
    fn run(&self) -> Vec<FileResult> {
        let parts = self.convert_all();
        let alive: Vec<usize> = (0..parts.len())
            .filter(|&i| matches!(parts[i], Part::Ready(_)))
            .collect();

        let mut results: Vec<FileResult> = parts
            .iter()
            .zip(self.files)
            .map(|(part, source)| FileResult {
                source: source.clone(),
                // Done is a placeholder for the files still in the running;
                // the merge below decides what they really end up as.
                state: match part {
                    Part::Ready(_) => FileState::Done,
                    Part::Out(state) => state.clone(),
                },
                outputs: Vec::new(),
            })
            .collect();

        let frames: Vec<PathBuf> = parts
            .into_iter()
            .filter_map(|part| match part {
                Part::Ready(frames) => Some(frames),
                Part::Out(_) => None,
            })
            .flatten()
            .collect();

        if self.job.cancel.is_cancelled() {
            for &i in &alive {
                results[i].state = FileState::Cancelled;
            }
        } else if !frames.is_empty() {
            let (state, outputs) = self.merge(&frames, &alive);
            for &i in &alive {
                results[i].state = state.clone();
            }
            if let Some(&owner) = alive.first() {
                if state == FileState::Done && self.job.rule.keep_dates {
                    crate::dates::copy_dates(&self.files[owner], &outputs);
                }
                results[owner].outputs = outputs;
            }
        }

        for (index, result) in results.iter().enumerate() {
            self.job.emit(JobEvent::FileFinished {
                index,
                result: result.clone(),
            });
        }
        results
    }

    /// Turns every input into frames in parallel, the same tiny work queue
    /// `run_job` uses. Returns one part per input, in input order, whatever
    /// order the workers finished in.
    fn convert_all(&self) -> Vec<Part> {
        let slots: Vec<OnceLock<Part>> = self.files.iter().map(|_| OnceLock::new()).collect();
        let next = AtomicUsize::new(0);
        let cores = thread::available_parallelism().map_or(1, |n| n.get());
        thread::scope(|scope| {
            for _ in 0..cores.min(self.files.len()) {
                scope.spawn(|| {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(source) = self.files.get(index) else {
                            break;
                        };
                        self.job.emit(JobEvent::FileStarted {
                            index,
                            source: source.clone(),
                        });
                        let _ = slots[index].set(self.convert_one(index, source));
                    }
                });
            }
        });
        slots
            .into_iter()
            .map(|slot| {
                slot.into_inner().unwrap_or_else(|| {
                    Part::Out(FileState::Failed {
                        key: ErrorKey::ConversionFailed,
                        detail: "the file was not processed".into(),
                    })
                })
            })
            .collect()
    }

    /// Turns one input into the frames it brings to the TIFF.
    fn convert_one(&self, index: usize, source: &Path) -> Part {
        let job = self.job;
        if job.cancel.is_cancelled() {
            return Part::Out(FileState::Cancelled);
        }
        let Some(format) = formats::detect(source) else {
            return Outcome::Failed(
                ErrorKey::UnsupportedSource,
                format!(".{}", formats::extension_of(source)),
            )
            .into();
        };
        if format.id == TIFF {
            // Already a stack of pages: straight onto the pile, no detour.
            job.emit(JobEvent::FileProgress {
                index,
                progress: CONVERT_SHARE,
            });
            return Part::Ready(vec![source.to_path_buf()]);
        }
        let Some(route) = job
            .planner
            .plan(format.id, PNG)
            .or_else(|| job.planner.plan(format.id, TIFF))
        else {
            return job.explain_missing_route(format.id, PNG).into();
        };
        let work_dir = match &job.work_dir {
            Ok(dir) => dir.path(),
            Err(why) => return Outcome::Failed(ErrorKey::ConversionFailed, why.clone()).into(),
        };

        // Every page lands in the work dir, and the work dir goes in the bin
        // when the job ends, failed or not. No sweeping up here.
        let run = panic::catch_unwind(AssertUnwindSafe(|| {
            self.run_hops(index, source, &route, work_dir)
        }));
        match run {
            Ok(Ok(frames)) if frames.is_empty() => Part::Out(FileState::Failed {
                key: ErrorKey::ConversionFailed,
                detail: "the conversion produced no pages".into(),
            }),
            Ok(Ok(frames)) => Part::Ready(frames),
            Ok(Err(_)) if job.cancel.is_cancelled() => Part::Out(FileState::Cancelled),
            Ok(Err(EngineError::Cancelled)) => Part::Out(FileState::Cancelled),
            Ok(Err(err)) => {
                tracing::warn!("converting {} to pages failed: {err}", source.display());
                Outcome::Failed(err.key(), err.to_string()).into()
            }
            Err(payload) => {
                let message = panic_message(payload.as_ref());
                tracing::warn!("converting {} panicked: {message}", source.display());
                Outcome::Failed(ErrorKey::ConversionFailed, message).into()
            }
        }
    }

    /// Runs every hop of `route` into the work dir, progress squeezed into
    /// the first [`CONVERT_SHARE`] of the bar. Merge mode has the same twin
    /// for the same reason: `run_route` would happily report 100% before the
    /// merge has even begun.
    fn run_hops(
        &self,
        index: usize,
        source: &Path,
        route: &Route,
        work_dir: &Path,
    ) -> Result<Vec<PathBuf>, EngineError> {
        let job = self.job;
        let steps = route.hops.len();
        let best = Cell::new(0.0_f32);
        let mut out = TempAllocator::new(work_dir, source);

        let mut inputs = vec![source.to_path_buf()];
        for (s, hop) in route.hops.iter().enumerate() {
            let engine = job.engines[hop.engine];
            let progress = |p: f32| {
                if p.is_nan() {
                    return;
                }
                let value = CONVERT_SHARE * (s as f32 + p.clamp(0.0, 1.0)) / steps as f32;
                if value > best.get() {
                    best.set(value);
                    job.emit(JobEvent::FileProgress {
                        index,
                        progress: value,
                    });
                }
            };
            let mut outputs = Vec::new();
            for input in &inputs {
                if job.cancel.is_cancelled() {
                    return Err(EngineError::Cancelled);
                }
                let step = Step {
                    input: input.clone(),
                    from: hop.from,
                    to: hop.to,
                    options: job.options.clone(),
                    work_dir: work_dir.to_path_buf(),
                };
                let _permit = match job.gates.get(engine.id()) {
                    Some(gate) => Some(gate.acquire(job.cancel).ok_or(EngineError::Cancelled)?),
                    None => None,
                };
                outputs.extend(engine.convert(&step, &mut out, &progress, job.cancel)?);
            }
            inputs = outputs;
        }
        Ok(inputs)
    }

    /// Writes `frames` into the job's one TIFF and returns the state every
    /// file in `alive` ends up in, plus the output(s) written.
    fn merge(&self, frames: &[PathBuf], alive: &[usize]) -> (FileState, Vec<PathBuf>) {
        let job = self.job;
        let best = Cell::new(CONVERT_SHARE);
        let progress = |p: f32| {
            if p.is_nan() {
                return;
            }
            let value = CONVERT_SHARE + (1.0 - CONVERT_SHARE) * p.clamp(0.0, 1.0);
            if value > best.get() {
                best.set(value);
                for &index in alive {
                    job.emit(JobEvent::FileProgress {
                        index,
                        progress: value,
                    });
                }
            }
        };

        // Named after file 1 ({index} = 1), whatever became of file 1.
        let created = RefCell::new(Vec::new());
        let run = panic::catch_unwind(AssertUnwindSafe(|| {
            let mut out = FinalAllocator {
                inner: job.final_allocator(0, &self.files[0], TIFF),
                created: &created,
            };
            let _permit = match job.gates.get(self.engine.id()) {
                Some(gate) => Some(gate.acquire(job.cancel).ok_or(EngineError::Cancelled)?),
                None => None,
            };
            self.merger
                .merge_tiff(frames, &job.options.image, &mut out, &progress, job.cancel)
        }));
        let state = match run {
            // The allocator said "skip" (the output exists, Skip policy).
            Ok(Ok(outputs)) if outputs.is_empty() => return (FileState::Skipped, outputs),
            Ok(Ok(outputs)) => return (FileState::Done, outputs),
            Ok(Err(_)) if job.cancel.is_cancelled() => FileState::Cancelled,
            Ok(Err(EngineError::Cancelled)) => FileState::Cancelled,
            Ok(Err(err)) => {
                tracing::warn!("merging {} pages into a TIFF failed: {err}", frames.len());
                FileState::Failed {
                    key: ErrorKey::ConversionFailed,
                    detail: err.to_string(),
                }
            }
            Err(payload) => {
                let message = panic_message(payload.as_ref());
                tracing::warn!("merging {} pages panicked: {message}", frames.len());
                FileState::Failed {
                    key: ErrorKey::ConversionFailed,
                    detail: message,
                }
            }
        };
        for path in created.borrow().iter() {
            super::remove_path(path);
        }
        (state, Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;

    use super::*;
    use crate::contract::{
        CancelToken, ConvertOptions, Edge, EngineStatus, JobState, OutputAllocator,
    };
    use crate::job::run_job;
    use crate::options::ImageOptions;
    use crate::output::{ConflictPolicy, OutputRule};
    use tempfile::TempDir;

    /// What the fake merger does when asked to stack pages.
    enum MergeBehaviour {
        /// Glue the frames' bytes together, in order, separated by `|`.
        Concat,
        /// Fail with this message.
        Fail(&'static str),
        /// Pull the plug on the job mid-merge.
        Cancel(CancelToken),
    }

    /// A page factory for tests. A PDF (or a txt) whose content is
    /// `a,b,c` renders into three PNG pages saying `a`, `b` and `c`; one
    /// saying `bad` fails. The merger concatenates, which is roughly how a
    /// multi-page TIFF works if you squint until your eyes water. Like a real
    /// merger, it only reads PNG and TIFF frames and chokes on anything else.
    struct Kit {
        merge: Option<MergeBehaviour>,
        merged: Mutex<Vec<Vec<PathBuf>>>,
        /// Every step the engine was asked to run, in call order.
        steps: Mutex<Vec<Step>>,
        /// Whether every frame handed to the merger still existed at the time.
        frames_existed: Mutex<bool>,
        /// Cancel this token when a page conversion starts.
        cancel_on_convert: Option<CancelToken>,
    }

    impl Kit {
        fn new(merge: Option<MergeBehaviour>) -> Self {
            Self {
                merge,
                merged: Mutex::new(Vec::new()),
                steps: Mutex::new(Vec::new()),
                frames_existed: Mutex::new(true),
                cancel_on_convert: None,
            }
        }

        fn merge_calls(&self) -> Vec<Vec<PathBuf>> {
            self.merged.lock().unwrap().clone()
        }

        fn steps(&self) -> Vec<Step> {
            self.steps.lock().unwrap().clone()
        }
    }

    impl Converter for Kit {
        fn id(&self) -> &'static str {
            "kit"
        }
        fn display_name(&self) -> &'static str {
            "Kit"
        }
        fn edges(&self) -> &[Edge] {
            const EDGES: &[Edge] = &[
                Edge::new("pdf", "png"),
                Edge::new("txt", "pdf"),
                Edge::new("png", "png"),
                Edge::new("jpg", "png"),
                Edge::new("heic", "png"),
                Edge::new("png", "tiff"),
            ];
            EDGES
        }
        fn status(&self) -> EngineStatus {
            EngineStatus::Available {
                path: PathBuf::from("/usr/bin/true"),
                version: "1".into(),
            }
        }
        fn tiff_merger(&self) -> Option<&dyn TiffMerger> {
            self.merge.as_ref().map(|_| self as &dyn TiffMerger)
        }
        fn convert(
            &self,
            step: &Step,
            out: &mut dyn OutputAllocator,
            progress: &dyn Fn(f32),
            _cancel: &CancelToken,
        ) -> Result<Vec<PathBuf>, EngineError> {
            self.steps.lock().unwrap().push(step.clone());
            if let Some(token) = &self.cancel_on_convert {
                token.cancel();
                return Err(EngineError::Cancelled);
            }
            let content = fs::read_to_string(&step.input)?;
            if content == "bad" {
                return Err(EngineError::Failed("boom".into()));
            }
            let mut written = Vec::new();
            if step.to.as_str() == "png" {
                for (n, page) in content.split(',').enumerate() {
                    let Some(path) = out.allocate("png", &format!("_p{}", n + 1)) else {
                        continue;
                    };
                    fs::write(&path, page)?;
                    written.push(path);
                }
            } else {
                let Some(path) = out.allocate(step.to.as_str(), "") else {
                    return Ok(written);
                };
                fs::write(&path, &content)?;
                written.push(path);
            }
            progress(1.0);
            Ok(written)
        }
    }

    impl TiffMerger for Kit {
        fn merge_tiff(
            &self,
            inputs: &[PathBuf],
            _options: &ImageOptions,
            out: &mut dyn OutputAllocator,
            progress: &dyn Fn(f32),
            _cancel: &CancelToken,
        ) -> Result<Vec<PathBuf>, EngineError> {
            self.merged.lock().unwrap().push(inputs.to_vec());
            if !inputs.iter().all(|p| p.exists()) {
                *self.frames_existed.lock().unwrap() = false;
            }
            match self.merge.as_ref() {
                Some(MergeBehaviour::Fail(why)) => return Err(EngineError::Failed((*why).into())),
                Some(MergeBehaviour::Cancel(token)) => {
                    token.cancel();
                    return Err(EngineError::Cancelled);
                }
                _ => {}
            }
            let readable = |p: &PathBuf| {
                p.extension()
                    .is_some_and(|e| e == "png" || e == "tiff" || e == "tif")
            };
            if let Some(bad) = inputs.iter().find(|p| !readable(p)) {
                return Err(EngineError::Failed(format!(
                    "no decode delegate for {}",
                    bad.display()
                )));
            }
            let Some(path) = out.allocate("tiff", "") else {
                return Ok(Vec::new());
            };
            let pages: Vec<String> = inputs
                .iter()
                .map(fs::read_to_string)
                .collect::<Result<_, _>>()?;
            fs::write(&path, pages.join("|"))?;
            progress(0.5);
            progress(1.0);
            Ok(vec![path])
        }
    }

    fn write(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        path
    }

    fn tiff_options() -> ConvertOptions {
        let mut options = ConvertOptions::default();
        options.image.multi_page_tiff = true;
        options
    }

    fn run_full(
        files: &[PathBuf],
        rule: &OutputRule,
        kit: &Kit,
        cancel: &CancelToken,
        on_event: &(dyn Fn(JobEvent) + Sync),
    ) -> Vec<FileResult> {
        run_with(files, &tiff_options(), rule, kit, cancel, on_event)
    }

    fn run_with(
        files: &[PathBuf],
        options: &ConvertOptions,
        rule: &OutputRule,
        kit: &Kit,
        cancel: &CancelToken,
        on_event: &(dyn Fn(JobEvent) + Sync),
    ) -> Vec<FileResult> {
        run_job(
            files,
            Target::Format(TIFF),
            options,
            rule,
            &[kit],
            cancel,
            on_event,
        )
    }

    fn run(files: &[PathBuf], kit: &Kit) -> Vec<FileResult> {
        run_full(
            files,
            &OutputRule::default(),
            kit,
            &CancelToken::new(),
            &|_| {},
        )
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    fn tiffs_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tiff"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn frames_go_in_input_order_and_the_tiff_is_named_after_file_one() {
        let dir = TempDir::new().unwrap();
        let files = [
            write(&dir, "a.png", "A"),
            write(&dir, "deck.pdf", "p1,p2"),
            write(&dir, "c.jpg", "C"),
            write(&dir, "notes.txt", "t1"),
        ];
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        let results = run(&files, &kit);

        assert!(results.iter().all(|r| r.state == FileState::Done));
        let out = dir.path().join("a.tiff");
        assert_eq!(results[0].outputs, vec![out.clone()]);
        assert!(results[1..].iter().all(|r| r.outputs.is_empty()));
        assert_eq!(read(&out), "A|p1|p2|C|t1");
        assert_eq!(tiffs_in(dir.path()), ["a.tiff"]);
        // Images take the engine detour too: the merger only sees pages
        // from the work dir, never the sources themselves.
        let calls = kit.merge_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].len(), 5);
        assert!(calls[0].iter().all(|p| !p.starts_with(dir.path())));
    }

    #[test]
    fn an_image_goes_through_the_engine_with_the_image_options() {
        let dir = TempDir::new().unwrap();
        let files = [write(&dir, "a.png", "A"), write(&dir, "b.pdf", "b1")];
        let mut options = tiff_options();
        options.image.grayscale = true;
        options.image.resize_percent = 50;
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        let results = run_with(
            &files,
            &options,
            &OutputRule::default(),
            &kit,
            &CancelToken::new(),
            &|_| {},
        );

        assert!(results.iter().all(|r| r.state == FileState::Done));
        assert_eq!(read(&dir.path().join("a.tiff")), "A|b1");
        let steps = kit.steps();
        let png = steps
            .iter()
            .find(|s| s.input == files[0])
            .expect("the PNG got a conversion hop");
        assert_eq!((png.from, png.to), (PNG, PNG));
        assert!(png.options.image.grayscale);
        assert_eq!(png.options.image.resize_percent, 50);
    }

    #[test]
    fn a_tiff_joins_as_it_is() {
        let dir = TempDir::new().unwrap();
        let files = [write(&dir, "scan.tif", "S"), write(&dir, "b.png", "B")];
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        let results = run(&files, &kit);

        assert!(results.iter().all(|r| r.state == FileState::Done));
        assert_eq!(read(&dir.path().join("scan.tiff")), "S|B");
        assert!(kit.steps().iter().all(|s| s.input != files[0]));
        assert_eq!(kit.merge_calls()[0][0], files[0]);
    }

    #[test]
    fn an_image_the_merger_cannot_read_fails_alone() {
        // A HEIC the engine cannot decode fails on its own trip to PNG; it
        // never reaches the merger, so it cannot spoil the other pages.
        let dir = TempDir::new().unwrap();
        let files = [
            write(&dir, "a.png", "A"),
            write(&dir, "phone.heic", "bad"),
            write(&dir, "c.jpg", "C"),
        ];
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        let results = run(&files, &kit);

        assert!(matches!(results[1].state, FileState::Failed { .. }));
        assert_eq!(results[0].state, FileState::Done);
        assert_eq!(results[2].state, FileState::Done);
        assert_eq!(read(&dir.path().join("a.tiff")), "A|C");
        assert!(kit.merge_calls()[0].iter().all(|p| p != &files[1]));
    }

    #[test]
    fn one_pdf_expands_to_its_pages() {
        let dir = TempDir::new().unwrap();
        let files = [write(&dir, "report.pdf", "one,two,three")];
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        let results = run(&files, &kit);

        assert_eq!(results[0].state, FileState::Done);
        assert_eq!(read(&dir.path().join("report.tiff")), "one|two|three");
        assert_eq!(kit.merge_calls()[0].len(), 3);
    }

    #[test]
    fn intermediate_pages_are_cleaned_up_after_the_job() {
        let dir = TempDir::new().unwrap();
        let files = [write(&dir, "a.pdf", "x,y")];
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        run(&files, &kit);

        assert!(*kit.frames_existed.lock().unwrap());
        let pages = &kit.merge_calls()[0];
        assert!(pages.iter().all(|p| !p.starts_with(dir.path())));
        assert!(pages.iter().all(|p| !p.exists()), "{pages:?}");
    }

    #[test]
    fn one_failing_input_is_failed_and_the_rest_still_merge() {
        let dir = TempDir::new().unwrap();
        let files = [
            write(&dir, "a.pdf", "bad"),
            write(&dir, "b.png", "B"),
            write(&dir, "c.pdf", "c1,c2"),
            write(&dir, "d.xyz-nope", "?"),
        ];
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        let results = run(&files, &kit);

        let FileState::Failed { key, detail } = &results[0].state else {
            panic!("expected a failure, got {:?}", results[0].state);
        };
        assert_eq!(*key, ErrorKey::ConversionFailed);
        assert!(detail.contains("boom"), "{detail}");
        assert!(matches!(
            results[3].state,
            FileState::Failed {
                key: ErrorKey::UnsupportedSource,
                ..
            }
        ));
        assert_eq!(results[1].state, FileState::Done);
        assert_eq!(results[2].state, FileState::Done);

        // Named after file 1, carried by the first file that made it.
        let out = dir.path().join("a.tiff");
        assert_eq!(read(&out), "B|c1|c2");
        assert!(results[0].outputs.is_empty());
        assert_eq!(results[1].outputs, vec![out]);
        assert_eq!(
            JobState::from_files(&results, false),
            JobState::CompletedWithErrors
        );
    }

    #[test]
    fn a_source_with_no_route_to_png_is_no_route() {
        let dir = TempDir::new().unwrap();
        let files = [write(&dir, "a.mp3", "?"), write(&dir, "b.png", "B")];
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        let results = run(&files, &kit);

        assert!(matches!(
            results[0].state,
            FileState::Failed {
                key: ErrorKey::NoRoute,
                ..
            }
        ));
        assert_eq!(results[1].state, FileState::Done);
    }

    #[test]
    fn a_failing_merger_fails_the_survivors_and_leaves_no_output() {
        let dir = TempDir::new().unwrap();
        let files = [write(&dir, "a.png", "A"), write(&dir, "b.png", "B")];
        let kit = Kit::new(Some(MergeBehaviour::Fail("disk on fire")));
        let results = run(&files, &kit);

        for result in &results {
            let FileState::Failed { key, detail } = &result.state else {
                panic!("expected a failure, got {:?}", result.state);
            };
            assert_eq!(*key, ErrorKey::ConversionFailed);
            assert!(detail.contains("disk on fire"), "{detail}");
            assert!(result.outputs.is_empty());
        }
        assert!(tiffs_in(dir.path()).is_empty());
    }

    #[test]
    fn cancel_during_the_merge_cancels_everyone_and_leaves_no_output() {
        let dir = TempDir::new().unwrap();
        let files = [write(&dir, "a.png", "A"), write(&dir, "b.pdf", "b1")];
        let cancel = CancelToken::new();
        let kit = Kit::new(Some(MergeBehaviour::Cancel(cancel.clone())));
        let results = run_full(&files, &OutputRule::default(), &kit, &cancel, &|_| {});

        assert!(results.iter().all(|r| r.state == FileState::Cancelled));
        assert!(tiffs_in(dir.path()).is_empty());
    }

    #[test]
    fn cancel_during_page_conversion_never_reaches_the_merger() {
        let dir = TempDir::new().unwrap();
        let files = [write(&dir, "a.pdf", "a1"), write(&dir, "b.png", "B")];
        let cancel = CancelToken::new();
        let mut kit = Kit::new(Some(MergeBehaviour::Concat));
        kit.cancel_on_convert = Some(cancel.clone());
        let results = run_full(&files, &OutputRule::default(), &kit, &cancel, &|_| {});

        assert!(results.iter().all(|r| r.state == FileState::Cancelled));
        assert!(kit.merge_calls().is_empty());
        assert!(tiffs_in(dir.path()).is_empty());
    }

    #[test]
    fn cancel_before_start_runs_nothing() {
        let dir = TempDir::new().unwrap();
        let files = [write(&dir, "a.png", "A")];
        let cancel = CancelToken::new();
        cancel.cancel();
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        let results = run_full(&files, &OutputRule::default(), &kit, &cancel, &|_| {});

        assert_eq!(results[0].state, FileState::Cancelled);
        assert!(kit.merge_calls().is_empty());
    }

    #[test]
    fn an_existing_output_under_skip_policy_is_skipped() {
        let dir = TempDir::new().unwrap();
        let files = [write(&dir, "a.png", "A"), write(&dir, "b.png", "B")];
        write(&dir, "a.tiff", "old");
        let rule = OutputRule {
            conflict: ConflictPolicy::Skip,
            ..OutputRule::default()
        };
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        let results = run_full(&files, &rule, &kit, &CancelToken::new(), &|_| {});

        assert!(results.iter().all(|r| r.state == FileState::Skipped));
        assert_eq!(read(&dir.path().join("a.tiff")), "old");
    }

    #[test]
    fn without_a_tiff_merger_files_convert_one_by_one_as_before() {
        let dir = TempDir::new().unwrap();
        let files = [write(&dir, "a.png", "A"), write(&dir, "b.png", "B")];
        let kit = Kit::new(None);
        let results = run(&files, &kit);

        assert!(results.iter().all(|r| r.state == FileState::Done));
        assert_eq!(tiffs_in(dir.path()), ["a.tiff", "b.tiff"]);
    }

    #[test]
    fn without_the_option_files_convert_one_by_one() {
        let dir = TempDir::new().unwrap();
        let files = [write(&dir, "a.png", "A"), write(&dir, "b.png", "B")];
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        let results = run_job(
            &files,
            Target::Format(TIFF),
            &ConvertOptions::default(),
            &OutputRule::default(),
            &[&kit],
            &CancelToken::new(),
            &|_| {},
        );

        assert!(results.iter().all(|r| r.state == FileState::Done));
        assert!(kit.merge_calls().is_empty());
        assert_eq!(tiffs_in(dir.path()), ["a.tiff", "b.tiff"]);
    }

    #[test]
    fn every_file_gets_started_rising_progress_and_finished() {
        let dir = TempDir::new().unwrap();
        let files = [write(&dir, "a.png", "A"), write(&dir, "b.pdf", "b1,b2")];
        let events = Mutex::new(Vec::new());
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        run_full(
            &files,
            &OutputRule::default(),
            &kit,
            &CancelToken::new(),
            &|e| events.lock().unwrap().push(e),
        );

        let events = events.into_inner().unwrap();
        for index in 0..files.len() {
            let mine: Vec<&JobEvent> = events
                .iter()
                .filter(|e| match e {
                    JobEvent::FileStarted { index: i, .. }
                    | JobEvent::FileProgress { index: i, .. }
                    | JobEvent::FileFinished { index: i, .. } => *i == index,
                })
                .collect();
            assert!(matches!(mine.first(), Some(JobEvent::FileStarted { .. })));
            assert!(matches!(mine.last(), Some(JobEvent::FileFinished { .. })));
            let progress: Vec<f32> = mine
                .iter()
                .filter_map(|e| match e {
                    JobEvent::FileProgress { progress, .. } => Some(*progress),
                    _ => None,
                })
                .collect();
            assert!(progress.windows(2).all(|w| w[0] < w[1]), "{progress:?}");
            assert_eq!(progress.last(), Some(&1.0));
        }
    }
}
