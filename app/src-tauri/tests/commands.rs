//! End to end without a window: the ring model and the conversion queue with
//! the real engines, exactly as the `convert` command drives them.

// Test helpers aren't #[test] fns, so allow-unwrap-in-tests doesn't reach them.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use morphring_app::commands::{Conversions, EventSink, Outcome, RingDone, RingEvent};
use morphring_engines::registry;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/engines/tests/fixtures")
        .join(name)
}

/// Prints a SKIP line and returns when `tool` is not on PATH, like the
/// engines' `require_engine!`.
macro_rules! require_tool {
    ($tool:expr) => {
        if which($tool).is_none() {
            eprintln!("SKIP: {} not found", $tool);
            return;
        }
    };
}

fn which(tool: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(tool))
            .find(|p| p.is_file())
    })
}

fn recorder() -> (EventSink, Arc<Mutex<Vec<RingEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&events);
    let sink: EventSink = Arc::new(move |e| seen.lock().unwrap().push(e));
    (sink, events)
}

#[test]
fn png_rings_and_converts_with_imagemagick() {
    require_tool!("magick");
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("sample.png");
    std::fs::copy(fixture("sample.png"), &source).unwrap();

    let (sink, events) = recorder();
    let conv = Conversions::new(registry::all(), sink);

    let model = conv.ring_model(std::slice::from_ref(&source));
    assert_eq!(model.title, "1 × PNG");
    let jpg = model.slices.iter().find(|s| s.target == "jpg").unwrap();
    assert!(jpg.enabled, "{model:?}");

    let summary =
        tauri::async_runtime::block_on(conv.convert(model.files.clone(), jpg.target.clone()))
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
    let conv = Conversions::new(registry::all(), sink);
    let summary = conv.submit(vec![source], "mp3").unwrap().recv().unwrap();

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
}
