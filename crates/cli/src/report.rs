//! Human and JSON reports over [`FileResult`]s, ported from Filee's
//! `src/Filee.Cli/Cli.cs` (`Describe`, `Error`, `Shorten`, `WriteJson`) and the
//! English texts in `src/Filee.Cli/EnglishTexts.cs`.
//!
//! Everything here is pure formatting: the convert command decides when to
//! print what, this module only decides what it looks like.

use std::borrow::Cow;
use std::io::{self, Write};
use std::path::Path;

use morphring_core::{ErrorKey, FileResult, FileState};
use serde::{Deserialize, Serialize};

/// The English text shown for an error key.
pub fn message(key: ErrorKey) -> &'static str {
    match key {
        ErrorKey::UnsupportedSource => "This file type isn't supported",
        ErrorKey::NoRoute => "No engine can do this conversion",
        ErrorKey::EngineMissing => "A required program is not installed",
        ErrorKey::ConversionFailed => "The engine reported an error",
    }
}

/// The error text of a file, `message (detail)`, or `None` when the file did
/// not fail. The detail is left out when it is blank.
pub fn error_text(state: &FileState) -> Option<String> {
    match state {
        FileState::Failed { key, detail } if detail.trim().is_empty() => {
            Some(message(*key).to_owned())
        }
        FileState::Failed { key, detail } => Some(format!("{} ({detail})", message(*key))),
        // Filee says "conversion failed" here because a cancelled file has no key.
        // That reads like something broke, so we say what actually happened.
        FileState::Cancelled => Some("cancelled".to_owned()),
        FileState::Done | FileState::Skipped => None,
    }
}

/// One human line for a finished file. Paths inside `cwd` are shown relative
/// to it, everything else in full.
pub fn file_line(file: &FileResult, cwd: &Path) -> String {
    let source = shorten(&file.source, cwd);
    match &file.state {
        // Filee prints a dangling "-> " for merge followers with no outputs;
        // we just leave the arrow off instead of pointing at nothing.
        FileState::Done if file.outputs.is_empty() => format!("ok    {source}"),
        FileState::Done => {
            let outputs: Vec<_> = file.outputs.iter().map(|p| shorten(p, cwd)).collect();
            format!("ok    {source} -> {}", outputs.join(", "))
        }
        FileState::Skipped => format!("skip  {source} (the output file already exists)"),
        FileState::Cancelled => format!("stop  {source} (cancelled)"),
        FileState::Failed { .. } => format!(
            "fail  {source}: {}",
            error_text(&file.state).unwrap_or_default()
        ),
    }
}

/// How many files ended in each state. `failed` counts cancelled files too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub converted: usize,
    pub skipped: usize,
    pub failed: usize,
    pub total: usize,
}

impl Counts {
    /// Tallies the files.
    pub fn of(files: &[FileResult]) -> Self {
        files.iter().fold(
            Self {
                total: files.len(),
                ..Self::default()
            },
            |mut c, f| {
                match f.state {
                    FileState::Done => c.converted += 1,
                    FileState::Skipped => c.skipped += 1,
                    FileState::Failed { .. } | FileState::Cancelled => c.failed += 1,
                }
                c
            },
        )
    }
}

/// The summary line, e.g. `Converted 2 of 3 file(s), 1 failed.`
/// Skipped files are not mentioned, as in Filee.
pub fn summary(files: &[FileResult]) -> String {
    let c = Counts::of(files);
    if c.failed == 0 {
        format!("Converted {} of {} file(s).", c.converted, c.total)
    } else {
        format!(
            "Converted {} of {} file(s), {} failed.",
            c.converted, c.total, c.failed
        )
    }
}

/// The `--json` document. Field order is Filee's and is part of the contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonReport {
    /// The preset's display name when `--preset` picked it, `null` for `--to`.
    pub preset: Option<String>,
    /// Target format id.
    pub target: String,
    pub converted: usize,
    pub skipped: usize,
    /// Failed plus cancelled files.
    pub failed: usize,
    /// One entry per input file, in input order.
    pub files: Vec<JsonFile>,
}

/// One file of the `--json` document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonFile {
    /// Absolute source path.
    pub source: String,
    /// `done`, `skipped`, `failed` or `cancelled`.
    pub state: String,
    /// Absolute output paths.
    pub outputs: Vec<String>,
    /// `message (detail)`, present only for failed and cancelled files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl JsonReport {
    /// Builds the report. `preset` is the display name of the preset given
    /// with `--preset` (`None` for `--to`). `files` must be in input order;
    /// relative paths are made absolute against `cwd`.
    pub fn new(preset: Option<&str>, target: &str, files: &[FileResult], cwd: &Path) -> Self {
        let c = Counts::of(files);
        Self {
            preset: preset.map(str::to_owned),
            target: target.to_owned(),
            converted: c.converted,
            skipped: c.skipped,
            failed: c.failed,
            files: files
                .iter()
                .map(|f| JsonFile {
                    source: absolute(&f.source, cwd),
                    state: state_name(&f.state).to_owned(),
                    outputs: f.outputs.iter().map(|p| absolute(p, cwd)).collect(),
                    error: error_text(&f.state),
                })
                .collect(),
        }
    }
}

/// Writes the `--json` document (2-space indent) followed by a newline.
/// `preset` is as in [`JsonReport::new`].
pub fn write_json(
    out: &mut dyn Write,
    preset: Option<&str>,
    target: &str,
    files: &[FileResult],
    cwd: &Path,
) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *out, &JsonReport::new(preset, target, files, cwd))?;
    writeln!(out)
}

fn state_name(state: &FileState) -> &'static str {
    match state {
        FileState::Done => "done",
        FileState::Skipped => "skipped",
        FileState::Failed { .. } => "failed",
        FileState::Cancelled => "cancelled",
    }
}

/// Relative to `cwd` when the path sits inside it, else the full path.
fn shorten<'a>(path: &'a Path, cwd: &Path) -> Cow<'a, str> {
    if !path.is_absolute() {
        return path.to_string_lossy();
    }
    match pathdiff::diff_paths(path, cwd) {
        // `..` means outside, an empty diff means the folder itself. `starts_with`
        // compares components, so "..foo" is a perfectly good file name inside.
        Some(rel) if !rel.as_os_str().is_empty() && !rel.starts_with("..") => {
            Cow::Owned(rel.to_string_lossy().into_owned())
        }
        _ => path.to_string_lossy(),
    }
}

/// Joins a relative path onto `cwd` without touching the file system
/// (no canonicalize: symlinks stay the way the user typed them).
fn absolute(path: &Path, cwd: &Path) -> String {
    if path.is_absolute() {
        path.to_string_lossy().into_owned()
    } else {
        cwd.join(path).to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    // Windows wants a drive letter before it believes a path is absolute, so
    // "/home/u/work" is merely a suggestion over there. Each OS gets a root it
    // actually takes seriously.
    #[cfg(windows)]
    const CWD: &str = r"C:\work";
    #[cfg(not(windows))]
    const CWD: &str = "/home/u/work";

    #[cfg(windows)]
    const ELSEWHERE: &str = r"C:\elsewhere";
    #[cfg(not(windows))]
    const ELSEWHERE: &str = "/elsewhere";

    /// `rel` inside [`CWD`], joined the way this OS joins paths.
    fn at(rel: &str) -> String {
        Path::new(CWD).join(rel).to_string_lossy().into_owned()
    }

    /// Folds Windows separators to `/` so one expected string fits every OS.
    /// Only ever used on the assertion side; on Unix it changes nothing, so a
    /// stray backslash there still fails loudly.
    fn norm(s: &str) -> String {
        if cfg!(windows) {
            s.replace('\\', "/")
        } else {
            s.to_owned()
        }
    }

    fn result(source: &str, state: FileState, outputs: &[&str]) -> FileResult {
        FileResult {
            source: PathBuf::from(source),
            state,
            outputs: outputs.iter().map(PathBuf::from).collect(),
        }
    }

    fn failed(key: ErrorKey, detail: &str) -> FileState {
        FileState::Failed {
            key,
            detail: detail.into(),
        }
    }

    fn line(file: &FileResult) -> String {
        norm(&file_line(file, Path::new(CWD)))
    }

    #[test]
    fn report_pins_the_four_human_line_shapes() {
        let done = result(
            &at("a.pdf"),
            FileState::Done,
            &[&at("a_p1.png"), &at("a_p2.png")],
        );
        assert_eq!(line(&done), "ok    a.pdf -> a_p1.png, a_p2.png");

        let skipped = result(&at("b.png"), FileState::Skipped, &[]);
        assert_eq!(
            line(&skipped),
            "skip  b.png (the output file already exists)"
        );

        let cancelled = result(&at("c.mp4"), FileState::Cancelled, &[]);
        assert_eq!(line(&cancelled), "stop  c.mp4 (cancelled)");

        let fail = result(
            &at("notes.txt"),
            failed(ErrorKey::NoRoute, "txt → mp3"),
            &[],
        );
        assert_eq!(
            line(&fail),
            "fail  notes.txt: No engine can do this conversion (txt → mp3)"
        );
    }

    #[test]
    fn report_engine_missing_line() {
        let fail = result(
            &at("x.docx"),
            failed(
                ErrorKey::EngineMissing,
                "needs LibreOffice: pacman -S libreoffice-fresh",
            ),
            &[],
        );
        assert_eq!(
            line(&fail),
            "fail  x.docx: A required program is not installed (needs LibreOffice: pacman -S libreoffice-fresh)"
        );
    }

    #[test]
    fn report_messages_for_every_key_and_blank_details() {
        let cases = [
            (
                ErrorKey::UnsupportedSource,
                "This file type isn't supported",
            ),
            (ErrorKey::NoRoute, "No engine can do this conversion"),
            (
                ErrorKey::EngineMissing,
                "A required program is not installed",
            ),
            (ErrorKey::ConversionFailed, "The engine reported an error"),
        ];
        for (key, text) in cases {
            assert_eq!(error_text(&failed(key, "  ")).as_deref(), Some(text));
        }
        assert_eq!(
            error_text(&failed(ErrorKey::UnsupportedSource, ".xyz")).as_deref(),
            Some("This file type isn't supported (.xyz)")
        );
        assert_eq!(error_text(&FileState::Done), None);
        assert_eq!(error_text(&FileState::Skipped), None);
    }

    #[test]
    fn report_paths_outside_cwd_stay_full() {
        let outside = Path::new(ELSEWHERE).join("a.png");
        let outside = outside.to_string_lossy();
        let done = result(&outside, FileState::Done, &[&at("out/a.jpg")]);
        assert_eq!(
            line(&done),
            format!("ok    {} -> out/a.jpg", norm(&outside))
        );

        // A sibling folder that merely shares a prefix is still outside.
        let sibling = Path::new(&format!("{CWD}shop")).join("a.png");
        let sibling = sibling.to_string_lossy();
        let skipped = result(&sibling, FileState::Skipped, &[]);
        assert!(line(&skipped).starts_with(&format!("skip  {} ", norm(&sibling))));

        // A file name that starts with two dots is inside, not a parent hop.
        let dotty = result(&at("..hidden.png"), FileState::Skipped, &[]);
        assert!(line(&dotty).starts_with("skip  ..hidden.png "));
    }

    #[test]
    fn report_done_without_outputs_has_no_dangling_arrow() {
        let done = result(&at("b.jpg"), FileState::Done, &[]);
        assert_eq!(line(&done), "ok    b.jpg");
    }

    #[test]
    fn report_summary_with_and_without_failures() {
        let ok = [
            result("a", FileState::Done, &["a.jpg"]),
            result("b", FileState::Skipped, &[]),
        ];
        assert_eq!(summary(&ok), "Converted 1 of 2 file(s).");

        let bad = [
            result("a", FileState::Done, &["a.jpg"]),
            result("b", failed(ErrorKey::ConversionFailed, "boom"), &[]),
            result("c", FileState::Cancelled, &[]),
        ];
        assert_eq!(summary(&bad), "Converted 1 of 3 file(s), 2 failed.");
        assert_eq!(summary(&[]), "Converted 0 of 0 file(s).");
    }

    fn sample() -> Vec<FileResult> {
        vec![
            result(&at("a.png"), FileState::Done, &[&at("a.webp")]),
            result("b.png", FileState::Skipped, &[]),
            result(&at("c.png"), FileState::Cancelled, &[]),
        ]
    }

    fn json_bytes(files: &[FileResult]) -> Vec<u8> {
        let mut buf = Vec::new();
        write_json(&mut buf, None, "webp", files, Path::new(CWD)).unwrap();
        buf
    }

    #[test]
    fn report_json_counts_and_error_only_on_cancelled() {
        let buf = json_bytes(&sample());
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["preset"], serde_json::Value::Null);
        assert_eq!(v["target"], "webp");
        assert_eq!(v["converted"], 1);
        assert_eq!(v["skipped"], 1);
        assert_eq!(v["failed"], 1);

        let files = v["files"].as_array().unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0]["state"], "done");
        assert_eq!(files[0]["outputs"][0], at("a.webp"));
        assert_eq!(files[1]["state"], "skipped");
        // Relative sources come out absolute.
        assert_eq!(files[1]["source"], at("b.png"));
        assert_eq!(files[2]["state"], "cancelled");
        assert!(files[0].get("error").is_none());
        assert!(files[1].get("error").is_none());
        assert_eq!(files[2]["error"], "cancelled");
    }

    #[test]
    fn report_json_failed_entry_carries_message_and_detail() {
        let files = [result(
            &at("notes.txt"),
            failed(ErrorKey::NoRoute, "txt → mp3"),
            &[],
        )];
        let report: JsonReport = serde_json::from_slice(&json_bytes(&files)).unwrap();
        assert_eq!(report.failed, 1);
        assert_eq!(report.files[0].state, "failed");
        assert_eq!(
            report.files[0].error.as_deref(),
            Some("No engine can do this conversion (txt → mp3)")
        );
    }

    #[test]
    fn report_json_round_trips_with_filee_key_order() {
        let files = sample();
        let buf = json_bytes(&files);
        let back: JsonReport = serde_json::from_slice(&buf).unwrap();
        assert_eq!(back, JsonReport::new(None, "webp", &files, Path::new(CWD)));

        let text = String::from_utf8(buf).unwrap();
        assert!(text.ends_with("}\n"));
        // 2-space indent, as Filee's Utf8JsonWriter.
        assert!(text.starts_with("{\n  \"preset\": null,\n  \"target\": \"webp\","));

        let pos = |key: &str| {
            text.find(&format!("\"{key}\""))
                .unwrap_or_else(|| panic!("missing {key}"))
        };
        let top = [
            "preset",
            "target",
            "converted",
            "skipped",
            "failed",
            "files",
        ];
        assert!(top.windows(2).all(|w| pos(w[0]) < pos(w[1])));

        // Within the cancelled entry (the only one with every key).
        let entry = &text[text.rfind("{").unwrap()..];
        let epos = |key: &str| entry.find(&format!("\"{key}\"")).unwrap();
        let keys = ["source", "state", "outputs", "error"];
        assert!(keys.windows(2).all(|w| epos(w[0]) < epos(w[1])));
    }

    #[test]
    fn report_json_names_the_preset_when_there_is_one() {
        let mut buf = Vec::new();
        write_json(&mut buf, Some("Half size"), "*", &sample(), Path::new(CWD)).unwrap();
        let text = String::from_utf8(buf).unwrap();
        // Still first in line: Filee's key order doesn't care who's asking.
        assert!(text.starts_with("{\n  \"preset\": \"Half size\",\n  \"target\": \"*\","));
    }

    #[test]
    fn report_json_keeps_unicode_names() {
        let name = at("한글 report's \"quoted\" 100%.png");
        let files = [result(&name, FileState::Skipped, &[])];
        let report: JsonReport = serde_json::from_slice(&json_bytes(&files)).unwrap();
        assert_eq!(report.files[0].source, name);
    }
}
