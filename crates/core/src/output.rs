//! Output naming: where a converted file goes and what it is called.
//!
//! [`resolve`] is a pure function ported from Filee's
//! `src/Filee.Core/Presets/OutputPathResolver.cs`. [`JobAllocator`] is the
//! [`OutputAllocator`] engines get for a final output: it shares one set of
//! reserved paths across every file of a job, so two files never get the same
//! output name.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::contract::OutputAllocator;
use crate::rename::{self, RenameStep};

/// Name of the subfolder used when [`OutputLocation::Subfolder`] has no usable name.
pub const DEFAULT_SUBFOLDER: &str = "converted";

/// File name pattern used when none is set: the source name.
pub const DEFAULT_PATTERN: &str = "{name}";

/// Where outputs are written.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all_fields = "camelCase")]
pub enum OutputLocation {
    /// Use the default output location from the settings. Whoever builds the
    /// rule swaps it for the real one; [`resolve`] treats a leftover one as
    /// [`OutputLocation::SameFolder`].
    Default,
    /// Next to the source file.
    #[default]
    SameFolder,
    /// In a subfolder next to the source file. The name may use pattern tokens.
    Subfolder(String),
    /// In a fixed folder.
    CustomFolder {
        path: PathBuf,
        /// A source below this folder keeps its subfolders below `path`
        /// (`in/2025/a.jpg` becomes `out/2025/a.png`). `None` writes every
        /// output straight into `path`.
        keep_folders_from: Option<PathBuf>,
    },
}

/// What to do when the output path is already taken.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConflictPolicy {
    /// Append ` (2)`, ` (3)`, ... to the name.
    #[default]
    Rename,
    /// Replace the existing file.
    Overwrite,
    /// Leave the existing file and skip this output.
    Skip,
}

/// How output paths are chosen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OutputRule {
    pub location: OutputLocation,
    /// File name without extension. Tokens: `{name}`, `{ext}`, `{preset}`,
    /// `{date}` (yyyyMMdd), `{time}` (HHmmss) and `{index}`, case-insensitive.
    pub pattern: String,
    /// Name rules applied in order to the expanded pattern, before it is
    /// sanitised (Filee's `renames`).
    pub renames: Vec<RenameStep>,
    pub conflict: ConflictPolicy,
    /// Give each output the creation and modification times of its source.
    pub keep_dates: bool,
}

impl Default for OutputRule {
    fn default() -> Self {
        Self {
            location: OutputLocation::SameFolder,
            pattern: DEFAULT_PATTERN.to_owned(),
            renames: Vec::new(),
            conflict: ConflictPolicy::Rename,
            keep_dates: false,
        }
    }
}

/// Values available to pattern tokens for one source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tokens {
    /// The source file being converted.
    pub source: PathBuf,
    /// Value of `{preset}`.
    pub preset: String,
    /// Value of `{index}`: the 1-based position of the file in the job.
    pub index: usize,
    /// Value of `{date}` and `{time}`: when the job was created.
    pub now: NaiveDateTime,
}

/// Computes the output path for one output file.
///
/// `target_ext` has no leading dot; an empty one asks for a folder. `suffix` is
/// appended after the expanded pattern (for example `_p3`). `exists` reports
/// whether a path is taken on disk; `reserved` holds paths already handed out in
/// this job. Returns `None` when the rule says to skip an existing file.
///
/// The source file is never returned, whatever the policy, and a reserved path is
/// always numbered, under every policy.
pub fn resolve(
    rule: &OutputRule,
    tokens: &Tokens,
    target_ext: &str,
    suffix: &str,
    exists: &dyn Fn(&Path) -> bool,
    reserved: &HashSet<PathBuf>,
) -> Option<PathBuf> {
    let source = absolute(&tokens.source);
    let source_dir = source.parent().map(Path::to_path_buf).unwrap_or_default();

    let dir = match &rule.location {
        // Default should have been swapped for the settings' choice by now; if it
        // slipped through, next to the source is the least surprising place.
        OutputLocation::Default | OutputLocation::SameFolder => source_dir,
        OutputLocation::Subfolder(name) => {
            source_dir.join(sanitize(&expand_pattern(name, tokens), DEFAULT_SUBFOLDER))
        }
        // An empty custom folder falls back to the source folder, like Filee
        // does for a whitespace-only setting.
        OutputLocation::CustomFolder { path, .. } if path.as_os_str().is_empty() => source_dir,
        OutputLocation::CustomFolder {
            path,
            keep_folders_from,
        } => match keep_folders_from {
            Some(root) => path.join(subfolder_below(root, &source_dir)),
            None => path.clone(),
        },
    };

    let source_name = name_without_extension(&tokens.source);
    // Pattern, then name rules, then sanitise: a rule may well produce a ':' or
    // strip the name bare, and sanitise is the bouncer for both.
    let renamed = rename::apply_all(&expand_pattern(&rule.pattern, tokens), &rule.renames);
    let name = sanitize(&renamed, &source_name) + suffix;
    let ext = target_ext.trim_start_matches('.');
    let path_for = |base: &str| {
        if ext.is_empty() {
            dir.join(base)
        } else {
            dir.join(format!("{base}.{ext}"))
        }
    };

    let candidate = path_for(&name);
    let is_source = same_path(&candidate, &source);
    let is_reserved = reserved.contains(&candidate);
    if !is_source && !is_reserved {
        if !exists(&candidate) {
            return Some(candidate);
        }
        match rule.conflict {
            ConflictPolicy::Overwrite => return Some(candidate),
            ConflictPolicy::Skip => return None,
            ConflictPolicy::Rename => {}
        }
    }
    // Filee checked "exists || reserved" as one thing and then let Overwrite win,
    // so two files of one job could both be told to write a.jpg and the slower one
    // won. Here a name another file already holds always gets a number instead.

    (2u32..)
        .map(|n| path_for(&format!("{name} ({n})")))
        .find(|p| !exists(p) && !reserved.contains(p) && !same_path(p, &source))
}

/// Replaces the pattern tokens in `pattern`. An empty pattern means `{name}`.
pub fn expand_pattern(pattern: &str, tokens: &Tokens) -> String {
    let pattern = if pattern.trim().is_empty() {
        DEFAULT_PATTERN
    } else {
        pattern
    };

    // One pass over the pattern instead of Filee's chain of Replace calls, so a
    // source literally named "{preset}.jpg" stays "{preset}" and doesn't turn into
    // "PNG" halfway through. Token names themselves still match case-insensitively.
    let mut out = String::with_capacity(pattern.len());
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        let close = after.find('}');
        let value = close.and_then(|c| token_value(&after[1..c], tokens));
        match (close, value) {
            (Some(c), Some(v)) => {
                out.push_str(&v);
                rest = &after[c + 1..];
            }
            _ => {
                out.push('{');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn token_value(token: &str, tokens: &Tokens) -> Option<String> {
    let value = match token.to_ascii_lowercase().as_str() {
        "name" => name_without_extension(&tokens.source),
        "ext" => extension_of(&tokens.source),
        "preset" => tokens.preset.clone(),
        "date" => tokens.now.format("%Y%m%d").to_string(),
        "time" => tokens.now.format("%H%M%S").to_string(),
        "index" => tokens.index.to_string(),
        _ => return None,
    };
    Some(value)
}

/// Replaces characters that are invalid in a Windows file name (and control
/// characters) with `_`, trims whitespace and trailing dots, and returns
/// `fallback` when nothing is left. Applied on every OS so names travel well.
pub fn sanitize(name: &str, fallback: &str) -> String {
    // Windows is the strictest neighbour, so we follow its rules everywhere. A
    // file named "a:b" is legal on ext4 and a lovely surprise on a USB stick.
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The path of `folder` below `root`, or empty when `folder` is `root` itself or
/// lies outside it.
fn subfolder_below(root: &Path, folder: &Path) -> PathBuf {
    folder
        .strip_prefix(absolute(root))
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// Compares two paths ignoring case, on every OS.
fn same_path(a: &Path, b: &Path) -> bool {
    // A Unix box can still mount a case-insensitive disk (exFAT sticks, Samba
    // shares). A needless " (2)" is cheap; overwriting the source is not.
    let fold = |p: &Path| absolute(p).to_string_lossy().to_lowercase();
    fold(a) == fold(b)
}

fn absolute(path: &Path) -> PathBuf {
    // absolute() only fails on an empty path or a missing cwd; the path as given
    // is the most honest answer we have left then.
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

// TODO(T05): switch to formats::extension_of once the registry lands; until then
// these are the two-part extensions Filee's registry knows (all tar flavours).
const COMPOUND_EXTENSIONS: &[&str] = &[
    "tar.bz2", "tar.gz", "tar.bz", "tar.xz", "tar.7z", "tar.lz", "tar.z",
];

/// The extension of `path` without the dot, treating known two-part extensions
/// such as `tar.gz` as one.
fn extension_of(path: &Path) -> String {
    let name = file_name(path);
    let lower = name.to_lowercase();
    for compound in COMPOUND_EXTENSIONS {
        if lower.len() > compound.len() + 1 && lower.ends_with(&format!(".{compound}")) {
            return name[name.len() - compound.len()..].to_owned();
        }
    }
    match name.rfind('.') {
        // A leading dot (".bashrc") is a hidden file, not an extension; Filee
        // disagreed and produced outputs named ".png". We don't.
        Some(dot) if dot > 0 => name[dot + 1..].to_owned(),
        _ => String::new(),
    }
}

/// The file name of `path` without its (possibly two-part) extension.
fn name_without_extension(path: &Path) -> String {
    let name = file_name(path);
    let ext = extension_of(path);
    if ext.is_empty() {
        // "notes." has an empty extension but still ends in a dot; the dot goes.
        name.strip_suffix('.').unwrap_or(&name).to_owned()
    } else {
        name[..name.len() - ext.len() - 1].to_owned()
    }
}

fn file_name(path: &Path) -> String {
    // Names are built as strings because patterns and sanitising are string work.
    // Hangul and friends are valid UTF-8 and survive; only truly broken byte
    // sequences get the lossy treatment.
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Paths handed out during one job, shared by all its files.
pub type Reservations = Arc<Mutex<HashSet<PathBuf>>>;

/// The [`OutputAllocator`] for final outputs of one source file.
///
/// Every file of a job gets its own allocator, all sharing one
/// [`Reservations`] set. The lock is held across resolving, checking the disk,
/// reserving and creating the parent folder, so two threads can never be handed
/// the same path.
#[derive(Clone, Debug)]
pub struct JobAllocator {
    rule: OutputRule,
    tokens: Tokens,
    reserved: Reservations,
}

impl JobAllocator {
    /// An allocator for the source in `tokens`, sharing `reserved` with the
    /// other files of the job.
    pub fn new(rule: OutputRule, tokens: Tokens, reserved: Reservations) -> Self {
        Self {
            rule,
            tokens,
            reserved,
        }
    }
}

impl OutputAllocator for JobAllocator {
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
        // A panic elsewhere can poison the lock, but the set is only ever changed
        // by a single insert, so whatever is inside is still consistent.
        let mut reserved = self.reserved.lock().unwrap_or_else(PoisonError::into_inner);
        // symlink_metadata so a dangling symlink still counts as taken; writing
        // through it would land somewhere the user didn't ask for.
        let on_disk = |p: &Path| p.symlink_metadata().is_ok();
        let path = resolve(&self.rule, &self.tokens, ext, suffix, &on_disk, &reserved)?;
        reserved.insert(path.clone());
        if let Some(parent) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                // The engine will fail to write there and report it with context;
                // a warning here is enough.
                tracing::warn!("could not create output folder {}: {err}", parent.display());
            }
        }
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use std::fs;
    use std::sync::Barrier;
    use tempfile::TempDir;

    /// `path` the way the resolver spells it: `/x/a.png` is drive-relative
    /// on Windows and comes back as `D:\x\a.png`, depending on the drive.
    fn abs(path: &str) -> PathBuf {
        std::path::absolute(path).unwrap()
    }

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 9, 30)
            .unwrap()
            .and_hms_opt(14, 5, 9)
            .unwrap()
    }

    fn tokens(source: impl Into<PathBuf>) -> Tokens {
        tokens_at(source, 1)
    }

    fn tokens_at(source: impl Into<PathBuf>, index: usize) -> Tokens {
        Tokens {
            source: source.into(),
            preset: "PNG".into(),
            index,
            now: now(),
        }
    }

    fn touch(dir: &TempDir, name: &str) -> PathBuf {
        touch_with(dir, name, "")
    }

    fn touch_with(dir: &TempDir, name: &str, contents: &str) -> PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, contents).unwrap();
        path
    }

    fn on_disk(p: &Path) -> bool {
        p.exists()
    }

    fn never(_: &Path) -> bool {
        false
    }

    fn rule(conflict: ConflictPolicy) -> OutputRule {
        OutputRule {
            conflict,
            ..OutputRule::default()
        }
    }

    const POLICIES: [ConflictPolicy; 3] = [
        ConflictPolicy::Overwrite,
        ConflictPolicy::Skip,
        ConflictPolicy::Rename,
    ];

    fn none() -> HashSet<PathBuf> {
        HashSet::new()
    }

    // The 10 cases of Filee's tests/Filee.Core.Tests/OutputPathResolverTests.cs.

    #[test]
    fn default_pattern_keeps_the_source_name() {
        let dir = TempDir::new().unwrap();
        let source = touch(&dir, "photo.jpg");
        let path = resolve(
            &OutputRule::default(),
            &tokens(source),
            "png",
            "",
            &on_disk,
            &none(),
        );
        assert_eq!(path, Some(dir.path().join("photo.png")));
    }

    #[test]
    fn two_part_extensions_are_one_extension() {
        let dir = TempDir::new().unwrap();
        let source = touch(&dir, "backup.tar.gz");
        let path = resolve(
            &OutputRule::default(),
            &tokens(&source),
            "zip",
            "",
            &on_disk,
            &none(),
        );
        assert_eq!(path, Some(dir.path().join("backup.zip")));
        assert_eq!(extension_of(&source), "tar.gz");
        assert_eq!(extension_of(Path::new("notes.txt.gz")), "gz");
        assert_eq!(
            name_without_extension(Path::new("notes.txt.gz")),
            "notes.txt"
        );
        assert_eq!(extension_of(Path::new("BACKUP.TAR.GZ")), "TAR.GZ");
        assert_eq!(extension_of(Path::new("README")), "");
        assert_eq!(name_without_extension(Path::new("README")), "README");
    }

    #[test]
    fn an_empty_extension_asks_for_a_folder() {
        let dir = TempDir::new().unwrap();
        let source = touch(&dir, "photos.zip");
        fs::create_dir(dir.path().join("photos")).unwrap();
        let path = resolve(
            &OutputRule::default(),
            &tokens(source),
            "",
            "",
            &on_disk,
            &none(),
        );
        assert_eq!(path, Some(dir.path().join("photos (2)")));
    }

    #[test]
    fn existing_file_gets_a_number_by_default() {
        let dir = TempDir::new().unwrap();
        let source = touch(&dir, "photo.jpg");
        touch(&dir, "photo.png");
        let path = resolve(
            &OutputRule::default(),
            &tokens(source),
            "png",
            "",
            &on_disk,
            &none(),
        );
        assert_eq!(path, Some(dir.path().join("photo (2).png")));
    }

    #[test]
    fn overwrite_and_skip_policies() {
        let dir = TempDir::new().unwrap();
        let source = touch(&dir, "photo.jpg");
        let existing = touch(&dir, "photo.png");
        let t = tokens(source);
        assert_eq!(
            resolve(
                &rule(ConflictPolicy::Overwrite),
                &t,
                "png",
                "",
                &on_disk,
                &none()
            ),
            Some(existing)
        );
        assert_eq!(
            resolve(
                &rule(ConflictPolicy::Skip),
                &t,
                "png",
                "",
                &on_disk,
                &none()
            ),
            None
        );
    }

    #[test]
    fn the_source_file_is_never_overwritten() {
        let dir = TempDir::new().unwrap();
        let source = touch(&dir, "photo.jpg");
        for policy in POLICIES {
            let path = resolve(
                &rule(policy),
                &tokens(&source),
                "jpg",
                "",
                &on_disk,
                &none(),
            );
            assert_eq!(path, Some(dir.path().join("photo (2).jpg")), "{policy:?}");
        }
    }

    #[test]
    fn tokens_are_expanded_and_invalid_characters_removed() {
        let rule = OutputRule {
            pattern: "{name}_{preset}_{date}_{index}:?".into(),
            ..OutputRule::default()
        };
        let path = resolve(&rule, &tokens_at("/x/a.jpg", 3), "png", "", &never, &none()).unwrap();
        assert_eq!(path.file_name().unwrap(), "a_PNG_20260930_3__.png");

        let upper = OutputRule {
            pattern: "{NAME}-{Ext}-{time}-{nope}".into(),
            ..OutputRule::default()
        };
        let path = resolve(&upper, &tokens("/x/a.jpg"), "png", "", &never, &none()).unwrap();
        assert_eq!(path.file_name().unwrap(), "a-jpg-140509-{nope}.png");
    }

    #[test]
    fn subfolder_name_supports_tokens() {
        let dir = TempDir::new().unwrap();
        let source = touch(&dir, "report.pdf");
        let rule = OutputRule {
            location: OutputLocation::Subfolder("{name}".into()),
            ..OutputRule::default()
        };
        let path = resolve(&rule, &tokens(source), "pdf", "_p1", &on_disk, &none());
        assert_eq!(path, Some(dir.path().join("report").join("report_p1.pdf")));
    }

    #[test]
    fn custom_folder_is_used_when_set() {
        let rule = OutputRule {
            location: OutputLocation::CustomFolder {
                path: PathBuf::from("/out"),
                keep_folders_from: None,
            },
            ..OutputRule::default()
        };
        let path = resolve(&rule, &tokens("/in/a.jpg"), "png", "", &never, &none());
        assert_eq!(path, Some(PathBuf::from("/out/a.png")));
    }

    #[test]
    fn a_case_alias_of_the_source_is_never_overwritten() {
        let dir = TempDir::new().unwrap();
        let source = touch_with(&dir, "photo.jpg", "original");
        for policy in POLICIES {
            let rule = OutputRule {
                pattern: "PHOTO".into(),
                conflict: policy,
                ..OutputRule::default()
            };
            let path = resolve(&rule, &tokens(&source), "jpg", "", &on_disk, &none());
            assert_eq!(path, Some(dir.path().join("PHOTO (2).jpg")), "{policy:?}");
            assert_eq!(fs::read_to_string(&source).unwrap(), "original");
        }
    }

    // MorphRing's own cases.

    #[test]
    fn overwrite_never_hands_out_a_reserved_name_twice() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("out");
        fs::create_dir(&out).unwrap();
        fs::write(out.join("a.jpg"), "old").unwrap();
        let rule = OutputRule {
            location: OutputLocation::CustomFolder {
                path: out.clone(),
                keep_folders_from: None,
            },
            conflict: ConflictPolicy::Overwrite,
            ..OutputRule::default()
        };
        let reserved: Reservations = Arc::default();
        let mut first = JobAllocator::new(rule.clone(), tokens("/in/x/a.png"), reserved.clone());
        let mut second = JobAllocator::new(rule, tokens_at("/in/y/a.bmp", 2), reserved);
        assert_eq!(first.allocate("jpg", ""), Some(out.join("a.jpg")));
        assert_eq!(second.allocate("jpg", ""), Some(out.join("a (2).jpg")));
    }

    #[test]
    fn a_reserved_name_is_numbered_under_every_policy() {
        let reserved: HashSet<PathBuf> = [abs("/x/a.png")].into();
        for policy in POLICIES {
            let path = resolve(
                &rule(policy),
                &tokens("/x/a.jpg"),
                "png",
                "",
                &never,
                &reserved,
            );
            assert_eq!(path, Some(abs("/x/a (2).png")), "{policy:?}");
        }
    }

    #[test]
    fn a_target_equal_to_the_source_in_different_case_is_numbered() {
        // No file on disk at all: case folding alone must catch the alias.
        let source = PathBuf::from("/x/Photo.JPG");
        for policy in POLICIES {
            let path = resolve(&rule(policy), &tokens(&source), "jpg", "", &never, &none());
            assert_eq!(path, Some(abs("/x/Photo (2).jpg")), "{policy:?}");
        }
    }

    #[test]
    fn skip_with_an_existing_file_returns_none() {
        let dir = TempDir::new().unwrap();
        let source = touch(&dir, "clip.mov");
        touch(&dir, "clip.mp4");
        let mut alloc =
            JobAllocator::new(rule(ConflictPolicy::Skip), tokens(source), Arc::default());
        assert_eq!(alloc.allocate("mp4", ""), None);
        assert_eq!(alloc.allocate("mkv", ""), Some(dir.path().join("clip.mkv")));
    }

    #[test]
    fn sixteen_threads_get_sixteen_distinct_paths() {
        let dir = TempDir::new().unwrap();
        let source = touch(&dir, "photo.jpg");
        let reserved: Reservations = Arc::default();
        let barrier = Barrier::new(16);
        let paths: Vec<PathBuf> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..16)
                .map(|i| {
                    let mut alloc = JobAllocator::new(
                        OutputRule::default(),
                        tokens_at(&source, i + 1),
                        reserved.clone(),
                    );
                    let barrier = &barrier;
                    s.spawn(move || {
                        barrier.wait();
                        alloc.allocate("png", "").unwrap()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let distinct: HashSet<_> = paths.iter().collect();
        assert_eq!(distinct.len(), 16);
        assert!(paths.contains(&dir.path().join("photo.png")));
        assert!(paths.contains(&dir.path().join("photo (16).png")));
    }

    #[test]
    fn allocator_creates_the_parent_folder() {
        let dir = TempDir::new().unwrap();
        let source = touch(&dir, "report.pdf");
        let rule = OutputRule {
            // A name that sanitises to nothing falls back to "converted".
            location: OutputLocation::Subfolder("...".into()),
            ..OutputRule::default()
        };
        let mut alloc = JobAllocator::new(rule, tokens(source), Arc::default());
        let path = alloc.allocate("png", "_p1").unwrap();
        assert_eq!(
            path,
            dir.path().join(DEFAULT_SUBFOLDER).join("report_p1.png")
        );
        assert!(path.parent().unwrap().is_dir());
    }

    #[test]
    fn keep_folders_mirrors_subfolders_below_the_root() {
        let rule = |root: &str| OutputRule {
            location: OutputLocation::CustomFolder {
                path: PathBuf::from("/out"),
                keep_folders_from: Some(PathBuf::from(root)),
            },
            ..OutputRule::default()
        };
        let t = tokens("/in/2025/scans/a.jpg");
        assert_eq!(
            resolve(&rule("/in"), &t, "pdf", "", &never, &none()),
            Some(PathBuf::from("/out/2025/scans/a.pdf"))
        );
        // Outside the root, or the root itself: flat.
        assert_eq!(
            resolve(&rule("/elsewhere"), &t, "pdf", "", &never, &none()),
            Some(PathBuf::from("/out/a.pdf"))
        );
        assert_eq!(
            resolve(&rule("/in/2025/scans"), &t, "pdf", "", &never, &none()),
            Some(PathBuf::from("/out/a.pdf"))
        );
    }

    #[test]
    fn sanitize_follows_windows_rules() {
        assert_eq!(
            sanitize("a<b>c:d\"e/f\\g|h?i*j", "x"),
            "a_b_c_d_e_f_g_h_i_j"
        );
        assert_eq!(sanitize("tab\there", "x"), "tab_here");
        assert_eq!(sanitize("  name. . ", "x"), "name. ");
        assert_eq!(sanitize("...", "fallback"), "fallback");
        assert_eq!(sanitize("   ", "fallback"), "fallback");
        assert_eq!(sanitize("사진 2026", "x"), "사진 2026");
    }

    #[test]
    fn a_source_named_like_a_token_is_not_expanded_twice() {
        let path = resolve(
            &OutputRule::default(),
            &tokens("/x/{preset}.jpg"),
            "png",
            "",
            &never,
            &none(),
        );
        assert_eq!(path, Some(abs("/x/{preset}.png")));
    }

    #[test]
    fn a_leftover_default_location_means_same_folder() {
        let rule = OutputRule {
            location: OutputLocation::Default,
            ..OutputRule::default()
        };
        let path = resolve(&rule, &tokens("/x/a.jpg"), "png", "", &never, &none());
        assert_eq!(path, Some(abs("/x/a.png")));
        assert!(!OutputRule::default().keep_dates);
    }

    #[test]
    fn rules_round_trip_through_json() {
        let rule = OutputRule {
            location: OutputLocation::CustomFolder {
                path: PathBuf::from("/out"),
                keep_folders_from: Some(PathBuf::from("/in")),
            },
            pattern: "{name}_50".into(),
            renames: vec![RenameStep::RemoveCopyNumber],
            conflict: ConflictPolicy::Skip,
            keep_dates: true,
        };
        let json = serde_json::to_value(&rule).unwrap();
        assert_eq!(json["keepDates"], true);
        assert_eq!(json["conflict"], "Skip");
        assert_eq!(json["location"]["CustomFolder"]["keepFoldersFrom"], "/in");
        let back: OutputRule = serde_json::from_value(json).unwrap();
        assert_eq!(back, rule);

        let sparse: OutputRule =
            serde_json::from_str(r#"{"location":"Default","pattern":"{name}_gray"}"#).unwrap();
        assert_eq!(sparse.location, OutputLocation::Default);
        assert_eq!(sparse.conflict, ConflictPolicy::Rename);
        assert!(!sparse.keep_dates);
        assert!(sparse.renames.is_empty());
    }

    #[test]
    fn output_renames_run_after_the_pattern_and_before_sanitize() {
        let rule = OutputRule {
            pattern: "{name}_{index}".into(),
            renames: vec![
                RenameStep::RemoveLeadingNumber,
                RenameStep::Regex {
                    pattern: r"IMG_(\d+)".into(),
                    replacement: "Photo:$1".into(),
                },
                RenameStep::SpacesToUnderscores,
            ],
            ..OutputRule::default()
        };
        let path = resolve(
            &rule,
            &tokens_at("/x/01 IMG_0412 (2).jpg", 7),
            "png",
            "_p1",
            &never,
            &none(),
        )
        .unwrap();
        // The ':' from the rule is sanitised; the suffix comes after the rules.
        assert_eq!(path.file_name().unwrap(), "Photo_0412_(2)_7_p1.png");
    }

    #[test]
    fn output_copy_number_is_removed_from_the_name() {
        let rule = OutputRule {
            renames: vec![RenameStep::RemoveCopyNumber],
            ..OutputRule::default()
        };
        let path = resolve(
            &rule,
            &tokens("/x/IMG_0001 (2).jpg"),
            "png",
            "",
            &never,
            &none(),
        );
        assert_eq!(path, Some(abs("/x/IMG_0001.png")));
    }

    #[test]
    fn output_a_name_renamed_to_nothing_falls_back_to_the_source_name() {
        let rule = OutputRule {
            renames: vec![RenameStep::Remove {
                text: "photo".into(),
            }],
            ..OutputRule::default()
        };
        let path = resolve(&rule, &tokens("/x/Photo.jpg"), "png", "", &never, &none());
        assert_eq!(path, Some(abs("/x/Photo.png")));
    }

    #[test]
    fn output_renames_load_from_filee_json() {
        let rule: OutputRule = serde_json::from_str(
            r#"{"pattern":"{name}","renames":[{"kind":"Replace","find":"IMG_","replace":"Photo_"}]}"#,
        )
        .unwrap();
        let path = resolve(
            &rule,
            &tokens("/x/IMG_0412.jpg"),
            "png",
            "",
            &never,
            &none(),
        );
        assert_eq!(path, Some(abs("/x/Photo_0412.png")));
    }

    #[test]
    fn hidden_files_keep_their_name() {
        assert_eq!(name_without_extension(Path::new(".bashrc")), ".bashrc");
        assert_eq!(extension_of(Path::new(".bashrc")), "");
        assert_eq!(name_without_extension(Path::new(".tar.gz")), ".tar");
    }
}
