//! Turns the command line's inputs (files, folders and wildcards) into the list
//! of files to convert. Ported from Filee's `ExpandInputs` (`src/Filee.Cli/Cli.cs`).

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

use glob::{MatchOptions, Pattern};
use indexmap::IndexSet;
use walkdir::WalkDir;

/// The result of [`expand`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Expanded {
    /// Absolute paths of the files to convert, in input order, without duplicates.
    pub files: Vec<PathBuf>,
    /// Inputs that do not exist and wildcards that matched nothing, as typed.
    pub missing: Vec<OsString>,
}

/// Expands `inputs` into files.
///
/// - An existing file is taken as given, whatever its extension.
/// - A folder contributes the files for which `is_known` returns true, sorted by
///   file name. With `recursive`, its subfolders are searched too.
/// - A last path segment containing `*`, `?` or `[` is a wildcard. It matches
///   files in its folder (and, with `recursive`, in all subfolders) whatever
///   their format. A wildcard that matches nothing is reported as missing.
/// - Anything else is reported as missing.
///
/// Relative inputs are resolved against `cwd`, never the process's current
/// folder. `.` and `..` are removed without resolving symbolic links. The first
/// occurrence of a path wins; comparison is exact, so names differing only in
/// case stay distinct.
pub fn expand(
    inputs: &[OsString],
    recursive: bool,
    cwd: &Path,
    is_known: &dyn Fn(&Path) -> bool,
) -> Expanded {
    let mut files = IndexSet::new();
    let mut missing = Vec::new();

    for input in inputs {
        let path = absolute(cwd, Path::new(input));
        // A real file called `photo[1].png` beats the wildcard reading of its
        // name. Filee never hit this because it only treated * and ? as magic;
        // we also honour [, so we have to be a bit more polite about it.
        if !path.exists()
            && let Some(pattern) = wildcard(input)
        {
            let folder = path.parent().unwrap_or(cwd);
            // Counting matches rather than comparing set sizes: a wildcard whose
            // every match was already listed still matched something.
            let mut matched = false;
            if folder.is_dir() {
                for file in walk(folder, recursive) {
                    if file.file_name().is_some_and(|name| matches(&pattern, name)) {
                        matched = true;
                        files.insert(file);
                    }
                }
            }
            if !matched {
                missing.push(input.clone());
            }
        } else if path.is_dir() {
            files.extend(walk(&path, recursive).filter(|file| is_known(file)));
        } else if path.exists() {
            files.insert(path);
        } else {
            missing.push(input.clone());
        }
    }

    Expanded {
        files: files.into_iter().collect(),
        missing,
    }
}

/// The glob pattern in the input's last segment, if that segment has one.
fn wildcard(input: &OsStr) -> Option<Pattern> {
    let name = Path::new(input).file_name()?.to_str()?;
    if !name.contains(['*', '?', '[']) {
        return None;
    }
    // An unclosed `[` is not a pattern; the caller then reports the input as
    // missing, which is exactly what a file that isn't there deserves.
    Pattern::new(name).ok()
}

fn matches(pattern: &Pattern, name: &OsStr) -> bool {
    // Same rules as .NET's EnumerateFiles on Linux: case-sensitive, and `*`
    // happily matches dot files too.
    let options = MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    pattern.matches_with(&name.to_string_lossy(), options)
}

/// Files under `folder`, sorted by name; subfolders only when `recursive`.
fn walk(folder: &Path, recursive: bool) -> impl Iterator<Item = PathBuf> {
    WalkDir::new(folder)
        .min_depth(1)
        .max_depth(if recursive { usize::MAX } else { 1 })
        // Symlinked files count as files, like Filee. walkdir spots link loops,
        // so a folder that contains itself won't send us round forever.
        .follow_links(true)
        .sort_by_file_name()
        .into_iter()
        // Unreadable folders and broken links are skipped, not fatal: one bad
        // apple shouldn't cancel the whole harvest.
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
}

/// Joins `path` onto `cwd` and drops `.` and `..` lexically, like .NET's
/// `Path.GetFullPath`. No canonicalize: symlinks stay the names the user typed.
fn absolute(cwd: &Path, path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in cwd.join(path).components() {
        match component {
            Component::CurDir => {}
            // At the root `pop` is a no-op, which is what `/..` means anyway.
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn png_or_txt(path: &Path) -> bool {
        matches!(
            path.extension().and_then(OsStr::to_str),
            Some("png" | "txt")
        )
    }

    fn touch(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, b"x").unwrap();
        path
    }

    fn run(dir: &Path, inputs: &[&str], recursive: bool) -> Expanded {
        let inputs: Vec<OsString> = inputs.iter().map(OsString::from).collect();
        expand(&inputs, recursive, dir, &png_or_txt)
    }

    #[test]
    fn folder_keeps_known_files_sorted() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("in");
        touch(&dir, "c.unknownext");
        touch(&dir, "b.txt");
        touch(&dir, "a.png");
        let got = run(tmp.path(), &["in"], false);
        assert_eq!(got.files, vec![dir.join("a.png"), dir.join("b.txt")]);
        assert!(got.missing.is_empty());
    }

    #[test]
    fn explicit_files_are_taken_whatever_the_extension() {
        let tmp = TempDir::new().unwrap();
        let odd = touch(tmp.path(), "c.unknownext");
        let got = run(tmp.path(), &["c.unknownext"], false);
        assert_eq!(got.files, vec![odd]);
    }

    #[test]
    fn missing_inputs_are_reported_as_typed() {
        let tmp = TempDir::new().unwrap();
        let a = touch(tmp.path(), "a.png");
        let got = run(tmp.path(), &["nope.png", "a.png", "gone/"], false);
        assert_eq!(got.files, vec![a]);
        assert_eq!(
            got.missing,
            vec![OsString::from("nope.png"), "gone/".into()]
        );
    }

    #[test]
    fn wildcard_without_match_is_missing() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "a.jpg");
        let got = run(tmp.path(), &["*.png"], false);
        assert!(got.files.is_empty());
        assert_eq!(got.missing, vec![OsString::from("*.png")]);
    }

    #[test]
    fn wildcard_in_missing_folder_is_missing() {
        let tmp = TempDir::new().unwrap();
        let got = run(tmp.path(), &["nowhere/*.png"], false);
        assert_eq!(got.missing, vec![OsString::from("nowhere/*.png")]);
    }

    #[test]
    fn wildcards_match_any_format_and_are_sorted() {
        let tmp = TempDir::new().unwrap();
        let b = touch(tmp.path(), "sub/b.weird");
        let a = touch(tmp.path(), "sub/a.weird");
        touch(tmp.path(), "sub/c.png");
        let got = run(tmp.path(), &["sub/*.weird"], false);
        assert_eq!(got.files, vec![a, b]);
        assert!(got.missing.is_empty());
    }

    #[test]
    fn question_mark_and_brackets_are_wildcards() {
        let tmp = TempDir::new().unwrap();
        let a1 = touch(tmp.path(), "a1.png");
        let a2 = touch(tmp.path(), "a2.png");
        touch(tmp.path(), "a10.png");
        assert_eq!(
            run(tmp.path(), &["a?.png"], false).files,
            vec![a1.clone(), a2]
        );
        assert_eq!(run(tmp.path(), &["a[1].png"], false).files, vec![a1]);
    }

    #[test]
    fn literal_file_with_brackets_beats_the_pattern() {
        let tmp = TempDir::new().unwrap();
        let literal = touch(tmp.path(), "photo[1].png");
        touch(tmp.path(), "photo1.png");
        assert_eq!(
            run(tmp.path(), &["photo[1].png"], false).files,
            vec![literal]
        );
    }

    #[test]
    fn recursive_descends_into_subfolders() {
        let tmp = TempDir::new().unwrap();
        let top = touch(tmp.path(), "in/a.png");
        let deep = touch(tmp.path(), "in/sub/deeper/b.png");

        assert_eq!(run(tmp.path(), &["in"], false).files, vec![top.clone()]);
        assert_eq!(
            run(tmp.path(), &["in"], true).files,
            vec![top.clone(), deep.clone()]
        );

        assert_eq!(
            run(tmp.path(), &["in/*.png"], false).files,
            vec![top.clone()]
        );
        assert_eq!(run(tmp.path(), &["in/*.png"], true).files, vec![top, deep]);
    }

    #[test]
    fn same_path_twice_is_kept_once() {
        let tmp = TempDir::new().unwrap();
        let a = touch(tmp.path(), "a.png");
        let abs = a.clone().into_os_string().into_string().unwrap();
        let got = run(
            tmp.path(),
            &["a.png", "./a.png", &abs, "x/../a.png", "*.png", "."],
            false,
        );
        assert_eq!(got.files, vec![a]);
        // `x/../a.png` is resolved lexically, so the missing `x` doesn't matter.
        assert!(got.missing.is_empty());
    }

    #[test]
    fn wildcard_matching_only_duplicates_is_not_missing() {
        let tmp = TempDir::new().unwrap();
        let a = touch(tmp.path(), "a.png");
        let got = run(tmp.path(), &["a.png", "*.png"], false);
        assert_eq!(got.files, vec![a]);
        assert!(got.missing.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn names_differing_in_case_stay_distinct() {
        let tmp = TempDir::new().unwrap();
        let upper = touch(tmp.path(), "A.png");
        let lower = touch(tmp.path(), "a.png");
        let got = run(tmp.path(), &["A.png", "a.png"], false);
        assert_eq!(got.files, vec![upper.clone(), lower.clone()]);
        assert_eq!(run(tmp.path(), &["."], false).files, vec![upper, lower]);
    }

    #[test]
    fn hangul_spaces_and_quotes_survive() {
        let tmp = TempDir::new().unwrap();
        let name = "사진 it's mine.png";
        let path = touch(tmp.path(), name);
        assert_eq!(run(tmp.path(), &[name], false).files, vec![path.clone()]);
        assert_eq!(run(tmp.path(), &["."], false).files, vec![path.clone()]);
        assert_eq!(run(tmp.path(), &["사진*"], false).files, vec![path]);
    }

    #[test]
    fn relative_inputs_use_the_given_cwd_not_the_process_one() {
        let tmp = TempDir::new().unwrap();
        let a = touch(tmp.path(), "deep/a.png");
        let got = run(&tmp.path().join("deep"), &["a.png"], false);
        assert_eq!(got.files, vec![a]);
    }

    #[cfg(unix)]
    #[test]
    fn absolute_normalizes_lexically() {
        let cwd = Path::new("/home/me/work");
        assert_eq!(
            absolute(cwd, Path::new("a.png")),
            Path::new("/home/me/work/a.png")
        );
        assert_eq!(
            absolute(cwd, Path::new("./x/../b.png")),
            Path::new("/home/me/work/b.png")
        );
        assert_eq!(
            absolute(cwd, Path::new("/tmp/./c.png")),
            Path::new("/tmp/c.png")
        );
        assert_eq!(absolute(cwd, Path::new("/../d.png")), Path::new("/d.png"));
    }
}
