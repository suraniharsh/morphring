//! Merge mode: many inputs, one output (Merge PDF). Ported from Filee's
//! `src/Filee.Core/Conversion/JobQueue.cs`, the merge branch of `RunAsync`
//! (`RunMergedAsync`, lines 116-154).
//!
//! The recipe: turn every non-PDF input into a PDF in the job's work dir, in
//! parallel, then hand the lot, in input order, to an engine's [`Merger`].
//! One output comes out, named after the first file of the job.
//!
//! Two departures from Filee, both on purpose:
//! - when file 1 failed, Filee still pins the merged PDF on it (a failed file
//!   holding the job's only output). Here the output goes to the first file
//!   that made it.
//! - a merge error does not escape and sink the job; every file still in the
//!   running gets a proper `ConversionFailed` instead.

use std::cell::{Cell, RefCell};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use super::{FinalAllocator, Job, JobEvent, Outcome, Target, TempAllocator, panic_message};
use crate::contract::{Converter, FileResult, FileState, FormatId, Merger, Step};
use crate::error::{EngineError, ErrorKey};
use crate::formats;
use crate::planner::Route;

/// The share of a file's progress bar spent converting it to PDF; the merge
/// gets the rest.
const CONVERT_SHARE: f32 = 0.8;

const PDF: FormatId = FormatId::new("pdf");

/// Runs the whole job as one merge when its options ask for it and an engine
/// can do it. `None` means "not a merge job"; the caller converts file by file.
pub(super) fn try_run(job: &Job<'_>, files: &[PathBuf]) -> Option<Vec<FileResult>> {
    if !job.options.pdf.merge_into_single || job.target != Target::Format(PDF) || files.len() < 2 {
        return None;
    }
    let (engine, merger) = find_merger(job)?;
    Some(
        Merge {
            job,
            files,
            engine,
            merger,
        }
        .run(),
    )
}

/// The first installed engine, willing to take this job's options, that can merge.
fn find_merger<'a>(job: &Job<'a>) -> Option<(&'a dyn Converter, &'a dyn Merger)> {
    job.engines.iter().find_map(|&engine| {
        if !engine.status().is_available() || !engine.accepts(job.options) {
            return None;
        }
        engine.merger().map(|merger| (engine, merger))
    })
}

/// Where one input stands once the conversion phase is over.
enum Part {
    /// The PDFs this input contributes to the merge, in order.
    Ready(Vec<PathBuf>),
    /// The input is out of the merge, for this reason.
    Out(FileState),
}

impl From<Outcome> for Part {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Finished(pdfs) => Self::Ready(pdfs),
            Outcome::Cancelled => Self::Out(FileState::Cancelled),
            Outcome::Failed(key, detail) => Self::Out(FileState::Failed { key, detail }),
        }
    }
}

/// One merge job in flight.
struct Merge<'j, 'a> {
    job: &'j Job<'a>,
    files: &'j [PathBuf],
    engine: &'a dyn Converter,
    merger: &'a dyn Merger,
}

impl Merge<'_, '_> {
    fn run(&self) -> Vec<FileResult> {
        let parts = self.convert_all();
        let alive: Vec<usize> = (0..parts.len())
            .filter(|&i| matches!(parts[i], Part::Ready(_)))
            .collect();

        // Everyone starts from where the conversion phase left them; the merge
        // below only rewrites the files that are still in the running.
        let mut results: Vec<FileResult> = parts
            .iter()
            .zip(self.files)
            .map(|(part, source)| FileResult {
                source: source.clone(),
                // Ready files that never reach the merge (nothing to merge,
                // cancel) get their real state below; Done is a placeholder.
                state: match part {
                    Part::Ready(_) => FileState::Done,
                    Part::Out(state) => state.clone(),
                },
                outputs: Vec::new(),
            })
            .collect();

        let inputs: Vec<PathBuf> = parts
            .into_iter()
            .filter_map(|part| match part {
                Part::Ready(pdfs) => Some(pdfs),
                Part::Out(_) => None,
            })
            .flatten()
            .collect();

        if self.job.cancel.is_cancelled() {
            for &i in &alive {
                results[i].state = FileState::Cancelled;
            }
        } else if !inputs.is_empty() {
            let (state, outputs) = self.merge(&inputs, &alive);
            for &i in &alive {
                results[i].state = state.clone();
            }
            // The output belongs to file 1, unless file 1 is out of the game:
            // then the first survivor carries it (Filee would hand it to the
            // failed file 1, which is a bit like giving the trophy to the
            // player who was sent off).
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

    /// Converts every input to PDF in parallel, the same tiny work queue
    /// `run_job` uses. Returns one part per input, in input order.
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

    /// Turns one input into the PDF(s) it brings to the merge.
    fn convert_one(&self, index: usize, source: &Path) -> Part {
        let job = self.job;
        if job.cancel.is_cancelled() {
            return Part::Out(FileState::Cancelled);
        }
        if formats::detect(source).is_some_and(|f| f.id == PDF) {
            // Already a PDF: it goes in as it is, no detour through an engine.
            job.emit(JobEvent::FileProgress {
                index,
                progress: CONVERT_SHARE,
            });
            return Part::Ready(vec![source.to_path_buf()]);
        }
        let route = match job.plan_file(source) {
            Ok((_, route)) => route,
            Err(outcome) => return outcome.into(),
        };
        let work_dir = match &job.work_dir {
            Ok(dir) => dir.path(),
            Err(why) => return Outcome::Failed(ErrorKey::ConversionFailed, why.clone()).into(),
        };

        // No cleanup on failure here: every byte lands in the work dir, and
        // the whole work dir goes in the bin when the job ends.
        let run = panic::catch_unwind(AssertUnwindSafe(|| {
            self.run_hops(index, source, &route, work_dir)
        }));
        match run {
            Ok(Ok(pdfs)) if pdfs.is_empty() => Part::Out(FileState::Failed {
                key: ErrorKey::ConversionFailed,
                detail: "the conversion produced no PDF".into(),
            }),
            Ok(Ok(pdfs)) => Part::Ready(pdfs),
            Ok(Err(_)) if job.cancel.is_cancelled() => Part::Out(FileState::Cancelled),
            Ok(Err(EngineError::Cancelled)) => Part::Out(FileState::Cancelled),
            Ok(Err(err)) => {
                tracing::warn!("converting {} to PDF failed: {err}", source.display());
                Outcome::Failed(err.key(), err.to_string()).into()
            }
            Err(payload) => {
                let message = panic_message(payload.as_ref());
                tracing::warn!("converting {} panicked: {message}", source.display());
                Outcome::Failed(ErrorKey::ConversionFailed, message).into()
            }
        }
    }

    /// `run_route`'s twin: every hop, last one included, writes into the work
    /// dir, and progress is squeezed into the first [`CONVERT_SHARE`] of the
    /// bar. It's a twin rather than a call because `run_route` reports
    /// progress on the full 0 to 1 scale, and a bar that hits 100% before the
    /// merge has even started would be telling fibs.
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

    /// Merges `inputs` into the job's one output and returns the state every
    /// file in `alive` ends up in, plus the output(s) written.
    fn merge(&self, inputs: &[PathBuf], alive: &[usize]) -> (FileState, Vec<PathBuf>) {
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
                inner: job.final_allocator(0, &self.files[0], PDF),
                created: &created,
            };
            let _permit = match job.gates.get(self.engine.id()) {
                Some(gate) => Some(gate.acquire(job.cancel).ok_or(EngineError::Cancelled)?),
                None => None,
            };
            self.merger.merge(inputs, &mut out, &progress, job.cancel)
        }));
        let state = match run {
            // The allocator said "skip" (the output exists, Skip policy).
            Ok(Ok(outputs)) if outputs.is_empty() => return (FileState::Skipped, outputs),
            Ok(Ok(outputs)) => return (FileState::Done, outputs),
            Ok(Err(_)) if job.cancel.is_cancelled() => FileState::Cancelled,
            Ok(Err(EngineError::Cancelled)) => FileState::Cancelled,
            Ok(Err(err)) => {
                tracing::warn!("merging {} files failed: {err}", inputs.len());
                FileState::Failed {
                    key: ErrorKey::ConversionFailed,
                    detail: err.to_string(),
                }
            }
            Err(payload) => {
                let message = panic_message(payload.as_ref());
                tracing::warn!("merging {} files panicked: {message}", inputs.len());
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
    use crate::options::PdfOptions;
    use crate::output::{ConflictPolicy, OutputRule};
    use tempfile::TempDir;

    type ConvertFn =
        dyn Fn(&Step, &mut dyn OutputAllocator) -> Result<Vec<PathBuf>, EngineError> + Send + Sync;

    /// What the fake merger does when asked to merge.
    enum MergeBehaviour {
        /// Glue the inputs' bytes together, in order.
        Concat,
        /// Fail with this message.
        Fail(&'static str),
        /// Pull the plug on the job mid-merge.
        Cancel(CancelToken),
    }

    /// A PDF toolkit for tests: converts with a closure and merges by
    /// concatenating bytes, which is how PDFs work if you squint really hard.
    struct Kit {
        edges: Vec<Edge>,
        convert: Box<ConvertFn>,
        merge: Option<MergeBehaviour>,
        merged: Mutex<Vec<Vec<PathBuf>>>,
    }

    impl Kit {
        /// png and gif to PDF; a source whose content is `bad` fails.
        fn new(merge: Option<MergeBehaviour>) -> Self {
            Self {
                edges: vec![Edge::new("png", "pdf"), Edge::new("gif", "pdf")],
                convert: Box::new(|step, out| {
                    let content = fs::read_to_string(&step.input)?;
                    if content == "bad" {
                        return Err(EngineError::Failed("boom".into()));
                    }
                    let Some(path) = out.allocate("pdf", "") else {
                        return Ok(Vec::new());
                    };
                    fs::write(&path, format!("[{content}]"))?;
                    Ok(vec![path])
                }),
                merge,
                merged: Mutex::new(Vec::new()),
            }
        }

        fn merge_calls(&self) -> Vec<Vec<PathBuf>> {
            self.merged.lock().unwrap().clone()
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
            &self.edges
        }
        fn status(&self) -> EngineStatus {
            EngineStatus::Available {
                path: PathBuf::from("/usr/bin/true"),
                version: "1".into(),
            }
        }
        fn merger(&self) -> Option<&dyn Merger> {
            self.merge.as_ref().map(|_| self as &dyn Merger)
        }
        fn convert(
            &self,
            step: &Step,
            out: &mut dyn OutputAllocator,
            progress: &dyn Fn(f32),
            _cancel: &CancelToken,
        ) -> Result<Vec<PathBuf>, EngineError> {
            let written = (self.convert)(step, out)?;
            progress(1.0);
            Ok(written)
        }
    }

    impl Merger for Kit {
        fn merge(
            &self,
            inputs: &[PathBuf],
            out: &mut dyn OutputAllocator,
            progress: &dyn Fn(f32),
            _cancel: &CancelToken,
        ) -> Result<Vec<PathBuf>, EngineError> {
            self.merged.lock().unwrap().push(inputs.to_vec());
            match self.merge.as_ref() {
                Some(MergeBehaviour::Fail(why)) => return Err(EngineError::Failed((*why).into())),
                Some(MergeBehaviour::Cancel(token)) => {
                    token.cancel();
                    return Err(EngineError::Cancelled);
                }
                _ => {}
            }
            let Some(path) = out.allocate("pdf", "") else {
                return Ok(Vec::new());
            };
            let mut bytes = Vec::new();
            for input in inputs {
                bytes.extend(fs::read(input)?);
            }
            fs::write(&path, bytes)?;
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

    fn merge_options() -> ConvertOptions {
        ConvertOptions {
            pdf: PdfOptions {
                merge_into_single: true,
                ..PdfOptions::default()
            },
            ..ConvertOptions::default()
        }
    }

    fn run_full(
        files: &[PathBuf],
        options: &ConvertOptions,
        rule: &OutputRule,
        kit: &Kit,
        cancel: &CancelToken,
        on_event: &(dyn Fn(JobEvent) + Sync),
    ) -> Vec<FileResult> {
        run_job(
            files,
            Target::Format(PDF),
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
            &merge_options(),
            &OutputRule::default(),
            kit,
            &CancelToken::new(),
            &|_| {},
        )
    }

    fn states(results: &[FileResult]) -> Vec<FileState> {
        results.iter().map(|r| r.state.clone()).collect()
    }

    fn boom() -> FileState {
        FileState::Failed {
            key: ErrorKey::ConversionFailed,
            detail: "boom".into(),
        }
    }

    #[test]
    fn merges_in_input_order_into_one_pdf_named_after_file_one() {
        let dir = tempfile::tempdir().unwrap();
        let files = [
            write(&dir, "a.png", "A"),
            write(&dir, "b.pdf", "B"),
            write(&dir, "c.gif", "C"),
            write(&dir, "d.png", "D"),
        ];
        let kit = Kit::new(Some(MergeBehaviour::Concat));

        let results = run(&files, &kit);

        let out = dir.path().join("a.pdf");
        assert_eq!(states(&results), vec![FileState::Done; 4]);
        assert_eq!(results[0].outputs, vec![out.clone()]);
        assert!(results[1..].iter().all(|r| r.outputs.is_empty()));
        assert_eq!(fs::read_to_string(&out).unwrap(), "[A]B[C][D]");
        // The PDF input went in untouched, from where it lives.
        let calls = kit.merge_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0][1], files[1]);
        // Nothing but the merged PDF was added next to the sources.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 5);
    }

    #[test]
    fn progress_splits_conversion_and_merge_and_only_goes_up() {
        let dir = tempfile::tempdir().unwrap();
        let files = [write(&dir, "a.png", "A"), write(&dir, "b.pdf", "B")];
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        let events = Mutex::new(Vec::new());

        let results = run_full(
            &files,
            &merge_options(),
            &OutputRule::default(),
            &kit,
            &CancelToken::new(),
            &|e| events.lock().unwrap().push(e),
        );

        assert_eq!(states(&results), vec![FileState::Done; 2]);
        let events = events.into_inner().unwrap();
        for index in 0..2 {
            let seen: Vec<f32> = events
                .iter()
                .filter_map(|e| match e {
                    JobEvent::FileProgress { index: i, progress } if *i == index => Some(*progress),
                    _ => None,
                })
                .collect();
            assert!(seen.windows(2).all(|w| w[0] < w[1]), "{seen:?}");
            assert!(seen.contains(&CONVERT_SHARE), "{seen:?}");
            // Something in the merge band, and a full bar at the end.
            assert!(seen.iter().any(|&p| p > 0.85 && p < 0.95), "{seen:?}");
            assert!((seen.last().unwrap() - 1.0).abs() < 1e-6, "{seen:?}");
            let started = events
                .iter()
                .filter(|e| matches!(e, JobEvent::FileStarted { index: i, .. } if *i == index));
            assert_eq!(started.count(), 1);
            let finished = events
                .iter()
                .filter(|e| matches!(e, JobEvent::FileFinished { index: i, .. } if *i == index));
            assert_eq!(finished.count(), 1);
        }
        assert!(matches!(events.last(), Some(JobEvent::FileFinished { .. })));
    }

    #[test]
    fn a_failing_middle_file_is_left_out_of_the_merge() {
        let dir = tempfile::tempdir().unwrap();
        let files = [
            write(&dir, "a.png", "A"),
            write(&dir, "b.png", "bad"),
            write(&dir, "c.png", "C"),
        ];
        let kit = Kit::new(Some(MergeBehaviour::Concat));

        let results = run(&files, &kit);

        assert_eq!(
            states(&results),
            vec![FileState::Done, boom(), FileState::Done]
        );
        let out = dir.path().join("a.pdf");
        assert_eq!(results[0].outputs, vec![out.clone()]);
        assert_eq!(fs::read_to_string(&out).unwrap(), "[A][C]");
        assert_eq!(
            JobState::from_files(&results, false),
            JobState::CompletedWithErrors
        );
    }

    #[test]
    fn when_file_one_fails_the_first_survivor_holds_the_output() {
        let dir = tempfile::tempdir().unwrap();
        let files = [
            write(&dir, "a.png", "bad"),
            write(&dir, "b.png", "B"),
            write(&dir, "c.png", "C"),
        ];
        let kit = Kit::new(Some(MergeBehaviour::Concat));

        let results = run(&files, &kit);

        assert_eq!(
            states(&results),
            vec![boom(), FileState::Done, FileState::Done]
        );
        // Still named after file 1, like Filee; just not owned by it.
        let out = dir.path().join("a.pdf");
        assert!(results[0].outputs.is_empty());
        assert_eq!(results[1].outputs, vec![out.clone()]);
        assert!(results[2].outputs.is_empty());
        assert_eq!(fs::read_to_string(&out).unwrap(), "[B][C]");
    }

    #[test]
    fn every_file_failing_means_no_merge() {
        let dir = tempfile::tempdir().unwrap();
        let files = [write(&dir, "a.png", "bad"), write(&dir, "b.png", "bad")];
        let kit = Kit::new(Some(MergeBehaviour::Concat));

        let results = run(&files, &kit);

        assert_eq!(states(&results), vec![boom(), boom()]);
        assert!(kit.merge_calls().is_empty());
        assert!(!dir.path().join("a.pdf").exists());
    }

    #[test]
    fn skip_policy_skips_every_file() {
        let dir = tempfile::tempdir().unwrap();
        let files = [write(&dir, "a.png", "A"), write(&dir, "b.png", "B")];
        let existing = write(&dir, "a.pdf", "old");
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        let rule = OutputRule {
            conflict: ConflictPolicy::Skip,
            ..OutputRule::default()
        };

        let results = run_full(
            &files,
            &merge_options(),
            &rule,
            &kit,
            &CancelToken::new(),
            &|_| {},
        );

        assert_eq!(states(&results), vec![FileState::Skipped; 2]);
        assert!(results.iter().all(|r| r.outputs.is_empty()));
        assert_eq!(fs::read_to_string(existing).unwrap(), "old");
        assert_eq!(JobState::from_files(&results, false), JobState::Completed);
    }

    #[test]
    fn a_merge_error_fails_every_surviving_file() {
        let dir = tempfile::tempdir().unwrap();
        let files = [
            write(&dir, "a.png", "A"),
            write(&dir, "b.png", "bad"),
            write(&dir, "c.png", "C"),
        ];
        let kit = Kit::new(Some(MergeBehaviour::Fail("merge went sideways")));

        let results = run(&files, &kit);

        let sideways = FileState::Failed {
            key: ErrorKey::ConversionFailed,
            detail: "merge went sideways".into(),
        };
        assert_eq!(states(&results), vec![sideways.clone(), boom(), sideways]);
        assert!(results.iter().all(|r| r.outputs.is_empty()));
        assert!(!dir.path().join("a.pdf").exists());
    }

    #[test]
    fn cancel_before_start_cancels_every_file() {
        let dir = tempfile::tempdir().unwrap();
        let files = [write(&dir, "a.png", "A"), write(&dir, "b.pdf", "B")];
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        let cancel = CancelToken::new();
        cancel.cancel();

        let results = run_full(
            &files,
            &merge_options(),
            &OutputRule::default(),
            &kit,
            &cancel,
            &|_| {},
        );

        assert_eq!(states(&results), vec![FileState::Cancelled; 2]);
        assert!(kit.merge_calls().is_empty());
        assert!(!dir.path().join("a.pdf").exists());
    }

    #[test]
    fn cancel_during_the_merge_cancels_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let files = [
            write(&dir, "a.png", "A"),
            write(&dir, "b.png", "bad"),
            write(&dir, "c.png", "C"),
        ];
        let cancel = CancelToken::new();
        let kit = Kit::new(Some(MergeBehaviour::Cancel(cancel.clone())));

        let results = run_full(
            &files,
            &merge_options(),
            &OutputRule::default(),
            &kit,
            &cancel,
            &|_| {},
        );

        assert_eq!(
            states(&results),
            vec![FileState::Cancelled, boom(), FileState::Cancelled]
        );
        assert!(!dir.path().join("a.pdf").exists());
    }

    #[test]
    fn not_a_merge_job_converts_file_by_file() {
        let dir = tempfile::tempdir().unwrap();
        let files = [write(&dir, "a.png", "A"), write(&dir, "b.png", "B")];

        // The option is off.
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        let results = run_full(
            &files,
            &ConvertOptions::default(),
            &OutputRule::default(),
            &kit,
            &CancelToken::new(),
            &|_| {},
        );
        assert_eq!(results[1].outputs, vec![dir.path().join("b.pdf")]);
        assert!(kit.merge_calls().is_empty());

        // No engine can merge.
        let dir = tempfile::tempdir().unwrap();
        let files = [write(&dir, "a.png", "A"), write(&dir, "b.png", "B")];
        let results = run(&files, &Kit::new(None));
        assert_eq!(results[1].outputs, vec![dir.path().join("b.pdf")]);

        // A single file has nothing to merge with.
        let dir = tempfile::tempdir().unwrap();
        let files = [write(&dir, "a.png", "A")];
        let kit = Kit::new(Some(MergeBehaviour::Concat));
        let results = run(&files, &kit);
        assert_eq!(results[0].outputs, vec![dir.path().join("a.pdf")]);
        assert_eq!(fs::read_to_string(dir.path().join("a.pdf")).unwrap(), "[A]");
        assert!(kit.merge_calls().is_empty());
    }
}
