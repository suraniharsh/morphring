//! Watch folders: the pure state machine that decides when a new file has
//! settled and is ready to convert. Ported from Filee's
//! `src/Filee.Core/Watching/FolderWatcher.cs`, `WatchRule.cs` and
//! `FileNameFilter.cs`.
//!
//! There is no OS watcher and no clock in here. The caller (the CLI's
//! `morphring watch`, later the app) owns the `notify` watcher and the 1 s
//! ticker, stats files itself and hands us `Instant`s, so every decision can be
//! tested without sleeping.
//!
//! A file is ready once its size and modification time have stayed the same
//! for the settle time ([`default_settle`]). Each ready file is remembered by
//! its handled key (`path|size|mtime`), so touching it again without changing
//! it does not convert it twice. Filee kept that set forever; here it is an LRU
//! bounded by the caller.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::formats;

/// Name of the default output folder inside the watched folder.
pub const DEFAULT_OUTPUT_NAME: &str = "converted";

/// Name of the folder inside the watched folder that receives converted
/// sources when [`AfterConversion::MoveToOriginals`] is set.
pub const ORIGINALS_NAME: &str = "originals";

/// Extensions of files that are still being written by a browser, a torrent
/// client or a copy tool.
const TEMPORARY_EXTENSIONS: &[&str] = &[
    "tmp",
    "temp",
    "part",
    "partial",
    "crdownload",
    "download",
    "opdownload",
    "!ut",
    "filepart",
];

/// Files the desktop drops into folders on its own.
const SYSTEM_FILES: &[&str] = &["desktop.ini", "thumbs.db"];

/// What happens to a source file once it was converted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AfterConversion {
    /// The source stays where it is.
    #[default]
    Keep,
    /// The source moves into [`WatchRule::originals_folder`], so the watch
    /// folder works like an inbox.
    MoveToOriginals,
}

/// One watched folder and where its conversions go.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WatchRule {
    /// The folder being watched.
    pub folder: PathBuf,
    /// Whether files in subfolders are converted too.
    pub include_subfolders: bool,
    /// Where converted files go; `None` (or an empty path) means `converted`
    /// inside [`WatchRule::folder`].
    pub output_folder: Option<PathBuf>,
    /// What happens to a source after it was converted.
    pub originals: AfterConversion,
}

impl WatchRule {
    /// The output folder actually used.
    pub fn resolved_output_folder(&self) -> PathBuf {
        match &self.output_folder {
            Some(dir) if !dir.as_os_str().to_string_lossy().trim().is_empty() => dir.clone(),
            _ => self.folder.join(DEFAULT_OUTPUT_NAME),
        }
    }

    /// Where converted sources go with [`AfterConversion::MoveToOriginals`].
    pub fn originals_folder(&self) -> PathBuf {
        self.folder.join(ORIGINALS_NAME)
    }
}

/// The settle time used when the user does not choose one: 2 s on Windows and
/// 5 s elsewhere. Windows locks a file while it is written, so an exclusive
/// open already tells a finished copy from a running one there; on macOS and
/// Linux only the wait does.
pub fn default_settle() -> Duration {
    if cfg!(windows) {
        Duration::from_secs(2)
    } else {
        Duration::from_secs(5)
    }
}

/// True for paths a watch folder never converts: directories, Office lock
/// files (`~$`), dot files, `desktop.ini` and `thumbs.db`, temporary download
/// extensions, anything inside the output or originals folder, files in
/// subfolders unless the rule includes them, and unknown formats.
pub fn should_ignore(rule: &WatchRule, path: &Path, is_dir: bool) -> bool {
    if is_dir {
        return true;
    }
    let Some(name) = path.file_name() else {
        return true;
    };
    let name = name.to_string_lossy();
    if name.is_empty() || name.starts_with("~$") || name.starts_with('.') {
        return true;
    }
    if SYSTEM_FILES.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
        return true;
    }
    // Filee looks at the last extension only (Path.GetExtension), so
    // `photo.jpg.crdownload` is caught by its `.crdownload`.
    let temporary = name.rsplit_once('.').is_some_and(|(_, ext)| {
        TEMPORARY_EXTENSIONS
            .iter()
            .any(|t| t.eq_ignore_ascii_case(ext))
    });
    if temporary {
        return true;
    }
    if is_inside(path, &rule.resolved_output_folder()) || is_inside(path, &rule.originals_folder())
    {
        return true;
    }
    if !rule.include_subfolders && !path.parent().is_some_and(|p| same_path(p, &rule.folder)) {
        return true;
    }
    formats::detect(path).is_none()
}

/// Where `source` goes when it is moved to the originals folder: the same
/// relative path under [`WatchRule::originals_folder`], numbered `name (2).ext`,
/// `name (3).ext` and so on while `exists` says the name is taken. `None` for a
/// source outside the watched folder or already inside the originals folder,
/// which stay where they are.
pub fn originals_target(
    rule: &WatchRule,
    source: &Path,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let originals = rule.originals_folder();
    if !is_inside(source, &rule.folder) || is_inside(source, &originals) {
        return None;
    }
    let relative = relative_to(source, &rule.folder)?;
    let target = originals.join(relative);
    if !exists(&target) {
        return Some(target);
    }
    let dir = target.parent()?.to_path_buf();
    let stem = formats::name_without_extension(&target);
    let ext = formats::extension_of(&target);
    // Unbounded on paper, but each step needs one more file on disk, so the
    // disk gives up long before the counter does.
    (2u64..).find_map(|n| {
        let mut name = OsString::from(&stem);
        name.push(format!(" ({n})"));
        if !ext.is_empty() {
            name.push(".");
            name.push(&ext);
        }
        let candidate = dir.join(name);
        (!exists(&candidate)).then_some(candidate)
    })
}

/// What a file looked like the last time we checked: its size and mtime.
pub type Stat = (u64, SystemTime);

/// A file waiting to settle.
#[derive(Debug, Clone)]
struct Candidate {
    /// `None` until the file has been stat'ed (a rescan, or a notice whose
    /// caller had no metadata at hand).
    stat: Option<Stat>,
    /// When the size and mtime were last seen to change.
    stable_since: Instant,
}

/// Decides when noticed files have settled. One per watched folder.
///
/// Feed it paths with [`Settler::notice`] (from the OS watcher's create,
/// modify and rename events, after [`should_ignore`]) and call
/// [`Settler::tick`] about once a second; it hands back the files that are
/// ready to convert.
#[derive(Debug)]
pub struct Settler {
    settle: Duration,
    pending: HashMap<PathBuf, Candidate>,
    handled: HandledSet,
}

impl Settler {
    /// A settler that waits `settle` for a file to stop changing and
    /// remembers at most `handled_cap` handled files (callers pass 10 000).
    pub fn new(settle: Duration, handled_cap: usize) -> Self {
        Self {
            settle,
            pending: HashMap::new(),
            handled: HandledSet::new(handled_cap),
        }
    }

    /// Records that `path` appeared or changed. `stat` is its size and mtime
    /// if the caller has them; `None` means "not looked at yet", and the next
    /// tick fills it in. A file whose size and mtime are unchanged keeps its
    /// place in the settle wait.
    pub fn notice(&mut self, path: PathBuf, stat: Option<Stat>, now: Instant) {
        match self.pending.get_mut(&path) {
            Some(candidate) => match (candidate.stat, stat) {
                (_, None) => {}
                (Some(old), Some(new)) if old == new => {}
                (None, Some(new)) => candidate.stat = Some(new),
                (Some(_), Some(new)) => {
                    *candidate = Candidate {
                        stat: Some(new),
                        stable_since: now,
                    }
                }
            },
            None => {
                self.pending.insert(
                    path,
                    Candidate {
                        stat,
                        stable_since: now,
                    },
                );
            }
        }
    }

    /// Notices every path again after the OS watcher lost events (an inotify
    /// overflow) or at start-up. Files already waiting keep their place.
    pub fn rescan(&mut self, paths: impl IntoIterator<Item = PathBuf>, now: Instant) {
        for path in paths {
            self.notice(path, None, now);
        }
    }

    /// Number of files still waiting to settle.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Re-checks every waiting file with `stat` and returns those that have
    /// kept the same size and mtime for the settle time, sorted. A file that is
    /// gone is dropped. A ready file that was already handled with the same
    /// size and mtime is dropped too, so it is not converted twice.
    pub fn tick(&mut self, now: Instant, stat: impl Fn(&Path) -> Option<Stat>) -> Vec<PathBuf> {
        let mut ready = Vec::new();
        self.pending.retain(|path, candidate| {
            let Some(current) = stat(path) else {
                return false; // gone before it settled
            };
            match candidate.stat {
                None => candidate.stat = Some(current),
                Some(old) if old != current => {
                    candidate.stat = Some(current);
                    candidate.stable_since = now;
                }
                Some(_) => {}
            }
            if now.saturating_duration_since(candidate.stable_since) >= self.settle {
                ready.push((path.clone(), current));
                false
            } else {
                true
            }
        });
        ready.sort_by(|a, b| a.0.cmp(&b.0));
        ready
            .into_iter()
            .filter(|(path, stat)| self.handled.insert(handled_key(path, *stat)))
            .map(|(path, _)| path)
            .collect()
    }
}

/// The key a handled file is remembered by: `path|size|mtime`, with mtime in
/// nanoseconds since the Unix epoch. Paths compare case-insensitively on
/// Windows only, like the file systems they live on.
fn handled_key(path: &Path, (size, mtime): Stat) -> String {
    let nanos = match mtime.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    };
    let path = path.to_string_lossy();
    let path = if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.into_owned()
    };
    format!("{path}|{size}|{nanos}")
}

/// A set of strings that forgets the least recently used one once it holds
/// more than `cap`. Filee's handled set only ever grew; a busy inbox running
/// for weeks would have kept every file it ever saw.
#[derive(Debug)]
struct HandledSet {
    cap: usize,
    clock: u64,
    by_key: HashMap<String, u64>,
    by_age: BTreeMap<u64, String>,
}

impl HandledSet {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            clock: 0,
            by_key: HashMap::new(),
            by_age: BTreeMap::new(),
        }
    }

    /// Adds `key`; true if it was new. A key seen again becomes the most
    /// recent one, so a file that keeps getting touched is not forgotten.
    fn insert(&mut self, key: String) -> bool {
        self.clock += 1;
        if let Some(age) = self.by_key.get_mut(&key) {
            self.by_age.remove(age);
            *age = self.clock;
            self.by_age.insert(self.clock, key);
            return false;
        }
        self.by_key.insert(key.clone(), self.clock);
        self.by_age.insert(self.clock, key);
        while self.by_key.len() > self.cap {
            let Some((_, oldest)) = self.by_age.pop_first() else {
                break;
            };
            self.by_key.remove(&oldest);
        }
        true
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.by_key.len()
    }
}

/// Path components, ASCII-folded on Windows so `C:\Inbox` and `c:\inbox` match.
fn components(path: &Path) -> Vec<OsString> {
    path.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| {
            let s = c.as_os_str();
            if cfg!(windows) {
                OsString::from(s.to_string_lossy().to_lowercase())
            } else {
                s.to_owned()
            }
        })
        .collect()
}

/// True when `path` is `folder` or anything below it.
fn is_inside(path: &Path, folder: &Path) -> bool {
    let (path, folder) = (components(path), components(folder));
    !folder.is_empty() && path.starts_with(&folder)
}

fn same_path(a: &Path, b: &Path) -> bool {
    components(a) == components(b)
}

/// `path` relative to `folder` (keeping `path`'s own casing).
fn relative_to(path: &Path, folder: &Path) -> Option<PathBuf> {
    let skip = components(folder).len();
    let rest: PathBuf = path
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .skip(skip)
        .collect();
    (!rest.as_os_str().is_empty()).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;

    fn inbox() -> PathBuf {
        std::env::temp_dir().join("inbox")
    }

    fn rule() -> WatchRule {
        WatchRule {
            folder: inbox(),
            ..WatchRule::default()
        }
    }

    fn t(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn default_wait_is_longer_where_files_being_written_are_not_locked() {
        let expected = if cfg!(windows) { 2 } else { 5 };
        assert_eq!(default_settle(), Duration::from_secs(expected));
    }

    #[test]
    fn output_folder_defaults_to_converted_inside_the_folder() {
        let mut rule = rule();
        assert_eq!(rule.resolved_output_folder(), inbox().join("converted"));
        assert_eq!(rule.originals_folder(), inbox().join("originals"));
        rule.output_folder = Some(PathBuf::new());
        assert_eq!(rule.resolved_output_folder(), inbox().join("converted"));
        let elsewhere = std::env::temp_dir().join("out");
        rule.output_folder = Some(elsewhere.clone());
        assert_eq!(rule.resolved_output_folder(), elsewhere);
    }

    #[test]
    fn temporary_unknown_and_own_files_are_ignored() {
        let rule = rule();
        for relative in [
            "photo.jpg.crdownload",
            "photo.jpg.PART",
            "movie.mp4.filepart",
            "x.!ut",
            "~$report.docx",
            ".hidden.jpg",
            "desktop.ini",
            "Thumbs.db",
            "notes.unknownformat",
            "noextension",
            "converted/photo.jpg",
            "originals/photo.jpg",
            "sub/photo.jpg",
        ] {
            assert!(
                should_ignore(&rule, &inbox().join(relative), false),
                "{relative} should be ignored"
            );
        }
    }

    #[test]
    fn a_plain_known_file_is_not_ignored_but_a_directory_is() {
        let rule = rule();
        assert!(!should_ignore(&rule, &inbox().join("photo.jpg"), false));
        assert!(!should_ignore(&rule, &inbox().join("Report.DOCX"), false));
        assert!(should_ignore(&rule, &inbox().join("photo.jpg"), true));
    }

    #[test]
    fn subfolders_count_only_when_the_rule_includes_them() {
        let file = inbox().join("sub").join("photo.jpg");
        assert!(should_ignore(&rule(), &file, false));
        let recursive = WatchRule {
            include_subfolders: true,
            ..rule()
        };
        assert!(!should_ignore(&recursive, &file, false));
        // The output folder stays off limits even when recursing.
        assert!(should_ignore(
            &recursive,
            &inbox().join("converted").join("photo.jpg"),
            false
        ));
    }

    #[test]
    fn a_custom_output_folder_inside_the_inbox_is_ignored() {
        let rule = WatchRule {
            include_subfolders: true,
            output_folder: Some(inbox().join("done")),
            ..rule()
        };
        assert!(should_ignore(&rule, &inbox().join("done/photo.jpg"), false));
        // "converted" is just a subfolder now.
        assert!(!should_ignore(
            &rule,
            &inbox().join("converted/photo.jpg"),
            false
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn a_case_distinct_folder_is_not_mistaken_for_the_output_folder() {
        let rule = WatchRule {
            include_subfolders: true,
            ..rule()
        };
        assert!(!should_ignore(
            &rule,
            &inbox().join("Converted/photo.jpg"),
            false
        ));
        assert!(should_ignore(
            &rule,
            &inbox().join("converted/photo.jpg"),
            false
        ));
    }

    #[test]
    fn a_file_is_ready_once_it_has_settled() {
        let start = Instant::now();
        let mut settler = Settler::new(Duration::from_secs(2), 10_000);
        let photo = inbox().join("photo.jpg");
        let stat = |_: &Path| Some((10, t(100)));

        settler.notice(photo.clone(), Some((10, t(100))), start);
        assert!(
            settler
                .tick(start + Duration::from_secs(1), stat)
                .is_empty()
        );
        assert_eq!(settler.pending_len(), 1);

        assert_eq!(
            settler.tick(start + Duration::from_secs(3), stat),
            vec![photo.clone()]
        );
        assert_eq!(settler.pending_len(), 0);

        // Another event for the unchanged file does not make it ready again.
        settler.notice(
            photo.clone(),
            Some((10, t(100))),
            start + Duration::from_secs(4),
        );
        assert!(
            settler
                .tick(start + Duration::from_secs(10), stat)
                .is_empty()
        );
        assert_eq!(settler.pending_len(), 0);
    }

    #[test]
    fn a_file_that_is_still_growing_waits() {
        let start = Instant::now();
        let mut settler = Settler::new(Duration::from_secs(2), 10_000);
        let photo = inbox().join("photo.jpg");
        let size = Mutex::new(4u64);
        let stat = |_: &Path| Some((*size.lock().unwrap(), t(100)));

        settler.notice(photo.clone(), Some((4, t(100))), start);
        *size.lock().unwrap() = 13; // the download continues
        assert!(
            settler
                .tick(start + Duration::from_secs(3), stat)
                .is_empty()
        );
        // The clock restarted at the change, so 1 s later is still too early.
        assert!(
            settler
                .tick(start + Duration::from_secs(4), stat)
                .is_empty()
        );
        assert_eq!(
            settler.tick(start + Duration::from_secs(5), stat),
            vec![photo]
        );
    }

    #[test]
    fn a_new_notice_with_a_changed_stat_restarts_the_wait() {
        let start = Instant::now();
        let mut settler = Settler::new(Duration::from_secs(2), 10_000);
        let photo = inbox().join("photo.jpg");
        settler.notice(photo.clone(), Some((1, t(100))), start);
        settler.notice(
            photo.clone(),
            Some((1, t(100))),
            start + Duration::from_secs(1),
        );
        settler.notice(
            photo.clone(),
            Some((2, t(101))),
            start + Duration::from_secs(2),
        );
        let stat = |_: &Path| Some((2, t(101)));
        assert!(
            settler
                .tick(start + Duration::from_secs(3), stat)
                .is_empty()
        );
        assert_eq!(
            settler.tick(start + Duration::from_secs(4), stat),
            vec![photo]
        );
    }

    #[test]
    fn a_file_that_disappears_is_dropped() {
        let start = Instant::now();
        let mut settler = Settler::new(Duration::from_secs(2), 10_000);
        settler.notice(inbox().join("photo.jpg"), Some((1, t(1))), start);
        assert!(
            settler
                .tick(start + Duration::from_secs(5), |_| None)
                .is_empty()
        );
        assert_eq!(settler.pending_len(), 0);
    }

    #[test]
    fn ready_files_come_back_sorted_and_a_changed_file_is_ready_again() {
        let start = Instant::now();
        let mut settler = Settler::new(Duration::from_secs(2), 10_000);
        let (a, b, c) = (
            inbox().join("a.jpg"),
            inbox().join("b.jpg"),
            inbox().join("c.jpg"),
        );
        for p in [&c, &a, &b] {
            settler.notice(p.clone(), Some((1, t(1))), start);
        }
        let stat = |_: &Path| Some((1, t(1)));
        assert_eq!(
            settler.tick(start + Duration::from_secs(2), stat),
            vec![a.clone(), b, c]
        );

        // Same path, new content: a new handled key, so it converts again.
        settler.notice(a.clone(), Some((2, t(5))), start + Duration::from_secs(3));
        let stat = |_: &Path| Some((2, t(5)));
        assert_eq!(settler.tick(start + Duration::from_secs(6), stat), vec![a]);
    }

    #[test]
    fn case_distinct_files_with_identical_metadata_are_both_ready() {
        let start = Instant::now();
        let mut settler = Settler::new(Duration::from_secs(2), 10_000);
        let lower = inbox().join("photo.jpg");
        let upper = inbox().join("PHOTO.jpg");
        settler.notice(lower.clone(), Some((3, t(9))), start);
        settler.notice(upper.clone(), Some((3, t(9))), start);
        let ready = settler.tick(start + Duration::from_secs(3), |_| Some((3, t(9))));
        if cfg!(windows) {
            assert_eq!(ready.len(), 1);
        } else {
            assert_eq!(ready, vec![upper, lower]);
        }
    }

    #[test]
    fn rescan_fills_in_the_stat_without_restarting_the_wait() {
        let start = Instant::now();
        let mut settler = Settler::new(Duration::from_secs(2), 10_000);
        let photo = inbox().join("photo.jpg");
        settler.rescan([photo.clone()], start);
        let stat = |_: &Path| Some((7, t(7)));
        assert!(
            settler
                .tick(start + Duration::from_secs(1), stat)
                .is_empty()
        );
        assert_eq!(
            settler.tick(start + Duration::from_secs(2), stat),
            vec![photo.clone()]
        );

        // Rescanning a file already waiting keeps its place.
        settler.notice(
            photo.clone(),
            Some((8, t(8))),
            start + Duration::from_secs(3),
        );
        settler.rescan([photo.clone()], start + Duration::from_secs(4));
        let stat = |_: &Path| Some((8, t(8)));
        assert_eq!(
            settler.tick(start + Duration::from_secs(5), stat),
            vec![photo]
        );
    }

    #[test]
    fn the_handled_set_is_bounded_and_forgets_the_oldest() {
        let start = Instant::now();
        let mut settler = Settler::new(Duration::ZERO, 3);
        let names: Vec<PathBuf> = (0..5).map(|i| inbox().join(format!("{i}.jpg"))).collect();
        let stat = |_: &Path| Some((1, t(1)));
        for p in &names {
            settler.notice(p.clone(), None, start);
        }
        assert_eq!(settler.tick(start, stat).len(), 5);
        assert_eq!(settler.handled.len(), 3);

        // 0 and 1 were forgotten, so they come back; 4 is still remembered.
        for p in [&names[0], &names[4]] {
            settler.notice(p.clone(), None, start);
        }
        assert_eq!(settler.tick(start, stat), vec![names[0].clone()]);
        assert_eq!(settler.handled.len(), 3);
    }

    #[test]
    fn a_handled_key_seen_again_becomes_the_most_recent() {
        let mut set = HandledSet::new(2);
        assert!(set.insert("a".into()));
        assert!(set.insert("b".into()));
        assert!(!set.insert("a".into())); // a is fresh again
        assert!(set.insert("c".into())); // evicts b, not a
        assert!(!set.insert("a".into()));
        assert!(set.insert("b".into()));
    }

    #[test]
    fn originals_keep_the_relative_path_and_get_numbered() {
        let rule = WatchRule {
            originals: AfterConversion::MoveToOriginals,
            ..rule()
        };
        let originals = rule.originals_folder();
        assert_eq!(
            originals_target(&rule, &inbox().join("sub/photo.jpg"), |_| false),
            Some(originals.join("sub/photo.jpg"))
        );

        let taken: HashSet<PathBuf> =
            [originals.join("photo.jpg"), originals.join("photo (2).jpg")].into();
        assert_eq!(
            originals_target(&rule, &inbox().join("photo.jpg"), |p| taken.contains(p)),
            Some(originals.join("photo (3).jpg"))
        );

        let taken: HashSet<PathBuf> =
            [originals.join("backup.tar.gz"), originals.join("README")].into();
        assert_eq!(
            originals_target(&rule, &inbox().join("backup.tar.gz"), |p| taken.contains(p)),
            Some(originals.join("backup (2).tar.gz"))
        );
        assert_eq!(
            originals_target(&rule, &inbox().join("README"), |p| taken.contains(p)),
            Some(originals.join("README (2)"))
        );
    }

    #[test]
    fn sources_outside_the_folder_or_already_in_originals_stay_put() {
        let rule = rule();
        let elsewhere = std::env::temp_dir().join("elsewhere/photo.jpg");
        assert_eq!(originals_target(&rule, &elsewhere, |_| false), None);
        let moved = rule.originals_folder().join("photo.jpg");
        assert_eq!(originals_target(&rule, &moved, |_| false), None);
        assert_eq!(originals_target(&rule, &inbox(), |_| false), None);
    }
}
