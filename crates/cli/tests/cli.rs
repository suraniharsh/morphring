//! The `morphring` command end to end, ported from Filee's
//! `tests/Filee.Cli.Tests/CliTests.cs`.
//!
//! Like Filee, these run the real engines on files in a temporary folder.
//! Everything goes through `run_with()` in-process, except help and version,
//! which run the built binary with an empty `PATH` to prove they need nothing.
//!
//! Still waiting for their features (v0.2 tasks P2-40 to P2-42):
//! - `Keep_dates_copies_the_dates_of_the_originals` (--keep-dates is parsed
//!   but not applied yet)
//! - `Watch_needs_an_existing_folder` and
//!   `Watch_converts_files_that_arrive_until_cancelled` (watch is a stub)
//! - the unknown-preset row of `Wrong_arguments_explain_themselves_with_exit_code_2`,
//!   the `to-png` check of `Formats_and_presets_are_listed`, and the --preset
//!   in `A_folder_converts_the_files_filee_knows` (here it says --to jpg)

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

use common::{Outcome, fixture, run_in};

/// Runs `morphring <args>` in `dir` with every installed engine.
fn run(dir: &Path, args: &[&str]) -> Outcome {
    run_in(dir, morphring_engines::registry::all(), args)
}

/// Puts a small PNG called `name` into `dir`. Filee painted an orange 40x30
/// rectangle with Magick.NET; I just copy the fixture, which is cheaper and
/// equally orange-agnostic.
fn png(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::copy(fixture("sample.png"), &path).unwrap_or_else(|e| panic!("copy to {path:?}: {e}"));
    path
}

fn str_of(path: &Path) -> &str {
    path.to_str()
        .unwrap_or_else(|| panic!("{path:?} is not UTF-8"))
}

fn json(out: &str) -> Value {
    serde_json::from_str(out)
        .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}): {out}"))
}

/// The first output of every file in a `--json` report.
fn first_outputs(v: &Value) -> Vec<PathBuf> {
    let Some(files) = v["files"].as_array() else {
        panic!("files is not an array: {v}")
    };
    files
        .iter()
        .map(|file| match file["outputs"][0].as_str() {
            Some(path) => PathBuf::from(path),
            None => panic!("no output path in {file}"),
        })
        .collect()
}

#[test]
fn converts_files_and_wildcards_into_an_output_folder() {
    require_engine!("magick");
    let tmp = TempDir::new().unwrap();
    png(tmp.path(), "a.png");
    png(tmp.path(), "b.png");
    let wildcard = tmp.path().join("*.png");
    let out = tmp.path().join("out");

    let r = run(
        tmp.path(),
        &[
            "convert",
            str_of(&wildcard),
            "--to",
            "jpg",
            "-o",
            str_of(&out),
            "--quality",
            "70",
        ],
    );

    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.err, "");
    assert!(r.out.contains("Converted 2 of 2"), "{}", r.out);
    assert!(out.join("a.jpg").is_file());
    assert!(out.join("b.jpg").is_file());
}

#[test]
fn json_output_lists_every_file() {
    require_engine!("magick");
    let tmp = TempDir::new().unwrap();
    let source = png(tmp.path(), "a.png");

    let r = run(
        tmp.path(),
        &["convert", str_of(&source), "--to", ".webp", "--json"],
    );

    assert_eq!(r.code, 0, "{r:?}");
    let v = json(&r.out);
    assert_eq!(v["converted"], 1, "{v}");
    assert_eq!(v["files"][0]["state"], "done", "{v}");
    let output = &first_outputs(&v)[0];
    assert!(output.ends_with("a.webp"), "{output:?}");
    assert!(output.is_file());
}

#[test]
fn a_folder_converts_the_files_it_knows() {
    require_engine!("magick");
    let tmp = TempDir::new().unwrap();
    png(tmp.path(), "a.png");
    fs::write(tmp.path().join("notes.unknownformat"), "x").unwrap();

    let r = run(tmp.path(), &["convert", str_of(tmp.path()), "--to", "jpg"]);

    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("Converted 1 of 1"), "{}", r.out);
    assert!(tmp.path().join("a.jpg").is_file());
}

#[test]
fn a_file_that_cannot_be_converted_fails_with_exit_code_1() {
    let tmp = TempDir::new().unwrap();
    let text = tmp.path().join("notes.txt");
    fs::write(&text, "hello").unwrap();

    // No engine turns text into sound, installed or not, so this one needs no
    // require_engine!: it fails the same way on an empty machine.
    let r = run(tmp.path(), &["convert", str_of(&text), "--to", "mp3"]);

    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.out.contains("fail"), "{}", r.out);
}

#[test]
fn wrong_arguments_explain_themselves_with_exit_code_2() {
    let tmp = TempDir::new().unwrap();
    let a = png(tmp.path(), "a.png");
    let a = str_of(&a);

    // Filee's table minus the unknown-preset row, which waits for P2-41.
    let cases: [(&[&str], &str); 6] = [
        (
            &["convert", a],
            "Use either --to <format> or --preset <preset>.",
        ),
        (
            &["convert", a, "--to", "jpg", "--preset", "to-png"],
            "Use either --to <format> or --preset <preset>.",
        ),
        (&["convert", a, "--to", "nosuchformat"], "unknown format"),
        (&["convert", "--to", "jpg"], "at least one file"),
        (
            &["convert", a, "--to", "jpg", "--quality", "300"],
            "--quality",
        ),
        (&["frobnicate"], "Unknown command"),
    ];
    for (args, message) in cases {
        let r = run(tmp.path(), args);
        assert_eq!(r.code, 2, "{args:?}: {r:?}");
        assert!(
            r.err.to_lowercase().contains(&message.to_lowercase()),
            "{args:?}: {}",
            r.err
        );
    }
}

#[test]
fn a_missing_input_is_reported() {
    let tmp = TempDir::new().unwrap();
    let missing = tmp.path().join("missing.png");

    let r = run(tmp.path(), &["convert", str_of(&missing), "--to", "jpg"]);

    assert_eq!(r.code, 2, "{r:?}");
    assert!(r.err.contains("not found"), "{}", r.err);
}

#[test]
fn presets_are_listed() {
    let tmp = TempDir::new().unwrap();

    let r = run(tmp.path(), &["presets"]);

    // The table itself arrives with P2-40; for now the command only has to
    // exist and succeed quietly.
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.err, "");
}

#[test]
fn formats_are_listed() {
    // What heic converts to depends on the installed engines; magick is the
    // one that reads it.
    require_engine!("magick");
    let tmp = TempDir::new().unwrap();

    let r = run(tmp.path(), &["formats", "heic"]);

    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("jpg"), "{}", r.out);

    let r = run(tmp.path(), &["formats"]);
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("png"), "{}", r.out);
}

#[test]
fn help_and_version_need_no_engines() {
    // A PATH pointing at an empty folder: if help or version went looking for
    // magick, they would find nothing and we'd know about it.
    let empty = TempDir::new().unwrap();
    let morphring = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_morphring"))
            .args(args)
            .env("PATH", empty.path())
            .current_dir(empty.path())
            .output()
            .unwrap_or_else(|e| panic!("cannot start morphring: {e}"));
        (
            output.status.code(),
            String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        )
    };

    let (code, out) = morphring(&["help", "convert"]);
    assert_eq!(code, Some(0));
    assert!(out.contains("--to"), "{out}");

    let (code, out) = morphring(&["help", "watch"]);
    assert_eq!(code, Some(0));
    assert!(out.contains("--move-originals"), "{out}");
    assert!(out.contains("--settle <seconds>"), "{out}");
    #[cfg(not(windows))]
    assert!(out.contains("(default 5)"), "{out}");

    let (code, out) = morphring(&["help", "presets"]);
    assert_eq!(code, Some(0));
    assert!(out.contains("--preset <id or name>"), "{out}");

    let (code, out) = morphring(&["--version"]);
    assert_eq!(code, Some(0));
    assert!(out.starts_with("morphring "), "{out}");
}

// Case-sensitivity is a property of the file system, not the OS, but Linux is
// the one place we know the temp folder tells PHOTO.png and photo.png apart.
#[cfg(target_os = "linux")]
#[test]
fn case_distinct_inputs_are_both_converted_and_exact_duplicates_are_removed() {
    require_engine!("magick");
    let tmp = TempDir::new().unwrap();
    let lower = png(tmp.path(), "photo.png");
    let upper = png(tmp.path(), "PHOTO.png");
    let out = tmp.path().join("out");

    let r = run(
        tmp.path(),
        &[
            "convert",
            str_of(&lower),
            str_of(&upper),
            str_of(&lower),
            "--to",
            "jpg",
            "-o",
            str_of(&out),
            "--json",
        ],
    );

    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.err, "");
    let v = json(&r.out);
    assert_eq!(v["converted"], 2, "{v}");
    assert_eq!(v["files"].as_array().map(Vec::len), Some(2), "{v}");
    let outputs = first_outputs(&v);
    // Filee compares these case-insensitively, because its job queue reserves
    // names with OrdinalIgnoreCase and Windows can't tell PHOTO.jpg from
    // photo.jpg anyway. Our reservations are exact paths (PLAN T09), and on
    // Linux those really are two files, so two different paths is the bar.
    assert_ne!(outputs[0], outputs[1]);
    for output in &outputs {
        assert!(output.is_file(), "{output:?}");
    }
}

#[test]
fn unicode_spaces_and_quotes_stay_in_a_single_input_argument() {
    require_engine!("magick");
    let tmp = TempDir::new().unwrap();
    // Windows forbids " in file names; curly quotes are allowed there and
    // still need quoting on a command line.
    let name = if cfg!(windows) {
        "한글 report's “quoted” 100%.png"
    } else {
        "한글 report's \"quoted\" 100%.png"
    };
    let source = png(tmp.path(), name);
    let out = tmp.path().join("결과 output's");

    let r = run(
        tmp.path(),
        &[
            "convert",
            str_of(&source),
            "--to",
            "jpg",
            "-o",
            str_of(&out),
            "--json",
        ],
    );

    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.err, "");
    let v = json(&r.out);
    assert_eq!(v["converted"], 1, "{v}");
    let result = &first_outputs(&v)[0];
    assert_eq!(result.parent(), Some(out.as_path()));
    let name = result.file_name().unwrap().to_string_lossy();
    assert!(name.contains("한글 report's"), "{name}");
    assert!(result.is_file());
    // Drops are copies: the source is still there.
    assert!(source.is_file());
}
