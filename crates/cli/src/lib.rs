//! The `morphring` command line. `main.rs` is a thin wrapper around [`run`];
//! tests call [`run_with`] with injected buffers, folder and engines.

pub mod args;
pub mod commands;
pub mod ctx;
pub mod inputs;
pub mod report;

use std::ffi::OsString;
use std::io::Write;

use clap::Parser;
use clap::error::ErrorKind;
use morphring_core::CancelToken;

use crate::args::{Cli, Command};
use crate::ctx::Ctx;

/// The version printed by `morphring --version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Runs the command line with the process's current folder and the installed
/// engines. `args` excludes the program name. Returns the exit code.
pub fn run(
    args: impl IntoIterator<Item = OsString>,
    out: &mut dyn Write,
    err: &mut dyn Write,
    cancel: CancelToken,
) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            let _ = writeln!(err, "morphring: cannot read the current folder: {e}");
            return 1;
        }
    };
    let mut ctx = Ctx {
        out,
        err,
        cwd,
        cancel,
        engines: morphring_engines::registry::all(),
    };
    run_with(args, &mut ctx)
}

/// Runs the command line against an explicit context. `args` excludes the
/// program name. Returns the exit code.
pub fn run_with(args: impl IntoIterator<Item = OsString>, ctx: &mut Ctx) -> i32 {
    let argv = std::iter::once(OsString::from("morphring")).chain(args);
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(e) if e.kind() == ErrorKind::DisplayHelp => {
            let _ = write!(ctx.out, "{e}");
            return 0;
        }
        Err(e) => {
            // TODO(T15): nicer usage errors. For now: clap's first line, minus its "error: ".
            let text = e.to_string();
            let first = text.lines().next().unwrap_or_default();
            let _ = writeln!(
                ctx.err,
                "morphring: {}",
                first.trim_start_matches("error: ")
            );
            let _ = writeln!(ctx.err, "Run 'morphring help' for usage.");
            return 2;
        }
    };

    if cli.version {
        return print_version(ctx);
    }
    match cli.command {
        Some(Command::Convert(args)) => commands::convert::run(&args, ctx),
        Some(Command::Formats(args)) => commands::formats::run(&args, ctx),
        Some(Command::Doctor(args)) => commands::doctor::run(&args, ctx),
        Some(Command::Version) => print_version(ctx),
        // TODO(T15): `help <command>`. Everybody gets the full help until then.
        Some(Command::Help(_)) | None => {
            use clap::CommandFactory;
            let _ = write!(ctx.out, "{}", Cli::command().render_help());
            0
        }
    }
}

fn print_version(ctx: &mut Ctx) -> i32 {
    let _ = writeln!(ctx.out, "morphring {VERSION}");
    0
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn run_args(args: &[&str]) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = {
            let mut ctx = Ctx {
                out: &mut out,
                err: &mut err,
                cwd: PathBuf::from("/"),
                cancel: CancelToken::new(),
                engines: Vec::new(),
            };
            run_with(args.iter().map(OsString::from), &mut ctx)
        };
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn version_flags_and_command_print_the_version() {
        for args in [&["--version"][..], &["-v"], &["version"]] {
            assert_eq!(
                run_args(args),
                (0, "morphring 0.1.0\n".into(), String::new())
            );
        }
    }

    #[test]
    fn stub_commands_exit_2() {
        let (code, out, err) = run_args(&["convert", "x.png", "--to", "jpg"]);
        assert_eq!(code, 2);
        assert!(out.is_empty());
        assert!(err.contains("not implemented yet"));
    }

    #[test]
    fn usage_errors_exit_2_with_a_hint() {
        let (code, out, err) = run_args(&["bogus"]);
        assert_eq!(code, 2);
        assert!(out.is_empty());
        assert!(err.contains("Run 'morphring help' for usage."));
    }
}
