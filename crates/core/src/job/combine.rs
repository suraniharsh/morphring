//! Combine-into-one-archive mode: many files in, one archive out ("Compress
//! into one archive", Filee's zip-all). Ported from Filee's
//! `src/Filee.Core/Conversion/JobQueue.cs`, `RunCombinedAsync` (lines 196-271),
//! plus the entry naming of `ArchiveConverter.UniqueEntries`.
//!
//! The recipe is refreshingly short: no planner, no routes, no intermediate
//! formats. Every input (file or folder, known type or a mystery `.xyz`) goes
//! into the archive exactly as it is, under its own name, and the archive is
//! named after the first input: `{first name}_files.<ext>`.
//!
//! The bits that need a little care:
//! - two inputs called `notes.txt` from different folders would collide inside
//!   the archive, so later ones are staged in the work dir as `notes (2).txt`
//!   (case-insensitively, like Filee, because someone will unpack it on
//!   Windows). The combiner then just packs every path under its file name.
//! - a cancel removes the half-written archive, like any other final output.
//! - keep dates is not applied: whose dates would a zip of 40 files wear?
//!   Filee skips combined outputs too.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use super::{FinalAllocator, Job, JobEvent, panic_message, remove_path};
use crate::contract::{
    Combiner, Converter, EngineStatus, FileResult, FileState, FormatId, OutputAllocator,
};
use crate::error::{EngineError, ErrorKey};
use crate::formats::{self, Category};
use crate::job::Target;

/// Appended to the first input's name to name the archive: `a_files.zip`.
const NAME_SUFFIX: &str = "_files";

/// Runs the whole job as one archive when its options ask for it and the
/// target is an archive format. `None` means "not a combine job"; the caller
/// carries on converting file by file.
pub(super) fn try_run(job: &Job<'_>, files: &[PathBuf]) -> Option<Vec<FileResult>> {
    if !job.options.archive.combine_into_one {
        return None;
    }
    let format = archive_target(job.target)?;
    Some(match find_combiner(job, format) {
        Some((engine, combiner)) => Combine {
            job,
            files,
            format,
            engine,
            combiner,
        }
        .run(),
        // Converting the files one by one instead would quietly hand back 40
        // single-file zips to someone who asked for one. Better to say no.
        None => fail_all(job, files, no_combiner(job, format)),
    })
}

/// The job's target when it is a real archive format (a folder is filed under
/// archives too, but packing files "into a folder" is just copying them).
fn archive_target(target: Target) -> Option<FormatId> {
    let Target::Format(format) = target else {
        return None;
    };
    let found = formats::find_by_id(format.as_str())?;
    (found.category == Category::Archive && !found.is_folder()).then_some(format)
}

/// The first installed engine, willing to take this job's options, whose
/// combiner can write `format`.
fn find_combiner<'a>(
    job: &Job<'a>,
    format: FormatId,
) -> Option<(&'a dyn Converter, &'a dyn Combiner)> {
    job.engines.iter().find_map(|&engine| {
        if !engine.status().is_available() || !engine.accepts(job.options) {
            return None;
        }
        engine
            .combiner()
            .filter(|combiner| combiner.can_combine_into(format))
            .map(|combiner| (engine, combiner))
    })
}

/// Why nobody can pack this job: an engine that could is not installed, or
/// there simply is no such engine.
fn no_combiner(job: &Job<'_>, format: FormatId) -> FileState {
    let missing: Vec<String> = job
        .engines
        .iter()
        .filter(|engine| engine.combiner().is_some())
        .filter_map(|engine| match engine.status() {
            EngineStatus::Missing { hint } => {
                Some(format!("needs {}: {hint}", engine.display_name()))
            }
            EngineStatus::Available { .. } => None,
        })
        .collect();
    if missing.is_empty() {
        FileState::Failed {
            key: ErrorKey::NoRoute,
            detail: format!("no installed engine can pack files into one {format} archive"),
        }
    } else {
        FileState::Failed {
            key: ErrorKey::EngineMissing,
            detail: missing.join("; "),
        }
    }
}

/// Ends every file of the job in `state`, with the usual started and finished
/// events so progress bars are not left hanging.
fn fail_all(job: &Job<'_>, files: &[PathBuf], state: FileState) -> Vec<FileResult> {
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
                state: state.clone(),
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

/// The name each input gets inside the archive, in input order: its own file
/// (or folder) name, and `stem (2).ext`, `stem (3).ext`... for later inputs
/// whose name is already taken, compared case-insensitively.
fn unique_entry_names(inputs: &[PathBuf]) -> Vec<OsString> {
    let mut used: HashSet<String> = HashSet::new();
    inputs
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .map_or_else(|| OsString::from("file"), OsString::from);
            let mut candidate = name.clone();
            let mut n = 2;
            while !used.insert(candidate.to_string_lossy().to_lowercase()) {
                candidate = numbered(&name, n);
                n += 1;
            }
            candidate
        })
        .collect()
}

/// `notes.txt` with `n = 2` is `notes (2).txt`. Like Filee (and
/// `Path.GetExtension`), only the last extension moves: `a.tar.gz` becomes
/// `a.tar (2).gz`, which is odd but at least predictable.
fn numbered(name: &OsString, n: usize) -> OsString {
    let path = Path::new(name);
    let stem = path.file_stem().unwrap_or(name.as_os_str());
    let mut out = stem.to_os_string();
    out.push(format!(" ({n})"));
    if let Some(ext) = path.extension() {
        out.push(".");
        out.push(ext);
    }
    out
}

/// Asks for the archive's path with `_files` tacked onto whatever suffix the
/// combiner wanted, so the archive is `{first name}_files.<ext>`.
struct FilesSuffix<'a>(&'a mut dyn OutputAllocator);

impl OutputAllocator for FilesSuffix<'_> {
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
        self.0.allocate(ext, &format!("{NAME_SUFFIX}{suffix}"))
    }
}

/// One combine job in flight.
struct Combine<'j, 'a> {
    job: &'j Job<'a>,
    files: &'j [PathBuf],
    format: FormatId,
    engine: &'a dyn Converter,
    combiner: &'a dyn Combiner,
}

impl Combine<'_, '_> {
    fn run(&self) -> Vec<FileResult> {
        let job = self.job;
        let mut results: Vec<FileResult> = self
            .files
            .iter()
            .map(|source| FileResult {
                source: source.clone(),
                // Placeholder; every file gets its real state below.
                state: FileState::Done,
                outputs: Vec::new(),
            })
            .collect();

        // Folders count too (they come from the command line; drops deliver
        // files). Whatever vanished since it was dropped is out.
        let mut alive = Vec::new();
        for (index, source) in self.files.iter().enumerate() {
            job.emit(JobEvent::FileStarted {
                index,
                source: source.clone(),
            });
            if source.symlink_metadata().is_ok() {
                alive.push(index);
            } else {
                results[index].state = FileState::Failed {
                    key: ErrorKey::ConversionFailed,
                    detail: "the file no longer exists".into(),
                };
            }
        }

        if let Some(&owner) = alive.first() {
            let (state, outputs) = if job.cancel.is_cancelled() {
                (FileState::Cancelled, Vec::new())
            } else {
                self.combine(&alive)
            };
            for &i in &alive {
                results[i].state = state.clone();
            }
            // The archive belongs to the first file that made it in, which is
            // also the one it is named after.
            results[owner].outputs = outputs;
        }

        for (index, result) in results.iter().enumerate() {
            job.emit(JobEvent::FileFinished {
                index,
                result: result.clone(),
            });
        }
        results
    }

    /// Packs the files at `alive` (indices into `files`) and returns the
    /// state they all end in, plus the archive written.
    fn combine(&self, alive: &[usize]) -> (FileState, Vec<PathBuf>) {
        let job = self.job;
        let sources: Vec<PathBuf> = alive.iter().map(|&i| self.files[i].clone()).collect();
        let owner = &sources[0];

        let best = Cell::new(0.0_f32);
        let progress = |p: f32| {
            if p.is_nan() {
                return;
            }
            // Engines may report from several threads and out of order: the
            // bar only ever moves forward.
            let value = p.clamp(0.0, 1.0);
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

        let created = RefCell::new(Vec::new());
        let run = panic::catch_unwind(AssertUnwindSafe(|| {
            let inputs = self.stage_duplicates(&sources)?;
            let mut final_out = FinalAllocator {
                inner: job.final_allocator(0, owner, self.format),
                created: &created,
            };
            let mut out = FilesSuffix(&mut final_out);
            let _permit = match job.gates.get(self.engine.id()) {
                Some(gate) => Some(gate.acquire(job.cancel).ok_or(EngineError::Cancelled)?),
                None => None,
            };
            self.combiner.combine(
                &inputs,
                self.format,
                job.options,
                &mut out,
                &progress,
                job.cancel,
            )
        }));
        let state = match run {
            // The allocator said "skip" (the archive exists, Skip policy).
            Ok(Ok(outputs)) if outputs.is_empty() => return (FileState::Skipped, outputs),
            Ok(Ok(outputs)) => return (FileState::Done, outputs),
            Ok(Err(_)) if job.cancel.is_cancelled() => FileState::Cancelled,
            Ok(Err(EngineError::Cancelled)) => FileState::Cancelled,
            Ok(Err(err)) => {
                tracing::warn!("packing {} file(s) failed: {err}", sources.len());
                FileState::Failed {
                    key: err.key(),
                    detail: err.to_string(),
                }
            }
            Err(payload) => {
                let message = panic_message(payload.as_ref());
                tracing::warn!("packing {} file(s) panicked: {message}", sources.len());
                FileState::Failed {
                    key: ErrorKey::ConversionFailed,
                    detail: message,
                }
            }
        };
        for path in created.borrow().iter() {
            remove_path(path);
        }
        (state, Vec::new())
    }

    /// The paths to hand the combiner: each source as it is, except those
    /// whose name is already taken, which are copied into the work dir under
    /// their `name (2).ext`. Copies, not links: a hard link cannot leave the
    /// filesystem and a symlink inside an archive is a surprise nobody wants.
    fn stage_duplicates(&self, sources: &[PathBuf]) -> Result<Vec<PathBuf>, EngineError> {
        let names = unique_entry_names(sources);
        let mut inputs = Vec::with_capacity(sources.len());
        for (source, name) in sources.iter().zip(names) {
            if source.file_name() == Some(name.as_os_str()) {
                inputs.push(source.clone());
                continue;
            }
            if self.job.cancel.is_cancelled() {
                return Err(EngineError::Cancelled);
            }
            let work_dir = match &self.job.work_dir {
                Ok(dir) => dir.path(),
                Err(why) => return Err(EngineError::Failed(why.clone())),
            };
            // Its own folder per copy, so `a (2).txt` can never meet another
            // `a (2).txt` that was there first. The work dir is binned when
            // the job ends, copies and all.
            let folder = tempfile::Builder::new()
                .prefix("entry-")
                .tempdir_in(work_dir)?
                .keep();
            let staged = folder.join(&name);
            copy_recursive(source, &staged)?;
            inputs.push(staged);
        }
        Ok(inputs)
    }
}

/// Copies a file, or a folder with everything in it, to `to`.
fn copy_recursive(from: &Path, to: &Path) -> io::Result<()> {
    if fs::metadata(from)?.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::contract::{CancelToken, ConvertOptions, Edge, JobState, Step};
    use crate::job::run_job;
    use crate::output::{ConflictPolicy, OutputRule};
    use tempfile::TempDir;

    /// What the fake combiner does when asked to pack.
    #[derive(Clone)]
    enum Pack {
        /// Write a manifest: one `entry name = content` line per input.
        Manifest,
        /// Write half an archive, then pull the plug on the job.
        CancelHalfway(CancelToken),
        /// Fail with this message.
        Fail(&'static str),
    }

    /// A pretend zip tool. It only "packs" a manifest of entry names, which is
    /// all a test needs and roughly what a zip is, if you squint at it from
    /// across the room.
    struct Zipper {
        status: EngineStatus,
        pack: Option<Pack>,
        calls: Mutex<Vec<Vec<PathBuf>>>,
    }

    impl Zipper {
        fn new(pack: Option<Pack>) -> Self {
            Self {
                status: EngineStatus::Available {
                    path: PathBuf::from("/usr/bin/zipper"),
                    version: "1".into(),
                },
                pack,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Vec<PathBuf>> {
            self.calls.lock().unwrap().clone()
        }
    }

    fn describe(path: &Path) -> String {
        if path.is_dir() {
            let mut names: Vec<String> = fs::read_dir(path)
                .unwrap()
                .map(|e| {
                    let e = e.unwrap();
                    format!(
                        "{}:{}",
                        e.file_name().to_string_lossy(),
                        describe(&e.path())
                    )
                })
                .collect();
            names.sort();
            format!("dir[{}]", names.join(","))
        } else {
            fs::read_to_string(path).unwrap()
        }
    }

    impl Combiner for Zipper {
        fn can_combine_into(&self, format: FormatId) -> bool {
            format.as_str() == "zip"
        }

        fn combine(
            &self,
            inputs: &[PathBuf],
            format: FormatId,
            _options: &ConvertOptions,
            out: &mut dyn OutputAllocator,
            progress: &dyn Fn(f32),
            cancel: &CancelToken,
        ) -> Result<Vec<PathBuf>, EngineError> {
            self.calls.lock().unwrap().push(inputs.to_vec());
            let pack = self.pack.clone().expect("this zipper does not combine");
            if let Pack::Fail(why) = pack {
                return Err(EngineError::Failed(why.into()));
            }
            let Some(path) = out.allocate(format.as_str(), "") else {
                return Ok(Vec::new());
            };
            let manifest: Vec<String> = inputs
                .iter()
                .map(|p| {
                    format!(
                        "{} = {}",
                        p.file_name().unwrap().to_string_lossy(),
                        describe(p)
                    )
                })
                .collect();
            fs::write(&path, manifest.join("\n"))?;
            progress(0.5);
            progress(0.25); // late and out of order; must be ignored
            if let Pack::CancelHalfway(token) = pack {
                token.cancel();
            }
            if cancel.is_cancelled() {
                return Err(EngineError::Cancelled);
            }
            progress(1.0);
            Ok(vec![path])
        }
    }

    impl Converter for Zipper {
        fn id(&self) -> &'static str {
            "zipper"
        }
        fn display_name(&self) -> &'static str {
            "Zipper"
        }
        fn edges(&self) -> &[Edge] {
            const EDGES: &[Edge] = &[Edge::new("txt", "zip")];
            EDGES
        }
        fn status(&self) -> EngineStatus {
            self.status.clone()
        }
        fn combiner(&self) -> Option<&dyn Combiner> {
            self.pack.as_ref().map(|_| self as &dyn Combiner)
        }
        fn convert(
            &self,
            step: &Step,
            out: &mut dyn OutputAllocator,
            _progress: &dyn Fn(f32),
            _cancel: &CancelToken,
        ) -> Result<Vec<PathBuf>, EngineError> {
            let Some(path) = out.allocate("zip", "") else {
                return Ok(Vec::new());
            };
            fs::copy(&step.input, &path)?;
            Ok(vec![path])
        }
    }

    fn write(dir: &Path, rel: &str, content: &str) -> PathBuf {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    struct Run {
        results: Vec<FileResult>,
        events: Vec<JobEvent>,
    }

    fn run_with(
        files: &[PathBuf],
        engine: &Zipper,
        options: &ConvertOptions,
        rule: &OutputRule,
        cancel: &CancelToken,
    ) -> Run {
        let engines: [&dyn Converter; 1] = [engine];
        let events = Mutex::new(Vec::new());
        let results = run_job(
            files,
            Target::Format(FormatId::new("zip")),
            options,
            rule,
            &engines,
            cancel,
            &|e| events.lock().unwrap().push(e),
        );
        Run {
            results,
            events: events.into_inner().unwrap(),
        }
    }

    fn combine_options() -> ConvertOptions {
        let mut options = ConvertOptions::default();
        options.archive.combine_into_one = true;
        options
    }

    fn run(files: &[PathBuf], engine: &Zipper) -> Run {
        run_with(
            files,
            engine,
            &combine_options(),
            &OutputRule::default(),
            &CancelToken::new(),
        )
    }

    #[test]
    fn every_file_lands_in_one_archive_named_after_the_first() {
        let dir = TempDir::new().unwrap();
        let files = vec![
            write(dir.path(), "a.txt", "A"),
            // Unknown types are welcome: they go in as they are.
            write(dir.path(), "b.mystery", "B"),
        ];
        let engine = Zipper::new(Some(Pack::Manifest));
        let run = run(&files, &engine);

        let archive = dir.path().join("a_files.zip");
        assert!(run.results.iter().all(|r| r.state == FileState::Done));
        assert_eq!(run.results[0].outputs, vec![archive.clone()]);
        assert!(run.results[1].outputs.is_empty());
        assert_eq!(engine.calls(), vec![files.clone()]);
        assert_eq!(
            fs::read_to_string(&archive).unwrap(),
            "a.txt = A\nb.mystery = B"
        );
        assert_eq!(
            JobState::from_files(&run.results, false),
            JobState::Completed
        );
        // Nothing got converted one by one behind our back.
        assert!(!dir.path().join("a.zip").exists());
    }

    #[test]
    fn one_file_is_enough_for_an_archive() {
        let dir = TempDir::new().unwrap();
        let files = vec![write(dir.path(), "solo.txt", "S")];
        let run = run(&files, &Zipper::new(Some(Pack::Manifest)));
        assert_eq!(
            run.results[0].outputs,
            vec![dir.path().join("solo_files.zip")]
        );
    }

    #[test]
    fn duplicate_names_get_numbered_inside_the_archive() {
        let dir = TempDir::new().unwrap();
        let files = vec![
            write(dir.path(), "one/notes.txt", "first"),
            write(dir.path(), "two/notes.txt", "second"),
            write(dir.path(), "three/NOTES.txt", "third"),
        ];
        let engine = Zipper::new(Some(Pack::Manifest));
        let run = run(&files, &engine);

        assert!(run.results.iter().all(|r| r.state == FileState::Done));
        let archive = &run.results[0].outputs[0];
        assert_eq!(archive, &dir.path().join("one/notes_files.zip"));
        assert_eq!(
            fs::read_to_string(archive).unwrap(),
            "notes.txt = first\nnotes (2).txt = second\nNOTES (3).txt = third"
        );
        // The first one goes in straight from the source, untouched.
        assert_eq!(engine.calls()[0][0], files[0]);
        // And the originals were never renamed.
        assert!(files.iter().all(|f| f.exists()));
    }

    #[test]
    fn unique_entry_names_follow_filee() {
        let names = unique_entry_names(&[
            PathBuf::from("/x/a.txt"),
            PathBuf::from("/y/a.txt"),
            PathBuf::from("/z/A.TXT"),
            PathBuf::from("/x/photos/"),
            PathBuf::from("/y/photos"),
            PathBuf::from("/x/archive.tar.gz"),
            PathBuf::from("/y/archive.tar.gz"),
            PathBuf::from("/x/a (2).txt"),
        ]);
        let names: Vec<String> = names
            .iter()
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "a.txt",
                "a (2).txt",
                "A (3).TXT",
                "photos",
                "photos (2)",
                "archive.tar.gz",
                "archive.tar (2).gz",
                // Taken by the second a.txt already, so it moves along too.
                "a (2) (2).txt",
            ]
        );
    }

    #[test]
    fn folders_go_in_with_their_content() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "one/photos/x.jpg", "x");
        write(dir.path(), "two/photos/y.jpg", "y");
        write(dir.path(), "two/photos/deeper/z.jpg", "z");
        let files = vec![dir.path().join("one/photos"), dir.path().join("two/photos")];
        let engine = Zipper::new(Some(Pack::Manifest));
        let run = run(&files, &engine);

        assert!(run.results.iter().all(|r| r.state == FileState::Done));
        let archive = dir.path().join("one/photos_files.zip");
        assert_eq!(run.results[0].outputs, vec![archive.clone()]);
        assert_eq!(
            fs::read_to_string(&archive).unwrap(),
            "photos = dir[x.jpg:x]\nphotos (2) = dir[deeper:dir[z.jpg:z],y.jpg:y]"
        );
        let staged = &engine.calls()[0][1];
        // The staged copy lived in the work dir, which is gone with the job.
        assert!(!staged.exists());
    }

    #[test]
    fn progress_only_moves_forward_and_reaches_every_file() {
        let dir = TempDir::new().unwrap();
        let files = vec![
            write(dir.path(), "a.txt", "A"),
            write(dir.path(), "b.txt", "B"),
        ];
        let run = run(&files, &Zipper::new(Some(Pack::Manifest)));
        for index in 0..2 {
            let progress: Vec<f32> = run
                .events
                .iter()
                .filter_map(|e| match e {
                    JobEvent::FileProgress { index: i, progress } if *i == index => Some(*progress),
                    _ => None,
                })
                .collect();
            assert_eq!(progress, [0.5, 1.0], "file {index}");
            let started = run
                .events
                .iter()
                .position(|e| matches!(e, JobEvent::FileStarted { index: i, .. } if *i == index));
            let finished = run
                .events
                .iter()
                .position(|e| matches!(e, JobEvent::FileFinished { index: i, .. } if *i == index));
            assert!(started.unwrap() < finished.unwrap());
        }
    }

    #[test]
    fn cancel_removes_the_partial_archive() {
        let dir = TempDir::new().unwrap();
        let files = vec![
            write(dir.path(), "a.txt", "A"),
            write(dir.path(), "b.txt", "B"),
        ];
        let cancel = CancelToken::new();
        let engine = Zipper::new(Some(Pack::CancelHalfway(cancel.clone())));
        let run = run_with(
            &files,
            &engine,
            &combine_options(),
            &OutputRule::default(),
            &cancel,
        );
        assert!(run.results.iter().all(|r| r.state == FileState::Cancelled));
        assert!(run.results.iter().all(|r| r.outputs.is_empty()));
        assert!(!dir.path().join("a_files.zip").exists());
        assert_eq!(
            JobState::from_files(&run.results, true),
            JobState::Cancelled
        );
    }

    #[test]
    fn a_job_cancelled_before_it_starts_packs_nothing() {
        let dir = TempDir::new().unwrap();
        let files = vec![write(dir.path(), "a.txt", "A")];
        let cancel = CancelToken::new();
        cancel.cancel();
        let engine = Zipper::new(Some(Pack::Manifest));
        let run = run_with(
            &files,
            &engine,
            &combine_options(),
            &OutputRule::default(),
            &cancel,
        );
        assert_eq!(run.results[0].state, FileState::Cancelled);
        assert!(engine.calls().is_empty());
    }

    #[test]
    fn a_failing_combiner_fails_every_file_and_leaves_nothing() {
        let dir = TempDir::new().unwrap();
        let files = vec![
            write(dir.path(), "a.txt", "A"),
            write(dir.path(), "b.txt", "B"),
        ];
        let run = run(&files, &Zipper::new(Some(Pack::Fail("disk full"))));
        for result in &run.results {
            assert_eq!(
                result.state,
                FileState::Failed {
                    key: ErrorKey::ConversionFailed,
                    detail: "disk full".into(),
                }
            );
        }
        assert!(!dir.path().join("a_files.zip").exists());
    }

    #[test]
    fn a_vanished_file_is_failed_and_the_rest_still_pack() {
        let dir = TempDir::new().unwrap();
        let files = vec![dir.path().join("gone.txt"), write(dir.path(), "b.txt", "B")];
        let engine = Zipper::new(Some(Pack::Manifest));
        let run = run(&files, &engine);
        assert!(matches!(
            &run.results[0].state,
            FileState::Failed { detail, .. } if detail.contains("no longer exists")
        ));
        assert_eq!(run.results[1].state, FileState::Done);
        // Named after the first file that actually made it in.
        assert_eq!(run.results[1].outputs, vec![dir.path().join("b_files.zip")]);
        assert_eq!(engine.calls(), vec![vec![files[1].clone()]]);
    }

    #[test]
    fn an_existing_archive_is_skipped_under_the_skip_policy() {
        let dir = TempDir::new().unwrap();
        let files = vec![write(dir.path(), "a.txt", "A")];
        write(dir.path(), "a_files.zip", "old");
        let rule = OutputRule {
            conflict: ConflictPolicy::Skip,
            ..OutputRule::default()
        };
        let run = run_with(
            &files,
            &Zipper::new(Some(Pack::Manifest)),
            &combine_options(),
            &rule,
            &CancelToken::new(),
        );
        assert_eq!(run.results[0].state, FileState::Skipped);
        assert_eq!(
            fs::read_to_string(dir.path().join("a_files.zip")).unwrap(),
            "old"
        );
    }

    #[test]
    fn keep_dates_leaves_the_archive_alone() {
        let dir = TempDir::new().unwrap();
        let source = write(dir.path(), "a.txt", "A");
        let old = filetime::FileTime::from_unix_time(1_000_000_000, 0);
        filetime::set_file_mtime(&source, old).unwrap();
        let rule = OutputRule {
            keep_dates: true,
            ..OutputRule::default()
        };
        let run = run_with(
            &[source],
            &Zipper::new(Some(Pack::Manifest)),
            &combine_options(),
            &rule,
            &CancelToken::new(),
        );
        let meta = fs::metadata(&run.results[0].outputs[0]).unwrap();
        assert_ne!(filetime::FileTime::from_last_modification_time(&meta), old);
    }

    #[test]
    fn without_a_combiner_every_file_fails_with_a_reason() {
        let dir = TempDir::new().unwrap();
        let files = vec![
            write(dir.path(), "a.txt", "A"),
            write(dir.path(), "b.txt", "B"),
        ];
        let run = run(&files, &Zipper::new(None));
        for result in &run.results {
            let FileState::Failed { key, detail } = &result.state else {
                panic!("expected a failure, got {:?}", result.state);
            };
            assert_eq!(*key, ErrorKey::NoRoute);
            assert!(detail.contains("zip archive"), "{detail}");
        }
        // Certainly not one zip per file.
        assert!(!dir.path().join("a.zip").exists());
    }

    #[test]
    fn a_missing_combiner_engine_says_what_to_install() {
        let dir = TempDir::new().unwrap();
        let files = vec![write(dir.path(), "a.txt", "A")];
        let mut engine = Zipper::new(Some(Pack::Manifest));
        engine.status = EngineStatus::Missing {
            hint: "install zipper".into(),
        };
        let run = run(&files, &engine);
        assert_eq!(
            run.results[0].state,
            FileState::Failed {
                key: ErrorKey::EngineMissing,
                detail: "needs Zipper: install zipper".into(),
            }
        );
    }

    #[test]
    fn the_mode_stays_off_unless_the_options_ask_for_it() {
        let dir = TempDir::new().unwrap();
        let files = vec![
            write(dir.path(), "a.txt", "A"),
            write(dir.path(), "b.txt", "B"),
        ];
        let engine = Zipper::new(Some(Pack::Manifest));
        let run = run_with(
            &files,
            &engine,
            &ConvertOptions::default(),
            &OutputRule::default(),
            &CancelToken::new(),
        );
        assert!(run.results.iter().all(|r| r.state == FileState::Done));
        assert!(dir.path().join("a.zip").exists() && dir.path().join("b.zip").exists());
        assert!(engine.calls().is_empty());
    }

    #[test]
    fn a_folder_is_not_an_archive_target() {
        assert_eq!(archive_target(Target::SameAsSource), None);
        assert_eq!(
            archive_target(Target::Format(FormatId::new("zip"))),
            Some(FormatId::new("zip"))
        );
        assert_eq!(archive_target(Target::Format(FormatId::new("png"))), None);
        let folder = formats::all().iter().find(|f| f.is_folder()).unwrap();
        assert_eq!(archive_target(Target::Format(folder.id)), None);
    }
}
