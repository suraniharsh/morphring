//! `morphring convert --preset`, `--keep-dates` and the settings' default
//! output, end to end through `run_with()`.
//!
//! Fake engines do the converting, so this runs on a bare machine. Whether a
//! real engine honours the resize is the engines' business; here we only
//! check that the request reaches them intact.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use filetime::FileTime;
use morphring_core::options::Resize;
use morphring_core::testing::FakeConverter;
use morphring_core::{
    CancelToken, ConvertOptions, Converter, Edge, EngineError, EngineStatus, OutputAllocator, Step,
};
use serde_json::Value;
use tempfile::TempDir;

use common::{run_in, run_in_with_config};

/// A fake that turns png into jpg and png into png (the self edge the
/// same-as-source presets ride on).
fn png_engines() -> Vec<Box<dyn Converter>> {
    vec![Box::new(
        FakeConverter::new("fake")
            .edge("png", "jpg", 10)
            .edge("png", "png", 10),
    )]
}

fn touch(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, b"x").unwrap_or_else(|e| panic!("write {path:?}: {e}"));
    path
}

#[test]
fn jpg_70_names_the_output_after_its_pattern() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "sample.png");
    let r = run_in(
        tmp.path(),
        png_engines(),
        &["convert", "sample.png", "--preset", "jpg-70"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    assert!(tmp.path().join("sample_70.jpg").is_file(), "{r:?}");
}

#[test]
fn presets_are_found_by_display_name_too() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "sample.png");
    let r = run_in(
        tmp.path(),
        png_engines(),
        &["convert", "sample.png", "-p", "jpg 70%"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    assert!(tmp.path().join("sample_70.jpg").is_file());
}

/// Records the options each step was given, then writes a stub output.
struct OptionsSpy {
    edges: Vec<Edge>,
    seen: Arc<Mutex<Vec<ConvertOptions>>>,
}

impl Converter for OptionsSpy {
    fn id(&self) -> &'static str {
        "spy"
    }

    fn display_name(&self) -> &'static str {
        "Spy"
    }

    fn edges(&self) -> &[Edge] {
        &self.edges
    }

    fn status(&self) -> EngineStatus {
        EngineStatus::Available {
            path: PathBuf::from("/usr/bin/spy"),
            version: "1".into(),
        }
    }

    fn convert(
        &self,
        step: &Step,
        out: &mut dyn OutputAllocator,
        _progress: &dyn Fn(f32),
        _cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(step.options.clone());
        let Some(path) = out.allocate(step.to.as_str(), "") else {
            return Ok(Vec::new());
        };
        fs::write(&path, b"spy")?;
        Ok(vec![path])
    }
}

#[test]
fn half_size_stays_png_and_asks_for_half_the_size() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "sample.png");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let spy = OptionsSpy {
        edges: FakeConverter::new("x")
            .edge("png", "png", 10)
            .edges()
            .to_vec(),
        seen: Arc::clone(&seen),
    };
    let r = run_in(
        tmp.path(),
        vec![Box::new(spy)],
        &["convert", "sample.png", "--preset", "half-size", "--json"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    assert!(tmp.path().join("sample_50.png").is_file());

    let seen = seen.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].image.resize, Resize::Percent);
    assert_eq!(seen[0].image.resize_percent, 50);

    // The report shows the target as the preset stores it.
    let report: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(report["target"], "*", "{}", r.out);
}

#[test]
fn keep_dates_copies_the_modified_time() {
    let tmp = TempDir::new().unwrap();
    let source = touch(tmp.path(), "a.png");
    let old = FileTime::from_unix_time(1_500_000_000, 0);
    filetime::set_file_mtime(&source, old).unwrap();

    let r = run_in(
        tmp.path(),
        png_engines(),
        &["convert", "a.png", "--to", "jpg", "--keep-dates"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    let meta = fs::metadata(tmp.path().join("a.jpg")).unwrap();
    assert_eq!(FileTime::from_last_modification_time(&meta), old);

    // Without the flag the output is born today, like any honest file.
    let r = run_in(
        tmp.path(),
        png_engines(),
        &["convert", "a.png", "--to", "jpg", "--overwrite"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    let meta = fs::metadata(tmp.path().join("a.jpg")).unwrap();
    assert_ne!(FileTime::from_last_modification_time(&meta), old);
}

#[test]
fn settings_default_output_sends_files_to_a_subfolder() {
    let tmp = TempDir::new().unwrap();
    let config = TempDir::new().unwrap();
    fs::write(
        config.path().join("settings.json"),
        r#"{ "defaultOutput": { "location": "Subfolder", "subfolderName": "converted" } }"#,
    )
    .unwrap();
    touch(tmp.path(), "a.png");

    let r = run_in_with_config(
        tmp.path(),
        config.path(),
        png_engines(),
        &["convert", "a.png", "--to", "jpg"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    assert!(tmp.path().join("converted/a.jpg").is_file(), "{r:?}");
    assert!(!tmp.path().join("a.jpg").exists());

    // -o still beats the settings.
    let r = run_in_with_config(
        tmp.path(),
        config.path(),
        png_engines(),
        &["convert", "a.png", "--to", "jpg", "-o", "out"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    assert!(tmp.path().join("out/a.jpg").is_file());
}

#[test]
fn a_preset_from_presets_json_is_used() {
    let tmp = TempDir::new().unwrap();
    let config = TempDir::new().unwrap();
    fs::write(
        config.path().join("presets.json"),
        r#"{ "presets": [ { "id": "mine", "name": "Mine", "target": "jpg",
             "output": { "location": "SameFolder", "pattern": "{name}-mine" } } ] }"#,
    )
    .unwrap();
    touch(tmp.path(), "a.png");
    let r = run_in_with_config(
        tmp.path(),
        config.path(),
        png_engines(),
        &["convert", "a.png", "--preset", "Mine"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    assert!(tmp.path().join("a-mine.jpg").is_file(), "{r:?}");
}

#[test]
fn an_unknown_preset_is_a_usage_error() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "a.png");
    let r = run_in(
        tmp.path(),
        png_engines(),
        &["convert", "a.png", "--preset", "nope"],
    );
    assert_eq!(r.code, 2);
    assert_eq!(r.out, "");
    assert_eq!(
        r.err,
        "morphring: no preset 'nope'. Run 'morphring presets' for the list.\n"
    );
}

/// A presets.json with two conversions, a `rotate` group holding them (plus
/// an id that isn't there, which loading drops) and a group with nobody left.
fn write_groups(config: &Path) {
    fs::write(
        config.join("presets.json"),
        r#"{ "presets": [
             { "id": "turn-left", "name": "Turn left", "target": "png" },
             { "id": "turn-right", "name": "Turn right", "target": "png" },
             { "id": "rotate", "name": "Rotate", "target": "group", "kind": "Group",
               "subPresetIds": ["turn-left", "gone", "turn-right"] },
             { "id": "empty", "name": "Empty", "target": "group", "kind": "Group",
               "subPresetIds": ["gone"] } ] }"#,
    )
    .unwrap_or_else(|e| panic!("write presets.json: {e}"));
}

#[test]
fn a_group_preset_names_its_members_and_exits_2() {
    let tmp = TempDir::new().unwrap();
    let config = TempDir::new().unwrap();
    write_groups(config.path());
    touch(tmp.path(), "a.png");
    // By id and by display name, as typed.
    for key in ["rotate", "ROTATE"] {
        let r = run_in_with_config(
            tmp.path(),
            config.path(),
            png_engines(),
            &["convert", "a.png", "--preset", key],
        );
        assert_eq!(r.code, 2, "{r:?}");
        assert_eq!(r.out, "");
        assert_eq!(
            r.err,
            format!(
                "morphring: '{key}' is a group on the donut, not a conversion. \
                 Use one of: turn-left, turn-right\n"
            )
        );
    }
    assert!(!tmp.path().join("a.png.png").exists());
}

#[test]
fn an_empty_group_points_at_the_list() {
    let tmp = TempDir::new().unwrap();
    let config = TempDir::new().unwrap();
    write_groups(config.path());
    touch(tmp.path(), "a.png");
    let r = run_in_with_config(
        tmp.path(),
        config.path(),
        png_engines(),
        &["convert", "a.png", "-p", "Empty", "--json"],
    );
    assert_eq!(r.code, 2, "{r:?}");
    assert_eq!(r.out, "");
    assert_eq!(
        r.err,
        "morphring: 'Empty' is a group on the donut and has no presets left. \
         Run 'morphring presets' for the list.\n"
    );
}

#[test]
fn watch_refuses_a_group_too() {
    let tmp = TempDir::new().unwrap();
    let config = TempDir::new().unwrap();
    write_groups(config.path());
    fs::create_dir(tmp.path().join("inbox")).unwrap();
    let r = run_in_with_config(
        tmp.path(),
        config.path(),
        png_engines(),
        &["watch", "inbox", "--preset", "rotate"],
    );
    assert_eq!(r.code, 2, "{r:?}");
    assert_eq!(
        r.err,
        "morphring: 'rotate' is a group on the donut, not a conversion. \
         Use one of: turn-left, turn-right\n"
    );
}

#[test]
fn a_conversion_wins_over_a_group_of_the_same_name() {
    let tmp = TempDir::new().unwrap();
    let config = TempDir::new().unwrap();
    // The user's own "Rotate" preset sits behind the `rotate` group.
    fs::write(
        config.path().join("presets.json"),
        r#"{ "presets": [
             { "id": "rotate", "name": "Rotate", "target": "group", "kind": "Group",
               "subPresetIds": ["mine"] },
             { "id": "mine", "name": "Rotate", "target": "jpg",
               "output": { "location": "SameFolder", "pattern": "{name}-mine" } } ] }"#,
    )
    .unwrap();
    touch(tmp.path(), "a.png");
    let r = run_in_with_config(
        tmp.path(),
        config.path(),
        png_engines(),
        &["convert", "a.png", "--preset", "rotate", "--json"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    assert!(tmp.path().join("a-mine.jpg").is_file(), "{r:?}");
    let json: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(json["preset"], "Rotate");
}

#[test]
fn presets_lists_groups_with_target_group() {
    let tmp = TempDir::new().unwrap();
    let config = TempDir::new().unwrap();
    write_groups(config.path());
    let r = run_in_with_config(tmp.path(), config.path(), Vec::new(), &["presets"]);
    assert_eq!(r.code, 0, "{r:?}");
    let rotate = r
        .out
        .lines()
        .find(|line| line.starts_with("rotate "))
        .unwrap_or_else(|| panic!("no rotate row in {}", r.out));
    assert_eq!(rotate, format!("{:<24} {:<28} group", "rotate", "Rotate"));

    let r = run_in_with_config(
        tmp.path(),
        config.path(),
        Vec::new(),
        &["presets", "--json"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.err, "");
    let json: Value = serde_json::from_str(&r.out).unwrap();
    let rows = json.as_array().unwrap();
    let row = |id: &str| rows.iter().find(|r| r["id"] == id).unwrap().clone();
    assert_eq!(
        row("rotate"),
        serde_json::json!({
            "id": "rotate", "name": "Rotate", "target": "group", "isGroup": true,
            "members": ["turn-left", "turn-right"]
        })
    );
    assert_eq!(row("empty")["members"], serde_json::json!([]));
    assert_eq!(
        row("turn-left"),
        serde_json::json!({
            "id": "turn-left", "name": "Turn left", "target": "png", "isGroup": false
        })
    );
}
