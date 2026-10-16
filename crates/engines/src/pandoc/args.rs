//! Pandoc's reader and writer names, conversion edges and argument list.
//!
//! Ported from Filee's `src/Filee.Engines/Office/PandocConverter.cs`
//! (`Readers`, `Writers`, `Edges` and the argument building in
//! `ConvertAsync`). Everything here is pure; [`super`] runs the process.

use std::ffi::OsString;
use std::path::Path;

use morphring_core::{Edge, FormatId};

/// Cost of the conversions only Pandoc does (Markdown, reStructuredText, LaTeX).
pub const COST: u32 = Edge::DEFAULT_COST;

/// Cost of HTML <-> DOCX/ODT, which LibreOffice also does. A little dearer
/// than the Pandoc-only edges, still well under LibreOffice's fallback.
pub const OVERLAP_COST: u32 = 14;

/// Sources that become Markdown, and the targets Markdown becomes.
const TO_MD: &[&str] = &["docx", "odt", "html", "rtf"];
const FROM_MD: &[&str] = &["docx", "odt", "html", "rtf", "txt"];

/// reStructuredText and LaTeX: Pandoc reads and writes them well.
const MARKUP: &[&str] = &["rst", "tex"];

/// Everything reStructuredText and LaTeX convert into (minus themselves).
const MARKUP_TARGETS: &[&str] = &[
    "md", "html", "docx", "odt", "rtf", "epub", "txt", "rst", "tex",
];

/// What converts into reStructuredText and LaTeX.
const MARKUP_SOURCES: &[&str] = &["md", "html", "docx", "odt"];

/// Pandoc's reader (`-f`) for `from`, or `None` when Pandoc does not read it.
pub fn reader(from: FormatId) -> Option<&'static str> {
    Some(match from.as_str() {
        "md" => "markdown",
        "html" => "html",
        "docx" => "docx",
        "odt" => "odt",
        "rtf" => "rtf",
        "rst" => "rst",
        "tex" => "latex",
        _ => return None,
    })
}

/// Pandoc's writer (`-t`) for `to`, or `None` when Pandoc does not write it.
///
/// Markdown comes out GitHub-flavoured, like Filee's.
pub fn writer(to: FormatId) -> Option<&'static str> {
    Some(match to.as_str() {
        "md" => "gfm",
        "html" => "html",
        "docx" => "docx",
        "odt" => "odt",
        "rtf" => "rtf",
        "txt" => "plain",
        "rst" => "rst",
        "tex" => "latex",
        "epub" => "epub",
        _ => return None,
    })
}

/// Every conversion the Pandoc engine offers. No PDF: that needs a TeX
/// engine, and installing half of CTAN to print a README is not the vibe.
pub fn edges() -> Vec<Edge> {
    let mut edges = Vec::new();
    // Markdown in and out: only Pandoc does this.
    edges.extend(TO_MD.iter().map(|&from| Edge::with_cost(from, "md", COST)));
    edges.extend(FROM_MD.iter().map(|&to| Edge::with_cost("md", to, COST)));
    // Overlaps with LibreOffice, which is the fallback and costs more anyway.
    for &doc in &["docx", "odt"] {
        edges.push(Edge::with_cost("html", doc, OVERLAP_COST));
    }
    for &doc in &["docx", "odt"] {
        edges.push(Edge::with_cost(doc, "html", OVERLAP_COST));
    }
    // reStructuredText and LaTeX in and out: only Pandoc does this either.
    for &markup in MARKUP {
        for &to in MARKUP_TARGETS {
            if to != markup {
                edges.push(Edge::with_cost(markup, to, COST));
            }
        }
    }
    for &markup in MARKUP {
        for &from in MARKUP_SOURCES {
            edges.push(Edge::with_cost(from, markup, COST));
        }
    }
    edges
}

/// True when the output needs `--standalone` (a full HTML page or a LaTeX
/// document with a preamble, rather than a fragment).
pub fn standalone(to: FormatId) -> bool {
    matches!(to.as_str(), "html" | "tex")
}

/// True when images in the input are written next to the output
/// (`--extract-media`), because the target only links to them.
pub fn extracts_media(to: FormatId) -> bool {
    matches!(to.as_str(), "md" | "html" | "rst" | "tex")
}

/// The folder that receives extracted images for `final_output`:
/// `<outdir>/<stem>_files`.
pub fn media_dir(final_output: &Path) -> std::path::PathBuf {
    let stem = final_output
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = final_output.parent().unwrap_or_else(|| Path::new(""));
    dir.join(format!("{stem}_files"))
}

/// The pandoc argument list that converts `input` (read as `reader`) into
/// `output` (written as `writer`).
///
/// `media_dir` is where linked images go; it is only passed for targets that
/// [`extracts_media`]. `output` may be a temp path, but `media_dir` should be
/// next to the final file, since the converted document links to it.
pub fn args(
    input: &Path,
    from: FormatId,
    to: FormatId,
    output: &Path,
    media_dir: &Path,
) -> Option<Vec<OsString>> {
    let mut args: Vec<OsString> = vec![
        input.as_os_str().to_owned(),
        "-f".into(),
        reader(from)?.into(),
        "-t".into(),
        writer(to)?.into(),
        "-o".into(),
        output.as_os_str().to_owned(),
    ];
    if standalone(to) {
        args.push("--standalone".into());
    }
    if extracts_media(to) {
        let mut flag = OsString::from("--extract-media=");
        flag.push(media_dir.as_os_str());
        args.push(flag);
    }
    Some(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: &'static str) -> FormatId {
        FormatId::new(id)
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn every_edge_has_a_reader_and_a_writer() {
        for e in edges() {
            assert!(reader(e.from).is_some(), "no reader for {}", e.from);
            assert!(writer(e.to).is_some(), "no writer for {}", e.to);
        }
    }

    #[test]
    fn md_to_docx_args() {
        let args = args(
            Path::new("/in/a.md"),
            f("md"),
            f("docx"),
            Path::new("/tmp/out.docx"),
            Path::new("/out/a_files"),
        )
        .unwrap();
        assert_eq!(
            strings(&args),
            [
                "/in/a.md",
                "-f",
                "markdown",
                "-t",
                "docx",
                "-o",
                "/tmp/out.docx"
            ]
        );
    }

    #[test]
    fn media_dir_is_next_to_the_output() {
        assert_eq!(
            media_dir(Path::new("/out/report.html")),
            Path::new("/out/report_files")
        );
    }

    #[test]
    fn unknown_formats_have_no_args() {
        let none = args(
            Path::new("a.pdf"),
            f("pdf"),
            f("md"),
            Path::new("o.md"),
            Path::new("o_files"),
        );
        assert!(none.is_none());
    }
}
