//! Multi-page TIFF mode: every file (and every page of every file) ends up in
//! one TIFF. Will port Filee's `src/Filee.Core/Conversion/JobQueue.cs`,
//! `RunMultiPageTiffAsync` (lines 160-193).
//!
//! Same deal as combine mode next door: the hook is wired, the room is empty.
//! The mode only answers when an engine offers a [`TiffMerger`], and none does
//! yet, so jobs keep converting file by file exactly as before.

use std::path::PathBuf;

use super::{Job, not_yet_supported};
use crate::contract::{Converter, FileResult, FormatId, TiffMerger};
use crate::job::Target;

const TIFF: FormatId = FormatId::new("tiff");

/// Runs the whole job as one multi-page TIFF when its options ask for it and
/// an engine can merge TIFFs. One file is enough: a 10-page PDF becomes one
/// 10-page TIFF. `None` means "not a multi-page TIFF job".
pub(super) fn try_run(job: &Job<'_>, files: &[PathBuf]) -> Option<Vec<FileResult>> {
    if !job.options.image.multi_page_tiff || job.target != Target::Format(TIFF) {
        return None;
    }
    let (_engine, _merger) = find_tiff_merger(job)?;
    Some(not_yet_supported(
        job,
        files,
        "multi-page TIFF output is not supported yet",
    ))
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::contract::{
        CancelToken, ConvertOptions, Edge, EngineStatus, FileState, OutputAllocator, Step,
    };
    use crate::error::{EngineError, ErrorKey};
    use crate::job::{JobEvent, run_job};
    use crate::options::ImageOptions;
    use crate::output::OutputRule;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// An image "engine" that copies PNGs into TIFFs (the bytes do not care)
    /// and may claim it can stack them.
    struct Stacker {
        merges: bool,
    }

    impl TiffMerger for Stacker {
        fn merge_tiff(
            &self,
            _inputs: &[PathBuf],
            _options: &ImageOptions,
            _out: &mut dyn OutputAllocator,
            _progress: &dyn Fn(f32),
            _cancel: &CancelToken,
        ) -> Result<Vec<PathBuf>, EngineError> {
            unreachable!("the stub never calls the merger")
        }
    }

    impl Converter for Stacker {
        fn id(&self) -> &'static str {
            "stacker"
        }
        fn display_name(&self) -> &'static str {
            "Stacker"
        }
        fn edges(&self) -> &[Edge] {
            const EDGES: &[Edge] = &[Edge::new("png", "tiff")];
            EDGES
        }
        fn status(&self) -> EngineStatus {
            EngineStatus::Available {
                path: PathBuf::from("/usr/bin/stacker"),
                version: "1".into(),
            }
        }
        fn tiff_merger(&self) -> Option<&dyn TiffMerger> {
            self.merges.then_some(self as &dyn TiffMerger)
        }
        fn convert(
            &self,
            step: &Step,
            out: &mut dyn OutputAllocator,
            _progress: &dyn Fn(f32),
            _cancel: &CancelToken,
        ) -> Result<Vec<PathBuf>, EngineError> {
            let Some(path) = out.allocate("tiff", "") else {
                return Ok(Vec::new());
            };
            fs::copy(&step.input, &path)?;
            Ok(vec![path])
        }
    }

    fn run(dir: &Path, merges: bool, events: &Mutex<Vec<JobEvent>>) -> Vec<FileResult> {
        let files: Vec<PathBuf> = ["a.png", "b.png"]
            .iter()
            .map(|name| {
                let path = dir.join(name);
                fs::write(&path, name).unwrap();
                path
            })
            .collect();
        let mut options = ConvertOptions::default();
        options.image.multi_page_tiff = true;
        let engine = Stacker { merges };
        let engines: [&dyn Converter; 1] = [&engine];
        run_job(
            &files,
            Target::Format(TIFF),
            &options,
            &OutputRule::default(),
            &engines,
            &CancelToken::new(),
            &|event| events.lock().unwrap().push(event),
        )
    }

    #[test]
    fn without_a_tiff_merger_files_convert_one_by_one_as_before() {
        let dir = TempDir::new().unwrap();
        let results = run(dir.path(), false, &Mutex::default());
        assert!(results.iter().all(|r| r.state == FileState::Done));
        assert!(dir.path().join("a.tiff").exists() && dir.path().join("b.tiff").exists());
    }

    #[test]
    fn with_a_tiff_merger_the_stub_reports_not_yet_supported() {
        let dir = TempDir::new().unwrap();
        let events = Mutex::default();
        let results = run(dir.path(), true, &events);
        for result in &results {
            let FileState::Failed { key, detail } = &result.state else {
                panic!("expected a failure, got {:?}", result.state);
            };
            assert_eq!(*key, ErrorKey::ConversionFailed);
            assert!(detail.contains("not supported yet"), "{detail}");
        }
        assert!(!dir.path().join("a.tiff").exists());

        // Every file still gets its start and finish, so a progress UI never
        // waits forever for a file that was turned away at the door.
        let events = events.into_inner().unwrap();
        for index in 0..2 {
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
        }
    }
}
