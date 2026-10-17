//! Combine-into-one-archive mode: many files in, one archive out ("Compress
//! into one archive", Filee's zip-all). Will port Filee's
//! `src/Filee.Core/Conversion/JobQueue.cs`, `RunCombinedAsync` (lines 196-271).
//!
//! For now this is the doorway without the room behind it: the job runner
//! knocks here first, and the mode only answers when an engine actually offers
//! a [`Combiner`]. None does yet, so every job keeps running file by file
//! exactly as before. Should one show up early, its files fail politely with
//! "not supported yet" instead of being zipped one by one behind its back.

use std::path::PathBuf;

use super::{Job, not_yet_supported};
use crate::contract::{Combiner, Converter, FileResult, FormatId};
use crate::formats::{self, Category};
use crate::job::Target;

/// Runs the whole job as one archive when its options ask for it and an
/// engine can pack into the target. `None` means "not a combine job"; the
/// caller carries on with the other modes.
pub(super) fn try_run(job: &Job<'_>, files: &[PathBuf]) -> Option<Vec<FileResult>> {
    if !job.options.archive.combine_into_one {
        return None;
    }
    let format = archive_target(job.target)?;
    let (_engine, _combiner) = find_combiner(job, format)?;
    Some(not_yet_supported(
        job,
        files,
        "combining files into one archive is not supported yet",
    ))
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::contract::{
        CancelToken, ConvertOptions, Edge, EngineStatus, FileState, OutputAllocator, Step,
    };
    use crate::error::{EngineError, ErrorKey};
    use crate::job::run_job;
    use crate::output::OutputRule;
    use tempfile::TempDir;

    /// A zip "engine" that converts txt to zip one file at a time, and may or
    /// may not pretend to know how to pack many files into one.
    struct Zipper {
        combines: bool,
    }

    impl Combiner for Zipper {
        fn can_combine_into(&self, format: FormatId) -> bool {
            format.as_str() == "zip"
        }

        fn combine(
            &self,
            _inputs: &[PathBuf],
            _format: FormatId,
            _options: &ConvertOptions,
            _out: &mut dyn OutputAllocator,
            _progress: &dyn Fn(f32),
            _cancel: &CancelToken,
        ) -> Result<Vec<PathBuf>, EngineError> {
            unreachable!("the stub never calls the combiner")
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
            EngineStatus::Available {
                path: PathBuf::from("/usr/bin/zipper"),
                version: "1".into(),
            }
        }
        fn combiner(&self) -> Option<&dyn Combiner> {
            self.combines.then_some(self as &dyn Combiner)
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

    fn run(dir: &Path, combines: bool, combine_into_one: bool) -> Vec<FileResult> {
        let files: Vec<PathBuf> = ["a.txt", "b.txt"]
            .iter()
            .map(|name| {
                let path = dir.join(name);
                fs::write(&path, name).unwrap();
                path
            })
            .collect();
        let mut options = ConvertOptions::default();
        options.archive.combine_into_one = combine_into_one;
        let engine = Zipper { combines };
        let engines: [&dyn Converter; 1] = [&engine];
        run_job(
            &files,
            Target::Format(FormatId::new("zip")),
            &options,
            &OutputRule::default(),
            &engines,
            &CancelToken::new(),
            &|_| {},
        )
    }

    #[test]
    fn without_a_combiner_files_convert_one_by_one_as_before() {
        let dir = TempDir::new().unwrap();
        let results = run(dir.path(), false, true);
        assert!(results.iter().all(|r| r.state == FileState::Done));
        assert!(dir.path().join("a.zip").exists() && dir.path().join("b.zip").exists());
    }

    #[test]
    fn with_a_combiner_the_stub_reports_not_yet_supported() {
        let dir = TempDir::new().unwrap();
        let results = run(dir.path(), true, true);
        for result in &results {
            let FileState::Failed { key, detail } = &result.state else {
                panic!("expected a failure, got {:?}", result.state);
            };
            assert_eq!(*key, ErrorKey::ConversionFailed);
            assert!(detail.contains("not supported yet"), "{detail}");
            assert!(result.outputs.is_empty());
        }
        assert!(!dir.path().join("a.zip").exists());
    }

    #[test]
    fn the_mode_stays_off_unless_the_options_ask_for_it() {
        let dir = TempDir::new().unwrap();
        let results = run(dir.path(), true, false);
        assert!(results.iter().all(|r| r.state == FileState::Done));
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
