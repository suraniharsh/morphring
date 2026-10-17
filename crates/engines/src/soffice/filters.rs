//! LibreOffice filter table, conversion edges and per-worker profiles.
//!
//! Ported from Filee's `src/Filee.Engines/Office/LibreOfficeConverter.cs`
//! (`FilterFor`, `FamilyOf`, `BuildEdges`, `RunAsync` and `ProfileDirectory`).
//! Everything here is pure: the engine (T25) runs the process.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use morphring_core::{Edge, FormatId};

/// Word processor formats LibreOffice reads and writes.
pub(crate) const WRITER: &[&str] = &["docx", "doc", "odt", "rtf"];
/// Spreadsheet formats LibreOffice reads and writes.
pub(crate) const CALC: &[&str] = &["xlsx", "xls", "ods", "csv"];
/// Presentation formats LibreOffice reads and writes.
pub(crate) const IMPRESS: &[&str] = &["pptx", "ppt", "odp"];

// Formats LibreOffice only reads, grouped by the application that opens them.
// Filee checked these against LibreOffice 26.2's filter registry, so I'm
// taking its word for it rather than spelunking through XCU files again.
pub(crate) const WRITER_IMPORT: &[&str] = &["dot", "wps", "wpd", "lwp", "abw", "pages", "sdw"];
pub(crate) const CALC_IMPORT: &[&str] = &["et", "numbers", "sdc"];
// "sda" also covers .sdd: StarDraw (.sda) opens in Draw, StarImpress (.sdd) in
// Impress. One format id, two personalities; see `family_of`.
pub(crate) const IMPRESS_IMPORT: &[&str] = &["key", "dps", "sda"];
// CGM opens in Impress but is really a drawing, so it exports like one.
pub(crate) const IMPRESS_GRAPHICS: &[&str] = &["cgm"];
pub(crate) const DRAW: &[&str] = &["pub", "cdr", "vsd", "odg", "emf", "wmf"];

/// Targets offered for drawings (Draw formats and CGM).
const DRAWING_TARGETS: &[&str] = &["pdf", "odg", "svg", "png"];

/// LibreOffice is the fallback engine: anything the other engines can do should win.
pub const FALLBACK_COST: u32 = 25;
/// Plain text and HTML into documents: a notch above the fallback, so a
/// dedicated engine (pandoc later) wins when it exists.
pub const TEXT_TO_DOCUMENT_COST: u32 = FALLBACK_COST + 3;
/// Presentations to PDF (Filee's `LibreOfficeForSlides`, on by default).
pub const SLIDES_TO_PDF_COST: u32 = 0;

/// The LibreOffice application that opens a format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Writer,
    Calc,
    Impress,
    Draw,
}

impl Family {
    /// The prefix LibreOffice uses in export filter names, such as `writer_pdf_Export`.
    pub fn as_str(self) -> &'static str {
        match self {
            Family::Writer => "writer",
            Family::Calc => "calc",
            Family::Impress => "impress",
            Family::Draw => "draw",
        }
    }
}

/// The application that opens `from`.
///
/// `extension` is the input file's extension (without the dot). It only
/// matters for `sda`, which covers both `.sda` (Draw) and `.sdd` (Impress).
/// Anything unknown, including `txt` and `html`, opens in Writer.
pub fn family_of(from: FormatId, extension: Option<&str>) -> Family {
    let from = from.as_str();
    if from == "sda" {
        return match extension {
            Some(ext) if ext.eq_ignore_ascii_case("sda") => Family::Draw,
            _ => Family::Impress,
        };
    }
    if CALC.contains(&from) || CALC_IMPORT.contains(&from) {
        Family::Calc
    } else if IMPRESS.contains(&from)
        || IMPRESS_IMPORT.contains(&from)
        || IMPRESS_GRAPHICS.contains(&from)
    {
        Family::Impress
    } else if DRAW.contains(&from) {
        Family::Draw
    } else {
        Family::Writer
    }
}

/// The `--convert-to` value for converting `from` into `to`.
///
/// `extension` is the input file's extension, see [`family_of`].
pub fn filter_for(from: FormatId, to: FormatId, extension: Option<&str>) -> String {
    let family = family_of(from, extension);
    let fam = family.as_str();
    // PDF/A is deliberately missing: Filee's JSON-options variant waits for presets in v0.2.
    match to.as_str() {
        "pdf" => format!("pdf:{fam}_pdf_Export"),
        "docx" => "docx:MS Word 2007 XML".into(),
        "doc" => "doc:MS Word 97".into(),
        "odt" => "odt".into(),
        "rtf" => "rtf:Rich Text Format".into(),
        "txt" => "txt:Text (encoded):UTF8".into(),
        "html" if family == Family::Calc => "html:HTML (StarCalc)".into(),
        "html" => "html:HTML (StarWriter)".into(),
        "xlsx" => "xlsx:Calc MS Excel 2007 XML".into(),
        "xls" => "xls:MS Excel 97".into(),
        "ods" => "ods".into(),
        // 44 = ',' separator, 34 = '"' quote, 76 = UTF-8. LibreOffice's filter
        // options are a numerology hobby; this one is the sane CSV.
        "csv" => "csv:Text - txt - csv (StarCalc):44,34,76".into(),
        "pptx" => "pptx:Impress MS PowerPoint 2007 XML".into(),
        "ppt" => "ppt:MS PowerPoint 97".into(),
        "odp" => "odp".into(),
        "odg" if family == Family::Impress => "odg:impress8_draw".into(),
        "odg" => "odg:draw8".into(),
        "svg" => format!("svg:{fam}_svg_Export"),
        "png" => format!("png:{fam}_png_Export"),
        other => other.into(),
    }
}

/// Every conversion the LibreOffice engine offers.
///
/// Self edges are left out: LibreOffice re-saving a file as itself is not a
/// conversion anyone asked for.
pub fn edges() -> Vec<Edge> {
    let mut edges = Vec::new();
    let mut add = |sources: &[&'static str], targets: &[&'static str], cost: u32| {
        for &s in sources {
            for &t in targets {
                if s != t {
                    edges.push(Edge::with_cost(s, t, cost));
                }
            }
        }
    };

    let writer_out = [WRITER, &["txt", "html"]].concat();
    let calc_out = [CALC, &["html"]].concat();

    add(WRITER, &writer_out, FALLBACK_COST);
    // Filee lets the user prefer LibreOffice for documents and sheets (cost 10).
    // Those toggles live in settings, which v0.1.0 doesn't have, so they're off.
    add(WRITER, &["pdf"], FALLBACK_COST);
    add(CALC, &calc_out, FALLBACK_COST);
    add(CALC, &["pdf"], FALLBACK_COST);
    add(IMPRESS, IMPRESS, FALLBACK_COST);
    // Slides are the one toggle Filee ships switched on: LibreOffice renders
    // decks far better than any route through a text model, so it's free.
    add(IMPRESS, &["pdf"], SLIDES_TO_PDF_COST);
    add(
        &["txt", "html"],
        &["docx", "odt", "pdf"],
        TEXT_TO_DOCUMENT_COST,
    );

    // Formats only LibreOffice reads, to PDF and the editable formats of their kind.
    add(
        WRITER_IMPORT,
        &[WRITER, &["pdf", "txt", "html"]].concat(),
        FALLBACK_COST,
    );
    add(
        CALC_IMPORT,
        &[CALC, &["pdf", "html"]].concat(),
        FALLBACK_COST,
    );
    add(IMPRESS_IMPORT, &[IMPRESS, &["pdf"]].concat(), FALLBACK_COST);
    add(
        &[IMPRESS_GRAPHICS, DRAW].concat(),
        DRAWING_TARGETS,
        FALLBACK_COST,
    );
    // TODO(v0.5): HWP/HWPX edges once we detect the H2Orestart extension.
    edges
}

/// The profile directory of LibreOffice worker `worker`:
/// `<data_dir>/libreoffice-profiles/worker-<worker>`.
pub fn profile_dir(data_dir: &Path, worker: usize) -> PathBuf {
    data_dir
        .join("libreoffice-profiles")
        .join(format!("worker-{worker}"))
}

/// The lock file that marks worker `worker`'s profile as taken:
/// `<data_dir>/libreoffice-profiles/worker-<worker>.lock`.
///
/// It sits next to the profile, not inside it, so LibreOffice never trips
/// over it and wiping a profile doesn't yank the lock from under its owner.
pub fn profile_lock(data_dir: &Path, worker: usize) -> PathBuf {
    data_dir
        .join("libreoffice-profiles")
        .join(format!("worker-{worker}.lock"))
}

/// LibreOffice's own lock file inside a profile, `<profile_dir>/.lock`.
///
/// soffice writes it while it runs and removes it on a clean exit, so finding
/// one after our own soffice has gone means some other LibreOffice process is
/// (or recently was) using the profile.
pub fn office_lock(profile_dir: &Path) -> PathBuf {
    profile_dir.join(".lock")
}

/// The `file://` URI LibreOffice wants for `-env:UserInstallation`.
fn profile_url(profile_dir: &Path) -> String {
    // from_directory_path insists on an absolute path. Our data dir always is
    // one, but if someone hands us a relative path I'd rather resolve it than
    // hand soffice garbage and watch it fail with exit 1 and zero explanation.
    let absolute = std::path::absolute(profile_dir).unwrap_or_else(|_| profile_dir.to_path_buf());
    match url::Url::from_directory_path(&absolute) {
        Ok(url) => url.into(),
        Err(()) => format!("file://{}", absolute.display()),
    }
}

/// The `-env:UserInstallation=...` argument that points soffice at
/// `profile_dir`. Any process carrying this exact argument is a LibreOffice
/// living in that profile, which is how we spot squatters.
pub fn user_installation_arg(profile_dir: &Path) -> String {
    format!("-env:UserInstallation={}", profile_url(profile_dir))
}

/// The soffice argument list that converts `input` with `filter` into `outdir`,
/// using the user profile in `profile_dir`.
pub fn args(input: &Path, filter: &str, outdir: &Path, profile_dir: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--headless",
        "--norestore",
        "--nologo",
        "--nodefault",
        "--nolockcheck",
        "--nofirststartwizard",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    // Two soffice processes sharing a profile don't fight, the second one just
    // quietly exits 1. Hence one profile per worker, passed as a file URI.
    args.push(user_installation_arg(profile_dir).into());
    args.push("--convert-to".into());
    args.push(filter.into());
    args.push("--outdir".into());
    args.push(outdir.as_os_str().to_owned());
    args.push(input.as_os_str().to_owned());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: &'static str) -> FormatId {
        FormatId::new(id)
    }

    fn cost(from: &str, to: &str) -> Option<u32> {
        edges()
            .into_iter()
            .find(|e| e.from.as_str() == from && e.to.as_str() == to)
            .map(|e| e.cost)
    }

    #[test]
    fn filter_strings_match_filee() {
        assert_eq!(
            filter_for(f("docx"), f("pdf"), None),
            "pdf:writer_pdf_Export"
        );
        assert_eq!(filter_for(f("xlsx"), f("pdf"), None), "pdf:calc_pdf_Export");
        assert_eq!(
            filter_for(f("pptx"), f("pdf"), None),
            "pdf:impress_pdf_Export"
        );
        assert_eq!(filter_for(f("vsd"), f("pdf"), None), "pdf:draw_pdf_Export");
        assert_eq!(
            filter_for(f("xlsx"), f("csv"), None),
            "csv:Text - txt - csv (StarCalc):44,34,76"
        );
        assert_eq!(
            filter_for(f("odt"), f("docx"), None),
            "docx:MS Word 2007 XML"
        );
        assert_eq!(filter_for(f("odt"), f("doc"), None), "doc:MS Word 97");
        assert_eq!(filter_for(f("doc"), f("rtf"), None), "rtf:Rich Text Format");
        assert_eq!(
            filter_for(f("doc"), f("txt"), None),
            "txt:Text (encoded):UTF8"
        );
        assert_eq!(
            filter_for(f("ods"), f("xlsx"), None),
            "xlsx:Calc MS Excel 2007 XML"
        );
        assert_eq!(filter_for(f("ods"), f("xls"), None), "xls:MS Excel 97");
        assert_eq!(
            filter_for(f("odp"), f("pptx"), None),
            "pptx:Impress MS PowerPoint 2007 XML"
        );
        assert_eq!(filter_for(f("odp"), f("ppt"), None), "ppt:MS PowerPoint 97");
        assert_eq!(filter_for(f("doc"), f("odt"), None), "odt");
        assert_eq!(filter_for(f("xls"), f("ods"), None), "ods");
        assert_eq!(filter_for(f("ppt"), f("odp"), None), "odp");
    }

    #[test]
    fn html_filter_depends_on_family() {
        assert_eq!(
            filter_for(f("xlsx"), f("html"), None),
            "html:HTML (StarCalc)"
        );
        assert_eq!(
            filter_for(f("numbers"), f("html"), None),
            "html:HTML (StarCalc)"
        );
        assert_eq!(
            filter_for(f("docx"), f("html"), None),
            "html:HTML (StarWriter)"
        );
    }

    #[test]
    fn drawing_filters() {
        assert_eq!(filter_for(f("cgm"), f("odg"), None), "odg:impress8_draw");
        assert_eq!(filter_for(f("cdr"), f("odg"), None), "odg:draw8");
        assert_eq!(filter_for(f("emf"), f("svg"), None), "svg:draw_svg_Export");
        assert_eq!(
            filter_for(f("cgm"), f("png"), None),
            "png:impress_png_Export"
        );
    }

    #[test]
    fn unknown_target_passes_through() {
        assert_eq!(filter_for(f("docx"), f("epub"), None), "epub");
    }

    #[test]
    fn family_of_sda_depends_on_extension() {
        assert_eq!(family_of(f("sda"), Some("sda")), Family::Draw);
        assert_eq!(family_of(f("sda"), Some("SDA")), Family::Draw);
        assert_eq!(family_of(f("sda"), Some("sdd")), Family::Impress);
        assert_eq!(family_of(f("sda"), None), Family::Impress);
        assert_eq!(
            filter_for(f("sda"), f("pdf"), Some("sda")),
            "pdf:draw_pdf_Export"
        );
        assert_eq!(
            filter_for(f("sda"), f("pdf"), Some("sdd")),
            "pdf:impress_pdf_Export"
        );
    }

    #[test]
    fn family_defaults_to_writer() {
        assert_eq!(family_of(f("txt"), None), Family::Writer);
        assert_eq!(family_of(f("html"), None), Family::Writer);
        assert_eq!(family_of(f("pages"), None), Family::Writer);
        assert_eq!(family_of(f("et"), None), Family::Calc);
        assert_eq!(family_of(f("key"), None), Family::Impress);
    }

    #[test]
    fn edge_costs() {
        assert_eq!(cost("pptx", "pdf"), Some(0));
        assert_eq!(cost("ppt", "pdf"), Some(0));
        assert_eq!(cost("odp", "pdf"), Some(0));
        assert_eq!(cost("docx", "pdf"), Some(25));
        assert_eq!(cost("xlsx", "pdf"), Some(25));
        assert_eq!(cost("doc", "docx"), Some(25));
        assert_eq!(cost("xlsx", "csv"), Some(25));
        assert_eq!(cost("txt", "docx"), Some(28));
        assert_eq!(cost("txt", "odt"), Some(28));
        assert_eq!(cost("html", "pdf"), Some(28));
        assert_eq!(cost("pages", "pdf"), Some(25));
        assert_eq!(cost("numbers", "xlsx"), Some(25));
        assert_eq!(cost("key", "pptx"), Some(25));
        assert_eq!(cost("vsd", "png"), Some(25));
    }

    #[test]
    fn missing_edges_stay_missing() {
        // Families don't cross: no deck from a spreadsheet, no PDF input, no self edges.
        assert_eq!(cost("xlsx", "docx"), None);
        assert_eq!(cost("pptx", "xlsx"), None);
        assert_eq!(cost("pdf", "docx"), None);
        assert_eq!(cost("txt", "rtf"), None);
        assert!(edges().iter().all(|e| e.from != e.to));
    }

    #[test]
    fn edges_are_unique() {
        let all = edges();
        for (i, a) in all.iter().enumerate() {
            assert!(
                all[i + 1..]
                    .iter()
                    .all(|b| (a.from, a.to) != (b.from, b.to)),
                "duplicate edge {} -> {}",
                a.from,
                a.to
            );
        }
    }

    #[test]
    fn no_edge_targets_a_read_only_format() {
        let read_only = [
            WRITER_IMPORT,
            CALC_IMPORT,
            IMPRESS_IMPORT,
            IMPRESS_GRAPHICS,
            &["pub", "cdr", "vsd", "emf", "wmf"],
        ]
        .concat();
        for e in edges() {
            assert!(
                !read_only.contains(&e.to.as_str()),
                "edge {} -> {} targets a read-only format",
                e.from,
                e.to
            );
        }
        // And every read-only format is actually readable.
        for fmt in read_only {
            assert!(edges().iter().any(|e| e.from.as_str() == fmt), "{fmt}");
        }
    }

    #[test]
    fn profile_dir_layout() {
        assert_eq!(
            profile_dir(Path::new("/home/me/.local/share/morphring"), 1),
            PathBuf::from("/home/me/.local/share/morphring/libreoffice-profiles/worker-1")
        );
    }

    #[test]
    fn profile_lock_sits_next_to_its_profile() {
        let data = Path::new("/home/me/.local/share/morphring");
        assert_eq!(
            profile_lock(data, 3),
            PathBuf::from("/home/me/.local/share/morphring/libreoffice-profiles/worker-3.lock")
        );
        assert_eq!(
            profile_lock(data, 3).parent(),
            profile_dir(data, 3).parent()
        );
    }

    #[test]
    fn office_lock_lives_inside_the_profile() {
        let profile = profile_dir(Path::new("/home/me/.local/share/morphring"), 0);
        assert_eq!(
            office_lock(&profile),
            PathBuf::from("/home/me/.local/share/morphring/libreoffice-profiles/worker-0/.lock")
        );
    }

    #[test]
    fn user_installation_arg_matches_what_args_passes() {
        let profile = Path::new("/data/libreoffice-profiles/worker-3");
        let arg = user_installation_arg(profile);
        assert!(arg.starts_with("-env:UserInstallation=file:///"), "{arg}");
        assert!(arg.ends_with("worker-3/"), "{arg}");
        let all = args(Path::new("/in/a.docx"), "pdf", Path::new("/out"), profile);
        assert!(all.iter().any(|a| *a == *arg), "{all:?}");
    }

    #[test]
    fn args_layout() {
        let a = args(
            Path::new("/in/report.docx"),
            "pdf:writer_pdf_Export",
            Path::new("/work/lo-1234abcd"),
            Path::new("/data/libreoffice-profiles/worker-0"),
        );
        let expected: Vec<OsString> = [
            "--headless",
            "--norestore",
            "--nologo",
            "--nodefault",
            "--nolockcheck",
            "--nofirststartwizard",
            // Absolute, so it gains a drive letter on Windows; its exact
            // spelling is user_installation_is_an_encoded_file_uri's job.
            &format!(
                "-env:UserInstallation={}",
                profile_url(Path::new("/data/libreoffice-profiles/worker-0"))
            ),
            "--convert-to",
            "pdf:writer_pdf_Export",
            "--outdir",
            "/work/lo-1234abcd",
            "/in/report.docx",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert_eq!(a, expected);
    }

    #[test]
    fn user_installation_is_an_encoded_file_uri() {
        let a = args(
            Path::new("/in/a b.docx"),
            "odt",
            Path::new("/out dir"),
            Path::new("/home/me/My Data/libreoffice-profiles/worker-1"),
        );
        let env = a
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .find(|s| s.starts_with("-env:UserInstallation="))
            .expect("UserInstallation argument");
        let value = env.trim_start_matches("-env:UserInstallation=");
        assert!(value.starts_with("file:///"), "{value}");
        assert!(value.contains("My%20Data"), "{value}");
        assert!(!value.contains(' '), "{value}");
        // Plain paths stay plain: soffice gets them as separate argv entries.
        assert_eq!(a[a.len() - 1], OsString::from("/in/a b.docx"));
        assert_eq!(a[a.len() - 2], OsString::from("/out dir"));
    }

    #[test]
    fn relative_profile_dir_still_yields_a_file_uri() {
        let a = args(
            Path::new("in.docx"),
            "odt",
            Path::new("out"),
            Path::new("rel/worker-0"),
        );
        let env = a[6].to_string_lossy();
        assert!(env.starts_with("-env:UserInstallation=file:///"), "{env}");
    }
}
