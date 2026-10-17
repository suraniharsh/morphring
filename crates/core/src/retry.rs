//! Retrying the failed files of a past job: work out which files to run again
//! and with what. Ported from Filee's `ConversionService.Retry` (with the
//! callers in `HomePageViewModel.Retry` and
//! `WatchFoldersPageViewModel.RetryFailures`).
//!
//! Everything here is pure: the caller says which paths still exist, and gets
//! back a [`RetryPlan`] to hand to the job queue, or to the watch folder the
//! files came from. Nothing is converted, enqueued or even `stat`-ed in this
//! file, which makes it the most relaxing module in the crate.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::history::{HistoryEntry, WatchFailure};
use crate::preset::Preset;

/// What to run again for one past job.
#[derive(Clone, Debug, PartialEq)]
pub struct RetryPlan {
    /// The preset exactly as the job ran it (output folder already decided),
    /// as stored in the history. `None` when the entry kept no preset: then
    /// there is nothing to retry with and [`RetryPlan::files`] is empty.
    pub preset: Option<Preset>,
    /// The failed files that are still there, in the order they failed, each
    /// listed once.
    pub files: Vec<PathBuf>,
    /// The failed files that are gone since; they are left out of the retry.
    pub skipped_missing: Vec<PathBuf>,
    /// The watch folder rule the job came from. When set, the files go back
    /// through that folder (so it also does what it does afterwards, such as
    /// moving the originals) instead of a direct job.
    pub watch_rule_id: Option<String>,
}

/// Where a retry should be sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryRoute<'a> {
    /// Nothing to run: no failed file is left, or no preset to run it with.
    Nothing,
    /// Start a new job with the plan's preset and files.
    Job,
    /// Hand the files and preset to the watch folder rule with this id.
    Watch(&'a str),
}

impl RetryPlan {
    /// True when there is nothing to convert again.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() || self.preset.is_none()
    }

    /// Where this plan should go (Filee: a direct `Start`, or `WatchRetry`).
    pub fn route(&self) -> RetryRoute<'_> {
        if self.is_empty() {
            RetryRoute::Nothing
        } else if let Some(rule) = self.watch_rule_id.as_deref() {
            RetryRoute::Watch(rule)
        } else {
            RetryRoute::Job
        }
    }
}

/// Plans the retry of `entry`'s failed files. Only files recorded as failures
/// are considered (converted and cancelled ones stay as they are); of those,
/// the ones `exists` says are gone land in [`RetryPlan::skipped_missing`].
///
/// An entry without a stored preset (older history, or nothing failed) cannot
/// be retried, like Filee's `CanRetry`: its failures are not split into
/// files and skipped ones, since nothing will run either way.
pub fn plan(entry: &HistoryEntry, exists: impl Fn(&Path) -> bool) -> RetryPlan {
    let sources = entry.failures.iter().map(|f| f.source.as_path());
    build(
        entry.preset.clone(),
        entry.watch_rule_id.clone(),
        sources,
        exists,
    )
}

/// Plans the retry of a watch folder's failures (the "Retry" on a watch rule's
/// card): one plan per distinct preset the failed jobs ran with, in the order
/// the presets first appear. Failures with no stored preset are dropped, as
/// Filee does.
pub fn plan_watch(
    rule_id: &str,
    failures: &[WatchFailure],
    exists: impl Fn(&Path) -> bool,
) -> Vec<RetryPlan> {
    // Preset has no Hash and a rule rarely has more than one or two presets,
    // so a linear group-by it is. Big O is welcome to file a complaint.
    let mut groups: Vec<(&Preset, Vec<&Path>)> = Vec::new();
    for failure in failures {
        let Some(preset) = failure.preset.as_ref() else {
            continue;
        };
        let source = failure.failure.source.as_path();
        match groups.iter_mut().find(|(p, _)| *p == preset) {
            Some((_, sources)) => sources.push(source),
            None => groups.push((preset, vec![source])),
        }
    }
    groups
        .into_iter()
        .map(|(preset, sources)| {
            build(
                Some(preset.clone()),
                Some(rule_id.to_owned()),
                sources.into_iter(),
                &exists,
            )
        })
        .collect()
}

fn build<'a>(
    preset: Option<Preset>,
    watch_rule_id: Option<String>,
    sources: impl Iterator<Item = &'a Path>,
    exists: impl Fn(&Path) -> bool,
) -> RetryPlan {
    let mut files = Vec::new();
    let mut skipped_missing = Vec::new();
    if preset.is_some() {
        // Filee dedupes ordinally, so we do too: same bytes, same file.
        let mut seen = HashSet::new();
        for source in sources {
            if !seen.insert(source) {
                continue;
            }
            if exists(source) {
                files.push(source.to_path_buf());
            } else {
                skipped_missing.push(source.to_path_buf());
            }
        }
    }
    RetryPlan {
        preset,
        files,
        skipped_missing,
        watch_rule_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{FileResult, FileState, JobState};
    use crate::error::ErrorKey;
    use crate::history::HistoryFailure;

    fn preset(id: &str) -> Preset {
        Preset {
            id: id.to_owned(),
            target: "pdf".to_owned(),
            ..Preset::default()
        }
    }

    fn failed(source: &str) -> FileResult {
        FileResult {
            source: PathBuf::from(source),
            state: FileState::Failed {
                key: ErrorKey::ConversionFailed,
                detail: "boom".to_owned(),
            },
            outputs: Vec::new(),
        }
    }

    fn done(source: &str) -> FileResult {
        FileResult {
            source: PathBuf::from(source),
            state: FileState::Done,
            outputs: vec![PathBuf::from(format!("{source}.pdf"))],
        }
    }

    fn entry(files: &[FileResult]) -> HistoryEntry {
        HistoryEntry::from_results("j1", &preset("p1"), JobState::CompletedWithErrors, files)
    }

    fn all_exist(_: &Path) -> bool {
        true
    }

    #[test]
    fn only_failed_files_are_retried() {
        let e = entry(&[done("/a.docx"), failed("/b.docx"), failed("/c.docx")]);
        let p = plan(&e, all_exist);
        assert_eq!(
            p.files,
            vec![PathBuf::from("/b.docx"), PathBuf::from("/c.docx")]
        );
        assert!(p.skipped_missing.is_empty());
        assert_eq!(p.preset, Some(preset("p1")));
        assert_eq!(p.route(), RetryRoute::Job);
    }

    #[test]
    fn missing_files_are_listed_as_skipped() {
        let e = entry(&[failed("/gone.docx"), failed("/here.docx")]);
        let p = plan(&e, |path| path != Path::new("/gone.docx"));
        assert_eq!(p.files, vec![PathBuf::from("/here.docx")]);
        assert_eq!(p.skipped_missing, vec![PathBuf::from("/gone.docx")]);
    }

    #[test]
    fn all_missing_means_nothing_to_run() {
        let e = entry(&[failed("/gone.docx")]);
        let p = plan(&e, |_| false);
        assert!(p.is_empty());
        assert_eq!(p.route(), RetryRoute::Nothing);
        assert_eq!(p.skipped_missing, vec![PathBuf::from("/gone.docx")]);
    }

    #[test]
    fn watch_entries_keep_the_rule_id() {
        let mut e = entry(&[failed("/in/a.png")]);
        e.watch_rule_id = Some("rule-7".to_owned());
        let p = plan(&e, all_exist);
        assert_eq!(p.watch_rule_id.as_deref(), Some("rule-7"));
        assert_eq!(p.route(), RetryRoute::Watch("rule-7"));
        assert_eq!(p.files, vec![PathBuf::from("/in/a.png")]);
    }

    #[test]
    fn no_failures_yield_an_empty_plan() {
        let e = entry(&[done("/a.docx"), done("/b.docx")]);
        let p = plan(&e, all_exist);
        assert!(p.is_empty());
        assert!(p.files.is_empty());
        assert!(p.skipped_missing.is_empty());
        assert_eq!(p.preset, None);
        assert_eq!(p.route(), RetryRoute::Nothing);
    }

    #[test]
    fn an_entry_without_a_preset_cannot_be_retried() {
        let mut e = entry(&[failed("/a.docx")]);
        e.preset = None;
        let p = plan(&e, all_exist);
        assert!(p.is_empty());
        assert!(p.files.is_empty());
        assert_eq!(p.route(), RetryRoute::Nothing);
    }

    #[test]
    fn duplicate_failures_are_retried_once() {
        let mut e = entry(&[failed("/a.docx")]);
        e.failures.push(HistoryFailure {
            source: PathBuf::from("/a.docx"),
            ..HistoryFailure::default()
        });
        let p = plan(&e, all_exist);
        assert_eq!(p.files, vec![PathBuf::from("/a.docx")]);
    }

    #[test]
    fn exists_is_asked_only_about_failed_files() {
        let e = entry(&[done("/ok.docx"), failed("/bad.docx")]);
        let asked = std::cell::RefCell::new(Vec::new());
        plan(&e, |p| {
            asked.borrow_mut().push(p.to_path_buf());
            true
        });
        assert_eq!(asked.into_inner(), vec![PathBuf::from("/bad.docx")]);
    }

    fn watch_failure(source: &str, preset: Option<Preset>) -> WatchFailure {
        WatchFailure {
            failure: HistoryFailure {
                source: PathBuf::from(source),
                ..HistoryFailure::default()
            },
            preset,
        }
    }

    #[test]
    fn watch_failures_group_by_preset() {
        let failures = [
            watch_failure("/in/a.png", Some(preset("p1"))),
            watch_failure("/in/b.png", Some(preset("p2"))),
            watch_failure("/in/c.png", None),
            watch_failure("/in/d.png", Some(preset("p1"))),
            watch_failure("/in/gone.png", Some(preset("p2"))),
        ];
        let plans = plan_watch("rule-1", &failures, |p| p != Path::new("/in/gone.png"));
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].preset, Some(preset("p1")));
        assert_eq!(
            plans[0].files,
            vec![PathBuf::from("/in/a.png"), PathBuf::from("/in/d.png")]
        );
        assert_eq!(plans[1].preset, Some(preset("p2")));
        assert_eq!(plans[1].files, vec![PathBuf::from("/in/b.png")]);
        assert_eq!(
            plans[1].skipped_missing,
            vec![PathBuf::from("/in/gone.png")]
        );
        assert!(
            plans
                .iter()
                .all(|p| p.route() == RetryRoute::Watch("rule-1"))
        );
    }
}
