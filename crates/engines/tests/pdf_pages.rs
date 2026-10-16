//! Golden tests for the built-in PDF engine's page operations: page ranges,
//! splitting and the plain re-save. Everything is checked with lopdf itself,
//! so no external tool is needed and nothing is skipped.

#![cfg(feature = "pdf")]

mod common;

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use lopdf::{Document, EncryptionState, EncryptionVersion, Object, Permissions, StringFormat};
use morphring_core::{
    CancelToken, ConvertOptions, Converter, EngineError, FormatId, OutputAllocator, Step,
};
use morphring_engines::pdf::PdfEngine;
use tempfile::TempDir;

use common::fixture;

/// Hands out `<dir>/<stem><suffix>.<ext>`, the way the job runner names outputs.
struct DirAllocator {
    dir: PathBuf,
    stem: String,
}

impl OutputAllocator for DirAllocator {
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
        Some(self.dir.join(format!("{}{suffix}.{ext}", self.stem)))
    }
}

/// The skip policy with every output already taken.
struct SkipAll;

impl OutputAllocator for SkipAll {
    fn allocate(&mut self, _ext: &str, _suffix: &str) -> Option<PathBuf> {
        None
    }
}

struct Run {
    // Held so the folders outlive the assertions.
    _tmp: TempDir,
    out_dir: PathBuf,
    result: Result<Vec<PathBuf>, EngineError>,
    progress: Vec<f32>,
}

fn options(page_range: &str, split_pages: bool) -> ConvertOptions {
    let mut options = ConvertOptions::default();
    options.pdf.page_range = page_range.to_owned();
    options.pdf.split_pages = split_pages;
    options
}

fn step(tmp: &Path, input: PathBuf, options: ConvertOptions) -> Step {
    Step {
        input,
        from: FormatId::new("pdf"),
        to: FormatId::new("pdf"),
        options,
        work_dir: tmp.join("work"),
    }
}

/// Runs the engine on `input` with `options`, writing into a fresh folder.
fn run(input: PathBuf, options: ConvertOptions, cancel: &CancelToken) -> Run {
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("temp dir: {e}"));
    let out_dir = tmp.path().join("out");
    fs::create_dir(&out_dir).unwrap_or_else(|e| panic!("out dir: {e}"));
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| panic!("{} has no stem", input.display()));
    let mut alloc = DirAllocator {
        dir: out_dir.clone(),
        stem,
    };
    let progress = RefCell::new(Vec::new());
    let result = PdfEngine::new().convert(
        &step(tmp.path(), input, options),
        &mut alloc,
        &|p| progress.borrow_mut().push(p),
        cancel,
    );
    Run {
        _tmp: tmp,
        out_dir,
        result,
        progress: progress.into_inner(),
    }
}

fn convert(page_range: &str, split_pages: bool) -> Run {
    run(
        fixture("sample.pdf"),
        options(page_range, split_pages),
        &CancelToken::new(),
    )
}

fn page_count(path: &Path) -> usize {
    Document::load(path)
        .unwrap_or_else(|e| panic!("load {}: {e}", path.display()))
        .get_pages()
        .len()
}

fn file_names(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// The files actually sitting in `dir`, sorted.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("dir entry: {e}"))
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// Writes an RC4-128 encrypted copy of sample.pdf with the given passwords.
fn encrypted_sample(dir: &Path, user_password: &str) -> PathBuf {
    let mut doc = Document::load(fixture("sample.pdf")).unwrap_or_else(|e| panic!("load: {e}"));
    // The standard security handler mixes the file ID into the key, so a
    // file without one gets a made-up ID before we lock it.
    if doc.trailer.get(b"ID").is_err() {
        let id = Object::String(b"morphring-test-id".to_vec(), StringFormat::Literal);
        doc.trailer.set("ID", Object::Array(vec![id.clone(), id]));
    }
    let version = EncryptionVersion::V2 {
        document: &doc,
        owner_password: "owner-secret",
        user_password,
        key_length: 128,
        permissions: Permissions::all(),
    };
    let state = EncryptionState::try_from(version).unwrap_or_else(|e| panic!("state: {e}"));
    doc.encrypt(&state)
        .unwrap_or_else(|e| panic!("encrypt: {e}"));
    let path = dir.join("locked.pdf");
    doc.save(&path).unwrap_or_else(|e| panic!("save: {e}"));
    path
}

#[test]
fn the_fixture_has_three_pages() {
    // Every count below leans on this, so pin it first.
    assert_eq!(page_count(&fixture("sample.pdf")), 3);
}

#[test]
fn a_page_range_keeps_only_those_pages() {
    let run = convert("2-3", false);
    let outputs = run.result.unwrap_or_else(|e| panic!("convert: {e}"));
    assert_eq!(file_names(&outputs), ["sample.pdf"]);
    assert_eq!(page_count(&outputs[0]), 2);
    assert_eq!(run.progress.last(), Some(&1.0));
}

#[test]
fn a_blank_range_re_saves_every_page() {
    let run = convert("  ", false);
    let outputs = run.result.unwrap_or_else(|e| panic!("convert: {e}"));
    assert_eq!(file_names(&outputs), ["sample.pdf"]);
    assert_eq!(page_count(&outputs[0]), 3);
}

#[test]
fn a_range_with_no_valid_page_keeps_every_page() {
    // PageRange.Parse's rule: nothing valid means everything.
    let run = convert("42-99", false);
    let outputs = run.result.unwrap_or_else(|e| panic!("convert: {e}"));
    assert_eq!(page_count(&outputs[0]), 3);
}

#[test]
fn split_writes_one_file_per_page() {
    let run = convert("", true);
    let outputs = run.result.unwrap_or_else(|e| panic!("convert: {e}"));
    assert_eq!(
        file_names(&outputs),
        ["sample_p1.pdf", "sample_p2.pdf", "sample_p3.pdf"]
    );
    for path in &outputs {
        assert_eq!(page_count(path), 1, "{}", path.display());
    }
    assert_eq!(listing(&run.out_dir), file_names(&outputs));
    assert_eq!(run.progress.len(), 3);
    assert!(run.progress.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(run.progress.last(), Some(&1.0));
}

#[test]
fn split_keeps_each_page_s_own_content() {
    // Splitting must not hand every file page 1: page n's content stream
    // has to be the source's page n's.
    let source = Document::load(fixture("sample.pdf")).unwrap_or_else(|e| panic!("load: {e}"));
    let source_pages = source.get_pages();
    let run = convert("", true);
    let outputs = run.result.unwrap_or_else(|e| panic!("convert: {e}"));
    for (number, path) in (1u32..).zip(&outputs) {
        let doc = Document::load(path).unwrap_or_else(|e| panic!("load: {e}"));
        let page = doc.get_pages()[&1];
        let expected = source.get_page_content(source_pages[&number]);
        let actual = doc.get_page_content(page);
        assert_eq!(actual, expected, "page {number}");
    }
}

#[test]
fn split_honours_the_page_range_and_names_by_source_page() {
    let run = convert("3,1", true);
    let outputs = run.result.unwrap_or_else(|e| panic!("convert: {e}"));
    assert_eq!(file_names(&outputs), ["sample_p1.pdf", "sample_p3.pdf"]);
}

#[test]
fn skipped_outputs_write_nothing() {
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("temp dir: {e}"));
    for split in [false, true] {
        let result = PdfEngine::new().convert(
            &step(tmp.path(), fixture("sample.pdf"), options("", split)),
            &mut SkipAll,
            &|_| {},
            &CancelToken::new(),
        );
        assert_eq!(
            result.unwrap_or_else(|e| panic!("convert: {e}")),
            Vec::<PathBuf>::new()
        );
    }
}

#[test]
fn a_cancelled_job_leaves_nothing_behind() {
    let cancel = CancelToken::new();
    cancel.cancel();
    for split in [false, true] {
        let run = run(fixture("sample.pdf"), options("", split), &cancel);
        assert!(matches!(run.result, Err(EngineError::Cancelled)));
        assert!(listing(&run.out_dir).is_empty());
    }
}

#[test]
fn the_source_is_never_modified() {
    let before = fs::read(fixture("sample.pdf")).unwrap_or_else(|e| panic!("read: {e}"));
    let _ = convert("2", false);
    let _ = convert("", true);
    let after = fs::read(fixture("sample.pdf")).unwrap_or_else(|e| panic!("read: {e}"));
    assert!(before == after, "sample.pdf changed");
}

#[test]
fn a_password_protected_pdf_fails_clearly() {
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("temp dir: {e}"));
    let locked = encrypted_sample(tmp.path(), "user-secret");
    let run = run(locked, options("", false), &CancelToken::new());
    let err = run.result.expect_err("an encrypted PDF must not convert");
    assert!(err.to_string().contains("password-protected"), "{err}");
    assert!(listing(&run.out_dir).is_empty());
}

#[test]
fn an_owner_locked_pdf_fails_clearly() {
    // An empty user password opens without asking, but saving it would
    // quietly drop the owner's restrictions.
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("temp dir: {e}"));
    let locked = encrypted_sample(tmp.path(), "");
    let run = run(locked, options("", true), &CancelToken::new());
    let err = run.result.expect_err("an encrypted PDF must not convert");
    assert!(err.to_string().contains("encrypted"), "{err}");
    assert!(listing(&run.out_dir).is_empty());
}

#[test]
fn a_file_that_is_not_a_pdf_fails_clearly() {
    let run = run(
        fixture("sample.txt"),
        options("", false),
        &CancelToken::new(),
    );
    let err = run.result.expect_err("text is not a PDF");
    assert!(
        err.to_string().starts_with("The PDF could not be read"),
        "{err}"
    );
}
