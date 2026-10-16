//! End to end without a window: the ring model and the conversion queue with
//! the real engines, exactly as the `convert` command drives them.

// Test helpers aren't #[test] fns, so allow-unwrap-in-tests doesn't reach them.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use morphring_app::commands::{Conversions, EventSink, Outcome, RingDone, RingEvent};
use morphring_core::profiles::Library;
use morphring_core::settings::{AppSettings, DefaultLocation};
use morphring_engines::registry;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/engines/tests/fixtures")
        .join(name)
}

/// The real engines with the factory presets and `settings`.
fn queue(sink: EventSink, settings: AppSettings) -> Conversions {
    Conversions::new(registry::all(), sink, Library::builtin(), settings)
}

/// `sample.png` copied into a fresh temp folder.
fn sample_png() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("sample.png");
    std::fs::copy(fixture("sample.png"), &source).unwrap();
    (dir, source)
}

fn recorder() -> (EventSink, Arc<Mutex<Vec<RingEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&events);
    let sink: EventSink = Arc::new(move |e| seen.lock().unwrap().push(e));
    (sink, events)
}

// Used to need ImageMagick; the native image engine now does png -> jpg on
// its own, so the name lost its magick and the test only checks the bytes.
#[test]
fn png_rings_and_converts() {
    let (dir, source) = sample_png();

    let (sink, events) = recorder();
    let conv = queue(sink, AppSettings::default());

    let model = conv.ring_model(std::slice::from_ref(&source));
    assert_eq!(model.title, "1 × PNG");
    let jpg = model
        .slices
        .iter()
        .find(|s| s.preset_id == "to-jpg")
        .unwrap();
    assert!(jpg.enabled, "{model:?}");

    let summary =
        tauri::async_runtime::block_on(conv.convert(model.files.clone(), jpg.preset_id.clone()))
            .unwrap();

    assert_eq!(summary.outcome, Outcome::Succeeded, "{summary:?}");
    let output = dir.path().join("sample.jpg");
    assert_eq!(summary.outputs, [output.to_string_lossy().into_owned()]);
    let bytes = std::fs::read(&output).unwrap();
    assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF], "not a JPEG");

    let events = events.lock().unwrap();
    assert_eq!(
        events.last(),
        Some(&RingEvent::Done(RingDone {
            outcome: Outcome::Succeeded,
            message: None
        }))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RingEvent::Progress(p) if p.progress == 1.0)),
        "{events:?}"
    );
}

#[test]
fn text_to_audio_fails_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("sample.txt");
    std::fs::copy(fixture("sample.txt"), &source).unwrap();

    let (sink, _) = recorder();
    let conv = queue(sink, AppSettings::default());
    let summary = conv.submit(vec![source], "to-mp3").unwrap().recv().unwrap();

    assert_eq!(summary.outcome, Outcome::Failed);
    assert_eq!(
        summary.message.as_deref(),
        Some("sample.txt: No engine can do this conversion (txt → mp3)")
    );
}

#[test]
fn ring_ui_listens_for_the_events_we_send() {
    use morphring_app::commands::{RING_DONE, RING_OPEN, RING_PROGRESS};
    let js = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/ring.js"))
        .unwrap();
    for event in [RING_OPEN, RING_PROGRESS, RING_DONE] {
        assert!(
            js.contains(&format!("on(\"{event}\"")),
            "ring.js ignores {event}"
        );
    }
    for command in ["convert", "ring_close"] {
        assert!(
            js.contains(&format!("invoke(\"{command}\"")),
            "ring.js never invokes {command}"
        );
    }
    assert!(
        js.contains("invoke(\"convert\", { files, presetId })"),
        "ring.js does not convert by preset id"
    );
}

#[test]
fn a_png_gets_the_images_profile_in_filee_order() {
    let (_dir, source) = sample_png();
    let (sink, _) = recorder();
    let conv = queue(sink, AppSettings::default());
    let model = conv.ring_model(std::slice::from_ref(&source));
    let ids: Vec<&str> = model.slices.iter().map(|s| s.preset_id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "to-png",
            "to-jpg",
            "to-webp",
            "to-pdf",
            "to-tiff",
            "to-bmp",
            "to-ico",
            "half-size"
        ]
    );
    let labels: Vec<&str> = model.slices.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "PNG",
            "JPG",
            "WEBP",
            "PDF",
            "TIFF",
            "BMP",
            "ICO",
            "Half size"
        ]
    );
    let half = model.slices.last().unwrap();
    assert_eq!(half.target, "*");
    assert!(half.enabled, "{model:?}");
}

#[test]
fn jpg_70_writes_name_70_jpg() {
    let (dir, source) = sample_png();
    let (sink, _) = recorder();
    let conv = queue(sink, AppSettings::default());
    let summary = conv.submit(vec![source], "jpg-70").unwrap().recv().unwrap();
    assert_eq!(summary.outcome, Outcome::Succeeded, "{summary:?}");
    let output = dir.path().join("sample_70.jpg");
    assert_eq!(summary.outputs, [output.to_string_lossy().into_owned()]);
    let bytes = std::fs::read(&output).unwrap();
    assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF], "not a JPEG");
}

#[test]
fn half_size_writes_name_50_png() {
    let (dir, source) = sample_png();
    let (sink, _) = recorder();
    let conv = queue(sink, AppSettings::default());
    let summary = conv
        .submit(vec![source], "half-size")
        .unwrap()
        .recv()
        .unwrap();
    assert_eq!(summary.outcome, Outcome::Succeeded, "{summary:?}");
    let output = dir.path().join("sample_50.png");
    let bytes = std::fs::read(&output).unwrap();
    assert_eq!(&bytes[..4], b"\x89PNG", "not a PNG");
}

#[test]
fn a_subfolder_default_output_is_honoured() {
    let (dir, source) = sample_png();
    let (sink, _) = recorder();
    let mut settings = AppSettings::default();
    settings.default_output.location = DefaultLocation::Subfolder;
    settings.default_output.subfolder_name = "converted here".into();
    let conv = queue(sink, settings);
    let summary = conv.submit(vec![source], "to-jpg").unwrap().recv().unwrap();
    assert_eq!(summary.outcome, Outcome::Succeeded, "{summary:?}");
    let output = dir.path().join("converted here").join("sample.jpg");
    assert_eq!(summary.outputs, [output.to_string_lossy().into_owned()]);
    assert!(output.is_file());
    assert!(!dir.path().join("sample.jpg").exists());
}
