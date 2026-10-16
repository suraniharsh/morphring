//! Pandoc engine tests: the argument lists and edge table (always run), and
//! golden conversions of sample.md (skipped when pandoc is not installed).

mod common;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use common::{fixture, read_text};
use morphring_core::{
    CancelToken, ConvertOptions, Converter, EngineError, FormatId, OutputAllocator, Step,
};
use morphring_engines::pandoc::{PandocEngine, args};

fn f(id: &'static str) -> FormatId {
    FormatId::new(id)
}

fn argv(from: &'static str, to: &'static str) -> Vec<String> {
    let out = format!("/tmp/w/out.{to}");
    args::args(
        Path::new("/docs/in.x"),
        f(from),
        f(to),
        Path::new(&out),
        Path::new("/out/in_files"),
    )
    .unwrap_or_else(|| panic!("no args for {from} -> {to}"))
    .iter()
    .map(|a: &OsString| a.to_string_lossy().into_owned())
    .collect()
}

fn cost(from: &str, to: &str) -> Option<u32> {
    args::edges()
        .into_iter()
        .find(|e| e.from.as_str() == from && e.to.as_str() == to)
        .map(|e| e.cost)
}

#[test]
fn md_to_docx_is_the_bare_command() {
    assert_eq!(
        argv("md", "docx"),
        [
            "/docs/in.x",
            "-f",
            "markdown",
            "-t",
            "docx",
            "-o",
            "/tmp/w/out.docx"
        ]
    );
}

#[test]
fn html_output_is_standalone_and_extracts_media() {
    assert_eq!(
        argv("md", "html"),
        [
            "/docs/in.x",
            "-f",
            "markdown",
            "-t",
            "html",
            "-o",
            "/tmp/w/out.html",
            "--standalone",
            "--extract-media=/out/in_files",
        ]
    );
}

#[test]
fn tex_output_is_standalone_and_extracts_media() {
    let a = argv("md", "tex");
    assert_eq!(&a[1..5], ["-f", "markdown", "-t", "latex"]);
    assert_eq!(&a[7..], ["--standalone", "--extract-media=/out/in_files"]);
}

#[test]
fn md_and_rst_output_extract_media_without_standalone() {
    let md = argv("docx", "md");
    assert_eq!(&md[1..5], ["-f", "docx", "-t", "gfm"]);
    assert_eq!(&md[7..], ["--extract-media=/out/in_files"]);
    let rst = argv("tex", "rst");
    assert_eq!(&rst[1..5], ["-f", "latex", "-t", "rst"]);
    assert_eq!(&rst[7..], ["--extract-media=/out/in_files"]);
}

#[test]
fn txt_odt_rtf_epub_get_no_extras() {
    for (to, writer) in [("txt", "plain"), ("odt", "odt"), ("rtf", "rtf")] {
        let a = argv("md", to);
        assert_eq!(a.len(), 7, "{to}: {a:?}");
        assert_eq!(a[4], writer);
    }
    assert_eq!(argv("rst", "epub").len(), 7);
}

#[test]
fn readers_and_writers_match_filee() {
    for (id, reader) in [
        ("md", "markdown"),
        ("html", "html"),
        ("docx", "docx"),
        ("odt", "odt"),
        ("rtf", "rtf"),
        ("rst", "rst"),
        ("tex", "latex"),
    ] {
        assert_eq!(args::reader(f(id)), Some(reader), "{id}");
    }
    assert_eq!(args::reader(f("txt")), None);
    assert_eq!(args::reader(f("pdf")), None);
    assert_eq!(args::writer(f("md")), Some("gfm"));
    assert_eq!(args::writer(f("txt")), Some("plain"));
    assert_eq!(args::writer(f("tex")), Some("latex"));
    assert_eq!(args::writer(f("epub")), Some("epub"));
    assert_eq!(args::writer(f("pdf")), None);
}

#[test]
fn edge_costs_match_filee() {
    for from in ["docx", "odt", "html", "rtf"] {
        assert_eq!(cost(from, "md"), Some(10), "{from} -> md");
    }
    for to in ["docx", "odt", "html", "rtf", "txt"] {
        assert_eq!(cost("md", to), Some(10), "md -> {to}");
    }
    assert_eq!(cost("html", "docx"), Some(14));
    assert_eq!(cost("html", "odt"), Some(14));
    assert_eq!(cost("docx", "html"), Some(14));
    assert_eq!(cost("odt", "html"), Some(14));
    for markup in ["rst", "tex"] {
        for to in ["md", "html", "docx", "odt", "rtf", "epub", "txt"] {
            assert_eq!(cost(markup, to), Some(10), "{markup} -> {to}");
        }
        for from in ["md", "html", "docx", "odt"] {
            assert_eq!(cost(from, markup), Some(10), "{from} -> {markup}");
        }
    }
    assert_eq!(cost("rst", "tex"), Some(10));
    assert_eq!(cost("tex", "rst"), Some(10));
}

#[test]
fn no_pdf_no_self_edges_no_duplicates() {
    let all = args::edges();
    assert_eq!(all.len(), 37);
    assert!(all.iter().all(|e| e.to.as_str() != "pdf"));
    assert!(all.iter().all(|e| e.from != e.to));
    for (i, a) in all.iter().enumerate() {
        assert!(
            all[i + 1..]
                .iter()
                .all(|b| (a.from, a.to) != (b.from, b.to)),
            "duplicate edge {} -> {}",
            a.from,
            a.to
        );
    }
    assert_eq!(cost("rtf", "docx"), None);
    assert_eq!(cost("txt", "md"), None);
}

// ---- golden conversions -------------------------------------------------

/// Hands out `<dir>/<stem>.<ext>`, or nothing when `skip` is set.
struct DirAllocator {
    dir: PathBuf,
    skip: bool,
}

impl OutputAllocator for DirAllocator {
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
        (!self.skip).then(|| self.dir.join(format!("sample{suffix}.{ext}")))
    }
}

fn convert_sample(to: &'static str, dir: &Path) -> Result<Vec<PathBuf>, EngineError> {
    convert_with(fixture("sample.md"), "md", to, dir, false)
}

fn convert_with(
    input: PathBuf,
    from: &'static str,
    to: &'static str,
    dir: &Path,
    skip: bool,
) -> Result<Vec<PathBuf>, EngineError> {
    let step = Step {
        input,
        from: f(from),
        to: f(to),
        options: ConvertOptions::default(),
        work_dir: dir.join("work"),
    };
    let mut out = DirAllocator {
        dir: dir.to_path_buf(),
        skip,
    };
    PandocEngine::new().convert(&step, &mut out, &|_| {}, &CancelToken::new())
}

#[test]
fn sample_md_to_docx() {
    require_engine!("pandoc");
    let dir = tempfile::tempdir().unwrap();
    let outputs = convert_sample("docx", dir.path()).expect("md -> docx");
    assert_eq!(outputs, [dir.path().join("sample.docx")]);
    let bytes = std::fs::read(&outputs[0]).unwrap();
    assert!(bytes.starts_with(b"PK"), "a docx is a zip");
    // Zip entry names sit uncompressed in the headers, so a byte search will do.
    assert!(
        bytes.windows(17).any(|w| w == b"word/document.xml"),
        "the docx has word/document.xml"
    );
    // The temp output never leaks next to the result.
    assert!(!dir.path().join("out.docx").exists());
}

#[test]
fn sample_md_to_html() {
    require_engine!("pandoc");
    let dir = tempfile::tempdir().unwrap();
    let outputs = convert_sample("html", dir.path()).expect("md -> html");
    let html = read_text(&outputs[0]);
    assert!(html.contains("<h1"), "{html}");
    assert!(html.contains("<html"), "standalone page: {html}");
}

#[test]
fn sample_md_to_txt() {
    require_engine!("pandoc");
    let dir = tempfile::tempdir().unwrap();
    let outputs = convert_sample("txt", dir.path()).expect("md -> txt");
    let text = read_text(&outputs[0]);
    assert!(text.contains("MorphRing sample"), "{text}");
    assert!(!text.contains('#'), "plain text has no markdown: {text}");
}

#[test]
fn a_skipped_output_converts_nothing() {
    require_engine!("pandoc");
    let dir = tempfile::tempdir().unwrap();
    let outputs = convert_with(fixture("sample.md"), "md", "docx", dir.path(), true).unwrap();
    assert!(outputs.is_empty());
}

#[test]
fn a_broken_input_reports_filees_message() {
    require_engine!("pandoc");
    let dir = tempfile::tempdir().unwrap();
    // A PNG pretending to be a docx: pandoc's zip reader gives up on it.
    let err = convert_with(fixture("sample.png"), "docx", "md", dir.path(), false).unwrap_err();
    let msg = err.to_string();
    assert!(msg.starts_with("Pandoc failed (exit "), "{msg}");
    assert!(!dir.path().join("sample.md").exists());
}
