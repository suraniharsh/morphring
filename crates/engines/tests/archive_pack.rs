//! Packing files and folders into ZIP, TAR and TAR.GZ with the built-in
//! archive engine, and reading every archive back with the zip and tar
//! readers to make sure it says what we meant it to say.

#![cfg(feature = "archive")]
// The readers below are test helpers, not #[test] functions, so clippy's
// allow-unwrap-in-tests does not cover them. A panic is the report here.
#![allow(clippy::unwrap_used)]

mod common;

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use morphring_core::job::run_job;
use morphring_core::output::OutputRule;
use morphring_core::{
    ArchiveLevel, CancelToken, ConvertOptions, Converter, EngineError, FileState, FormatId,
    OutputAllocator, Step, Target,
};
use morphring_engines::archive::ArchiveEngine;
use tempfile::TempDir;

use common::fixture;

/// Hands out `<dir>/<stem>.<ext>` and remembers the extension it was asked for.
struct DirAllocator {
    dir: PathBuf,
    stem: String,
}

impl OutputAllocator for DirAllocator {
    fn allocate(&mut self, ext: &str, _suffix: &str) -> Option<PathBuf> {
        assert!(!ext.is_empty(), "an archive is a file, not a folder");
        Some(self.dir.join(format!("{}.{ext}", self.stem)))
    }
}

/// The skip policy with the output already taken.
struct SkipAll;

impl OutputAllocator for SkipAll {
    fn allocate(&mut self, _ext: &str, _suffix: &str) -> Option<PathBuf> {
        None
    }
}

fn pack_with(
    input: &Path,
    from: &'static str,
    to: &'static str,
    level: ArchiveLevel,
    out_dir: &Path,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    let mut options = ConvertOptions::default();
    options.archive.level = level;
    let step = Step {
        input: input.to_path_buf(),
        from: FormatId::new(from),
        to: FormatId::new(to),
        options,
        work_dir: out_dir.join("work"),
    };
    let stem = input
        .file_name()
        .unwrap()
        .to_string_lossy()
        .split('.')
        .next()
        .unwrap()
        .to_owned();
    ArchiveEngine::new().convert(
        &step,
        &mut DirAllocator {
            dir: out_dir.to_path_buf(),
            stem,
        },
        &|_| {},
        cancel,
    )
}

fn pack(input: &Path, from: &'static str, to: &'static str, out_dir: &Path) -> PathBuf {
    let outputs = pack_with(
        input,
        from,
        to,
        ArchiveLevel::Normal,
        out_dir,
        &CancelToken::new(),
    )
    .unwrap_or_else(|e| panic!("{from} -> {to}: {e}"));
    assert_eq!(outputs.len(), 1);
    outputs.into_iter().next().unwrap()
}

/// `(name, contents)` of every entry of a ZIP; folders have `None`.
fn read_zip(path: &Path) -> Vec<(String, Option<Vec<u8>>)> {
    let mut zip = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
    (0..zip.len())
        .map(|i| {
            let mut entry = zip.by_index(i).unwrap();
            let name = entry.name().unwrap().into_owned();
            if entry.is_dir() {
                (name, None)
            } else {
                let mut data = Vec::new();
                entry.read_to_end(&mut data).unwrap();
                (name, Some(data))
            }
        })
        .collect()
}

/// `(name, contents, mode)` of every entry of a TAR stream; folders have `None`.
fn read_tar(reader: impl Read) -> Vec<(String, Option<Vec<u8>>, u32)> {
    let mut tar = tar::Archive::new(reader);
    tar.entries()
        .unwrap()
        .map(|entry| {
            let mut entry = entry.unwrap();
            let name = String::from_utf8(entry.path_bytes().into_owned()).unwrap();
            let mode = entry.header().mode().unwrap();
            if entry.header().entry_type().is_dir() {
                (name, None, mode)
            } else {
                let mut data = Vec::new();
                entry.read_to_end(&mut data).unwrap();
                (name, Some(data), mode)
            }
        })
        .collect()
}

fn read_any_tar(path: &Path) -> Vec<(String, Option<Vec<u8>>, u32)> {
    if path.to_string_lossy().ends_with(".tar.gz") {
        read_tar(GzDecoder::new(File::open(path).unwrap()))
    } else {
        read_tar(File::open(path).unwrap())
    }
}

/// A small tree with a nested folder and a name that is not ASCII:
/// `stuff/a.txt`, `stuff/sub/b.txt`, `stuff/sub/señor 日本.txt`.
fn tree(root: &Path) -> PathBuf {
    let folder = root.join("stuff");
    fs::create_dir_all(folder.join("sub")).unwrap();
    fs::write(folder.join("a.txt"), b"alpha").unwrap();
    fs::write(folder.join("sub").join("b.txt"), b"bravo").unwrap();
    fs::write(
        folder.join("sub").join("se\u{f1}or \u{65e5}\u{672c}.txt"),
        b"hola",
    )
    .unwrap();
    folder
}

const TREE_NAMES: [&str; 5] = [
    "stuff/",
    "stuff/a.txt",
    "stuff/sub/",
    "stuff/sub/b.txt",
    "stuff/sub/se\u{f1}or \u{65e5}\u{672c}.txt",
];

#[test]
fn sample_txt_goes_into_a_zip() {
    let tmp = TempDir::new().unwrap();
    let zip = pack(&fixture("sample.txt"), "txt", "zip", tmp.path());
    assert_eq!(zip, tmp.path().join("sample.zip"));
    let entries = read_zip(&zip);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, "sample.txt");
    assert_eq!(
        entries[0].1.as_deref(),
        Some(fs::read(fixture("sample.txt")).unwrap().as_slice())
    );
}

#[test]
fn sample_txt_goes_into_a_tar_and_a_tgz() {
    let tmp = TempDir::new().unwrap();
    let original = fs::read(fixture("sample.txt")).unwrap();
    for (to, ext) in [("tar", "tar"), ("tgz", "tar.gz")] {
        let archive = pack(&fixture("sample.txt"), "txt", to, tmp.path());
        assert_eq!(archive, tmp.path().join(format!("sample.{ext}")));
        let entries = read_any_tar(&archive);
        assert_eq!(entries.len(), 1, "{to}");
        assert_eq!(entries[0].0, "sample.txt");
        assert_eq!(entries[0].1.as_deref(), Some(original.as_slice()));
        assert_eq!(entries[0].2, 0o644);
    }
}

#[test]
fn a_folder_goes_into_a_zip_with_utf8_names() {
    let tmp = TempDir::new().unwrap();
    let folder = tree(tmp.path());
    let out = tmp.path().join("out");
    let zip = pack(&folder, "folder", "zip", &out);
    let entries = read_zip(&zip);
    let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, TREE_NAMES);
    assert_eq!(entries[1].1.as_deref(), Some(&b"alpha"[..]));
    assert_eq!(entries[4].1.as_deref(), Some(&b"hola"[..]));

    // Bit 11 of the general purpose flags: "this name is UTF-8". Without it,
    // Windows Explorer greets you with mojibake.
    let raw = fs::read(&zip).unwrap();
    let needle = "se\u{f1}or".as_bytes();
    let at = raw.windows(needle.len()).position(|w| w == needle).unwrap();
    // The local header is 30 bytes and the name sits right after it; the
    // flags are at offset 6. The name starts with "stuff/sub/" (10 bytes).
    let header = at - 10 - 30;
    assert_eq!(&raw[header..header + 4], b"PK\x03\x04");
    let flags = u16::from_le_bytes([raw[header + 6], raw[header + 7]]);
    assert_ne!(flags & (1 << 11), 0, "the UTF-8 flag must be set");
}

#[test]
fn a_folder_goes_into_a_tar_and_a_tgz_with_modes() {
    let tmp = TempDir::new().unwrap();
    let folder = tree(tmp.path());
    let out = tmp.path().join("out");
    for to in ["tar", "tgz"] {
        let archive = pack(&folder, "folder", to, &out);
        let entries = read_any_tar(&archive);
        let names: Vec<&str> = entries.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(names, TREE_NAMES, "{to}");
        assert_eq!(entries[0].2, 0o755);
        assert_eq!(entries[1].2, 0o644);
        assert_eq!(entries[3].1.as_deref(), Some(&b"bravo"[..]));
        assert_eq!(entries[4].1.as_deref(), Some(&b"hola"[..]));
    }
}

#[test]
fn a_long_name_survives_the_tar_through_pax() {
    // Over 100 bytes in one segment, so ustar's prefix/name split cannot save
    // it, yet short enough overall that Windows' MAX_PATH stays out of it.
    let tmp = TempDir::new().unwrap();
    let deep = tmp.path().join("deep");
    let long_dir = "b".repeat(101);
    let long_file = "c".repeat(110) + ".txt";
    fs::create_dir_all(deep.join(&long_dir)).unwrap();
    fs::write(deep.join(&long_file), b"long").unwrap();
    fs::write(deep.join(&long_dir).join("x.txt"), b"inside").unwrap();
    let out = tmp.path().join("out");
    for to in ["tar", "tgz"] {
        let entries = read_any_tar(&pack(&deep, "folder", to, &out));
        let names: Vec<String> = entries.iter().map(|(n, _, _)| n.clone()).collect();
        assert_eq!(
            names,
            [
                "deep/".to_owned(),
                format!("deep/{long_dir}/"),
                format!("deep/{long_dir}/x.txt"),
                format!("deep/{long_file}"),
            ],
            "{to}"
        );
        assert_eq!(entries[3].1.as_deref(), Some(&b"long"[..]));
        assert_eq!(entries[2].1.as_deref(), Some(&b"inside"[..]));
    }
}

#[test]
fn store_writes_stored_entries_and_maximum_deflates() {
    let tmp = TempDir::new().unwrap();
    let input = tmp.path().join("big.txt");
    fs::write(&input, "all work and no play ".repeat(1000)).unwrap();
    for (level, method) in [
        (ArchiveLevel::Store, zip::CompressionMethod::Stored),
        (ArchiveLevel::Fast, zip::CompressionMethod::Deflated),
        (ArchiveLevel::Maximum, zip::CompressionMethod::Deflated),
    ] {
        let out = tmp.path().join(format!("{level:?}"));
        let zip = pack_with(&input, "txt", "zip", level, &out, &CancelToken::new()).unwrap();
        let mut archive = zip::ZipArchive::new(File::open(&zip[0]).unwrap()).unwrap();
        let entry = archive.by_index(0).unwrap();
        assert_eq!(entry.compression(), method, "{level:?}");
        if level == ArchiveLevel::Store {
            assert_eq!(entry.compressed_size(), entry.size());
        } else {
            assert!(entry.compressed_size() < entry.size() / 10);
        }
    }
}

#[test]
fn store_gzip_is_bigger_than_maximum_gzip() {
    let tmp = TempDir::new().unwrap();
    let input = tmp.path().join("big.txt");
    fs::write(&input, "all work and no play ".repeat(1000)).unwrap();
    let size = |level| {
        let out = tmp.path().join(format!("{level:?}"));
        let tgz = pack_with(&input, "txt", "tgz", level, &out, &CancelToken::new()).unwrap();
        fs::metadata(&tgz[0]).unwrap().len()
    };
    assert!(size(ArchiveLevel::Store) > 21_000);
    assert!(size(ArchiveLevel::Maximum) < 2_000);
}

#[test]
fn an_existing_archive_is_replaced_whole_and_nothing_is_left_behind() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("out");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("sample.zip"), b"not a zip at all").unwrap();
    let zip = pack(&fixture("sample.txt"), "txt", "zip", &out);
    assert_eq!(read_zip(&zip).len(), 1);
    let left: Vec<_> = fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["sample.zip"], "no temp file may stay behind");
}

#[test]
fn a_cancelled_pack_leaves_nothing_and_keeps_the_old_archive() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("out");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("sample.tar"), b"precious").unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();
    let err = pack_with(
        &fixture("sample.txt"),
        "txt",
        "tar",
        ArchiveLevel::Normal,
        &out,
        &cancel,
    )
    .unwrap_err();
    assert!(matches!(err, EngineError::Cancelled), "{err}");
    assert_eq!(fs::read(out.join("sample.tar")).unwrap(), b"precious");
    assert_eq!(fs::read_dir(&out).unwrap().count(), 1);
}

#[test]
fn the_skip_policy_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let step = Step {
        input: fixture("sample.txt"),
        from: FormatId::new("txt"),
        to: FormatId::new("zip"),
        options: ConvertOptions::default(),
        work_dir: tmp.path().join("work"),
    };
    let outputs = ArchiveEngine::new()
        .convert(&step, &mut SkipAll, &|_| {}, &CancelToken::new())
        .unwrap();
    assert!(outputs.is_empty());
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn a_missing_input_is_an_error() {
    let tmp = TempDir::new().unwrap();
    let err = pack_with(
        &tmp.path().join("ghost.txt"),
        "txt",
        "zip",
        ArchiveLevel::Normal,
        tmp.path(),
        &CancelToken::new(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("was not found"), "{err}");
}

#[cfg(unix)]
#[test]
fn archives_are_readable_by_others_like_any_other_output() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let input = tmp.path().join("sample.txt");
    fs::copy(fixture("sample.txt"), &input).unwrap();
    // What File::create would give a fresh file here, umask and all.
    let probe = tmp.path().join("probe");
    File::create(&probe).unwrap();
    let expected = fs::metadata(&probe).unwrap().permissions().mode() & 0o777;
    let out = tmp.path().join("out");
    for to in ["zip", "tar", "tgz"] {
        let archive = pack(&input, "txt", to, &out);
        let mode = fs::metadata(&archive).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, expected, "{to}: {mode:o} vs {expected:o}");
    }
}

#[cfg(unix)]
#[test]
fn links_inside_a_folder_are_skipped() {
    let tmp = TempDir::new().unwrap();
    let folder = tmp.path().join("linked");
    fs::create_dir_all(&folder).unwrap();
    fs::write(tmp.path().join("secret.txt"), b"outside").unwrap();
    fs::write(folder.join("real.txt"), b"inside").unwrap();
    std::os::unix::fs::symlink(tmp.path().join("secret.txt"), folder.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(tmp.path(), folder.join("loop")).unwrap();
    let out = tmp.path().join("out");
    let zip = pack(&folder, "folder", "zip", &out);
    let names: Vec<String> = read_zip(&zip).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["linked/", "linked/real.txt"]);
    let tar = pack(&folder, "folder", "tar", &out);
    let names: Vec<String> = read_any_tar(&tar).into_iter().map(|(n, _, _)| n).collect();
    assert_eq!(names, ["linked/", "linked/real.txt"]);
}

#[test]
fn zip_to_tgz_routes_through_a_folder_in_run_job() {
    let tmp = TempDir::new().unwrap();
    let zip = tmp.path().join("sample.zip");
    fs::copy(fixture("sample.zip"), &zip).unwrap();

    let engine = ArchiveEngine::new();
    let engines: Vec<&dyn Converter> = vec![&engine];
    let results = run_job(
        std::slice::from_ref(&zip),
        Target::Format(FormatId::new("tgz")),
        &ConvertOptions::default(),
        &OutputRule::default(),
        &engines,
        &CancelToken::new(),
        &|_| {},
    );
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].state, FileState::Done, "{results:?}");
    assert_eq!(results[0].outputs.len(), 1, "{results:?}");
    let tgz = &results[0].outputs[0];
    assert!(tgz.to_string_lossy().ends_with(".tar.gz"), "{tgz:?}");

    // The unpacked folder goes in under its own name, so the files keep
    // their relative layout one level down.
    let entries = read_any_tar(tgz);
    let files: Vec<(&str, &[u8])> = entries
        .iter()
        .filter_map(|(n, d, _)| d.as_deref().map(|d| (n.as_str(), d)))
        .collect();
    let mut original = read_zip(&fixture("sample.zip"));
    original.retain(|(_, d)| d.is_some());
    assert_eq!(files.len(), original.len(), "{entries:?}");
    for (name, data) in &original {
        let found = files
            .iter()
            .find(|(n, _)| n.ends_with(&format!("/{name}")))
            .unwrap_or_else(|| panic!("{name} is missing from {entries:?}"));
        assert_eq!(found.1, data.as_deref().unwrap());
    }
}
