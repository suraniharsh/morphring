//! Golden and hostile-archive tests for unpacking ZIP, TAR and TAR.GZ into a
//! folder. Every hostile archive is built right here with the zip and tar
//! writers (plus a little byte surgery where the writers are too polite to
//! lie), so there are no scary binaries in the fixtures folder.

#![cfg(feature = "archive")]

mod common;

use std::cell::RefCell;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use morphring_core::{
    CancelToken, ConvertOptions, Converter, EngineError, FormatId, OutputAllocator, Step,
};
use morphring_engines::archive::ArchiveEngine;
use tempfile::TempDir;

use common::fixture;

/// Hands out `<dir>/<name>` for a folder, the way the job runner would.
struct FolderAllocator {
    path: PathBuf,
}

impl OutputAllocator for FolderAllocator {
    fn allocate(&mut self, ext: &str, _suffix: &str) -> Option<PathBuf> {
        assert_eq!(ext, "", "the folder output has no extension");
        Some(self.path.clone())
    }
}

/// The skip policy with the output already taken.
struct SkipAll;

impl OutputAllocator for SkipAll {
    fn allocate(&mut self, _ext: &str, _suffix: &str) -> Option<PathBuf> {
        None
    }
}

/// A sandbox: `root/in` holds inputs, `root/out/<name>` is the output and
/// `root/outside` is where an escaping entry would like to go.
struct Sandbox {
    tmp: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("temp dir: {e}"));
        for dir in ["in", "out", "outside"] {
            fs::create_dir(tmp.path().join(dir)).unwrap_or_else(|e| panic!("{dir}: {e}"));
        }
        Self { tmp }
    }

    fn root(&self) -> &Path {
        self.tmp.path()
    }

    fn input(&self, name: &str) -> PathBuf {
        self.root().join("in").join(name)
    }

    fn output(&self) -> PathBuf {
        self.root().join("out").join("result")
    }

    fn outside(&self) -> PathBuf {
        self.root().join("outside")
    }

    /// Unpacks `input` (format `from`) into [`Self::output`].
    fn run(&self, input: &Path, from: &'static str) -> Result<Vec<PathBuf>, EngineError> {
        self.run_with(input, from, &CancelToken::new(), &|_| {})
    }

    fn run_with(
        &self,
        input: &Path,
        from: &'static str,
        cancel: &CancelToken,
        progress: &dyn Fn(f32),
    ) -> Result<Vec<PathBuf>, EngineError> {
        let step = step(self.root(), input, from);
        let mut alloc = FolderAllocator {
            path: self.output(),
        };
        ArchiveEngine::new().convert(&step, &mut alloc, progress, cancel)
    }

    /// Every file and folder under the sandbox except the inputs, relative
    /// and with `/` separators, sorted.
    fn tree(&self) -> Vec<String> {
        let mut found = Vec::new();
        walk(self.root(), self.root(), &mut found);
        found.retain(|p| p != "in" && !p.starts_with("in/"));
        found.sort();
        found
    }

    /// Asserts nothing escaped: `outside` is empty and nothing but the output
    /// sits in `out` (no leftover staging folder either).
    fn assert_contained(&self) {
        let outside: Vec<_> = fs::read_dir(self.outside())
            .unwrap_or_else(|e| panic!("{e}"))
            .collect();
        assert!(outside.is_empty(), "something escaped: {:?}", self.tree());
        for entry in fs::read_dir(self.root().join("out")).unwrap_or_else(|e| panic!("{e}")) {
            let entry = entry.unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(
                entry.path(),
                self.output(),
                "unexpected leftover: {:?}",
                self.tree()
            );
        }
        // And nothing new appeared next to the sandbox folders.
        for entry in fs::read_dir(self.root()).unwrap_or_else(|e| panic!("{e}")) {
            let name = entry
                .unwrap_or_else(|e| panic!("{e}"))
                .file_name()
                .to_string_lossy()
                .into_owned();
            assert!(["in", "out", "outside"].contains(&name.as_str()), "{name}");
        }
    }
}

fn walk(root: &Path, dir: &Path, found: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("{e}")) {
        let entry = entry.unwrap_or_else(|e| panic!("{e}"));
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap_or_else(|e| panic!("{e}"))
            .to_string_lossy()
            .replace('\\', "/");
        found.push(rel);
        if entry.file_type().unwrap_or_else(|e| panic!("{e}")).is_dir() {
            walk(root, &path, found);
        }
    }
}

fn step(tmp: &Path, input: &Path, from: &'static str) -> Step {
    Step {
        input: input.to_path_buf(),
        from: FormatId::new(from),
        to: FormatId::new("folder"),
        options: ConvertOptions::default(),
        work_dir: tmp.join("work-not-used"),
    }
}

fn message(result: Result<Vec<PathBuf>, EngineError>) -> String {
    match result {
        Ok(paths) => panic!("expected an error, got {paths:?}"),
        Err(e) => e.to_string(),
    }
}

/// A ZIP with `entries` as (name, contents), stored so the bytes are easy to
/// find for the tests that tamper with them.
fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap_or_else(|e| panic!("{e}")));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in entries {
        if name.ends_with('/') {
            zip.add_directory(*name, options)
                .unwrap_or_else(|e| panic!("{e}"));
        } else {
            zip.start_file(*name, options)
                .unwrap_or_else(|e| panic!("{e}"));
            zip.write_all(data).unwrap_or_else(|e| panic!("{e}"));
        }
    }
    zip.finish().unwrap_or_else(|e| panic!("{e}"));
}

/// One raw TAR entry: the name goes into the header byte for byte, so the
/// builder's "no absolute paths, no .." manners do not get in the way.
enum TarEntry<'a> {
    File(&'a str, &'a [u8]),
    Dir(&'a str),
    Symlink(&'a str, &'a str),
    HardLink(&'a str, &'a str),
}

fn write_tar(path: &Path, entries: &[TarEntry<'_>]) {
    let mut tar = tar::Builder::new(File::create(path).unwrap_or_else(|e| panic!("{e}")));
    for entry in entries {
        let mut header = tar::Header::new_gnu();
        let (name, data, kind, link): (&str, &[u8], tar::EntryType, Option<&str>) = match entry {
            TarEntry::File(name, data) => (name, data, tar::EntryType::Regular, None),
            TarEntry::Dir(name) => (name, &[], tar::EntryType::Directory, None),
            TarEntry::Symlink(name, target) => (name, &[], tar::EntryType::Symlink, Some(target)),
            TarEntry::HardLink(name, target) => (name, &[], tar::EntryType::Link, Some(target)),
        };
        let raw = &mut header.as_old_mut().name;
        raw[..name.len()].copy_from_slice(name.as_bytes());
        if let Some(target) = link {
            let raw = &mut header.as_old_mut().linkname;
            raw[..target.len()].copy_from_slice(target.as_bytes());
        }
        header.set_entry_type(kind);
        header.set_mode(0o644);
        header.set_size(data.len() as u64);
        header.set_cksum();
        tar.append(&header, data).unwrap_or_else(|e| panic!("{e}"));
    }
    tar.finish().unwrap_or_else(|e| panic!("{e}"));
}

/// Offsets of every `signature` in `bytes`.
fn find_all(bytes: &[u8], signature: &[u8; 4]) -> Vec<usize> {
    bytes
        .windows(4)
        .enumerate()
        .filter(|(_, w)| w == signature)
        .map(|(i, _)| i)
        .collect()
}

const LOCAL: &[u8; 4] = b"PK\x03\x04";
const CENTRAL: &[u8; 4] = b"PK\x01\x02";

// --- golden -----------------------------------------------------------------

#[test]
fn sample_zip_unpacks_to_its_two_files() {
    let sandbox = Sandbox::new();
    let written = sandbox.run(&fixture("sample.zip"), "zip").unwrap();
    assert_eq!(written, [sandbox.output()]);
    assert_eq!(
        sandbox.tree(),
        [
            "out",
            "out/result",
            "out/result/dir",
            "out/result/dir/inner.txt",
            "out/result/hello.txt",
            "outside"
        ]
    );
    let hello = fs::read_to_string(sandbox.output().join("hello.txt")).unwrap();
    assert!(!hello.is_empty());
    sandbox.assert_contained();
}

#[test]
fn sample_tar_gz_unpacks_to_the_same_two_files() {
    let sandbox = Sandbox::new();
    sandbox.run(&fixture("sample.tar.gz"), "tgz").unwrap();
    assert!(sandbox.output().join("hello.txt").is_file());
    assert!(sandbox.output().join("dir").join("inner.txt").is_file());
    let from_zip = Sandbox::new();
    from_zip.run(&fixture("sample.zip"), "zip").unwrap();
    for name in ["hello.txt", "dir/inner.txt"] {
        assert_eq!(
            fs::read(sandbox.output().join(name)).unwrap(),
            fs::read(from_zip.output().join(name)).unwrap(),
            "{name}"
        );
    }
    sandbox.assert_contained();
}

#[test]
fn a_plain_tar_unpacks_too() {
    let sandbox = Sandbox::new();
    let input = sandbox.input("plain.tar");
    write_tar(
        &input,
        &[
            TarEntry::Dir("docs/"),
            TarEntry::File("docs/a.txt", b"alpha"),
            TarEntry::File("b.txt", b"beta"),
        ],
    );
    sandbox.run(&input, "tar").unwrap();
    assert_eq!(
        fs::read(sandbox.output().join("docs").join("a.txt")).unwrap(),
        b"alpha"
    );
    assert_eq!(fs::read(sandbox.output().join("b.txt")).unwrap(), b"beta");
    sandbox.assert_contained();
}

#[test]
fn a_single_top_folder_is_not_doubled() {
    let sandbox = Sandbox::new();
    let input = sandbox.input("photos.zip");
    write_zip(
        &input,
        &[
            ("photos/", b""),
            ("photos/one.jpg", b"1"),
            ("photos/two.jpg", b"2"),
        ],
    );
    sandbox.run(&input, "zip").unwrap();
    assert!(sandbox.output().join("one.jpg").is_file());
    assert!(!sandbox.output().join("photos").exists());
    sandbox.assert_contained();
}

#[test]
fn an_existing_folder_is_merged_into() {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.output().join("dir")).unwrap();
    fs::write(sandbox.output().join("keep.txt"), "mine").unwrap();
    fs::write(sandbox.output().join("hello.txt"), "old").unwrap();
    sandbox.run(&fixture("sample.zip"), "zip").unwrap();
    assert_eq!(
        fs::read_to_string(sandbox.output().join("keep.txt")).unwrap(),
        "mine"
    );
    assert_ne!(
        fs::read_to_string(sandbox.output().join("hello.txt")).unwrap(),
        "old"
    );
    assert!(sandbox.output().join("dir").join("inner.txt").is_file());
    sandbox.assert_contained();
}

#[test]
fn a_file_in_the_way_of_the_folder_is_an_error() {
    let sandbox = Sandbox::new();
    fs::write(sandbox.output(), "squatter").unwrap();
    let err = message(sandbox.run(&fixture("sample.zip"), "zip"));
    assert_eq!(
        err,
        "A file named result is in the way of the extracted folder."
    );
    assert_eq!(fs::read_to_string(sandbox.output()).unwrap(), "squatter");
    sandbox.assert_contained();
}

#[test]
fn the_skip_policy_writes_nothing() {
    let sandbox = Sandbox::new();
    let step = step(sandbox.root(), &fixture("sample.zip"), "zip");
    let written = ArchiveEngine::new()
        .convert(&step, &mut SkipAll, &|_| {}, &CancelToken::new())
        .unwrap();
    assert!(written.is_empty());
    assert_eq!(sandbox.tree(), ["out", "outside"]);
}

#[test]
fn progress_only_goes_up_and_ends_at_one() {
    let sandbox = Sandbox::new();
    let seen = RefCell::new(Vec::new());
    sandbox
        .run_with(&fixture("sample.zip"), "zip", &CancelToken::new(), &|p| {
            seen.borrow_mut().push(p)
        })
        .unwrap();
    let seen = seen.into_inner();
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    assert_eq!(seen.last().copied(), Some(1.0));
}

#[test]
fn a_cancelled_job_leaves_nothing_behind() {
    let sandbox = Sandbox::new();
    let cancel = CancelToken::new();
    cancel.cancel();
    let result = sandbox.run_with(&fixture("sample.tar.gz"), "tgz", &cancel, &|_| {});
    assert!(matches!(result, Err(EngineError::Cancelled)), "{result:?}");
    assert_eq!(sandbox.tree(), ["out", "outside"]);
}

// --- hostile ----------------------------------------------------------------

#[test]
fn a_zip_climbing_out_with_dotdot_is_refused_whole() {
    let sandbox = Sandbox::new();
    let input = sandbox.input("evil.zip");
    write_zip(
        &input,
        &[("fine.txt", b"ok"), ("../outside/evil.txt", b"gotcha")],
    );
    let err = message(sandbox.run(&input, "zip"));
    assert_eq!(
        err,
        "evil.zip was not unpacked: the entry \"../outside/evil.txt\" points outside the target folder."
    );
    assert_eq!(sandbox.tree(), ["out", "outside"]);
}

#[test]
fn a_tar_climbing_out_with_dotdot_is_refused() {
    let sandbox = Sandbox::new();
    let input = sandbox.input("evil.tar");
    write_tar(
        &input,
        &[
            TarEntry::File("fine.txt", b"ok"),
            TarEntry::File("../outside/evil.txt", b"gotcha"),
        ],
    );
    let err = message(sandbox.run(&input, "tar"));
    assert!(err.contains("points outside the target folder"), "{err}");
    // The half that did unpack went with the staging folder.
    assert_eq!(sandbox.tree(), ["out", "outside"]);
}

#[test]
fn absolute_names_are_refused_in_both_formats() {
    let target = Sandbox::new().outside().join("abs.txt");
    let absolute = target.to_string_lossy().replace('\\', "/");
    for name in ["/etc/morphring-abs.txt", absolute.as_str()] {
        let sandbox = Sandbox::new();
        let zip = sandbox.input("abs.zip");
        write_zip(&zip, &[(name, b"x")]);
        let err = message(sandbox.run(&zip, "zip"));
        assert!(err.contains("points outside"), "{name}: {err}");

        let tar = sandbox.input("abs.tar");
        write_tar(&tar, &[TarEntry::File(name, b"x")]);
        let err = message(sandbox.run(&tar, "tar"));
        assert!(err.contains("points outside"), "{name}: {err}");
        sandbox.assert_contained();
        assert_eq!(sandbox.tree(), ["out", "outside"]);
    }
}

#[test]
fn drive_letter_names_are_refused_on_every_os() {
    let sandbox = Sandbox::new();
    let zip = sandbox.input("drive.zip");
    write_zip(&zip, &[("C:\\x", b"x")]);
    let err = message(sandbox.run(&zip, "zip"));
    assert!(err.contains("\"C:\\x\" points outside"), "{err}");

    let tar = sandbox.input("drive.tar");
    write_tar(&tar, &[TarEntry::File("c:/x", b"x")]);
    let err = message(sandbox.run(&tar, "tar"));
    assert!(err.contains("points outside"), "{err}");
    assert_eq!(sandbox.tree(), ["out", "outside"]);
}

#[test]
fn a_symlink_then_a_file_through_it_stays_inside() {
    let sandbox = Sandbox::new();
    let outside = sandbox.outside().to_string_lossy().replace('\\', "/");
    let input = sandbox.input("link.tar");
    write_tar(
        &input,
        &[
            TarEntry::Symlink("link", &outside),
            TarEntry::Symlink("up", "../../outside"),
            TarEntry::File("link/evil.txt", b"gotcha"),
            TarEntry::File("up/evil2.txt", b"gotcha"),
        ],
    );
    sandbox.run(&input, "tar").unwrap();
    // The links were skipped, so the files landed in plain folders inside.
    let link = sandbox.output().join("link");
    assert!(!link.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(link.join("evil.txt").is_file());
    assert!(sandbox.output().join("up").join("evil2.txt").is_file());
    sandbox.assert_contained();
}

#[test]
fn a_hard_link_out_of_the_tree_is_refused_and_the_victim_untouched() {
    let sandbox = Sandbox::new();
    let victim = sandbox.outside().join("victim.txt");
    fs::write(&victim, "precious").unwrap();
    let input = sandbox.input("hard.tar");
    let target = victim.to_string_lossy().replace('\\', "/");
    write_tar(
        &input,
        &[
            TarEntry::File("real.txt", b"ok"),
            TarEntry::HardLink("hl", &target),
        ],
    );
    let err = message(sandbox.run(&input, "tar"));
    assert_eq!(
        err,
        format!("hard.tar has a link 'hl' to '{target}', which is not in the archive.")
    );
    // Not even real.txt, which came first: the half-done staging is binned.
    assert_eq!(sandbox.tree(), ["out", "outside", "outside/victim.txt"]);
    assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
}

/// What plain GNU tar writes for `report.txt` with a second hard-linked name:
/// the first name it meets carries the bytes, the second is a link entry.
fn write_hard_linked_tar(path: &Path) {
    write_tar(
        path,
        &[
            TarEntry::Dir("hl/"),
            TarEntry::Dir("hl/docs/"),
            TarEntry::File("hl/docs/report-copy.txt", b"important\n"),
            TarEntry::HardLink("hl/docs/report.txt", "hl/docs/report-copy.txt"),
        ],
    );
}

#[test]
fn a_hard_link_to_an_earlier_file_unpacks_as_a_copy() {
    let sandbox = Sandbox::new();
    let input = sandbox.input("hardlinked.tar");
    write_hard_linked_tar(&input);
    sandbox.run(&input, "tar").unwrap();

    assert_eq!(
        sandbox.tree(),
        [
            "out",
            "out/result",
            "out/result/docs",
            "out/result/docs/report-copy.txt",
            "out/result/docs/report.txt",
            "outside",
        ]
    );
    let docs = sandbox.output().join("docs");
    assert_eq!(fs::read(docs.join("report.txt")).unwrap(), b"important\n");
    assert_eq!(
        fs::read(docs.join("report-copy.txt")).unwrap(),
        b"important\n"
    );
    // A copy, not a real link: writing one name must not touch the other.
    fs::write(docs.join("report.txt"), "edited").unwrap();
    assert_eq!(
        fs::read(docs.join("report-copy.txt")).unwrap(),
        b"important\n"
    );
    sandbox.assert_contained();
}

/// Hands out `<dir>/<stem>.<ext>` for an archive file.
struct FileAllocator {
    dir: PathBuf,
    stem: &'static str,
}

impl OutputAllocator for FileAllocator {
    fn allocate(&mut self, ext: &str, _suffix: &str) -> Option<PathBuf> {
        Some(self.dir.join(format!("{}.{ext}", self.stem)))
    }
}

#[test]
fn a_hard_linked_tar_repacked_to_zip_keeps_both_files() {
    let sandbox = Sandbox::new();
    let input = sandbox.input("hardlinked.tar");
    write_hard_linked_tar(&input);
    // tar -> zip is tar -> folder -> zip, so the unpack is where files go missing.
    sandbox.run(&input, "tar").unwrap();
    let pack = Step {
        input: sandbox.output(),
        from: FormatId::new("folder"),
        to: FormatId::new("zip"),
        options: ConvertOptions::default(),
        work_dir: sandbox.root().join("work-not-used"),
    };
    let mut alloc = FileAllocator {
        dir: sandbox.root().join("in"),
        stem: "repacked",
    };
    let zips = ArchiveEngine::new()
        .convert(&pack, &mut alloc, &|_| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(zips.len(), 1);

    let mut zip = zip::ZipArchive::new(File::open(&zips[0]).unwrap()).unwrap();
    let mut files: Vec<(String, String)> = Vec::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).unwrap();
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().unwrap().into_owned();
        let mut text = String::new();
        std::io::Read::read_to_string(&mut entry, &mut text).unwrap();
        files.push((name, text));
    }
    files.sort();
    assert_eq!(files.len(), 2, "{files:?}");
    assert!(files[0].0.ends_with("docs/report-copy.txt"), "{files:?}");
    assert!(files[1].0.ends_with("docs/report.txt"), "{files:?}");
    assert!(files.iter().all(|(_, text)| text == "important\n"));
}

#[test]
fn a_hard_link_climbing_out_with_dotdot_is_refused() {
    let sandbox = Sandbox::new();
    let input = sandbox.input("climb.tar");
    write_tar(
        &input,
        &[
            TarEntry::File("a.txt", b"a"),
            TarEntry::HardLink("b.txt", "../outside"),
        ],
    );
    let err = message(sandbox.run(&input, "tar"));
    assert_eq!(
        err,
        "climb.tar has a link 'b.txt' to '../outside', which is not in the archive."
    );
    assert_eq!(sandbox.tree(), ["out", "outside"]);
}

#[test]
fn a_hard_link_to_a_missing_or_later_entry_is_refused() {
    let sandbox = Sandbox::new();
    let input = sandbox.input("dangling.tar");
    write_tar(
        &input,
        &[
            TarEntry::File("a.txt", b"a"),
            TarEntry::HardLink("b.txt", "nowhere.txt"),
        ],
    );
    let err = message(sandbox.run(&input, "tar"));
    assert_eq!(
        err,
        "dangling.tar has a link 'b.txt' to 'nowhere.txt', which is not in the archive."
    );
    assert_eq!(sandbox.tree(), ["out", "outside"]);

    // A target that only shows up after the link is just as missing when the
    // link is written, since TAR streams strictly front to back.
    write_tar(
        &input,
        &[
            TarEntry::HardLink("b.txt", "a.txt"),
            TarEntry::File("a.txt", b"a"),
        ],
    );
    let err = message(sandbox.run(&input, "tar"));
    assert_eq!(
        err,
        "dangling.tar has a link 'b.txt' to 'a.txt', which is not in the archive."
    );
    assert_eq!(sandbox.tree(), ["out", "outside"]);

    // A folder is not a file to copy, even when the archive made it.
    write_tar(
        &input,
        &[TarEntry::Dir("d/"), TarEntry::HardLink("b.txt", "d")],
    );
    let err = message(sandbox.run(&input, "tar"));
    assert_eq!(
        err,
        "dangling.tar has a link 'b.txt' to 'd', which is not in the archive."
    );
    assert_eq!(sandbox.tree(), ["out", "outside"]);
}

#[test]
fn zip_symlinks_are_skipped() {
    let sandbox = Sandbox::new();
    let input = sandbox.input("link.zip");
    let mut zip = zip::ZipWriter::new(File::create(&input).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    zip.add_symlink("link", "../../outside", options).unwrap();
    zip.start_file("link/evil.txt", options).unwrap();
    zip.write_all(b"gotcha").unwrap();
    zip.start_file("real.txt", options).unwrap();
    zip.write_all(b"ok").unwrap();
    zip.finish().unwrap();

    sandbox.run(&input, "zip").unwrap();
    let link = sandbox.output().join("link");
    assert!(!link.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(link.join("evil.txt").is_file());
    assert!(sandbox.output().join("real.txt").is_file());
    sandbox.assert_contained();
}

#[test]
fn a_zip_entry_lying_about_its_size_is_refused() {
    let sandbox = Sandbox::new();
    let input = sandbox.input("liar.zip");
    let data = vec![b'a'; 4096];
    write_zip(&input, &[("big.txt", &data)]);

    // Claim 16 bytes in both headers; the 4096 real ones still follow.
    let mut bytes = fs::read(&input).unwrap();
    for at in find_all(&bytes, LOCAL) {
        bytes[at + 22..at + 26].copy_from_slice(&16u32.to_le_bytes());
    }
    for at in find_all(&bytes, CENTRAL) {
        bytes[at + 24..at + 28].copy_from_slice(&16u32.to_le_bytes());
    }
    fs::write(&input, bytes).unwrap();

    let err = message(sandbox.run(&input, "zip"));
    assert!(err.contains("liar.zip is damaged or incomplete"), "{err}");
    assert_eq!(sandbox.tree(), ["out", "outside"]);
}

#[test]
fn an_encrypted_zip_entry_is_refused_with_filees_message() {
    let sandbox = Sandbox::new();
    let input = sandbox.input("secret.zip");
    write_zip(&input, &[("plain.txt", b"ok"), ("secret.txt", b"shh")]);

    // Flip the "encrypted" bit on the second entry, in both headers.
    let mut bytes = fs::read(&input).unwrap();
    let local = find_all(&bytes, LOCAL)[1];
    bytes[local + 6] |= 1;
    let central = find_all(&bytes, CENTRAL)[1];
    bytes[central + 8] |= 1;
    fs::write(&input, bytes).unwrap();

    let err = message(sandbox.run(&input, "zip"));
    assert_eq!(
        err,
        "secret.zip is password-protected. MorphRing cannot unpack encrypted archives."
    );
    assert_eq!(sandbox.tree(), ["out", "outside"]);
}

#[test]
fn garbage_is_reported_as_damaged() {
    let sandbox = Sandbox::new();
    let input = sandbox.input("junk.zip");
    fs::write(&input, b"this is not a zip file at all").unwrap();
    let err = message(sandbox.run(&input, "zip"));
    assert!(
        err.starts_with("junk.zip is damaged or incomplete ("),
        "{err}"
    );

    let input = sandbox.input("junk.tar.gz");
    fs::write(&input, b"not gzip either").unwrap();
    let err = message(sandbox.run(&input, "tgz"));
    assert!(
        err.starts_with("junk.tar.gz is damaged or incomplete ("),
        "{err}"
    );
    assert_eq!(sandbox.tree(), ["out", "outside"]);
}

#[test]
fn windows_hostile_names_are_made_safe() {
    let sandbox = Sandbox::new();
    let input = sandbox.input("names.zip");
    write_zip(
        &input,
        &[("CON.txt", b"1"), ("what?.txt", b"2"), ("a/b", b"3")],
    );
    sandbox.run(&input, "zip").unwrap();
    assert!(sandbox.output().join("_CON.txt").is_file());
    assert!(sandbox.output().join("what_.txt").is_file());
    assert!(sandbox.output().join("a").join("b").is_file());
    sandbox.assert_contained();
}

// --- damage found mid-entry and file/folder clashes (FX-13) -------------------

/// Bytes that refuse to compress (a little xorshift), so a cut through the
/// gzip lands squarely inside an entry instead of in a tidy header.
fn noise(len: usize, mut seed: u64) -> Vec<u8> {
    (0..len)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        })
        .collect()
}

#[test]
fn a_tgz_cut_off_mid_entry_is_damaged_not_a_codec_error() {
    let sandbox = Sandbox::new();
    let tar = sandbox.input("multi.tar");
    let (one, two) = (noise(300_000, 1), noise(300_000, 2));
    write_tar(
        &tar,
        &[
            TarEntry::Dir("folder one/"),
            TarEntry::File("folder one/a.bin", &one),
            TarEntry::File("rose.png", &two),
        ],
    );
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&fs::read(&tar).unwrap()).unwrap();
    let full = gz.finish().unwrap();
    fs::remove_file(&tar).unwrap();

    for cut in [1000, 2000, 5000, 7000] {
        let input = sandbox.input("tr.tgz");
        fs::write(&input, &full[..cut]).unwrap();
        let err = message(sandbox.run(&input, "tgz"));
        assert!(
            err.starts_with("tr.tgz is damaged or incomplete ("),
            "cut at {cut}: {err}"
        );
        assert_eq!(sandbox.tree(), ["out", "outside"], "cut at {cut}");
    }
}

#[test]
fn a_zip_with_corrupt_deflate_data_is_damaged_not_a_codec_error() {
    let sandbox = Sandbox::new();
    let input = sandbox.input("bent.zip");
    let text = "the quick brown fox jumps over the lazy dog. ".repeat(2000);
    {
        let mut zip = zip::ZipWriter::new(File::create(&input).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("fox.txt", options).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
        zip.finish().unwrap();
    }

    // Flip a byte a little way into the compressed data (local header is 30
    // bytes plus the name).
    let mut bytes = fs::read(&input).unwrap();
    let data = find_all(&bytes, LOCAL)[0] + 30 + "fox.txt".len();
    bytes[data + 3] ^= 0xFF;
    fs::write(&input, bytes).unwrap();

    let err = message(sandbox.run(&input, "zip"));
    assert!(
        err.starts_with("bent.zip is damaged or incomplete ("),
        "{err}"
    );
    assert_eq!(sandbox.tree(), ["out", "outside"]);
}

#[test]
fn a_tar_with_a_file_then_a_folder_of_the_same_name_names_the_clash() {
    let sandbox = Sandbox::new();
    let input = sandbox.input("filedir.tar");
    write_tar(
        &input,
        &[TarEntry::File("x", b"a"), TarEntry::File("x/y", b"b")],
    );
    let err = message(sandbox.run(&input, "tar"));
    assert_eq!(err, "filedir.tar has both a file and a folder called 'x'.");
    assert_eq!(sandbox.tree(), ["out", "outside"]);

    // The same squatter, this time with an explicit folder entry and a
    // deeper path, still gets named rather than errno 17'd.
    write_tar(
        &input,
        &[TarEntry::File("a/b", b"a"), TarEntry::Dir("a/b/c/")],
    );
    let err = message(sandbox.run(&input, "tar"));
    assert_eq!(
        err,
        "filedir.tar has both a file and a folder called 'a/b'."
    );
    assert_eq!(sandbox.tree(), ["out", "outside"]);
}

#[test]
fn a_zip_with_a_file_then_a_folder_of_the_same_name_names_the_clash() {
    let sandbox = Sandbox::new();
    let input = sandbox.input("filedir.zip");
    write_zip(&input, &[("x", b"a"), ("x/y", b"b")]);
    let err = message(sandbox.run(&input, "zip"));
    assert_eq!(err, "filedir.zip has both a file and a folder called 'x'.");
    assert_eq!(sandbox.tree(), ["out", "outside"]);
}
