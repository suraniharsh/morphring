//! `morphring formats`: the format registry, or what one format converts to.
//! Ported from `ListFormats` in Filee's `src/Filee.Cli/Cli.cs`.

use morphring_core::formats::{self, Category, Format};
use morphring_core::planner::Planner;
use morphring_core::{Converter, EngineStatus};

use crate::EXIT_USAGE;
use crate::args::FormatsArgs;
use crate::ctx::Ctx;

/// Runs `morphring formats` and returns the exit code.
pub fn run(args: &FormatsArgs, ctx: &mut Ctx) -> i32 {
    let text = match args.format.as_deref() {
        None => list_all(),
        Some(name) => match formats::lookup_target(name) {
            Some(from) => list_targets(from, &ctx.engines),
            None => {
                let _ = writeln!(ctx.err, "morphring: unknown format '{name}'.");
                return EXIT_USAGE;
            }
        },
    };
    let _ = ctx.out.write_all(text.as_bytes());
    0
}

/// Every format but the folder, grouped by category in order of first
/// appearance (LINQ's GroupBy, which is what Filee used).
fn list_all() -> String {
    let known: Vec<&Format> = formats::all().iter().filter(|f| !f.is_folder()).collect();
    let mut categories: Vec<Category> = Vec::new();
    for format in &known {
        if !categories.contains(&format.category) {
            categories.push(format.category);
        }
    }

    let mut text = String::new();
    for category in categories {
        text.push_str(category.name());
        text.push_str(":\n");
        for format in known.iter().filter(|f| f.category == category) {
            let read_only = if format.writable { "" } else { " (read only)" };
            text.push_str(&format!(
                "  {:<10} {}{read_only}  .{}\n",
                format.id.as_str(),
                format.name,
                format.extensions.join(" .")
            ));
        }
    }
    text.push_str("\nRun 'morphring formats <format>' to see what a format converts to.\n");
    text
}

/// What `from` converts to with the installed engines, then what it would
/// convert to if the user installed the missing ones (and which ones, and how).
fn list_targets(from: &Format, engines: &[Box<dyn Converter>]) -> String {
    let engines: Vec<&dyn Converter> = engines.iter().map(Box::as_ref).collect();
    let installed = Planner::new(&engines, false);
    let dream = Planner::new(&engines, true);

    // Same-format re-encodes are left out on purpose: "png converts to png"
    // reads like a bug report, not a feature.
    let targets = formats::all()
        .iter()
        .filter(|t| t.writable && t.id != from.id && !t.is_folder());

    let mut now: Vec<&str> = Vec::new();
    // (what to install, the targets it unlocks), in order of first appearance,
    // so "docx, odt, pdf" share one LibreOffice line instead of three.
    let mut later: Vec<(String, Vec<&str>)> = Vec::new();
    for target in targets {
        if installed.can_convert(from.id, target.id) {
            now.push(target.id.as_str());
            continue;
        }
        let Some(route) = dream.plan(from.id, target.id) else {
            continue;
        };
        let needs = missing_engines(route.engines().map(|i| engines[i]));
        match later.iter_mut().find(|(n, _)| *n == needs) {
            Some((_, ids)) => ids.push(target.id.as_str()),
            None => later.push((needs, vec![target.id.as_str()])),
        }
    }

    let mut text = format!(
        "{} ({}) converts to:\n",
        from.name,
        from.extensions.join(", ")
    );
    if now.is_empty() {
        text.push_str("  (nothing with the engines installed now)\n");
    } else {
        text.push_str(&format!("  {}\n", now.join(", ")));
    }
    if !later.is_empty() {
        text.push_str("With optional engines:\n");
        for (needs, ids) in later {
            text.push_str(&format!("  {}  ({needs})\n", ids.join(", ")));
        }
    }
    text
}

/// `needs LibreOffice: pacman -S libreoffice-fresh`, one clause per missing
/// engine on the route, worded like the job runner's `error.engine_missing`.
fn missing_engines<'a>(route: impl Iterator<Item = &'a dyn Converter>) -> String {
    let mut lines: Vec<String> = Vec::new();
    for engine in route {
        if let EngineStatus::Missing { hint } = engine.status() {
            let line = format!("needs {}: {hint}", engine.display_name());
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
    }
    lines.join("; ")
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use morphring_core::CancelToken;
    use morphring_core::testing::FakeConverter;

    use super::*;
    use crate::run_with;

    fn run_args(args: &[&str], engines: Vec<FakeConverter>) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = {
            let mut ctx = Ctx {
                out: &mut out,
                err: &mut err,
                cwd: PathBuf::from("/"),
                cancel: CancelToken::new(),
                engines: engines
                    .into_iter()
                    .map(|e| Box::new(e) as Box<dyn Converter>)
                    .collect(),
            };
            run_with(args.iter().map(OsString::from), &mut ctx)
        };
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn soffice_missing() -> FakeConverter {
        FakeConverter::new("soffice")
            .display_name("LibreOffice")
            .edge("docx", "pdf", 25)
            .edge("docx", "odt", 25)
            .missing("pacman -S libreoffice-fresh")
    }

    #[test]
    fn listing_groups_by_category_and_marks_read_only() {
        let (code, out, err) = run_args(&["formats"], vec![]);
        assert_eq!(code, 0);
        assert_eq!(err, "");
        assert!(out.starts_with("Image:\n"), "{out}");
        assert!(out.contains("\n  png        PNG  .png\n"), "{out}");
        assert!(out.contains("\n  jpg        JPG  .jpg .jpeg"), "{out}");
        assert!(out.contains("\n  docx "), "{out}");
        let raw = out.lines().find(|l| l.starts_with("  raw ")).unwrap();
        assert!(raw.contains("(read only)"), "{raw}");
        assert!(!out.contains("folder"), "{out}");
        assert!(
            out.ends_with(
                "\n\nRun 'morphring formats <format>' to see what a format converts to.\n"
            )
        );
    }

    #[test]
    fn every_category_heading_appears_once() {
        let (_, out, _) = run_args(&["formats"], vec![]);
        let headings: Vec<&str> = out.lines().filter(|l| l.ends_with(':')).collect();
        assert!(headings.contains(&"Document:"), "{out}");
        for heading in &headings {
            let count = headings.iter().filter(|h| *h == heading).count();
            assert_eq!(count, 1, "{heading}");
        }
    }

    #[test]
    fn targets_use_the_installed_engines_and_skip_the_source() {
        let magick = FakeConverter::new("magick")
            .edge("png", "jpg", 10)
            .edge("png", "png", 10)
            .edge("jpg", "webp", 10);
        let (code, out, err) = run_args(&["formats", "png"], vec![magick]);
        assert_eq!((code, err.as_str()), (0, ""));
        // webp arrives over two hops; the png self edge is not a target.
        assert_eq!(out, "PNG (png) converts to:\n  jpg, webp\n");
    }

    #[test]
    fn nothing_installed_says_so() {
        let (code, out, _) = run_args(&["formats", "jpeg"], vec![]);
        assert_eq!(code, 0);
        assert_eq!(
            out,
            "JPG (jpg, jpeg, jpe, jfif) converts to:\n  (nothing with the engines installed now)\n"
        );
    }

    #[test]
    fn missing_engine_targets_are_listed_with_the_hint() {
        let (code, out, _) = run_args(&["formats", "docx"], vec![soffice_missing()]);
        assert_eq!(code, 0);
        assert_eq!(
            out,
            "DOCX (docx) converts to:\n\
             \x20 (nothing with the engines installed now)\n\
             With optional engines:\n\
             \x20 pdf, odt  (needs LibreOffice: pacman -S libreoffice-fresh)\n"
        );
    }

    #[test]
    fn installed_targets_are_not_repeated_as_optional() {
        let pandoc = FakeConverter::new("pandoc").edge("docx", "odt", 10);
        let (_, out, _) = run_args(&["formats", ".DOCX"], vec![pandoc, soffice_missing()]);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[1], "  odt");
        assert_eq!(
            lines[3],
            "  pdf  (needs LibreOffice: pacman -S libreoffice-fresh)"
        );
    }

    #[test]
    fn unknown_format_is_a_usage_error() {
        let (code, out, err) = run_args(&["formats", "nope"], vec![]);
        assert_eq!(code, 2);
        assert_eq!(out, "");
        assert_eq!(err, "morphring: unknown format 'nope'.\n");
    }
}
