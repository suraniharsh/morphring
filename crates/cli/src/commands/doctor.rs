//! `morphring doctor`: which engines are installed, where MorphRing keeps its
//! files, and how to install whatever is missing.
//!
//! Filee has no doctor; its Settings → Engines page is the closest relative.
//! This is the command-line answer to "why won't my docx convert?" and it
//! never touches the network.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use morphring_core::EngineStatus;
use serde::Serialize;

use crate::args::DoctorArgs;
use crate::ctx::Ctx;

/// The folders MorphRing would use on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folders {
    /// Where `settings.json` lives, or `None` without a home folder.
    pub config: Option<PathBuf>,
    /// Where engine data (LibreOffice profiles) lives, or `None` without a home folder.
    pub data: Option<PathBuf>,
    /// The parent of every job's temp folder.
    pub temp: PathBuf,
}

impl Folders {
    /// The folders for the current user, per ENGINEERING.md section 7.
    pub fn current() -> Self {
        let dirs = directories::ProjectDirs::from("", "", "morphring");
        Self {
            config: dirs.as_ref().map(|d| d.config_dir().to_path_buf()),
            data: dirs.as_ref().map(|d| d.data_dir().to_path_buf()),
            temp: std::env::temp_dir().join("morphring"),
        }
    }
}

/// Runs `morphring doctor` and returns the exit code.
pub fn run(args: &DoctorArgs, ctx: &mut Ctx) -> i32 {
    // The one place doctor peeks at the environment (HOME, XDG_*, TMPDIR), and
    // only to read it. Tests go through `run_in` with folders of their own.
    run_in(args, ctx, &Folders::current())
}

/// [`run`] with the folders passed in. Returns 0 even when engines are
/// missing, and 1 only when the report could not be written.
pub fn run_in(args: &DoctorArgs, ctx: &mut Ctx, folders: &Folders) -> i32 {
    let rows: Vec<Row> = ctx
        .engines
        .iter()
        .map(|engine| Row::new(engine.id(), engine.status()))
        .collect();

    let written = if args.json {
        write_json(ctx.out, &rows)
    } else {
        write_human(ctx.out, &rows, folders)
    };
    match written {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(ctx.err, "morphring: cannot write the doctor report: {e}");
            1
        }
    }
}

/// One engine as `--json` prints it. `path` and `version` are set when the
/// engine is available, `hint` when it is missing; the others are `null`.
#[derive(Debug, Serialize)]
struct Row {
    id: &'static str,
    status: &'static str,
    path: Option<String>,
    version: Option<String>,
    hint: Option<String>,
}

impl Row {
    fn new(id: &'static str, status: EngineStatus) -> Self {
        match status {
            EngineStatus::Available { path, version } => Self {
                id,
                status: "ok",
                path: Some(path.to_string_lossy().into_owned()),
                version: Some(version),
                hint: None,
            },
            EngineStatus::Missing { hint } => Self {
                id,
                status: "missing",
                path: None,
                version: None,
                hint: Some(hint),
            },
        }
    }
}

fn write_json(out: &mut dyn Write, rows: &[Row]) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *out, rows)?;
    writeln!(out)
}

fn write_human(out: &mut dyn Write, rows: &[Row], folders: &Folders) -> io::Result<()> {
    // Versions get a column as wide as the widest one, so paths line up even
    // when somebody's ffmpeg calls itself "n7.1-3-g8e8b2b4b0c-dirty".
    let version_width = rows
        .iter()
        .filter_map(|row| row.version.as_deref())
        .map(str::len)
        .max()
        .unwrap_or(0)
        .max(8);

    for row in rows {
        match (&row.path, &row.version, &row.hint) {
            (Some(path), Some(version), _) => writeln!(
                out,
                "{:<8} {:<8} {version:<version_width$}  {path}",
                row.status, row.id
            )?,
            (_, _, hint) => writeln!(
                out,
                "{:<8} {:<8} install: {}",
                row.status,
                row.id,
                hint.as_deref().unwrap_or_default()
            )?,
        }
    }

    writeln!(out)?;
    writeln!(out, "{:<8} {}", "config", folder(folders.config.as_deref()))?;
    writeln!(out, "{:<8} {}", "data", folder(folders.data.as_deref()))?;
    writeln!(out, "{:<8} {}", "temp", folders.temp.display())?;
    writeln!(out)?;

    let available = rows.iter().filter(|row| row.status == "ok").count();
    writeln!(out, "{available} of {} engines available.", rows.len())
}

fn folder(path: Option<&Path>) -> String {
    match path {
        Some(path) => path.display().to_string(),
        None => "(no home folder, so nothing is saved)".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use morphring_core::testing::FakeConverter;
    use morphring_core::{CancelToken, Converter};

    use super::*;

    fn folders() -> Folders {
        Folders {
            config: Some(PathBuf::from("/home/me/.config/morphring")),
            data: Some(PathBuf::from("/home/me/.local/share/morphring")),
            temp: PathBuf::from("/tmp/morphring"),
        }
    }

    fn doctor(json: bool, engines: Vec<Box<dyn Converter>>) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = {
            let mut ctx = Ctx {
                out: &mut out,
                err: &mut err,
                cwd: PathBuf::from("/"),
                cancel: CancelToken::new(),
                quit: CancelToken::new(),
                engines,
                config_dir: None,
            };
            run_in(&DoctorArgs { json }, &mut ctx, &folders())
        };
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn all_missing() -> Vec<Box<dyn Converter>> {
        vec![
            Box::new(FakeConverter::new("magick").missing("pacman -S imagemagick")),
            Box::new(FakeConverter::new("ffmpeg").missing("pacman -S ffmpeg")),
            Box::new(FakeConverter::new("soffice").missing("pacman -S libreoffice-fresh")),
        ]
    }

    #[test]
    fn missing_engines_print_their_hints_and_still_exit_0() {
        let (code, out, err) = doctor(false, all_missing());
        assert_eq!(code, 0);
        assert_eq!(err, "");
        let missing: Vec<&str> = out.lines().filter(|l| l.starts_with("missing")).collect();
        assert_eq!(
            missing,
            [
                "missing  magick   install: pacman -S imagemagick",
                "missing  ffmpeg   install: pacman -S ffmpeg",
                "missing  soffice  install: pacman -S libreoffice-fresh",
            ]
        );
        assert!(out.ends_with("\n0 of 3 engines available.\n"), "{out}");
    }

    #[test]
    fn available_engines_show_version_and_path_in_columns() {
        let engines: Vec<Box<dyn Converter>> = vec![
            Box::new(FakeConverter::new("magick")),
            Box::new(FakeConverter::new("soffice").missing("pacman -S libreoffice-fresh")),
        ];
        let (code, out, _) = doctor(false, engines);
        assert_eq!(code, 0);
        assert_eq!(
            out,
            "\
ok       magick   1.0       /usr/bin/magick
missing  soffice  install: pacman -S libreoffice-fresh

config   /home/me/.config/morphring
data     /home/me/.local/share/morphring
temp     /tmp/morphring

1 of 2 engines available.
"
        );
    }

    #[test]
    fn long_versions_widen_the_column() {
        let mut rows = vec![
            Row::new(
                "magick",
                EngineStatus::Available {
                    path: "/usr/bin/magick".into(),
                    version: "7.1.2-32".into(),
                },
            ),
            Row::new(
                "ffmpeg",
                EngineStatus::Available {
                    path: "/usr/bin/ffmpeg".into(),
                    version: "n7.1-3-gdirty".into(),
                },
            ),
        ];
        let mut out = Vec::new();
        write_human(&mut out, &rows, &folders()).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("ok       magick   7.1.2-32       /usr/bin/magick\n"));
        assert!(out.contains("ok       ffmpeg   n7.1-3-gdirty  /usr/bin/ffmpeg\n"));

        // The PLAN.md example line, verbatim.
        rows.pop();
        let mut out = Vec::new();
        write_human(&mut out, &rows, &folders()).unwrap();
        assert!(
            String::from_utf8(out)
                .unwrap()
                .starts_with("ok       magick   7.1.2-32  /usr/bin/magick\n")
        );
    }

    #[test]
    fn no_home_folder_is_said_out_loud() {
        let homeless = Folders {
            config: None,
            data: None,
            temp: PathBuf::from("/tmp/morphring"),
        };
        let mut out = Vec::new();
        write_human(&mut out, &[], &homeless).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("config   (no home folder, so nothing is saved)\n"));
        assert!(out.ends_with("0 of 0 engines available.\n"));
    }

    #[test]
    fn json_lists_every_engine_with_nulls_where_nothing_applies() {
        let engines: Vec<Box<dyn Converter>> = vec![
            Box::new(FakeConverter::new("magick")),
            Box::new(FakeConverter::new("ffmpeg").missing("pacman -S ffmpeg")),
            Box::new(FakeConverter::new("soffice").missing("pacman -S libreoffice-fresh")),
        ];
        let (code, out, err) = doctor(true, engines);
        assert_eq!((code, err.as_str()), (0, ""));
        let json: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                {"id": "magick", "status": "ok", "path": "/usr/bin/magick",
                 "version": "1.0", "hint": null},
                {"id": "ffmpeg", "status": "missing", "path": null,
                 "version": null, "hint": "pacman -S ffmpeg"},
                {"id": "soffice", "status": "missing", "path": null,
                 "version": null, "hint": "pacman -S libreoffice-fresh"},
            ])
        );
        // --json means only JSON: no folder lines, no summary.
        assert!(!out.contains("engines available"));
    }

    /// A writer that refuses everything, like a closed pipe.
    struct Broken;

    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "pipe closed"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn an_unwritable_report_is_the_only_exit_1() {
        for json in [false, true] {
            let mut err = Vec::new();
            let code = {
                let mut ctx = Ctx {
                    out: &mut Broken,
                    err: &mut err,
                    cwd: PathBuf::from("/"),
                    cancel: CancelToken::new(),
                    quit: CancelToken::new(),
                    engines: all_missing(),
                    config_dir: None,
                };
                run_in(&DoctorArgs { json }, &mut ctx, &folders())
            };
            assert_eq!(code, 1);
            assert!(
                String::from_utf8(err)
                    .unwrap()
                    .starts_with("morphring: cannot write the doctor report:")
            );
        }
    }

    #[test]
    fn current_folders_end_in_morphring() {
        let folders = Folders::current();
        assert!(folders.temp.ends_with("morphring"));
        for dir in [&folders.config, &folders.data].into_iter().flatten() {
            // ProjectDirs files Windows folders one level deeper, as
            // %APPDATA%\morphring\config and ...\data, so over there the
            // `morphring` part is the parent rather than the last component.
            #[cfg(windows)]
            let ours = dir
                .parent()
                .is_some_and(|parent| parent.ends_with("morphring"));
            #[cfg(not(windows))]
            let ours = dir.ends_with("morphring");
            assert!(ours, "{}", dir.display());
        }
    }
}
