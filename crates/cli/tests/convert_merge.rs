//! `morphring convert ... --preset "Merge PDF"` end to end, with the real
//! engines. Only built-in ones are involved (lopdf merges, krilla turns an
//! image into a PDF), so this runs on a bare machine.

mod common;

use std::fs;
use std::path::Path;

use lopdf::Document;
use tempfile::TempDir;

use common::{fixture, run_in};

fn pages(path: &Path) -> usize {
    Document::load(path)
        .unwrap_or_else(|e| panic!("lopdf reads {path:?}: {e:?}"))
        .get_pages()
        .len()
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {dir:?}: {e}"))
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("read {dir:?}: {e}"))
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

#[test]
fn two_pdfs_become_one_named_after_the_first() {
    let tmp = TempDir::new().unwrap();
    fs::copy(fixture("sample.pdf"), tmp.path().join("a.pdf")).unwrap();
    fs::copy(fixture("sample.pdf"), tmp.path().join("b.pdf")).unwrap();

    let r = run_in(
        tmp.path(),
        morphring_engines::registry::all(),
        &["convert", "a.pdf", "b.pdf", "--preset", "Merge PDF"],
    );
    assert_eq!(r.code, 0, "{r:?}");

    // The built-in preset's pattern is {name}_merged, named after file 1.
    assert_eq!(
        names(tmp.path()),
        ["a.pdf", "a_merged.pdf", "b.pdf"],
        "{r:?}"
    );
    assert_eq!(pages(&tmp.path().join("a_merged.pdf")), 6);
    // The inputs are left as they were.
    assert_eq!(pages(&tmp.path().join("a.pdf")), 3);
    assert_eq!(pages(&tmp.path().join("b.pdf")), 3);
}

#[test]
fn an_image_joins_the_merge_as_a_page() {
    let tmp = TempDir::new().unwrap();
    fs::copy(fixture("sample.png"), tmp.path().join("cover.png")).unwrap();
    fs::copy(fixture("sample.pdf"), tmp.path().join("body.pdf")).unwrap();

    let r = run_in(
        tmp.path(),
        morphring_engines::registry::all(),
        &["convert", "cover.png", "body.pdf", "--preset", "merge-pdf"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(
        names(tmp.path()),
        ["body.pdf", "cover.png", "cover_merged.pdf"],
        "{r:?}"
    );
    assert_eq!(pages(&tmp.path().join("cover_merged.pdf")), 4);
}
