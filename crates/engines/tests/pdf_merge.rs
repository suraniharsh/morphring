//! Merging PDFs with the built-in PDF engine's merger (lopdf), on its own and
//! through `run_job` with the merge option on.
//!
//! Everything here is pure Rust; pdfinfo, when installed, gets a second
//! opinion on the merged file.

#![cfg(feature = "pdf")]

mod common;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lopdf::{Document, EncryptionState, EncryptionVersion, Object, Permissions, dictionary};
use morphring_core::job::run_job;
use morphring_core::output::OutputRule;
use morphring_core::{
    CancelToken, ConvertOptions, Converter, EngineError, FileState, FormatId, Merger,
    OutputAllocator, Target,
};
use morphring_engines::pdf::PdfEngine;
use tempfile::TempDir;

use common::{engine_available, fixture, pdf_page_count};

/// Hands out `<dir>/merged<suffix>.<ext>`.
struct DirAllocator {
    dir: PathBuf,
}

impl OutputAllocator for DirAllocator {
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
        Some(self.dir.join(format!("merged{suffix}.{ext}")))
    }
}

/// The skip policy with every output already taken.
struct SkipAll;

impl OutputAllocator for SkipAll {
    fn allocate(&mut self, _ext: &str, _suffix: &str) -> Option<PathBuf> {
        None
    }
}

fn merger() -> &'static dyn Merger {
    // A const-promoted unit struct, so the 'static comes for free.
    const ENGINE: &PdfEngine = &PdfEngine;
    ENGINE
        .merger()
        .unwrap_or_else(|| panic!("the PDF engine can merge"))
}

struct Run {
    tmp: TempDir,
    result: Result<Vec<PathBuf>, EngineError>,
    progress: Vec<f32>,
}

fn merge_with(inputs: &[PathBuf], cancel: &CancelToken, skip: bool) -> Run {
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e:?}"));
    let mut dir = DirAllocator {
        dir: tmp.path().to_path_buf(),
    };
    let out: &mut dyn OutputAllocator = if skip { &mut SkipAll } else { &mut dir };
    let progress = RefCell::new(Vec::new());
    let result = merger().merge(inputs, out, &|p| progress.borrow_mut().push(p), cancel);
    Run {
        tmp,
        result,
        progress: progress.into_inner(),
    }
}

fn merge(inputs: &[PathBuf]) -> Run {
    merge_with(inputs, &CancelToken::new(), false)
}

fn only_output(run: &Run) -> PathBuf {
    let outputs = run
        .result
        .as_ref()
        .unwrap_or_else(|e| panic!("merge succeeds: {e:?}"));
    assert_eq!(outputs.len(), 1, "{outputs:?}");
    outputs[0].clone()
}

fn load(path: &Path) -> Document {
    Document::load(path).unwrap_or_else(|e| panic!("lopdf reads {path:?}: {e:?}"))
}

/// Writes `doc` to `dir/name` and returns the path.
fn save(mut doc: Document, dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    doc.save(&path)
        .unwrap_or_else(|e| panic!("save {path:?}: {e:?}"));
    path
}

/// A one-page PDF whose MediaBox width is `width`, so each input's pages can
/// be told apart in the result. With `link`, the page carries a link
/// annotation pointing at itself.
fn tagged(width: i64, link: bool) -> Document {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let page_id = doc.new_object_id();
    let mut page = dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), width.into(), 300.into()],
    };
    if link {
        let annot = doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Link",
            "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
            "Dest" => vec![Object::Reference(page_id), "Fit".into()],
        });
        page.set("Annots", vec![Object::Reference(annot)]);
    }
    doc.objects.insert(page_id, Object::Dictionary(page));
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        }),
    );
    let outline = doc.add_object(dictionary! { "Type" => "Outlines", "Count" => 0 });
    let catalog = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
        "Outlines" => outline,
    });
    doc.trailer.set("Root", catalog);
    doc
}

fn width_of(doc: &Document, page: lopdf::ObjectId) -> i64 {
    doc.get_dictionary(page)
        .and_then(|dict| dict.get(b"MediaBox"))
        .and_then(Object::as_array)
        .and_then(|media| media[2].as_i64())
        .unwrap_or_else(|e| panic!("page {page:?} has a MediaBox: {e:?}"))
}

#[test]
fn sample_merged_with_itself_has_six_pages() {
    let sample = fixture("sample.pdf");
    let run = merge(&[sample.clone(), sample]);
    let path = only_output(&run);
    assert!(path.starts_with(run.tmp.path()));
    assert_eq!(load(&path).get_pages().len(), 6);
    if engine_available("pdfinfo") {
        assert_eq!(pdf_page_count(&path), Some(6), "pdfinfo agrees");
    }
}

#[test]
fn pages_come_out_in_input_order() {
    let tmp = TempDir::new().unwrap();
    let inputs: Vec<PathBuf> = [100, 200, 300]
        .iter()
        .map(|&w| save(tagged(w, false), tmp.path(), &format!("w{w}.pdf")))
        .collect();
    // The 100 one twice, to make sure a repeat doesn't collapse into one.
    let run = merge(&[
        inputs[2].clone(),
        inputs[0].clone(),
        inputs[1].clone(),
        inputs[0].clone(),
    ]);
    let doc = load(&only_output(&run));
    let widths: Vec<i64> = doc
        .get_pages()
        .values()
        .map(|&id| width_of(&doc, id))
        .collect();
    assert_eq!(widths, [300, 100, 200, 100]);
}

#[test]
fn links_survive_and_outlines_do_not() {
    let tmp = TempDir::new().unwrap();
    let a = save(tagged(100, true), tmp.path(), "a.pdf");
    let b = save(tagged(200, true), tmp.path(), "b.pdf");
    let doc = load(&only_output(&merge(&[a, b])));

    let catalog = doc.catalog().unwrap();
    assert!(!catalog.has(b"Outlines"), "old outlines are dropped");
    let catalogs = doc
        .objects
        .values()
        .filter(|o| o.type_name().is_ok_and(|t| t == b"Catalog"))
        .count();
    assert_eq!(catalogs, 1);

    for (_, page_id) in doc.get_pages() {
        let page = doc.get_dictionary(page_id).unwrap();
        let annots = page.get(b"Annots").unwrap().as_array().unwrap();
        let annot = doc
            .get_dictionary(annots[0].as_reference().unwrap())
            .unwrap();
        let dest = annot.get(b"Dest").unwrap().as_array().unwrap();
        assert_eq!(
            dest[0].as_reference().unwrap(),
            page_id,
            "the link still points at its own page"
        );
    }
}

#[test]
fn progress_climbs_once_per_input_and_ends_at_one() {
    let sample = fixture("sample.pdf");
    let run = merge(&[sample.clone(), sample.clone(), sample]);
    only_output(&run);
    assert!(run.progress.len() >= 3, "{:?}", run.progress);
    assert!(run.progress.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(run.progress.last(), Some(&1.0));
}

#[test]
fn encrypted_inputs_are_refused() {
    let tmp = TempDir::new().unwrap();
    for (name, user) in [("empty-user.pdf", ""), ("locked.pdf", "secret")] {
        let mut doc = load(&fixture("sample.pdf"));
        let state = EncryptionState::try_from(EncryptionVersion::V2 {
            document: &doc,
            owner_password: "owner",
            user_password: user,
            key_length: 128,
            permissions: Permissions::all(),
        })
        .unwrap_or_else(|e| panic!("encryption state: {e:?}"));
        doc.encrypt(&state)
            .unwrap_or_else(|e| panic!("encrypt: {e:?}"));
        let locked = save(doc, tmp.path(), name);

        let run = merge(&[fixture("sample.pdf"), locked]);
        let err = run.result.expect_err("an encrypted input is refused");
        assert!(
            err.to_string().contains("password-protected"),
            "{name}: {err}"
        );
        assert!(
            std::fs::read_dir(run.tmp.path()).unwrap().next().is_none(),
            "{name}: nothing is left behind"
        );
    }
}

#[test]
fn a_file_that_is_not_a_pdf_fails_cleanly() {
    let run = merge(&[fixture("sample.pdf"), fixture("sample.txt")]);
    let err = run.result.expect_err("text is not a PDF");
    assert!(err.to_string().contains("sample.txt"), "{err}");
}

#[test]
fn a_skipped_output_writes_nothing() {
    let sample = fixture("sample.pdf");
    let run = merge_with(&[sample.clone(), sample], &CancelToken::new(), true);
    assert_eq!(run.result.unwrap(), Vec::<PathBuf>::new());
}

#[test]
fn a_cancelled_merge_writes_nothing() {
    let cancel = CancelToken::new();
    cancel.cancel();
    let sample = fixture("sample.pdf");
    let run = merge_with(&[sample.clone(), sample], &cancel, false);
    assert!(matches!(run.result, Err(EngineError::Cancelled)));
    assert!(std::fs::read_dir(run.tmp.path()).unwrap().next().is_none());
}

#[test]
fn run_job_merges_an_image_and_a_pdf_into_file_one() {
    let tmp = TempDir::new().unwrap();
    let png = tmp.path().join("sample.png");
    let pdf = tmp.path().join("sample.pdf");
    std::fs::copy(fixture("sample.png"), &png).unwrap();
    std::fs::copy(fixture("sample.pdf"), &pdf).unwrap();

    let mut options = ConvertOptions::default();
    options.pdf.merge_into_single = true;
    let engine = PdfEngine::new();
    let engines: Vec<&dyn Converter> = vec![&engine];
    let results = run_job(
        &[png.clone(), pdf.clone()],
        Target::Format(FormatId::new("pdf")),
        &options,
        &OutputRule::default(),
        &engines,
        &CancelToken::new(),
        &|_| {},
    );

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].state, FileState::Done, "{results:?}");
    assert_eq!(results[1].state, FileState::Done, "{results:?}");
    assert_eq!(results[0].outputs.len(), 1, "{results:?}");
    assert!(results[1].outputs.is_empty(), "{results:?}");

    let merged = &results[0].outputs[0];
    assert_eq!(merged.parent(), Some(tmp.path()));
    assert_eq!(load(merged).get_pages().len(), 4);

    // The inputs are untouched, and nothing but the output joined them.
    let mut names: BTreeMap<String, ()> = BTreeMap::new();
    for entry in std::fs::read_dir(tmp.path()).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        names.insert(name, ());
    }
    assert_eq!(names.len(), 3, "{names:?}");
    assert_eq!(load(&pdf).get_pages().len(), 3);
}
