//! Sanity checks for the committed fixtures and the shared test helpers.

mod common;

use common::{duration_secs, fixture, frame_count, image_size};

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
