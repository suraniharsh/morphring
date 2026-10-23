//! PDF to plain text with `pdftotext`.
//!
//! Filee reads text with its own PdfPig layout analyser (reverse-engineering/02
//! section 9.3); we let poppler do the reading and keep Filee's output contract:
//! UTF-8 with a BOM, a blank line between pages, and a clear error when the PDF
//! has no text layer at all. Porting the analyser is a v1.0 parity item.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use morphring_core::{CancelToken, Edge, EngineError, OutputAllocator, Step};

use crate::discovery::{self, Tool};
use crate::process::{self, RunError, RunOptions};
use crate::progress::Estimate;

/// How long `pdftotext` may run before it is stopped.
const TIMEOUT: Duration = Duration::from_secs(120);

/// How long a text extraction usually takes, for the progress estimate.
const TYPICAL: Duration = Duration::from_secs(1);

/// The program name, next to `pdftoppm`.
const PROGRAM: &str = "pdftotext";

/// What every byte-order-mark enjoyer (Filee, Notepad, Excel) expects up front.
const BOM: &[u8] = b"\xEF\xBB\xBF";

/// The error for a PDF without a text layer. The PDF is often our own
/// intermediate (an SVG or a document printed on the way), so the message talks
/// about "this file" rather than accusing the user's drawing of being a PDF.
pub(crate) const NO_TEXT: &str =
    "No text was found in this file (it may be scanned or contain only pictures).";

/// The edges this part handles.
pub(crate) fn edges() -> Vec<Edge> {
    // Text only exists in what the user gave us; a PDF made from a picture
    // is a frame around pixels, so the planner keeps pictures away from here.
    vec![Edge::with_cost("pdf", "txt", 10).needs_original_content()]
}

/// Converts `step`, which is one of [`edges`].
pub(crate) fn convert(
    step: &Step,
    out: &mut dyn OutputAllocator,
    progress: &dyn Fn(f32),
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    if cancel.is_cancelled() {
        return Err(EngineError::Cancelled);
    }
    let program = locate()?;
    progress(0.0);

    let Some(target) = out.allocate("txt", "") else {
        // The skip policy said no; nothing to do and nothing to complain about.
        return Ok(Vec::new());
    };

    // Absolute, so an input called "-oops.pdf" can't pass itself off as a flag.
    let input = std::path::absolute(&step.input)?;
    std::fs::create_dir_all(&step.work_dir)?;
    let temp = tempfile::Builder::new()
        .prefix("pdftotext-")
        .tempdir_in(&step.work_dir)?;
    let raw = temp.path().join("raw.txt");

    let argv = args(&input, &raw);
    // pdftotext says nothing while it works, so the ring gets an estimate.
    Estimate::during(progress, TYPICAL, || {
        process::run(
            &program,
            &argv,
            RunOptions {
                timeout: Some(TIMEOUT),
                ..RunOptions::default()
            },
            cancel,
        )
    })
    .map_err(run_error)?;

    let bytes = std::fs::read(&raw)
        .map_err(|err| EngineError::Failed(format!("pdftotext wrote no output ({err})")))?;
    let text = tidy(&String::from_utf8_lossy(&bytes));
    if text.is_empty() {
        return Err(EngineError::Failed(NO_TEXT.to_owned()));
    }

    let finished = temp.path().join("out.txt");
    std::fs::write(&finished, with_bom(&text))?;
    if cancel.is_cancelled() {
        return Err(EngineError::Cancelled);
    }
    process::move_into_place(&finished, &target)?;
    progress(1.0);
    Ok(vec![target])
}

/// Finds `pdftotext`, which poppler installs next to `pdftoppm`.
fn locate() -> Result<PathBuf, EngineError> {
    let missing = || EngineError::NotInstalled {
        tool: Tool::Pdftoppm.display_name(),
        hint: discovery::install_hint_here(Tool::Pdftoppm),
    };
    let found = discovery::find(Tool::Pdftoppm).ok_or_else(missing)?;
    let program = found.companion(PROGRAM);
    // Discovery already insists on all three poppler programs, but a distro
    // that splits poppler into seven packages is only ever one update away.
    if program.is_file() {
        Ok(program)
    } else {
        Err(missing())
    }
}

/// `pdftotext -enc UTF-8 <input> <output>`. No `-layout`: one logical line per
/// text line reads better than a page of carefully aligned spaces.
fn args(input: &Path, output: &Path) -> Vec<OsString> {
    vec![
        "-enc".into(),
        "UTF-8".into(),
        input.as_os_str().to_owned(),
        output.as_os_str().to_owned(),
    ]
}

/// Maps a process failure onto the engine error the job runner reports.
fn run_error(err: RunError) -> EngineError {
    match err {
        RunError::Cancelled => EngineError::Cancelled,
        RunError::TimedOut => EngineError::Failed(format!(
            "pdftotext took longer than {} s and was stopped",
            TIMEOUT.as_secs()
        )),
        // A locked PDF gets a sentence, not pdftotext's command-line grumbling.
        other => super::password_error(&other)
            .unwrap_or_else(|| EngineError::Failed(format!("pdftotext {other}"))),
    }
}

/// Cleans up what `pdftotext` printed: `\n` line ends, every form feed (page
/// break) turned into a blank line, trailing spaces gone, never more than one
/// blank line in a row, and exactly one newline at the end.
///
/// Returns an empty string when there is no text worth keeping.
pub(crate) fn tidy(raw: &str) -> String {
    // A BOM from the tool itself would end up doubled behind ours.
    let raw = raw.strip_prefix('\u{FEFF}').unwrap_or(raw);
    let unix = raw.replace("\r\n", "\n").replace('\r', "\n");
    // A form feed sits glued to the first line of the next page, so it becomes
    // a paragraph break; the collapse below mops up any surplus newlines.
    let paged = unix.replace('\u{000C}', "\n\n");

    let mut text = String::with_capacity(paged.len());
    let mut newlines = 0;
    for line in paged.split('\n') {
        let line = line.trim_end();
        if line.is_empty() {
            newlines += 1;
            continue;
        }
        if !text.is_empty() {
            // `newlines` counts the empty lines in between, plus the break
            // that ended the previous line. Three or more becomes two.
            text.push_str(if newlines >= 2 { "\n\n" } else { "\n" });
        }
        text.push_str(line);
        newlines = 1;
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

/// `text` as UTF-8 bytes with a byte order mark in front.
pub(crate) fn with_bom(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(BOM.len() + text.len());
    bytes.extend_from_slice(BOM);
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_one_edge_at_the_default_cost() {
        let edges = edges();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from.as_str(), "pdf");
        assert_eq!(edges[0].to.as_str(), "txt");
        assert_eq!(edges[0].cost, 10);
        // A picture's PDF has no text layer, so no route may get here via one.
        assert!(edges[0].flags.needs_original_content);
    }

    #[test]
    fn form_feeds_become_a_blank_line_between_pages() {
        let raw = "Page 1\nCafé déjà vu\n\n\u{c}Page 2\n\n\u{c}Page 3\n\n\u{c}";
        assert_eq!(tidy(raw), "Page 1\nCafé déjà vu\n\nPage 2\n\nPage 3\n");
    }

    #[test]
    fn a_form_feed_right_after_a_line_still_leaves_a_blank_line() {
        assert_eq!(tidy("one\u{c}two\u{c}"), "one\n\ntwo\n");
    }

    #[test]
    fn three_or_more_newlines_collapse_to_two() {
        assert_eq!(tidy("a\n\n\nb\n\n\n\n\n\nc"), "a\n\nb\n\nc\n");
    }

    #[test]
    fn single_and_double_newlines_are_kept() {
        assert_eq!(tidy("a\nb\n\nc"), "a\nb\n\nc\n");
    }

    #[test]
    fn whitespace_only_lines_count_as_blank() {
        assert_eq!(tidy("a\n   \n\t\n \nb"), "a\n\nb\n");
    }

    #[test]
    fn windows_and_old_mac_line_ends_become_unix() {
        assert_eq!(tidy("a\r\nb\r\n\r\n\r\nc\rd"), "a\nb\n\nc\nd\n");
    }

    #[test]
    fn leading_blank_lines_and_trailing_spaces_go() {
        assert_eq!(tidy("\n\n\u{c}  indented  \nend   "), "  indented\nend\n");
    }

    #[test]
    fn a_bom_from_the_tool_is_not_kept() {
        assert_eq!(tidy("\u{FEFF}hello"), "hello\n");
    }

    #[test]
    fn empty_or_whitespace_only_text_tidies_to_nothing() {
        for raw in ["", "\u{c}", "\u{c}\u{c}\u{c}", "  \n\t\n\u{c}\r\n"] {
            assert_eq!(tidy(raw), "", "{raw:?}");
        }
    }

    #[test]
    fn the_bom_comes_first() {
        assert_eq!(with_bom("hé"), b"\xEF\xBB\xBFh\xC3\xA9");
    }

    #[test]
    fn args_never_ask_for_layout() {
        let argv = args(Path::new("/in.pdf"), Path::new("/out.txt"));
        assert_eq!(argv, ["-enc", "UTF-8", "/in.pdf", "/out.txt"]);
    }

    #[test]
    fn the_no_text_message_does_not_assume_a_pdf() {
        assert_eq!(
            NO_TEXT,
            "No text was found in this file (it may be scanned or contain only pictures)."
        );
    }
}
