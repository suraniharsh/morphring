//! Sanity checks on `tauri.conf.json` and the files it points at. A typo in
//! there only shows up at runtime as a blank window, so we'd rather fail here.

// The helpers below aren't #[test] fns, so clippy's allow-unwrap-in-tests skips
// them. This whole file is test code; panicking on a broken config is the point.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use serde_json::Value;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn config() -> Value {
    let text = std::fs::read_to_string(manifest_dir().join("tauri.conf.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn window<'a>(config: &'a Value, label: &str) -> &'a Value {
    config["app"]["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["label"] == label)
        .unwrap_or_else(|| panic!("no window labelled {label}"))
}

fn frontend_dir(config: &Value) -> PathBuf {
    manifest_dir().join(config["build"]["frontendDist"].as_str().unwrap())
}

#[test]
fn main_window_serves_the_drop_zone() {
    let config = config();
    let main = window(&config, "main");
    assert_eq!(main["dragDropEnabled"], true);
    let page = frontend_dir(&config).join(main["url"].as_str().unwrap());
    let html = std::fs::read_to_string(&page).unwrap();
    assert!(
        html.contains("id=\"drop-zone\""),
        "{} has no drop zone",
        page.display()
    );
}

#[test]
fn ring_window_starts_hidden_and_undecorated() {
    let config = config();
    let ring = window(&config, "ring");
    assert_eq!(ring["visible"], false);
    assert_eq!(ring["decorations"], false);
    assert_eq!(ring["transparent"], true);
    assert_eq!(ring["focus"], false);
}

#[test]
fn global_tauri_api_is_on_for_the_bundler_free_ui() {
    assert_eq!(config()["app"]["withGlobalTauri"], true);
}

#[test]
fn every_bundle_icon_exists() {
    let config = config();
    for icon in config["bundle"]["icon"].as_array().unwrap() {
        let path = manifest_dir().join(icon.as_str().unwrap());
        assert!(is_non_empty(&path), "missing icon {}", path.display());
    }
}

#[test]
fn ui_scripts_and_styles_referenced_by_index_exist() {
    let config = config();
    let dir = frontend_dir(&config);
    let html = std::fs::read_to_string(dir.join("index.html")).unwrap();
    for asset in ["main.js", "style.css"] {
        assert!(html.contains(asset), "index.html does not load {asset}");
        assert!(is_non_empty(&dir.join(asset)), "missing {asset}");
    }
}

/// Every window the app declares, with the page it loads.
const WINDOWS: [(&str, &str); 5] = [
    ("main", "index.html"),
    ("ring", "ring.html"),
    ("settings", "settings.html"),
    ("history", "history.html"),
    ("toast", "toast.html"),
];

#[test]
fn every_window_is_declared_with_its_page() {
    let config = config();
    let windows = config["app"]["windows"].as_array().unwrap();
    assert_eq!(windows.len(), WINDOWS.len(), "unexpected windows");
    for (label, page) in WINDOWS {
        assert_eq!(window(&config, label)["url"], page, "{label}");
    }
}

#[test]
fn every_window_has_the_default_capability() {
    let text = std::fs::read_to_string(manifest_dir().join("capabilities/default.json")).unwrap();
    let caps: Value = serde_json::from_str(&text).unwrap();
    let labels: Vec<&str> = caps["windows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    for (label, _) in WINDOWS {
        assert!(labels.contains(&label), "{label} has no capability");
    }
    let permissions = caps["permissions"].as_array().unwrap();
    assert!(
        permissions.iter().any(|p| p == "dialog:default"),
        "the settings page needs the folder picker"
    );
}

#[test]
fn every_page_exists_and_loads_only_its_own_scripts() {
    let config = config();
    let dir = frontend_dir(&config);
    for (label, page) in WINDOWS {
        let html = std::fs::read_to_string(dir.join(page))
            .unwrap_or_else(|e| panic!("{label}: cannot read {page}: {e}"));
        let scripts = script_sources(&html, page);
        assert!(!scripts.is_empty(), "{page} loads no script");
        for src in scripts {
            // The CSP says script-src 'self': anything from elsewhere would be a
            // blank window at runtime, so it is a failed test here.
            assert!(
                !src.contains("://") && !src.starts_with("//") && !src.starts_with("data:"),
                "{page} loads a script from elsewhere: {src}"
            );
            assert!(
                is_non_empty(&dir.join(&src)),
                "{page} loads {src}, which is missing"
            );
        }
    }
}

#[test]
fn settings_and_history_start_hidden() {
    let config = config();
    for (label, width, height) in [("settings", 640, 560), ("history", 640, 520)] {
        let w = window(&config, label);
        assert_eq!(w["visible"], false, "{label}");
        assert_eq!(
            (w["width"].as_u64(), w["height"].as_u64()),
            (Some(width), Some(height))
        );
    }
}

#[test]
fn toast_window_never_steals_the_focus() {
    let config = config();
    let toast = window(&config, "toast");
    for (key, value) in [
        ("visible", false),
        ("decorations", false),
        ("transparent", true),
        ("alwaysOnTop", true),
        ("skipTaskbar", true),
        ("focus", false),
        ("shadow", false),
    ] {
        assert_eq!(toast[key], value, "toast {key}");
    }
    assert_eq!(toast["width"], 360);
    assert_eq!(toast["height"], 120);
}

/// The `src` of every `<script>` in `html`. Panics on an inline script, which
/// the CSP would refuse to run anyway.
fn script_sources(html: &str, page: &str) -> Vec<String> {
    // A regex crate for a handful of hand-written pages would be overkill; a
    // careful split on "<script" does the job and stays readable.
    html.split("<script")
        .skip(1)
        .map(|rest| {
            let (tag, after) = rest.split_once('>').unwrap();
            let body = after.split("</script>").next().unwrap();
            assert!(body.trim().is_empty(), "{page} has an inline script");
            let src = tag
                .split_once("src=\"")
                .and_then(|(_, v)| v.split_once('"'))
                .map(|(v, _)| v.to_owned());
            src.unwrap_or_else(|| panic!("{page} has a script without src"))
        })
        .collect()
}

fn is_non_empty(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0)
}
