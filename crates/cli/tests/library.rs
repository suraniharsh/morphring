//! `morphring presets export|import|reset`, against temporary config folders
//! so the real `~/.config/morphring` sleeps through all of it.

mod common;

use std::fs;
use std::path::Path;

use morphring_core::bundle::BUNDLE_FORMAT;
use morphring_core::profiles::{Library, PRESETS_FILE};
use morphring_core::settings::Store;
use serde_json::Value;
use tempfile::TempDir;

use common::{Outcome, run_in, run_in_with_config};

/// A presets.json with one preset of our own, so a test can tell a custom
/// library from the factory one.
const CUSTOM_PRESETS: &str = r#"{
  "schemaVersion": 2,
  "presets": [
    { "id": "tiny-webp", "name": "Tiny WebP", "target": "webp" }
  ]
}"#;

fn temp() -> TempDir {
    TempDir::new().unwrap_or_else(|e| panic!("no temp dir: {e}"))
}

fn run(cwd: &Path, config: &Path, args: &[&str]) -> Outcome {
    run_in_with_config(cwd, config, Vec::new(), args)
}

fn library(config: &Path) -> Library {
    Library::load(&Store::new(Some(config.to_path_buf())))
}

fn item_count(library: &Library) -> usize {
    library.presets.len() + library.profiles.len()
}

#[test]
fn export_writes_a_bundle_relative_to_the_cwd() {
    let (cwd, config) = (temp(), temp());
    fs::write(config.path().join(PRESETS_FILE), CUSTOM_PRESETS).unwrap();

    let outcome = run(
        cwd.path(),
        config.path(),
        &["presets", "export", "morphring-presets.json"],
    );
    assert_eq!(outcome.code, 0, "stderr: {}", outcome.err);
    assert!(outcome.err.is_empty(), "stderr: {}", outcome.err);

    let file = cwd.path().join("morphring-presets.json");
    let value: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(value["format"], BUNDLE_FORMAT);
    let ids: Vec<&str> = value["presets"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["id"].as_str())
        .collect();
    assert!(ids.contains(&"tiny-webp"), "{ids:?}");

    let count = item_count(&library(config.path()));
    assert_eq!(
        outcome.out,
        format!("Exported {count} items to {}\n", file.display())
    );
}

#[test]
fn export_to_a_missing_folder_fails() {
    let (cwd, config) = (temp(), temp());
    let outcome = run(
        cwd.path(),
        config.path(),
        &["presets", "export", "nope/out.json"],
    );
    assert_eq!(outcome.code, 1);
    assert!(outcome.out.is_empty(), "{}", outcome.out);
    assert!(
        outcome.err.starts_with("morphring: cannot write "),
        "{}",
        outcome.err
    );
}

#[test]
fn export_then_import_into_a_fresh_config_round_trips() {
    let (cwd, from, to) = (temp(), temp(), temp());
    fs::write(from.path().join(PRESETS_FILE), CUSTOM_PRESETS).unwrap();
    let bundle = cwd.path().join("bundle.json");
    let bundle_arg = bundle.to_str().unwrap();

    let exported = run(cwd.path(), from.path(), &["presets", "export", bundle_arg]);
    assert_eq!(exported.code, 0, "stderr: {}", exported.err);

    let imported = run(cwd.path(), to.path(), &["presets", "import", bundle_arg]);
    assert_eq!(imported.code, 0, "stderr: {}", imported.err);
    assert!(imported.err.is_empty(), "stderr: {}", imported.err);

    let source = library(from.path());
    let count = item_count(&source);
    assert_eq!(imported.out, format!("Imported {count} items\n"));
    // Saved to disk, not just admired in memory.
    assert!(to.path().join(PRESETS_FILE).exists());
    let target = library(to.path());
    assert!(target.preset("tiny-webp").is_some());
    // Import merges: everything exported arrives, and the fresh config's
    // built-ins stay put alongside it.
    for preset in &source.presets {
        assert_eq!(target.preset(&preset.id), Some(preset), "{}", preset.id);
    }
    assert!(target.preset("to-png").is_some());
}

#[test]
fn import_takes_a_filee_bundle() {
    let (cwd, config) = (temp(), temp());
    fs::write(
        cwd.path().join("filee-presets.json"),
        r#"{
  "schemaVersion": 1,
  "presets": [{ "id": "from-filee", "name": "From Filee", "targetFormat": "jpg" }],
  "profiles": []
}"#,
    )
    .unwrap();

    let outcome = run(
        cwd.path(),
        config.path(),
        &["presets", "import", "filee-presets.json"],
    );
    assert_eq!(outcome.code, 0, "stderr: {}", outcome.err);
    assert_eq!(outcome.out, "Imported 1 items\n");
    assert_eq!(
        library(config.path()).preset("from-filee").unwrap().target,
        "jpg"
    );
}

#[test]
fn a_bad_import_changes_nothing() {
    let (cwd, config) = (temp(), temp());
    fs::write(config.path().join(PRESETS_FILE), CUSTOM_PRESETS).unwrap();
    let before = fs::read_to_string(config.path().join(PRESETS_FILE)).unwrap();
    fs::write(cwd.path().join("settings.json"), r#"{ "theme": "dark" }"#).unwrap();
    fs::write(cwd.path().join("broken.json"), "{ not json").unwrap();

    for (file, needle) in [
        ("settings.json", "not a preset library"),
        ("broken.json", "not a valid preset library"),
        ("missing.json", "cannot read "),
    ] {
        let outcome = run(cwd.path(), config.path(), &["presets", "import", file]);
        assert_eq!(outcome.code, 1, "{file}");
        assert!(outcome.out.is_empty(), "{file}: {}", outcome.out);
        assert!(outcome.err.starts_with("morphring: "), "{}", outcome.err);
        assert!(outcome.err.contains(needle), "{file}: {}", outcome.err);
        assert_eq!(
            fs::read_to_string(config.path().join(PRESETS_FILE)).unwrap(),
            before,
            "{file} touched presets.json"
        );
    }
}

#[test]
fn reset_with_yes_restores_the_built_ins() {
    let (cwd, config) = (temp(), temp());
    fs::write(config.path().join(PRESETS_FILE), CUSTOM_PRESETS).unwrap();
    assert!(library(config.path()).preset("tiny-webp").is_some());

    for flag in ["--yes", "-y"] {
        let outcome = run(cwd.path(), config.path(), &["presets", "reset", flag]);
        assert_eq!(outcome.code, 0, "stderr: {}", outcome.err);
        assert!(outcome.err.is_empty(), "stderr: {}", outcome.err);
        assert_eq!(
            outcome.out,
            "Presets and toolbar profiles are back to the defaults.\n"
        );
        assert_eq!(library(config.path()), Library::builtin());
    }
}

#[test]
fn import_and_reset_refuse_without_a_config_folder() {
    let cwd = temp();
    fs::write(cwd.path().join("b.json"), r#"{ "presets": [] }"#).unwrap();
    for args in [
        &["presets", "import", "b.json"][..],
        &["presets", "reset", "--yes"][..],
    ] {
        let outcome = run_in(cwd.path(), Vec::new(), args);
        assert_eq!(outcome.code, 1, "{args:?}");
        assert_eq!(
            outcome.err,
            "morphring: no config folder to keep the presets in.\n"
        );
    }
}

/// The prompt path needs a real stdin, so these run the binary. Linux only:
/// that is where `XDG_CONFIG_HOME` decides the config folder.
#[cfg(target_os = "linux")]
mod binary {
    use std::io::Write;
    use std::process::{Command, Stdio};

    use super::*;

    /// Runs the real `morphring presets reset` with `stdin_text` piped in (or
    /// a null stdin) and the config folder under `xdg`.
    fn reset(xdg: &Path, stdin_text: Option<&str>) -> std::process::Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_morphring"))
            .args(["presets", "reset"])
            .env("XDG_CONFIG_HOME", xdg)
            .stdin(if stdin_text.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("cannot start morphring: {e}"));
        if let Some(text) = stdin_text {
            let mut stdin = child
                .stdin
                .take()
                .unwrap_or_else(|| panic!("no stdin pipe"));
            stdin
                .write_all(text.as_bytes())
                .unwrap_or_else(|e| panic!("cannot write stdin: {e}"));
        }
        child
            .wait_with_output()
            .unwrap_or_else(|e| panic!("morphring did not finish: {e}"))
    }

    #[test]
    fn reset_without_yes_off_a_terminal_exits_2_and_changes_nothing() {
        let xdg = temp();
        let config = xdg.path().join("morphring");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join(PRESETS_FILE), CUSTOM_PRESETS).unwrap();

        // A piped "y" is still not a person at a terminal.
        for stdin_text in [None, Some("y\n")] {
            let output = reset(xdg.path(), stdin_text);
            assert_eq!(output.status.code(), Some(2), "{stdin_text:?}");
            assert!(output.stdout.is_empty());
            assert_eq!(
                String::from_utf8_lossy(&output.stderr),
                "morphring: 'presets reset' needs --yes when not run from a terminal.\n"
            );
            assert_eq!(
                fs::read_to_string(config.join(PRESETS_FILE)).unwrap(),
                CUSTOM_PRESETS
            );
        }
    }
}
