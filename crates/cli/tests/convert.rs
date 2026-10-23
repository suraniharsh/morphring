//! `morphring convert`, end to end through `run_with()`.
//!
//! Most cases run on fake engines so they need nothing installed; the golden
//! cases at the bottom use the real tools and skip when one is missing.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use morphring_core::testing::{Behaviour, FakeConverter};
use morphring_core::{
    CancelToken, Converter, Edge, EngineError, EngineStatus, OutputAllocator, Step,
};
use serde_json::Value;
use tempfile::TempDir;

use common::{copy_fixture, fixture, run_in, run_with_cancel};

fn png_to_jpg() -> Vec<Box<dyn Converter>> {
    vec![Box::new(FakeConverter::new("fake").edge("png", "jpg", 10))]
}

fn touch(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("mkdir {parent:?}: {e}"));
    }
    fs::write(&path, b"x").unwrap_or_else(|e| panic!("write {path:?}: {e}"));
    path
}

#[test]
fn converts_next_to_the_source_by_default() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "a.png");
    let r = run_in(
        tmp.path(),
        png_to_jpg(),
        &["convert", "a.png", "--to", "jpg"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out, "ok    a.png -> a.jpg\nConverted 1 of 1 file(s).\n");
    assert_eq!(r.err, "");
    assert!(tmp.path().join("a.jpg").is_file());
}

#[test]
fn target_accepts_ids_extensions_and_dots() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "a.png");
    for to in ["JPEG", ".jpg"] {
        let r = run_in(tmp.path(), png_to_jpg(), &["convert", "a.png", "--to", to]);
        assert_eq!(r.code, 0, "{to}: {r:?}");
    }
    assert!(tmp.path().join("a.jpg").is_file());
    assert!(tmp.path().join("a (2).jpg").is_file());
}

#[test]
fn output_folder_is_flat_and_conflicts_follow_the_policy() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "in/sub/a.png");
    let base = ["convert", "in", "-r", "--to", "jpg", "-o", "out"];

    let r = run_in(tmp.path(), png_to_jpg(), &base);
    assert_eq!(r.code, 0, "{r:?}");
    let out = tmp.path().join("out");
    // Flat: the sub folder of the source is not recreated below -o.
    assert!(out.join("a.jpg").is_file());
    assert!(!out.join("sub").exists());

    // Default policy: the second run adds a number.
    let r = run_in(tmp.path(), png_to_jpg(), &base);
    assert_eq!(r.code, 0);
    // The report prints native separators; compare in Unix spelling.
    assert!(
        r.out.replace('\\', "/").contains("-> out/a (2).jpg"),
        "{}",
        r.out
    );
    assert!(out.join("a (2).jpg").is_file());

    // --skip leaves things be and still counts as success.
    let r = run_in(tmp.path(), png_to_jpg(), &[&base[..], &["--skip"]].concat());
    assert_eq!(r.code, 0);
    assert_eq!(
        r.out.replace('\\', "/"),
        "skip  in/sub/a.png (the output file already exists)\nConverted 0 of 1 file(s).\n"
    );
    assert!(!out.join("a (3).jpg").exists());

    // --overwrite replaces the first output instead of numbering a third.
    fs::write(out.join("a.jpg"), b"old").unwrap();
    let r = run_in(
        tmp.path(),
        png_to_jpg(),
        &[&base[..], &["--overwrite"]].concat(),
    );
    assert_eq!(r.code, 0);
    assert_ne!(fs::read(out.join("a.jpg")).unwrap(), b"old");
    assert!(!out.join("a (3).jpg").exists());
}

#[test]
fn json_prints_only_the_document_and_beats_quiet() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "a.png");
    touch(tmp.path(), "b.png");
    for extra in [&[][..], &["--quiet"]] {
        let args = [&["convert", "*.png", "--to", "jpg", "--json"][..], extra].concat();
        let r = run_in(tmp.path(), png_to_jpg(), &args);
        assert_eq!(r.code, 0, "{r:?}");
        let v: Value = serde_json::from_str(&r.out).expect("stdout is one JSON document");
        assert_eq!(v["target"], "jpg");
        assert_eq!(v["skipped"], 0);
        assert_eq!(v["failed"], 0);
        let files = v["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        // Input order, absolute paths.
        let a = tmp.path().join("a.png");
        assert_eq!(files[0]["source"], a.to_str().unwrap());
        assert_eq!(files[0]["state"], "done");
    }
}

#[test]
fn quiet_prints_only_the_failures_on_stderr() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "a.png");
    touch(tmp.path(), "notes.unknownext");
    let r = run_in(
        tmp.path(),
        png_to_jpg(),
        &[
            "convert",
            "a.png",
            "notes.unknownext",
            "--to",
            "jpg",
            "--quiet",
        ],
    );
    assert_eq!(r.code, 1);
    // No ok line, no summary: the exit code does the talking.
    assert_eq!(r.out, "");
    assert_eq!(
        r.err,
        "fail  notes.unknownext: This file type isn't supported (.unknownext)\n"
    );
    assert!(tmp.path().join("a.jpg").is_file());
}

#[test]
fn quiet_is_silent_when_everything_worked() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "a.png");
    let r = run_in(
        tmp.path(),
        png_to_jpg(),
        &["convert", "a.png", "--to", "jpg", "--quiet"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out, "");
    assert_eq!(r.err, "");
    assert!(tmp.path().join("a.jpg").is_file());
}

#[test]
fn json_preset_is_null_with_to_and_the_display_name_with_preset() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "a.png");
    let r = run_in(
        tmp.path(),
        png_to_jpg(),
        &["convert", "a.png", "--to", "jpg", "--json"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(v["preset"], Value::Null);

    // Asked for by id, reported by the name people see in the ring.
    let r = run_in(
        tmp.path(),
        png_to_jpg(),
        &["convert", "a.png", "--preset", "jpg-70", "--json"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    let v: Value = serde_json::from_str(&r.out).unwrap();
    let name = v["preset"].as_str().expect("preset is a string");
    assert!(!name.is_empty() && name != "jpg-70", "{}", r.out);
    assert_eq!(v["target"], "jpg");

    // Asked for by display name, the same name comes back.
    let r = run_in(
        tmp.path(),
        png_to_jpg(),
        &["convert", "a.png", "--preset", name, "--json"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    let again: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(again["preset"], name);
}

#[test]
fn every_missing_input_is_reported_before_anything_converts() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "a.png");
    let fake = FakeConverter::new("fake").edge("png", "jpg", 10);
    let counters = fake.counters();
    let r = run_in(
        tmp.path(),
        vec![Box::new(fake)],
        &["convert", "missing.png", "a.png", "*.gif", "--to", "jpg"],
    );
    assert_eq!(r.code, 2);
    assert_eq!(r.out, "");
    assert_eq!(
        r.err,
        "morphring: not found: missing.png\nmorphring: not found: *.gif\n"
    );
    assert_eq!(counters.calls(), 0);
    assert!(!tmp.path().join("a.jpg").exists());
}

#[test]
fn an_empty_folder_has_nothing_to_convert() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir(tmp.path().join("empty")).unwrap();
    // Unknown files in a folder are ignored, so this one is empty too.
    touch(tmp.path(), "odd/notes.unknownext");
    for folder in ["empty", "odd"] {
        let r = run_in(
            tmp.path(),
            png_to_jpg(),
            &["convert", folder, "--to", "jpg"],
        );
        assert_eq!(r.code, 3, "{folder}");
        assert_eq!(r.err, "morphring: no files to convert.\n");
        assert_eq!(r.out, "");
    }
}

#[test]
fn unknown_and_read_only_targets_are_usage_errors() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "a.png");

    let r = run_in(
        tmp.path(),
        png_to_jpg(),
        &["convert", "a.png", "--to", "nope"],
    );
    assert_eq!(r.code, 2);
    assert!(
        r.err.starts_with("morphring: unknown format 'nope'."),
        "{}",
        r.err
    );

    // Something has to read raw for "reads" to be true (FX-08).
    let raw_reader = FakeConverter::new("fake").edge("raw", "png", 10);
    let r = run_in(
        tmp.path(),
        vec![Box::new(raw_reader)],
        &["convert", "a.png", "--to", "raw"],
    );
    assert_eq!(r.code, 2);
    assert_eq!(
        r.err,
        "morphring: MorphRing reads RAW but can't write it.\n"
    );

    // The target is checked first: a bad target beats a missing input.
    let r = run_in(
        tmp.path(),
        png_to_jpg(),
        &["convert", "gone.png", "--to", "nope"],
    );
    assert_eq!(r.code, 2);
    assert!(!r.err.contains("not found"));
}

#[test]
fn an_unsupported_file_fails_with_exit_1() {
    let tmp = TempDir::new().unwrap();
    copy_fixture("notes.unknownext", tmp.path());
    let r = run_in(
        tmp.path(),
        png_to_jpg(),
        // jpg, which the fake engine writes: --to pdf would now be refused
        // before the file is even looked at (FX-08).
        &["convert", "notes.unknownext", "--to", "jpg"],
    );
    assert_eq!(r.code, 1);
    assert!(r.out.starts_with("fail  notes.unknownext: "), "{}", r.out);
    assert!(r.out.contains("isn't supported (.unknownext)"), "{}", r.out);
    assert!(r.out.ends_with("Converted 0 of 1 file(s), 1 failed.\n"));
}

#[test]
fn a_missing_engine_names_the_program_and_its_hint() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "report.docx");
    let soffice = FakeConverter::new("soffice")
        .display_name("LibreOffice")
        .edge("docx", "pdf", 10)
        .missing("pacman -S libreoffice-fresh");
    let r = run_in(
        tmp.path(),
        vec![Box::new(soffice)],
        &["convert", "report.docx", "--to", "pdf"],
    );
    assert_eq!(r.code, 1);
    let fail = r.out.lines().next().unwrap();
    assert!(fail.starts_with("fail  report.docx: "), "{fail}");
    assert!(fail.contains("needs"), "{fail}");
    assert!(fail.contains("pacman -S libreoffice-fresh"), "{fail}");
}

#[test]
fn an_engine_failure_fails_only_its_file() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "a.png");
    touch(tmp.path(), "b.gif");
    let engines: Vec<Box<dyn Converter>> = vec![
        Box::new(FakeConverter::new("good").edge("png", "jpg", 10)),
        Box::new(
            FakeConverter::new("bad")
                .edge("gif", "jpg", 10)
                .behaviour(Behaviour::Fail("boom".into())),
        ),
    ];
    let r = run_in(tmp.path(), engines, &["convert", ".", "--to", "jpg"]);
    assert_eq!(r.code, 1);
    assert!(r.out.contains("ok    a.png -> a.jpg\n"), "{}", r.out);
    assert!(r.out.contains("fail  b.gif: The engine reported an error"));
    assert!(r.out.ends_with("Converted 1 of 2 file(s), 1 failed.\n"));
}

#[test]
fn a_cancelled_job_exits_1_and_says_stop() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "a.png");
    let cancel = CancelToken::new();
    cancel.cancel();
    let r = run_with_cancel(
        tmp.path(),
        png_to_jpg(),
        &["convert", "a.png", "--to", "jpg"],
        cancel,
    );
    assert_eq!(r.code, 1);
    assert_eq!(
        r.out,
        "stop  a.png (cancelled)\nConverted 0 of 1 file(s), 1 failed.\n"
    );
    assert!(!tmp.path().join("a.jpg").exists());
}

/// Remembers the quality every step was asked for.
struct QualitySpy {
    edges: Vec<Edge>,
    seen: Arc<Mutex<Vec<Option<u8>>>>,
}

impl Converter for QualitySpy {
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
            .push(step.options.quality);
        let Some(path) = out.allocate(step.to.as_str(), "") else {
            return Ok(Vec::new());
        };
        fs::write(&path, b"spy")?;
        Ok(vec![path])
    }
}

#[test]
fn quality_reaches_the_engine() {
    let tmp = TempDir::new().unwrap();
    touch(tmp.path(), "a.png");
    for (extra, want) in [(&["-q", "42"][..], Some(42)), (&[], None)] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let spy = QualitySpy {
            edges: vec![Edge::new("png", "jpg")],
            seen: Arc::clone(&seen),
        };
        let args = [&["convert", "a.png", "--to", "jpg"][..], extra].concat();
        let r = run_in(tmp.path(), vec![Box::new(spy)], &args);
        assert_eq!(r.code, 0, "{r:?}");
        assert_eq!(*seen.lock().unwrap(), vec![want]);
    }
}

// Golden cases with the real engines. They check what came out, not its bytes.

#[test]
fn magick_converts_png_to_jpg() {
    require_engine!("magick");
    let tmp = TempDir::new().unwrap();
    let png = fixture("sample.png");
    let png = png.to_str().unwrap();
    let r = run_in(
        tmp.path(),
        morphring_engines::registry::all(),
        &["convert", png, "--to", "jpg", "-o", "out"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    let jpg = tmp.path().join("out/sample.jpg");
    assert_eq!(&fs::read(&jpg).unwrap()[..2], b"\xff\xd8", "not a JPEG");
}

#[test]
fn mp4_to_jpg_takes_two_hops() {
    require_engine!("ffmpeg");
    require_engine!("magick");
    let tmp = TempDir::new().unwrap();
    let mp4 = fixture("sample.mp4");
    // -o keeps the output out of the fixtures folder in the repo.
    let r = run_in(
        tmp.path(),
        morphring_engines::registry::all(),
        &[
            "convert",
            mp4.to_str().unwrap(),
            "--to",
            "jpg",
            "-o",
            "out",
            "--json",
        ],
    );
    assert_eq!(r.code, 0, "{r:?}");
    let v: Value = serde_json::from_str(&r.out).expect("stdout is one JSON document");
    assert_eq!(v["converted"], 1, "{v}");
    let outputs = v["files"][0]["outputs"].as_array().unwrap();
    assert!(!outputs.is_empty());
    for output in outputs {
        let path = Path::new(output.as_str().unwrap());
        assert!(path.starts_with(tmp.path().join("out")), "{path:?}");
        assert_eq!(&fs::read(path).unwrap()[..2], b"\xff\xd8", "not a JPEG");
    }
}

#[test]
fn soffice_converts_docx_to_pdf() {
    require_engine!("soffice");
    let tmp = TempDir::new().unwrap();
    copy_fixture("sample.docx", tmp.path());
    let r = run_in(
        tmp.path(),
        morphring_engines::registry::all(),
        &["convert", "sample.docx", "--to", "pdf"],
    );
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(
        r.out.lines().next(),
        Some("ok    sample.docx -> sample.pdf")
    );
    assert_eq!(
        &fs::read(tmp.path().join("sample.pdf")).unwrap()[..5],
        b"%PDF-"
    );
}
