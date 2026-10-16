//! Conversion history, capped so it can't eat the disk.
//! Ported from Filee's `src/Filee.Core/History/HistoryEntry.cs`,
//! `WatchResults.cs` and the history half of `UserDataStore.cs`.
//!
//! Every finished job leaves one [`HistoryEntry`] in `history.json`, newest
//! first, and we keep the last [`MAX_HISTORY`] of them. Anything older falls
//! off the end, which is roughly how my own memory works too.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset, Local};
use serde::{Deserialize, Deserializer, Serialize};

use crate::contract::{FileResult, FileState, JobState};
use crate::preset::Preset;
use crate::settings::{AppSettings, Store};

/// File name of the history inside the store dir.
pub const HISTORY_FILE: &str = "history.json";

/// Current `schemaVersion` of `history.json`.
pub const HISTORY_SCHEMA_VERSION: u32 = 1;

/// How many entries the history keeps (Filee's `MaxHistory`).
pub const MAX_HISTORY: usize = 200;

/// The summary of one finished job.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HistoryEntry {
    pub job_id: String,
    /// When the job ended, in local time. Written as RFC 3339.
    #[serde(with = "rfc3339")]
    pub finished_at: DateTime<FixedOffset>,
    pub preset_id: String,
    /// The preset's display name at the time the job ran.
    pub preset_name: String,
    #[serde(with = "job_state")]
    pub state: JobState,
    pub sources: Vec<PathBuf>,
    pub outputs: Vec<PathBuf>,
    /// The watch folder rule the job came from, if any.
    pub watch_rule_id: Option<String>,
    /// The files that failed, with the reason.
    pub failures: Vec<HistoryFailure>,
    /// The preset exactly as the job ran it, kept only when something failed
    /// so a retry behaves the same even after the preset was edited or
    /// deleted.
    pub preset: Option<Preset>,
}

impl Default for HistoryEntry {
    fn default() -> Self {
        Self {
            job_id: String::new(),
            finished_at: DateTime::UNIX_EPOCH.fixed_offset(),
            preset_id: String::new(),
            preset_name: String::new(),
            state: JobState::Completed,
            sources: Vec::new(),
            outputs: Vec::new(),
            watch_rule_id: None,
            failures: Vec::new(),
            preset: None,
        }
    }
}

impl HistoryEntry {
    /// Filee's `HistoryEntry.From(job)`: the entry for a job that just
    /// finished, stamped with the current local time. `preset` is the preset
    /// as the job ran it (output folder already decided); it is kept in the
    /// entry only when at least one file failed. The caller sets
    /// `watch_rule_id` when the job came from a watch folder.
    pub fn from_results(
        job_id: impl Into<String>,
        preset: &Preset,
        state: JobState,
        files: &[FileResult],
    ) -> Self {
        let failures: Vec<HistoryFailure> = files
            .iter()
            .filter_map(|f| match &f.state {
                FileState::Failed { key, detail } => Some(HistoryFailure {
                    source: f.source.clone(),
                    error_key: Some(key.as_str().to_owned()),
                    error_detail: (!detail.is_empty()).then(|| detail.clone()),
                }),
                _ => None,
            })
            .collect();
        Self {
            job_id: job_id.into(),
            finished_at: Local::now().fixed_offset(),
            preset_id: preset.id.clone(),
            preset_name: preset.display_name(),
            state,
            sources: files.iter().map(|f| f.source.clone()).collect(),
            outputs: files.iter().flat_map(|f| f.outputs.clone()).collect(),
            watch_rule_id: None,
            preset: (!failures.is_empty()).then(|| preset.clone()),
            failures,
        }
    }
}

/// A file a job could not convert.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HistoryFailure {
    /// Full path of the source file.
    pub source: PathBuf,
    /// Stable key of the reason, e.g. `error.no_route`.
    pub error_key: Option<String>,
    /// Detail of the reason (an engine's message, the formats of a missing
    /// route).
    pub error_detail: Option<String>,
}

/// The whole `history.json`, newest entry first.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct History {
    pub schema_version: u32,
    pub entries: Vec<HistoryEntry>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            schema_version: HISTORY_SCHEMA_VERSION,
            entries: Vec::new(),
        }
    }
}

impl<'de> Deserialize<'de> for History {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Filee writes a bare array; we write an object with a schemaVersion.
        // Take either, because a history imported from Filee shouldn't end
        // up in the `.broken` pile just for lacking a version number.
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", default)]
        struct Versioned {
            schema_version: u32,
            entries: Vec<HistoryEntry>,
        }
        impl Default for Versioned {
            fn default() -> Self {
                Self {
                    schema_version: HISTORY_SCHEMA_VERSION,
                    entries: Vec::new(),
                }
            }
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Shape {
            Bare(Vec<HistoryEntry>),
            Versioned(Versioned),
        }
        Ok(match Shape::deserialize(d)? {
            Shape::Bare(entries) => Self {
                schema_version: HISTORY_SCHEMA_VERSION,
                entries,
            },
            Shape::Versioned(v) => Self {
                schema_version: v.schema_version,
                entries: v.entries,
            },
        })
    }
}

impl History {
    /// Reads `history.json`. Missing or unreadable gives an empty history
    /// (an unreadable file is copied to `.broken` first, by the store).
    pub fn load(store: &Store) -> Self {
        let mut history: Self = store.read_json(HISTORY_FILE);
        history.entries.truncate(MAX_HISTORY);
        history
    }

    /// Filee's `AddHistory`: puts `entry` first, trims to [`MAX_HISTORY`]
    /// and rewrites the file atomically. Does nothing at all when the user
    /// turned `keep_history` off.
    pub fn add(
        &mut self,
        store: &Store,
        settings: &AppSettings,
        entry: HistoryEntry,
    ) -> io::Result<()> {
        if !settings.keep_history {
            return Ok(());
        }
        self.entries.insert(0, entry);
        self.entries.truncate(MAX_HISTORY);
        self.save(store)
    }

    /// Empties the history and writes the empty file.
    pub fn clear(&mut self, store: &Store) -> io::Result<()> {
        self.entries.clear();
        self.save(store)
    }

    fn save(&mut self, store: &Store) -> io::Result<()> {
        self.schema_version = HISTORY_SCHEMA_VERSION;
        store.write_json(HISTORY_FILE, self)
    }
}

/// A watch folder file whose last conversion failed, with the preset that
/// job ran with (for a retry).
#[derive(Clone, Debug, PartialEq)]
pub struct WatchFailure {
    pub failure: HistoryFailure,
    pub preset: Option<Preset>,
}

/// Filee's `WatchResults.Of`: what the watch rule `rule_id` did lately, as
/// `(converted, failures)`. `history` is newest first, and each source is
/// decided by its newest entry only, so a file that failed and later went
/// through counts as converted.
pub fn watch_results(history: &[HistoryEntry], rule_id: &str) -> (usize, Vec<WatchFailure>) {
    let mut seen = HashSet::new();
    let mut converted = 0;
    let mut failures = Vec::new();
    for entry in history
        .iter()
        .filter(|e| e.watch_rule_id.as_deref() == Some(rule_id))
    {
        for source in &entry.sources {
            if !seen.insert(path_key(source)) {
                continue; // a newer job already had the final word on this one
            }
            let failure = entry
                .failures
                .iter()
                .find(|f| path_key(&f.source) == path_key(source));
            match failure {
                None => converted += 1,
                Some(f) => failures.push(WatchFailure {
                    failure: f.clone(),
                    preset: entry.preset.clone(),
                }),
            }
        }
    }
    (converted, failures)
}

/// How paths compare: as-is on Unix, ignoring case on Windows (Filee's
/// `FileSystemPaths.Comparer`, which is ordinal, so no Unicode folding
/// beyond plain lowercase either).
fn path_key(path: &Path) -> String {
    let s = path.to_string_lossy();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s.into_owned()
    }
}

/// `finishedAt` as RFC 3339. A date we can't parse becomes the Unix epoch
/// rather than taking the whole history down with it.
mod rfc3339 {
    use chrono::{DateTime, FixedOffset};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(at: &DateTime<FixedOffset>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&at.to_rfc3339())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime<FixedOffset>, D::Error> {
        let raw = String::deserialize(d)?;
        Ok(DateTime::parse_from_rfc3339(raw.trim())
            .unwrap_or_else(|_| DateTime::UNIX_EPOCH.fixed_offset()))
    }
}

/// [`JobState`] as its name. Read case-insensitively; a name we don't know
/// reads as `Failed`, the least flattering guess.
mod job_state {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::contract::JobState;

    const ALL: [JobState; 4] = [
        JobState::Completed,
        JobState::CompletedWithErrors,
        JobState::Failed,
        JobState::Cancelled,
    ];

    fn name(state: JobState) -> &'static str {
        match state {
            JobState::Completed => "Completed",
            JobState::CompletedWithErrors => "CompletedWithErrors",
            JobState::Failed => "Failed",
            JobState::Cancelled => "Cancelled",
        }
    }

    pub fn serialize<S: Serializer>(state: &JobState, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(name(*state))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<JobState, D::Error> {
        let raw = String::deserialize(d)?;
        Ok(ALL
            .into_iter()
            .find(|s| name(*s).eq_ignore_ascii_case(raw.trim()))
            .unwrap_or(JobState::Failed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKey;
    use tempfile::TempDir;

    fn store(tmp: &TempDir) -> Store {
        Store::new(Some(tmp.path().to_path_buf()))
    }

    fn entry(job: &str) -> HistoryEntry {
        HistoryEntry {
            job_id: job.to_owned(),
            ..HistoryEntry::default()
        }
    }

    fn done(src: &str, out: &str) -> FileResult {
        FileResult {
            source: PathBuf::from(src),
            state: FileState::Done,
            outputs: vec![PathBuf::from(out)],
        }
    }

    fn failed(src: &str) -> FileResult {
        FileResult {
            source: PathBuf::from(src),
            state: FileState::Failed {
                key: ErrorKey::NoRoute,
                detail: "mp3 -> png".to_owned(),
            },
            outputs: vec![],
        }
    }

    fn preset() -> Preset {
        Preset {
            id: "to-png".to_owned(),
            name: "PNG please".to_owned(),
            ..Preset::default()
        }
    }

    fn watched(rule: &str, files: &[FileResult]) -> HistoryEntry {
        let mut e = HistoryEntry::from_results("j", &preset(), JobState::Completed, files);
        e.watch_rule_id = Some(rule.to_owned());
        e
    }

    #[test]
    fn add_puts_newest_first() {
        let tmp = TempDir::new().unwrap();
        let s = store(&tmp);
        let mut h = History::load(&s);
        let settings = AppSettings::default();
        h.add(&s, &settings, entry("a")).unwrap();
        h.add(&s, &settings, entry("b")).unwrap();
        h.add(&s, &settings, entry("c")).unwrap();
        let ids: Vec<_> = History::load(&s)
            .entries
            .iter()
            .map(|e| e.job_id.clone())
            .collect();
        assert_eq!(ids, ["c", "b", "a"]);
    }

    #[test]
    fn add_caps_at_200() {
        let tmp = TempDir::new().unwrap();
        let s = store(&tmp);
        let mut h = History::default();
        let settings = AppSettings::default();
        for i in 0..(MAX_HISTORY + 5) {
            h.add(&s, &settings, entry(&i.to_string())).unwrap();
        }
        let loaded = History::load(&s);
        assert_eq!(loaded.entries.len(), MAX_HISTORY);
        assert_eq!(loaded.entries[0].job_id, (MAX_HISTORY + 4).to_string());
        assert_eq!(loaded.entries[MAX_HISTORY - 1].job_id, "5");
    }

    #[test]
    fn keep_history_off_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let s = store(&tmp);
        let mut h = History::default();
        let settings = AppSettings {
            keep_history: false,
            ..AppSettings::default()
        };
        h.add(&s, &settings, entry("a")).unwrap();
        assert!(h.entries.is_empty());
        assert!(!tmp.path().join(HISTORY_FILE).exists());
    }

    #[test]
    fn clear_empties_the_file() {
        let tmp = TempDir::new().unwrap();
        let s = store(&tmp);
        let mut h = History::default();
        h.add(&s, &AppSettings::default(), entry("a")).unwrap();
        h.clear(&s).unwrap();
        assert!(h.entries.is_empty());
        assert!(History::load(&s).entries.is_empty());
        assert!(tmp.path().join(HISTORY_FILE).exists());
    }

    #[test]
    fn round_trip_keeps_everything() {
        let tmp = TempDir::new().unwrap();
        let s = store(&tmp);
        let mut e = HistoryEntry::from_results(
            "job-1",
            &preset(),
            JobState::CompletedWithErrors,
            &[done("/in/a.jpg", "/in/a.png"), failed("/in/b.mp3")],
        );
        e.watch_rule_id = Some("rule".to_owned());
        let mut h = History::default();
        h.add(&s, &AppSettings::default(), e.clone()).unwrap();
        let loaded = History::load(&s);
        assert_eq!(loaded.schema_version, HISTORY_SCHEMA_VERSION);
        assert_eq!(loaded.entries, vec![e]);

        let raw = std::fs::read_to_string(tmp.path().join(HISTORY_FILE)).unwrap();
        assert!(raw.contains("\"state\": \"CompletedWithErrors\""));
        assert!(raw.contains("\"watchRuleId\": \"rule\""));
        assert!(raw.contains("\"errorKey\": \"error.no_route\""));
    }

    #[test]
    fn from_results_fills_the_entry() {
        let e = HistoryEntry::from_results(
            "job-1",
            &preset(),
            JobState::CompletedWithErrors,
            &[done("/in/a.jpg", "/in/a.png"), failed("/in/b.mp3")],
        );
        assert_eq!(e.preset_id, "to-png");
        assert_eq!(e.preset_name, "PNG please");
        assert_eq!(
            e.sources,
            [PathBuf::from("/in/a.jpg"), PathBuf::from("/in/b.mp3")]
        );
        assert_eq!(e.outputs, [PathBuf::from("/in/a.png")]);
        assert_eq!(
            e.failures,
            [HistoryFailure {
                source: PathBuf::from("/in/b.mp3"),
                error_key: Some("error.no_route".to_owned()),
                error_detail: Some("mp3 -> png".to_owned()),
            }]
        );
        assert_eq!(e.preset, Some(preset()));

        let ok = HistoryEntry::from_results("j", &preset(), JobState::Completed, &[done("a", "b")]);
        assert!(ok.failures.is_empty());
        assert_eq!(ok.preset, None, "no failure, no preset to retry with");
    }

    #[test]
    fn reads_filee_bare_array_and_odd_values() {
        let tmp = TempDir::new().unwrap();
        let s = store(&tmp);
        std::fs::write(
            tmp.path().join(HISTORY_FILE),
            r#"[{"jobId":"x","finishedAt":"2025-03-01T10:20:30+09:00","state":"cancelled","sources":["/a"]},
                {"jobId":"y","finishedAt":"yesterday-ish","state":"Exploded"}]"#,
        )
        .unwrap();
        let h = History::load(&s);
        assert_eq!(h.entries.len(), 2);
        assert_eq!(h.entries[0].state, JobState::Cancelled);
        assert_eq!(
            h.entries[0].finished_at.to_rfc3339(),
            "2025-03-01T10:20:30+09:00"
        );
        assert_eq!(h.entries[1].state, JobState::Failed);
        assert_eq!(
            h.entries[1].finished_at,
            DateTime::UNIX_EPOCH.fixed_offset()
        );
    }

    #[test]
    fn corrupt_file_gives_empty_history() {
        let tmp = TempDir::new().unwrap();
        let s = store(&tmp);
        std::fs::write(tmp.path().join(HISTORY_FILE), "{ nope").unwrap();
        assert!(History::load(&s).entries.is_empty());
        assert!(tmp.path().join("history.json.broken").exists());
    }

    #[test]
    fn no_store_dir_is_fine() {
        let s = Store::new(None);
        let mut h = History::load(&s);
        h.add(&s, &AppSettings::default(), entry("a")).unwrap();
        assert_eq!(h.entries.len(), 1);
    }

    #[test]
    fn watch_results_newest_entry_decides() {
        // Newest first: b.mp3 failed then converted later, c.mp3 converted
        // then failed later, d.mp3 failed once.
        let history = vec![
            watched("r1", &[done("/w/b.mp3", "/w/b.png"), failed("/w/c.mp3")]),
            watched("r2", &[failed("/w/z.mp3")]),
            watched("r1", &[failed("/w/b.mp3"), done("/w/c.mp3", "/w/c.png")]),
            watched("r1", &[done("/w/a.jpg", "/w/a.png"), failed("/w/d.mp3")]),
            entry("manual"),
        ];
        let (converted, failures) = watch_results(&history, "r1");
        assert_eq!(converted, 2, "a and b");
        let failed: Vec<_> = failures.iter().map(|f| f.failure.source.clone()).collect();
        assert_eq!(
            failed,
            [PathBuf::from("/w/c.mp3"), PathBuf::from("/w/d.mp3")]
        );
        assert!(failures.iter().all(|f| f.preset == Some(preset())));

        let (converted, failures) = watch_results(&history, "r2");
        assert_eq!((converted, failures.len()), (0, 1));
        assert_eq!(watch_results(&history, "nope").0, 0);
    }

    #[test]
    fn watch_results_path_case() {
        let history = vec![
            watched("r", &[done("/w/A.jpg", "/w/A.png")]),
            watched("r", &[failed("/w/a.jpg")]),
        ];
        let (converted, failures) = watch_results(&history, "r");
        if cfg!(windows) {
            assert_eq!((converted, failures.len()), (1, 0));
        } else {
            assert_eq!((converted, failures.len()), (1, 1));
        }
    }
}
