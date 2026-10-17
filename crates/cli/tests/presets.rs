//! `morphring presets`, against a temporary config folder so the real
//! `~/.config/morphring` never gets a say.

mod common;

use std::fs;

use tempfile::TempDir;

use common::run_in_with_config;

/// The hint printed under the table.
const HINT: &str = "Use one with: morphring convert <files> --preset <id or name>";

/// Runs `morphring presets` with `config` as the config folder.
fn presets(config: &TempDir) -> common::Outcome {
    let cwd = TempDir::new().unwrap_or_else(|e| panic!("no temp dir: {e}"));
    run_in_with_config(cwd.path(), config.path(), Vec::new(), &["presets"])
}

/// The first whitespace-separated word of every table row (the ids).
fn ids(out: &str) -> Vec<&str> {
    out.lines()
        .skip(1)
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .collect()
}

#[test]
fn defaults_list_the_built_ins() {
    let config = TempDir::new().unwrap();
    let outcome = presets(&config);

    assert_eq!(outcome.code, 0, "stderr: {}", outcome.err);
    assert!(outcome.err.is_empty(), "stderr: {}", outcome.err);
    let mut lines = outcome.out.lines();
    let header = lines.next().unwrap();
    assert_eq!(header, format!("{:<24} {:<28} TARGET", "ID", "NAME"));

    let ids = ids(&outcome.out);
    assert!(ids.contains(&"to-png"), "{}", outcome.out);
    assert!(ids.contains(&"merge-pdf"), "{}", outcome.out);
    assert_eq!(
        ids.len(),
        morphring_core::builtin::presets().len(),
        "{}",
        outcome.out
    );

    // Keyed names come out in English, not as `preset.merge_pdf`.
    let merge = outcome
        .out
        .lines()
        .find(|line| line.starts_with("merge-pdf "))
        .unwrap();
    assert!(merge.contains("Merge PDF"), "{merge}");

    assert!(
        outcome.out.ends_with(&format!("\n\n{HINT}\n")),
        "{}",
        outcome.out
    );
    // Listing is read-only: no presets.json appears out of thin air.
    assert!(!config.path().join("presets.json").exists());
}

#[test]
fn a_custom_presets_file_lists_its_own_presets_in_order() {
    let config = TempDir::new().unwrap();
    fs::write(
        config.path().join("presets.json"),
        r#"{
  "schemaVersion": 2,
  "presets": [
    { "id": "tiny-webp", "name": "Tiny WebP", "target": "webp" },
    { "id": "my-merge", "nameKey": "preset.merge_pdf", "target": "pdf" }
  ]
}"#,
    )
    .unwrap();

    let outcome = presets(&config);

    assert_eq!(outcome.code, 0, "stderr: {}", outcome.err);
    assert_eq!(ids(&outcome.out), ["tiny-webp", "my-merge"]);
    let lines: Vec<&str> = outcome.out.lines().collect();
    assert_eq!(
        lines[1],
        format!("{:<24} {:<28} webp", "tiny-webp", "Tiny WebP")
    );
    assert_eq!(
        lines[2],
        format!("{:<24} {:<28} pdf", "my-merge", "Merge PDF")
    );
    assert_eq!(lines.len(), 5, "{}", outcome.out);
    assert_eq!(lines[4], HINT);
}

#[test]
fn a_corrupt_presets_file_still_lists_the_built_ins() {
    let config = TempDir::new().unwrap();
    let file = config.path().join("presets.json");
    fs::write(&file, "{ this is not json").unwrap();

    let outcome = presets(&config);

    assert_eq!(outcome.code, 0, "stderr: {}", outcome.err);
    let ids = ids(&outcome.out);
    assert!(ids.contains(&"to-png"), "{}", outcome.out);
    assert!(ids.contains(&"merge-pdf"), "{}", outcome.out);
    assert!(
        config.path().join("presets.json.broken").exists(),
        "the corrupt file should be kept as presets.json.broken"
    );
}
