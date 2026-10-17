//! The job runner: converts a batch of files to one target format, in
//! parallel, with a concurrency limit per engine.
//!
//! Ported from Filee's `src/Filee.Core/Conversion/JobQueue.cs`, minus the
//! queue itself (the CLI runs a single job). The job modes live next door:
//! merge in [`merge`], multi-page TIFF in [`tiff`] and combine-into-one-archive
//! in [`combine`] (the last two are still hooks waiting for their engines).

mod combine;
mod merge;
mod tiff;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::Duration;

use crate::contract::{
    CancelToken, ConvertOptions, Converter, Edge, EngineStatus, FileResult, FileState, FormatId,
    OutputAllocator, Step,
};
use crate::error::{EngineError, ErrorKey};
use crate::formats;
use crate::output::{JobAllocator, OutputRule, Reservations, Tokens};
use crate::planner::{Planner, Route};

/// What a job converts its files to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// Every file becomes this format.
    Format(FormatId),
    /// Every file stays in its own format, re-encoded through the engine's self
    /// edge (`png -> png`). Used by presets such as "Half size" whose options do
    /// the work while the format stays put.
    SameAsSource,
}

impl Target {
    /// The format `source_format` is converted to under this target.
    pub fn for_source(self, source_format: FormatId) -> FormatId {
        match self {
            Self::Format(format) => format,
            Self::SameAsSource => source_format,
        }
    }
}

impl From<FormatId> for Target {
    fn from(format: FormatId) -> Self {
        Self::Format(format)
    }
}

/// Something that happened to one file of a running job.
///
/// `index` is the file's position in the slice given to [`run_job`]. Events
/// arrive from worker threads, possibly interleaved between files.
#[derive(Clone, Debug, PartialEq)]
pub enum JobEvent {
    /// Work on the file began. Sent once per file, before any other event for it.
    FileStarted { index: usize, source: PathBuf },
    /// The file's overall progress, in `0.0..=1.0`. Values only go up.
    FileProgress { index: usize, progress: f32 },
    /// The file reached its final state. Sent once per file, last.
    FileFinished { index: usize, result: FileResult },
}

/// Converts every file in `files` to `target` and returns one result per
/// file, in input order.
///
/// Routes are planned once for the job over the engines whose status is
/// available and that accept `options` ([`Converter::accepts`]). Files run in parallel, at most one per CPU core, and each engine
/// runs at most `max_parallelism()` steps at a time (0 means one per core).
/// Intermediate files of multi-step routes live in a private folder below
/// `$TMPDIR/morphring/`, which is removed before this returns. Final outputs
/// are named by `rule`; with `rule.keep_dates` a finished file's outputs get
/// its dates. A failure, panic or cancellation only affects its own
/// file; partial outputs of a failed or cancelled file are deleted.
///
/// Use [`JobState::from_files`](crate::JobState::from_files) on the result to
/// get the state of the whole job.
pub fn run_job(
    files: &[PathBuf],
    target: Target,
    options: &ConvertOptions,
    rule: &OutputRule,
    engines: &[&dyn Converter],
    cancel: &CancelToken,
    on_event: &(dyn Fn(JobEvent) + Sync),
) -> Vec<FileResult> {
    if files.is_empty() {
        return Vec::new();
    }

    // An engine that turns these options down sits this job out. It is swapped
    // for a stand-in with no edges rather than removed, so route hops still
    // index into `engines` and registry priorities stay exactly where they were.
    let installed: Vec<&dyn Converter> = engines
        .iter()
        .map(|&e| if e.accepts(options) { e } else { &BENCHED })
        .collect();

    let cores = thread::available_parallelism().map_or(1, |n| n.get());
    let job = Job {
        target,
        options,
        rule,
        engines,
        cancel,
        on_event,
        planner: Planner::new(&installed, false),
        // Only consulted when a route is missing, but building it costs about as
        // much as a blink, and doing it once beats racing to do it lazily.
        dream_planner: Planner::new(engines, true),
        gates: engines
            .iter()
            .map(|e| {
                (
                    e.id(),
                    Gate::new(effective_limit(e.max_parallelism(), cores)),
                )
            })
            .collect(),
        reserved: Reservations::default(),
        now: chrono::Local::now().naive_local(),
        work_dir: create_work_dir(),
    };

    // Filee's order (JobQueue.cs:81-86): merge, then multi-page TIFF, then
    // combine. The first mode that claims the job runs all of it.
    if let Some(results) = merge::try_run(&job, files)
        .or_else(|| tiff::try_run(&job, files))
        .or_else(|| combine::try_run(&job, files))
    {
        return results;
    }

    let slots: Vec<OnceLock<FileResult>> = files.iter().map(|_| OnceLock::new()).collect();
    let next = AtomicUsize::new(0);
    let workers = cores.min(files.len());
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                // A tiny work queue: whoever is free grabs the next file. Cheaper
                // than a channel and just as fair when every item is "a file".
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(source) = files.get(index) else {
                        break;
                    };
                    let result = job.run_file(index, source);
                    job.emit(JobEvent::FileFinished {
                        index,
                        result: result.clone(),
                    });
                    let _ = slots[index].set(result);
                }
            });
        }
    });

    // Dropping `job` removes the work dir (TempDir does the honours).
    drop(job);

    slots
        .into_iter()
        .zip(files)
        .map(|(slot, source)| {
            slot.into_inner().unwrap_or_else(|| FileResult {
                source: source.clone(),
                state: FileState::Failed {
                    key: ErrorKey::ConversionFailed,
                    detail: "the file was not processed".into(),
                },
                outputs: Vec::new(),
            })
        })
        .collect()
}

/// Fails every file of a job with `detail`, for a job mode that is wired up
/// but not implemented yet. Each file still gets its started and finished
/// events, so nobody's progress bar waits for Godot.
fn not_yet_supported(job: &Job<'_>, files: &[PathBuf], detail: &str) -> Vec<FileResult> {
    files
        .iter()
        .enumerate()
        .map(|(index, source)| {
            job.emit(JobEvent::FileStarted {
                index,
                source: source.clone(),
            });
            let result = FileResult {
                source: source.clone(),
                state: FileState::Failed {
                    key: ErrorKey::ConversionFailed,
                    detail: detail.to_owned(),
                },
                outputs: Vec::new(),
            };
            job.emit(JobEvent::FileFinished {
                index,
                result: result.clone(),
            });
            result
        })
        .collect()
}

/// Everything the workers of one job share.
struct Job<'a> {
    target: Target,
    options: &'a ConvertOptions,
    rule: &'a OutputRule,
    engines: &'a [&'a dyn Converter],
    cancel: &'a CancelToken,
    on_event: &'a (dyn Fn(JobEvent) + Sync),
    /// Routes over the installed engines that accept the job's options.
    planner: Planner,
    /// Routes over every engine, installed or not; explains `engine_missing`.
    dream_planner: Planner,
    gates: HashMap<&'static str, Gate>,
    reserved: Reservations,
    now: chrono::NaiveDateTime,
    /// `$TMPDIR/morphring/<random>/`, or why it could not be created.
    work_dir: Result<tempfile::TempDir, String>,
}

/// How a file's conversion ended, before it is turned into a `FileResult`.
pub(super) enum Outcome {
    Finished(Vec<PathBuf>),
    Cancelled,
    Failed(ErrorKey, String),
}

impl Job<'_> {
    fn emit(&self, event: JobEvent) {
        // Filee shrugs off a throwing JobUpdated handler, and so do we: a broken
        // progress bar is not a reason to abandon someone's 400 photos.
        if panic::catch_unwind(AssertUnwindSafe(|| (self.on_event)(event))).is_err() {
            tracing::warn!("a job event handler panicked");
        }
    }

    fn run_file(&self, index: usize, source: &Path) -> FileResult {
        self.emit(JobEvent::FileStarted {
            index,
            source: source.to_path_buf(),
        });
        let (state, outputs) = match self.convert_file(index, source) {
            Outcome::Finished(outputs) if outputs.is_empty() => (FileState::Skipped, outputs),
            Outcome::Finished(outputs) => (FileState::Done, outputs),
            Outcome::Cancelled => (FileState::Cancelled, Vec::new()),
            Outcome::Failed(key, detail) => (FileState::Failed { key, detail }, Vec::new()),
        };
        if state == FileState::Done && self.rule.keep_dates {
            crate::dates::copy_dates(source, &outputs);
        }
        FileResult {
            source: source.to_path_buf(),
            state,
            outputs,
        }
    }

    fn convert_file(&self, index: usize, source: &Path) -> Outcome {
        if self.cancel.is_cancelled() {
            return Outcome::Cancelled;
        }
        let (target, route) = match self.plan_file(source) {
            Ok(planned) => planned,
            Err(outcome) => return outcome,
        };
        let work_dir = match &self.work_dir {
            Ok(dir) => dir.path(),
            Err(why) => return Outcome::Failed(ErrorKey::ConversionFailed, why.clone()),
        };

        // Final outputs this file created from nothing. They are deleted again
        // if the file fails or is cancelled; anything that was already on disk
        // (the Overwrite policy) is left alone, because the engine writes to a
        // temp file first and the user's old file is still intact.
        let created = RefCell::new(Vec::new());
        let run = panic::catch_unwind(AssertUnwindSafe(|| {
            let mut final_out = FinalAllocator {
                inner: self.final_allocator(index, source, target),
                created: &created,
            };
            self.run_route(index, source, &route, work_dir, &mut final_out)
        }));
        let outcome = match run {
            Ok(Ok(outputs)) => return Outcome::Finished(outputs),
            // An engine whose child we just killed may report a plain failure;
            // it was still us who pulled the plug.
            Ok(Err(_)) if self.cancel.is_cancelled() => Outcome::Cancelled,
            Ok(Err(EngineError::Cancelled)) => Outcome::Cancelled,
            Ok(Err(err)) => {
                tracing::warn!("converting {} failed: {err}", source.display());
                Outcome::Failed(err.key(), err.to_string())
            }
            Err(payload) => {
                let message = panic_message(payload.as_ref());
                tracing::warn!("converting {} panicked: {message}", source.display());
                Outcome::Failed(ErrorKey::ConversionFailed, message)
            }
        };
        for path in created.borrow().iter() {
            remove_path(path);
        }
        outcome
    }

    /// Detects the format of `source` and plans its route to the job's target.
    /// Returns the per-file target with the route, or the outcome that ends the
    /// file right here (unknown format, no route).
    pub(super) fn plan_file(&self, source: &Path) -> Result<(FormatId, Route), Outcome> {
        let Some(format) = formats::detect(source) else {
            return Err(Outcome::Failed(
                ErrorKey::UnsupportedSource,
                format!(".{}", formats::extension_of(source)),
            ));
        };
        let target = self.target.for_source(format.id);
        match self.planner.plan(format.id, target) {
            Some(route) => Ok((target, route)),
            None => Err(self.explain_missing_route(format.id, target)),
        }
    }

    /// The allocator for the final outputs of the file at `index`, converted
    /// to `target`.
    pub(super) fn final_allocator(
        &self,
        index: usize,
        source: &Path,
        target: FormatId,
    ) -> JobAllocator {
        // {preset} is the target's display name for now; presets bring their own
        // names along with P2-10.
        let preset = formats::find_by_id(target.as_str())
            .map_or_else(|| target.as_str().to_owned(), |f| f.name.to_owned());
        JobAllocator::new(
            self.rule.clone(),
            Tokens {
                source: source.to_path_buf(),
                preset,
                index: index + 1,
                now: self.now,
            },
            Arc::clone(&self.reserved),
        )
    }

    /// No route with the installed engines: find out whether installing
    /// something would help (D10), or whether it is simply impossible.
    fn explain_missing_route(&self, from: FormatId, target: FormatId) -> Outcome {
        let mut missing: Vec<String> = Vec::new();
        if let Some(route) = self.dream_planner.plan(from, target) {
            for engine in route.engines().map(|i| self.engines[i]) {
                if let EngineStatus::Missing { hint } = engine.status() {
                    let line = format!("needs {}: {hint}", engine.display_name());
                    if !missing.contains(&line) {
                        missing.push(line);
                    }
                }
            }
        }
        if missing.is_empty() {
            Outcome::Failed(ErrorKey::NoRoute, format!("{from} → {target}"))
        } else {
            Outcome::Failed(ErrorKey::EngineMissing, missing.join("; "))
        }
    }

    /// Runs every hop of `route`. A hop that produces several files (pages,
    /// frames) runs the next hop once per file. The last hop writes through
    /// `final_out`, every earlier one into `work_dir`.
    pub(super) fn run_route(
        &self,
        index: usize,
        source: &Path,
        route: &Route,
        work_dir: &Path,
        final_out: &mut dyn OutputAllocator,
    ) -> Result<Vec<PathBuf>, EngineError> {
        let steps = route.hops.len();
        let best = Cell::new(0.0_f32);
        let mut temp_out = TempAllocator::new(work_dir, source);

        let mut inputs = vec![source.to_path_buf()];
        for (s, hop) in route.hops.iter().enumerate() {
            let engine = self.engines[hop.engine];
            let out: &mut dyn OutputAllocator = if s + 1 == steps {
                &mut *final_out
            } else {
                &mut temp_out
            };
            // Progress arrives as "this step is p done"; the file is
            // (finished steps + p) / steps done, and it never goes backwards, even
            // when an engine gets excited and reports 0.9 before 0.4.
            let progress = |p: f32| {
                if p.is_nan() {
                    return;
                }
                let value = (s as f32 + p.clamp(0.0, 1.0)) / steps as f32;
                if value > best.get() {
                    best.set(value);
                    self.emit(JobEvent::FileProgress {
                        index,
                        progress: value,
                    });
                }
            };

            let mut outputs = Vec::new();
            for input in &inputs {
                if self.cancel.is_cancelled() {
                    return Err(EngineError::Cancelled);
                }
                let step = Step {
                    input: input.clone(),
                    from: hop.from,
                    to: hop.to,
                    options: self.options.clone(),
                    work_dir: work_dir.to_path_buf(),
                };
                tracing::debug!(
                    "{}: {} -> {} with {}",
                    input.display(),
                    hop.from,
                    hop.to,
                    engine.id()
                );
                // The permit covers this single step, not the whole file, so a
                // file waiting on its ffmpeg hop doesn't sit on a magick slot.
                let _permit = match self.gates.get(engine.id()) {
                    Some(gate) => Some(gate.acquire(self.cancel).ok_or(EngineError::Cancelled)?),
                    None => None,
                };
                outputs.extend(engine.convert(&step, out, &progress, self.cancel)?);
            }
            inputs = outputs;
        }
        Ok(inputs)
    }
}

/// The job's `JobAllocator`, plus a note of every path that did not exist
/// before it was handed out, so a failed file can clean up after itself.
struct FinalAllocator<'a> {
    inner: JobAllocator,
    created: &'a RefCell<Vec<PathBuf>>,
}

impl OutputAllocator for FinalAllocator<'_> {
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
        let path = self.inner.allocate(ext, suffix)?;
        // JobAllocator only makes the parent folder, so if the path itself is
        // already there, it was there before us (Overwrite) and it isn't ours.
        if path.symlink_metadata().is_err() {
            self.created.borrow_mut().push(path.clone());
        }
        Some(path)
    }
}

/// Hands out intermediate paths: `<work dir>/<random>/<name><suffix>.<ext>`.
///
/// Every output gets its own random folder, so two files called `a.png` and
/// `a.jpg` can't trip over each other's `a.pdf` halfway through a route.
pub(super) struct TempAllocator<'a> {
    work_dir: &'a Path,
    base: OsString,
}

impl<'a> TempAllocator<'a> {
    /// Intermediates of `source`, named after it, below `work_dir`.
    pub(super) fn new(work_dir: &'a Path, source: &Path) -> Self {
        Self {
            work_dir,
            base: formats::name_without_extension(source),
        }
    }
}

impl OutputAllocator for TempAllocator<'_> {
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
        let dir = match tempfile::Builder::new()
            .prefix("")
            .rand_bytes(8)
            .tempdir_in(self.work_dir)
        {
            // The whole work dir goes at the end of the job, so this one may stay.
            Ok(dir) => dir.keep(),
            Err(err) => {
                // Hand out a path anyway; the engine fails to write there and
                // reports it properly, which beats a mysterious "skipped".
                tracing::warn!("could not create a temp folder: {err}");
                self.work_dir.to_path_buf()
            }
        };
        let mut name = self.base.clone();
        name.push(suffix);
        let ext = ext.trim_start_matches('.');
        if !ext.is_empty() {
            name.push(".");
            name.push(ext);
        }
        Some(dir.join(name))
    }
}

/// Stands in for an engine that declined the job's options: no edges, never
/// available, never asked to convert anything.
struct Benched;

static BENCHED: Benched = Benched;

impl Converter for Benched {
    fn id(&self) -> &'static str {
        "benched"
    }

    fn display_name(&self) -> &'static str {
        "benched"
    }

    fn edges(&self) -> &[Edge] {
        &[]
    }

    fn status(&self) -> EngineStatus {
        EngineStatus::Missing {
            hint: String::new(),
        }
    }

    fn convert(
        &self,
        _step: &Step,
        _out: &mut dyn OutputAllocator,
        _progress: &dyn Fn(f32),
        _cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError> {
        // The planner never routes through an engine without edges, so getting
        // here means someone rewired the planner without telling us.
        Err(EngineError::Failed(
            "this engine is not used for this job".into(),
        ))
    }
}

/// A counting semaphore: at most `limit` holders at a time.
struct Gate {
    limit: usize,
    in_use: Mutex<usize>,
    freed: Condvar,
}

/// One slot of a [`Gate`]; dropping it frees the slot, panics included.
struct Permit<'a>(&'a Gate);

impl Gate {
    fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            in_use: Mutex::new(0),
            freed: Condvar::new(),
        }
    }

    /// Waits for a free slot. Returns `None` if `cancel` fires while waiting.
    fn acquire(&self, cancel: &CancelToken) -> Option<Permit<'_>> {
        // The counter is a plain number changed in one statement, so a poisoned
        // lock still holds a sane value.
        let mut in_use = self.in_use.lock().unwrap_or_else(PoisonError::into_inner);
        while *in_use >= self.limit {
            if cancel.is_cancelled() {
                return None;
            }
            // Nobody rings a bell when cancel fires, so wake up now and then to
            // check. 20 ms is invisible to a human and free for a CPU.
            in_use = self
                .freed
                .wait_timeout(in_use, Duration::from_millis(20))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        *in_use += 1;
        Some(Permit(self))
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut in_use = self.0.in_use.lock().unwrap_or_else(PoisonError::into_inner);
        *in_use = in_use.saturating_sub(1);
        self.0.freed.notify_one();
    }
}

/// An engine's step limit: its own `max_parallelism`, or one per core for 0.
fn effective_limit(max_parallelism: usize, cores: usize) -> usize {
    if max_parallelism == 0 {
        cores
    } else {
        max_parallelism
    }
}

/// Creates `$TMPDIR/morphring/<random>/`, removed when the returned `TempDir`
/// is dropped.
fn create_work_dir() -> Result<tempfile::TempDir, String> {
    let root = std::env::temp_dir().join("morphring");
    fs::create_dir_all(&root)
        .and_then(|()| tempfile::Builder::new().prefix("job-").tempdir_in(&root))
        .map_err(|err| {
            format!(
                "could not create a work folder in {}: {err}",
                root.display()
            )
        })
}

/// Deletes a partial output, file or folder. Already gone is fine.
pub(super) fn remove_path(path: &Path) {
    let result = match path.symlink_metadata() {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(err) => Err(err),
    };
    match result {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => tracing::warn!("could not delete partial output {}: {err}", path.display()),
    }
}

/// The text of a panic payload, for the file's error detail.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "the converter stopped unexpectedly".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::contract::{Edge, JobState};
    use crate::output::ConflictPolicy;
    use crate::testing::{Behaviour, FakeConverter};
    use tempfile::TempDir;

    type ConvertFn = dyn Fn(
            &Step,
            &mut dyn OutputAllocator,
            &dyn Fn(f32),
            &CancelToken,
        ) -> Result<Vec<PathBuf>, EngineError>
        + Send
        + Sync;

    /// A converter whose `convert` is a closure, for the odd cases the
    /// FakeConverter doesn't cover (recording steps, writing partial files).
    struct FnConverter {
        id: &'static str,
        edges: Vec<Edge>,
        convert: Box<ConvertFn>,
    }

    impl FnConverter {
        fn new(id: &'static str, edges: Vec<Edge>, convert: Box<ConvertFn>) -> Self {
            Self { id, edges, convert }
        }
    }

    impl Converter for FnConverter {
        fn id(&self) -> &'static str {
            self.id
        }
        fn display_name(&self) -> &'static str {
            self.id
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
        fn convert(
            &self,
            step: &Step,
            out: &mut dyn OutputAllocator,
            progress: &dyn Fn(f32),
            cancel: &CancelToken,
        ) -> Result<Vec<PathBuf>, EngineError> {
            (self.convert)(step, out, progress, cancel)
        }
    }

    fn touch(dir: &TempDir, name: &str) -> PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, "x").unwrap();
        path
    }

    fn run(files: &[PathBuf], target: &'static str, engines: &[&dyn Converter]) -> Vec<FileResult> {
        run_with(
            files,
            target,
            &OutputRule::default(),
            engines,
            &CancelToken::new(),
        )
    }

    fn run_with(
        files: &[PathBuf],
        target: &'static str,
        rule: &OutputRule,
        engines: &[&dyn Converter],
        cancel: &CancelToken,
    ) -> Vec<FileResult> {
        run_job(
            files,
            Target::Format(FormatId::new(target)),
            &ConvertOptions::default(),
            rule,
            engines,
            cancel,
            &|_| {},
        )
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn failed(result: &FileResult) -> (ErrorKey, &str) {
        match &result.state {
            FileState::Failed { key, detail } => (*key, detail.as_str()),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn converts_every_file() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "a.jpg"), touch(&dir, "b.jpg")];
        let magick = FakeConverter::new("magick").edge("jpg", "png", 10);

        let results = run(&files, "png", &[&magick]);

        assert_eq!(JobState::from_files(&results, false), JobState::Completed);
        assert_eq!(results[0].outputs, vec![dir.path().join("a.png")]);
        assert_eq!(results[1].outputs, vec![dir.path().join("b.png")]);
        assert!(dir.path().join("a.png").is_file());
        assert!(dir.path().join("b.png").is_file());
    }

    #[test]
    fn results_come_back_in_input_order() {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<PathBuf> = (0..12)
            .map(|i| touch(&dir, &format!("f{i:02}.jpg")))
            .collect();
        let magick = FakeConverter::new("magick").edge("jpg", "png", 10);

        let results = run(&files, "png", &[&magick]);

        let sources: Vec<_> = results.iter().map(|r| r.source.clone()).collect();
        assert_eq!(sources, files);
    }

    #[test]
    fn engine_limit_caps_concurrent_steps() {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<PathBuf> = (0..8).map(|i| touch(&dir, &format!("{i}.png"))).collect();
        let fake = FakeConverter::new("fake")
            .edge("png", "jpg", 10)
            .max_parallelism(2)
            .behaviour(Behaviour::Sleep(Duration::from_millis(50)));
        let counters = fake.counters();

        let results = run(&files, "jpg", &[&fake]);

        assert!(results.iter().all(|r| r.state == FileState::Done));
        assert_eq!(counters.calls(), 8);
        assert!(
            counters.max_concurrent() <= 2,
            "{}",
            counters.max_concurrent()
        );
        assert!(counters.max_concurrent() >= 1);
    }

    #[test]
    fn engine_limit_of_zero_means_one_per_core() {
        assert_eq!(effective_limit(0, 8), 8);
        assert_eq!(effective_limit(2, 8), 2);
        assert_eq!(Gate::new(0).limit, 1);
    }

    #[test]
    fn gate_never_hands_out_more_than_its_limit() {
        let gate = Gate::new(3);
        let running = AtomicUsize::new(0);
        let high = AtomicUsize::new(0);
        let cancel = CancelToken::new();
        thread::scope(|s| {
            for _ in 0..10 {
                s.spawn(|| {
                    let _permit = gate.acquire(&cancel).unwrap();
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    high.fetch_max(now, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(10));
                    running.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        assert!(high.load(Ordering::SeqCst) <= 3);
        assert_eq!(*gate.in_use.lock().unwrap(), 0);
    }

    #[test]
    fn gate_wait_gives_up_on_cancel() {
        let gate = Gate::new(1);
        let cancel = CancelToken::new();
        let _held = gate.acquire(&cancel).unwrap();
        cancel.cancel();
        assert!(gate.acquire(&cancel).is_none());
    }

    #[test]
    fn one_failure_among_five_is_isolated() {
        let dir = tempfile::tempdir().unwrap();
        let mut files: Vec<PathBuf> = (0..4).map(|i| touch(&dir, &format!("{i}.png"))).collect();
        files.insert(2, touch(&dir, "bad.gif"));
        let good = FakeConverter::new("good").edge("png", "jpg", 10);
        let bad = FakeConverter::new("bad")
            .edge("gif", "jpg", 10)
            .behaviour(Behaviour::Fail("boom".into()));

        let results = run(&files, "jpg", &[&good, &bad]);

        let done = results
            .iter()
            .filter(|r| r.state == FileState::Done)
            .count();
        assert_eq!(done, 4);
        assert_eq!(failed(&results[2]), (ErrorKey::ConversionFailed, "boom"));
        assert!(results[2].outputs.is_empty());
        assert_eq!(
            JobState::from_files(&results, false),
            JobState::CompletedWithErrors
        );
    }

    #[test]
    fn unknown_extension_is_unsupported_source() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "a.jpg"), touch(&dir, "b.xyz")];
        let magick = FakeConverter::new("magick").edge("jpg", "png", 10);

        let results = run(&files, "png", &[&magick]);

        assert_eq!(results[0].state, FileState::Done);
        assert_eq!(failed(&results[1]), (ErrorKey::UnsupportedSource, ".xyz"));
        assert_eq!(
            JobState::from_files(&results, false),
            JobState::CompletedWithErrors
        );
    }

    #[test]
    fn no_route_at_all_is_no_route() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "notes.txt")];
        let magick = FakeConverter::new("magick").edge("png", "jpg", 10);

        let results = run(&files, "mp3", &[&magick]);

        assert_eq!(failed(&results[0]), (ErrorKey::NoRoute, "txt → mp3"));
        assert_eq!(JobState::from_files(&results, false), JobState::Failed);

        let nobody = run(&files, "mp3", &[]);
        assert_eq!(failed(&nobody[0]).0, ErrorKey::NoRoute);
    }

    #[test]
    fn route_through_a_missing_engine_names_it() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "report.docx")];
        let magick = FakeConverter::new("magick").edge("pdf", "png", 10);
        let soffice = FakeConverter::new("soffice")
            .display_name("LibreOffice")
            .edge("docx", "pdf", 10)
            .missing("pacman -S libreoffice-fresh");
        let counters = soffice.counters();

        let results = run(&files, "png", &[&magick, &soffice]);

        assert_eq!(
            failed(&results[0]),
            (
                ErrorKey::EngineMissing,
                "needs LibreOffice: pacman -S libreoffice-fresh"
            )
        );
        assert_eq!(counters.calls(), 0);
    }

    #[test]
    fn two_hop_route_keeps_intermediates_in_the_job_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "doc.docx")];
        let to_pdf = FakeConverter::new("soffice").edge("docx", "pdf", 10);
        // Remembers what the second hop was fed and whether it was really there.
        let seen: Arc<Mutex<Vec<(PathBuf, PathBuf, bool)>>> = Arc::default();
        let paged = FakeConverter::new("pages")
            .edge("pdf", "png", 10)
            .behaviour(Behaviour::Outputs(2));
        let log = Arc::clone(&seen);
        let to_png = FnConverter::new(
            "pages",
            vec![Edge::new("pdf", "png")],
            Box::new(move |step, out, progress, cancel| {
                log.lock().unwrap().push((
                    step.input.clone(),
                    step.work_dir.clone(),
                    step.input.is_file(),
                ));
                paged.convert(step, out, progress, cancel)
            }),
        );

        let results = run(&files, "png", &[&to_pdf, &to_png]);

        assert_eq!(results[0].state, FileState::Done);
        assert_eq!(
            names_in(dir.path()),
            ["doc.docx", "doc_p1.png", "doc_p2.png"]
        );
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let (input, work_dir, existed) = &seen[0];
        assert!(existed);
        assert!(input.starts_with(work_dir));
        assert!(work_dir.starts_with(std::env::temp_dir().join("morphring")));
        assert_eq!(input.file_name().unwrap(), "doc.pdf");
        assert!(
            !work_dir.exists(),
            "work dir {} was left behind",
            work_dir.display()
        );
    }

    #[test]
    fn fan_out_runs_the_next_hop_per_output() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "clip.pdf")];
        let pages = FakeConverter::new("pages")
            .edge("pdf", "png", 10)
            .behaviour(Behaviour::Outputs(3));
        let magick = FakeConverter::new("magick").edge("png", "webp", 10);
        let counters = magick.counters();

        let results = run(&files, "webp", &[&pages, &magick]);

        assert_eq!(counters.calls(), 3);
        assert_eq!(results[0].outputs.len(), 3);
        // Each page asks for the plain name, so the shared reservations number
        // them, exactly like Filee does.
        assert_eq!(
            names_in(dir.path()),
            ["clip (2).webp", "clip (3).webp", "clip.pdf", "clip.webp"]
        );
    }

    #[test]
    fn cancel_during_a_slow_step_leaves_no_output() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "a.png")];
        let slow = FakeConverter::new("slow")
            .edge("png", "jpg", 10)
            .behaviour(Behaviour::Sleep(Duration::from_secs(30)));
        let cancel = CancelToken::new();
        let start = Instant::now();

        let results = thread::scope(|s| {
            s.spawn(|| {
                thread::sleep(Duration::from_millis(150));
                cancel.cancel();
            });
            run_with(&files, "jpg", &OutputRule::default(), &[&slow], &cancel)
        });

        assert_eq!(results[0].state, FileState::Cancelled);
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(!dir.path().join("a.jpg").exists());
        assert_eq!(JobState::from_files(&results, true), JobState::Cancelled);
    }

    #[test]
    fn cancel_deletes_a_partially_written_output() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "a.png")];
        let partial = FnConverter::new(
            "partial",
            vec![Edge::new("png", "jpg")],
            Box::new(|_step, out, _progress, cancel| {
                let path = out.allocate("jpg", "").unwrap();
                fs::write(&path, "half a jp").unwrap();
                while !cancel.is_cancelled() {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(EngineError::Failed("killed".into()))
            }),
        );
        let cancel = CancelToken::new();

        let results = thread::scope(|s| {
            s.spawn(|| {
                while !dir.path().join("a.jpg").exists() {
                    thread::sleep(Duration::from_millis(5));
                }
                cancel.cancel();
            });
            run_with(&files, "jpg", &OutputRule::default(), &[&partial], &cancel)
        });

        assert_eq!(results[0].state, FileState::Cancelled);
        assert_eq!(names_in(dir.path()), ["a.png"]);
    }

    #[test]
    fn failure_keeps_a_file_that_was_there_before() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "a.png")];
        let existing = dir.path().join("a.jpg");
        fs::write(&existing, "precious").unwrap();
        let failing = FnConverter::new(
            "failing",
            vec![Edge::new("png", "jpg")],
            Box::new(|_step, out, _progress, _cancel| {
                let path = out.allocate("jpg", "").unwrap();
                Err(EngineError::Failed(format!(
                    "could not write {}",
                    path.display()
                )))
            }),
        );
        let rule = OutputRule {
            conflict: ConflictPolicy::Overwrite,
            ..OutputRule::default()
        };

        let results = run_with(&files, "jpg", &rule, &[&failing], &CancelToken::new());

        assert_eq!(failed(&results[0]).0, ErrorKey::ConversionFailed);
        assert_eq!(fs::read_to_string(&existing).unwrap(), "precious");
    }

    #[test]
    fn cancel_before_start_runs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "a.png"), touch(&dir, "b.png")];
        let fake = FakeConverter::new("fake").edge("png", "jpg", 10);
        let counters = fake.counters();
        let cancel = CancelToken::new();
        cancel.cancel();

        let results = run_with(&files, "jpg", &OutputRule::default(), &[&fake], &cancel);

        assert!(results.iter().all(|r| r.state == FileState::Cancelled));
        assert_eq!(counters.calls(), 0);
    }

    #[test]
    fn progress_events_per_file_only_rise() {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<PathBuf> = (0..4).map(|i| touch(&dir, &format!("{i}.docx"))).collect();
        // Reports out of order on purpose, like a chatty engine on a bad day.
        let jumpy = |id| {
            FnConverter::new(
                id,
                if id == "first" {
                    vec![Edge::new("docx", "pdf")]
                } else {
                    vec![Edge::new("pdf", "png")]
                },
                Box::new(|step, out, progress, _cancel| {
                    for p in [0.5, 0.2, 0.9, f32::NAN, 0.6, 1.0, 7.0] {
                        progress(p);
                    }
                    let path = out.allocate(step.to.as_str(), "").unwrap();
                    fs::write(&path, "x")?;
                    Ok(vec![path])
                }),
            )
        };
        let (first, second) = (jumpy("first"), jumpy("second"));
        let events = Mutex::new(Vec::new());

        let results = run_job(
            &files,
            Target::Format(FormatId::new("png")),
            &ConvertOptions::default(),
            &OutputRule::default(),
            &[&first, &second],
            &CancelToken::new(),
            &|e| events.lock().unwrap().push(e),
        );

        assert!(results.iter().all(|r| r.state == FileState::Done));
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
            let values: Vec<f32> = mine
                .iter()
                .filter_map(|e| match e {
                    JobEvent::FileProgress { progress, .. } => Some(*progress),
                    _ => None,
                })
                .collect();
            let expected = [0.25, 0.45, 0.5, 0.75, 0.95, 1.0];
            assert_eq!(values.len(), expected.len(), "file {index}: {values:?}");
            for (got, want) in values.iter().zip(expected) {
                assert!((got - want).abs() < 1e-6, "file {index}: {values:?}");
            }
        }
    }

    #[test]
    fn a_panicking_engine_fails_only_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "a.png"), touch(&dir, "b.gif")];
        let fine = FakeConverter::new("fine").edge("png", "jpg", 10);
        let grumpy = FakeConverter::new("grumpy")
            .edge("gif", "jpg", 10)
            .max_parallelism(1)
            .behaviour(Behaviour::Panic);

        let results = run(&files, "jpg", &[&fine, &grumpy]);

        assert_eq!(results[0].state, FileState::Done);
        let (key, detail) = failed(&results[1]);
        assert_eq!(key, ErrorKey::ConversionFailed);
        assert!(detail.contains("panicked on purpose"), "{detail}");
        assert_eq!(
            JobState::from_files(&results, false),
            JobState::CompletedWithErrors
        );

        // The panic must also have handed back its engine slot.
        let again = run(&[touch(&dir, "c.gif")], "jpg", &[&grumpy]);
        assert_eq!(failed(&again[0]).0, ErrorKey::ConversionFailed);
    }

    #[test]
    fn a_panicking_event_handler_does_not_sink_the_job() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "a.png")];
        let fake = FakeConverter::new("fake").edge("png", "jpg", 10);

        let results = run_job(
            &files,
            Target::Format(FormatId::new("jpg")),
            &ConvertOptions::default(),
            &OutputRule::default(),
            &[&fake],
            &CancelToken::new(),
            &|_| panic!("the progress bar fell over"),
        );

        assert_eq!(results[0].state, FileState::Done);
    }

    #[test]
    fn same_format_target_uses_the_self_edge() {
        let dir = tempfile::tempdir().unwrap();
        let source = touch(&dir, "a.jpg");
        let magick = FakeConverter::new("magick").edge("jpg", "jpg", 10);
        let rule = OutputRule {
            pattern: "{name}_50".into(),
            ..OutputRule::default()
        };

        let results = run_with(
            std::slice::from_ref(&source),
            "jpg",
            &rule,
            &[&magick],
            &CancelToken::new(),
        );

        assert_eq!(results[0].state, FileState::Done);
        assert!(dir.path().join("a_50.jpg").is_file());
        assert_eq!(fs::read_to_string(&source).unwrap(), "x");
    }

    #[test]
    fn same_as_source_keeps_each_file_in_its_own_format() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "a.png"), touch(&dir, "b.jpg")];
        let magick = FakeConverter::new("magick")
            .edge("png", "png", 10)
            .edge("jpg", "jpg", 10)
            // Bait: a cheap cross-format edge SameAsSource must not take.
            .edge("png", "jpg", 1);
        let rule = OutputRule {
            pattern: "{name}_50".into(),
            ..OutputRule::default()
        };

        let results = run_job(
            &files,
            Target::SameAsSource,
            &ConvertOptions::default(),
            &rule,
            &[&magick],
            &CancelToken::new(),
            &|_| {},
        );

        assert!(results.iter().all(|r| r.state == FileState::Done));
        assert_eq!(results[0].outputs, [dir.path().join("a_50.png")]);
        assert_eq!(results[1].outputs, [dir.path().join("b_50.jpg")]);
        assert_eq!(
            fs::read_to_string(&results[0].outputs[0]).unwrap(),
            "fake png -> png\n"
        );
    }

    #[test]
    fn same_as_source_without_a_self_edge_is_no_route() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "a.gif")];
        let magick = FakeConverter::new("magick").edge("gif", "png", 10);

        let results = run_job(
            &files,
            Target::SameAsSource,
            &ConvertOptions::default(),
            &OutputRule::default(),
            &[&magick],
            &CancelToken::new(),
            &|_| {},
        );

        assert_eq!(failed(&results[0]), (ErrorKey::NoRoute, "gif → gif"));
    }

    #[test]
    fn an_engine_that_declines_the_options_is_never_used() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "a.png")];
        // The picky engine is first in line and cheaper, so it would win if it
        // were allowed to play.
        let picky = FakeConverter::new("picky")
            .edge("png", "jpg", 1)
            .declining();
        let steady = FakeConverter::new("steady").edge("png", "jpg", 10);
        let (picky_calls, steady_calls) = (picky.counters(), steady.counters());

        let results = run(&files, "jpg", &[&picky, &steady]);

        assert_eq!(results[0].state, FileState::Done);
        assert_eq!(picky_calls.calls(), 0);
        assert_eq!(steady_calls.calls(), 1);
    }

    #[test]
    fn a_route_only_a_declining_engine_has_still_reports_as_before() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "a.png")];
        let picky = FakeConverter::new("picky")
            .edge("png", "jpg", 10)
            .declining();
        let counters = picky.counters();

        let results = run(&files, "jpg", &[&picky]);
        assert_eq!(failed(&results[0]), (ErrorKey::NoRoute, "png → jpg"));
        assert_eq!(counters.calls(), 0);

        // A route through a declining engine and a missing one still names the
        // missing one, since installing it is part of the answer.
        let docs = [touch(&dir, "report.docx")];
        let to_png = FakeConverter::new("magick")
            .edge("pdf", "png", 10)
            .declining();
        let soffice = FakeConverter::new("soffice")
            .display_name("LibreOffice")
            .edge("docx", "pdf", 10)
            .missing("pacman -S libreoffice-fresh");
        let results = run(&docs, "png", &[&to_png, &soffice]);
        assert_eq!(
            failed(&results[0]),
            (
                ErrorKey::EngineMissing,
                "needs LibreOffice: pacman -S libreoffice-fresh"
            )
        );
    }

    #[test]
    fn target_maps_per_source() {
        let png = FormatId::new("png");
        let jpg = FormatId::new("jpg");
        assert_eq!(Target::Format(jpg).for_source(png), jpg);
        assert_eq!(Target::SameAsSource.for_source(png), png);
        assert_eq!(Target::from(jpg), Target::Format(jpg));
    }

    #[test]
    fn parallel_files_with_the_same_name_do_not_collide() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "same.jpg"), touch(&dir, "same.png")];
        let magick = FakeConverter::new("magick")
            .edge("jpg", "webp", 10)
            .edge("png", "webp", 10);

        let results = run(&files, "webp", &[&magick]);

        let mut outputs: Vec<_> = results.iter().flat_map(|r| r.outputs.clone()).collect();
        outputs.sort();
        outputs.dedup();
        assert_eq!(outputs.len(), 2);
    }

    #[test]
    fn existing_output_under_skip_policy_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let files = [touch(&dir, "a.png")];
        touch(&dir, "a.jpg");
        let fake = FakeConverter::new("fake").edge("png", "jpg", 10);
        let rule = OutputRule {
            conflict: ConflictPolicy::Skip,
            ..OutputRule::default()
        };

        let results = run_with(&files, "jpg", &rule, &[&fake], &CancelToken::new());

        assert_eq!(results[0].state, FileState::Skipped);
        assert!(results[0].outputs.is_empty());
        assert_eq!(fs::read_to_string(dir.path().join("a.jpg")).unwrap(), "x");
    }

    #[test]
    fn empty_job_returns_nothing() {
        assert!(run(&[], "png", &[]).is_empty());
    }

    #[test]
    fn panic_messages_are_extracted() {
        assert_eq!(panic_message(&"static"), "static");
        assert_eq!(panic_message(&String::from("owned")), "owned");
        assert_eq!(panic_message(&42), "the converter stopped unexpectedly");
    }
}
