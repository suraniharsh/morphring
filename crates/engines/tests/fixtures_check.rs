//! Sanity checks for the committed fixtures and the shared test helpers.

mod common;

use common::{
    duration_secs, fixture, frame_count, image_size, is_dir_with, pdf_page_count, read_text,
};
use std::process::Command;

const FIXTURES: &[&str] = &[
    "sample.png",
    "sample.jpg",
    "sample.gif",
    "sample.wav",
    "sample.mp4",
    "sample.txt",
    "sample.docx",
    "sample.odt",
    "notes.unknownext",
    "sample.pdf",
    "sample.eps",
    "sample.ps",
    "sample.svg",
    "sample.svgz",
    "sample.md",
    "sample.zip",
    "sample.tar.gz",
];

/// (fixture, byte offset, expected bytes). Plain-text fixtures with no real
/// magic (sample.txt, notes.unknownext) are left to the size check.
const MAGIC: &[(&str, usize, &[u8])] = &[
    ("sample.png", 0, b"\x89PNG\r\n\x1a\n"),
    ("sample.jpg", 0, b"\xff\xd8\xff"),
    ("sample.gif", 0, b"GIF89a"),
    ("sample.wav", 0, b"RIFF"),
    ("sample.wav", 8, b"WAVE"),
    ("sample.mp4", 4, b"ftyp"),
    ("sample.docx", 0, b"PK\x03\x04"),
    ("sample.odt", 0, b"PK\x03\x04"),
    ("sample.pdf", 0, b"%PDF-"),
    ("sample.eps", 0, b"%!PS-Adobe-3.0 EPSF-3.0"),
    ("sample.ps", 0, b"%!PS"),
    ("sample.svg", 0, b"<?xml"),
    ("sample.svgz", 0, b"\x1f\x8b"),
    ("sample.md", 0, b"# "),
    ("sample.zip", 0, b"PK\x03\x04"),
    ("sample.tar.gz", 0, b"\x1f\x8b"),
];

#[test]
fn every_fixture_exists_and_is_small() {
    for name in FIXTURES {
        let meta = std::fs::metadata(fixture(name))
            .unwrap_or_else(|e| panic!("missing fixture {name}: {e}"));
        assert!(meta.len() > 0, "{name} is empty");
        assert!(meta.len() < 100 * 1024, "{name} is over 100 KB");
    }
}

#[test]
fn every_fixture_starts_with_its_magic() {
    for (name, offset, magic) in MAGIC {
        let bytes = std::fs::read(fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let end = offset + magic.len();
        assert!(bytes.len() >= end, "{name} is too short for its magic");
        assert_eq!(&bytes[*offset..end], *magic, "{name} has the wrong magic");
    }
}

#[test]
fn eps_fixture_declares_a_bounding_box() {
    let text = read_text(&fixture("sample.eps"));
    assert!(text.contains("%%BoundingBox: 0 0 100 80"));
}

#[test]
fn svg_fixture_has_rect_circle_and_text() {
    let text = read_text(&fixture("sample.svg"));
    assert!(text.contains(r#"width="120" height="80""#));
    for tag in ["<rect", "<circle", "<text"] {
        assert!(text.contains(tag), "sample.svg lacks {tag}");
    }
}

#[test]
fn markdown_fixture_has_heading_list_link_and_emphasis() {
    let text = read_text(&fixture("sample.md"));
    assert!(text.starts_with("# "));
    assert!(text.lines().any(|l| l.starts_with("- ")));
    assert!(text.contains("](https://"));
    assert!(text.contains("*tiny*"));
}

#[test]
fn pdf_fixture_has_three_pages() {
    require_engine!("pdfinfo");
    assert_eq!(pdf_page_count(&fixture("sample.pdf")), Some(3));
}

#[test]
fn pdf_page_count_is_none_for_a_non_pdf() {
    require_engine!("pdfinfo");
    assert_eq!(pdf_page_count(&fixture("sample.md")), None);
}

#[test]
fn pdf_fixture_text_survives_pdftotext() {
    require_engine!("pdftotext");
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("sample.txt");
    let status = Command::new("pdftotext")
        .args(["-enc", "UTF-8"])
        .arg(fixture("sample.pdf"))
        .arg(&out)
        .status()
        .expect("run pdftotext");
    assert!(status.success());
    let text = read_text(&out);
    for line in ["Page 1", "Page 2", "Page 3", "Café déjà vu"] {
        assert!(text.contains(line), "pdftotext lost {line:?}: {text:?}");
    }
}

#[test]
fn archive_fixtures_unpack_to_the_expected_tree() {
    require_engine!("bsdtar");
    for name in ["sample.zip", "sample.tar.gz"] {
        let dir = tempfile::tempdir().expect("tempdir");
        let status = Command::new("bsdtar")
            .arg("-xf")
            .arg(fixture(name))
            .arg("-C")
            .arg(dir.path())
            .status()
            .expect("run bsdtar");
        assert!(status.success(), "bsdtar failed on {name}");
        assert!(
            is_dir_with(dir.path(), &["hello.txt", "dir/inner.txt"]),
            "{name} unpacked to the wrong tree"
        );
        assert!(!is_dir_with(dir.path(), &["nope.txt"]));
    }
}

#[test]
fn read_text_strips_bom_and_crlf() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bom.txt");
    std::fs::write(&path, "\u{feff}one\r\ntwo\r\n").expect("write");
    assert_eq!(read_text(&path), "one\ntwo\n");
}

#[test]
fn sample_txt_has_hangul_and_emoji() {
    let text = std::fs::read_to_string(fixture("sample.txt")).expect("sample.txt is UTF-8");
    assert_eq!(text.lines().count(), 2);
    assert!(text.contains('안'));
    assert!(text.contains('🌍'));
}

#[test]
fn image_fixtures_have_expected_sizes() {
    require_engine!("magick");
    assert_eq!(image_size(&fixture("sample.png")), (64, 48));
    assert_eq!(image_size(&fixture("sample.jpg")), (64, 48));
    assert_eq!(image_size(&fixture("sample.gif")), (32, 32));
}

#[test]
fn gif_fixture_has_three_frames() {
    require_engine!("magick");
    assert_eq!(frame_count(&fixture("sample.gif")), 3);
    assert_eq!(frame_count(&fixture("sample.png")), 1);
}

#[test]
fn media_fixtures_last_about_one_second() {
    require_engine!("ffprobe");
    for name in ["sample.mp4", "sample.wav"] {
        let d = duration_secs(&fixture(name));
        assert!((d - 1.0).abs() < 0.1, "{name} lasts {d}s");
    }
}

#[test]
fn missing_engine_skips_instead_of_failing() {
    require_engine!("morphring-no-such-tool-xyz");
    panic!("require_engine! should have returned early");
}
