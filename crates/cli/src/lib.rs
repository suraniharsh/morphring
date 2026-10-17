//! The `morphring` command line. `main.rs` is a thin wrapper around [`run`];
//! tests call [`run_with`] with injected buffers, folder and engines.

pub mod args;
pub mod commands;
pub mod ctx;
pub mod inputs;
pub mod report;
mod resolve;

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

use clap::Parser;
use clap::error::{ContextKind, ContextValue, ErrorKind};
use morphring_core::CancelToken;

use crate::args::{Cli, Command};
use crate::ctx::Ctx;

/// The version printed by `morphring --version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Exit code for wrong arguments (Filee's `UsageError`).
pub const EXIT_USAGE: i32 = 2;

/// Runs the command line with the process's current folder and the installed
/// engines. `args` excludes the program name. Returns the exit code.
pub fn run(
    args: impl IntoIterator<Item = OsString>,
    out: &mut dyn Write,
    err: &mut dyn Write,
    cancel: CancelToken,
) -> i32 {
    // Help, version and typos are answered before we go looking for engines,
    // so `morphring --help` works on a machine with nothing installed.
    let command = match parse(args, out, err) {
        Parsed::Exit(code) => return code,
        Parsed::Run(command) => command,
    };
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
        config_dir: config_dir(),
    };
    dispatch(command, &mut ctx)
}

/// The folder that holds settings.json and presets.json, the same one the
/// app uses. `None` when the OS gives us no home to put it in, in which case
/// everything runs on the built-in defaults.
fn config_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "morphring").map(|dirs| dirs.config_dir().to_path_buf())
}

/// Runs the command line against an explicit context. `args` excludes the
/// program name. Returns the exit code.
pub fn run_with(args: impl IntoIterator<Item = OsString>, ctx: &mut Ctx) -> i32 {
    match parse(args, ctx.out, ctx.err) {
        Parsed::Exit(code) => code,
        Parsed::Run(command) => dispatch(command, ctx),
    }
}

/// What parsing left for us to do.
enum Parsed {
    /// Help, version or a usage error was already printed; exit with this code.
    Exit(i32),
    /// A command that needs the context (and so, the engines).
    Run(Command),
}

fn parse(
    args: impl IntoIterator<Item = OsString>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Parsed {
    let argv = std::iter::once(OsString::from("morphring")).chain(help_aliases(args));
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(e) if e.kind() == ErrorKind::DisplayHelp => {
            // `--help` anywhere: clap renders the override_help of the command
            // it was attached to, which is the same text `help <cmd>` prints.
            let _ = write!(out, "{}", e.render());
            return Parsed::Exit(0);
        }
        Err(e) => return Parsed::Exit(usage_error(err, &usage_message(&e))),
    };

    if cli.version {
        return Parsed::Exit(print_version(out));
    }
    match cli.command {
        None => {
            let _ = write!(out, "{}", args::help_text(None));
            Parsed::Exit(0)
        }
        Some(Command::Help(help)) => {
            let _ = write!(out, "{}", args::help_text(help.command.as_deref()));
            Parsed::Exit(0)
        }
        Some(Command::Version) => Parsed::Exit(print_version(out)),
        Some(command) => match target_choice_error(&command) {
            Some(message) => Parsed::Exit(usage_error(err, message)),
            None => Parsed::Run(command),
        },
    }
}

/// The Windows-style help switch, which Filee accepts on every OS.
const SLASH_HELP: &str = "/?";

/// Rewrites `/?` to `--help` before clap sees it, so it behaves like `-h`
/// wherever that is accepted: general help when first, the command's help
/// after a command. Anything after `--` is left alone; a file genuinely
/// called `/?` deserves the escape hatch, if not our sympathy.
fn help_aliases(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut after_dashes = false;
    args.into_iter()
        .map(|arg| {
            if after_dashes {
                arg
            } else if arg == "--" {
                after_dashes = true;
                arg
            } else if arg == SLASH_HELP {
                OsString::from("--help")
            } else {
                arg
            }
        })
        .collect()
}

/// `convert` and `watch` need exactly one of `--to` and `--preset`. Checked
/// after clap, so a missing input still wins (Filee's order: `convert --to
/// jpg` says "at least one file", `convert a.png` says this).
fn target_choice_error(command: &Command) -> Option<&'static str> {
    let (to, preset) = match command {
        Command::Convert(args) => (&args.to, &args.preset),
        Command::Watch(args) => (&args.to, &args.preset),
        _ => return None,
    };
    (to.is_some() == preset.is_some()).then_some("Use either --to <format> or --preset <preset>.")
}

fn dispatch(command: Command, ctx: &mut Ctx) -> i32 {
    match command {
        Command::Convert(args) => commands::convert::run(&args, ctx),
        Command::Watch(args) => commands::watch::run(&args, ctx),
        Command::Presets(args) => match &args.action {
            None => commands::presets::run(args.json, ctx),
            Some(action) => commands::library::run(action, ctx),
        },
        Command::Formats(args) => commands::formats::run(&args, ctx),
        Command::Doctor(args) => commands::doctor::run(&args, ctx),
        Command::Integrate(args) => {
            commands::integrate::run(&args, ctx, &commands::integrate::Host::from_process())
        }
        // parse() already answered these; repeating ourselves is cheaper than
        // a second enum that differs by two variants.
        Command::Help(help) => {
            let _ = write!(ctx.out, "{}", args::help_text(help.command.as_deref()));
            0
        }
        Command::Version => print_version(ctx.out),
    }
}

fn print_version(out: &mut dyn Write) -> i32 {
    let _ = writeln!(out, "morphring {VERSION}");
    0
}

/// Prints a usage error the way Filee does and returns exit code 2.
fn usage_error(err: &mut dyn Write, message: &str) -> i32 {
    let _ = writeln!(err, "morphring: {message}");
    let _ = writeln!(err, "Run 'morphring help' for usage.");
    EXIT_USAGE
}

/// Turns a clap error into one Filee-style sentence (src/Filee.Cli/CliOptions.cs).
fn usage_message(e: &clap::Error) -> String {
    let string = |kind| match e.get(kind) {
        Some(ContextValue::String(s)) => Some(s.as_str()),
        _ => None,
    };
    // clap names an option as "--quality <1-100>"; we only want "--quality".
    let option = || {
        string(ContextKind::InvalidArg)
            .and_then(|s| s.split_whitespace().next())
            .unwrap_or("an option")
    };

    match e.kind() {
        ErrorKind::InvalidSubcommand => {
            let name = string(ContextKind::InvalidSubcommand).unwrap_or_default();
            format!("Unknown command '{name}'.")
        }
        ErrorKind::UnknownArgument => match string(ContextKind::InvalidArg) {
            Some(arg) if arg.starts_with('-') => format!("Unknown option '{arg}'."),
            Some(arg) => format!("Unexpected argument '{arg}'."),
            None => fallback_message(e),
        },
        ErrorKind::MissingRequiredArgument => {
            let missing = match e.get(ContextKind::InvalidArg) {
                Some(ContextValue::Strings(v)) => v.join(" "),
                Some(ContextValue::String(s)) => s.clone(),
                _ => String::new(),
            };
            // --to/--preset are checked after clap (target_choice_error), so
            // the inputs are the only thing clap can find missing here.
            if missing.contains("INPUTS") {
                "Give at least one file or folder to convert.".to_owned()
            } else if missing.contains("FOLDER") {
                "Give exactly one folder to watch.".to_owned()
            } else if missing.contains("FILE") {
                "Give the presets file.".to_owned()
            } else {
                fallback_message(e)
            }
        }
        ErrorKind::ValueValidation | ErrorKind::InvalidValue
            if option() == "--quality"
                && string(ContextKind::InvalidValue).is_some_and(|v| !v.is_empty()) =>
        {
            "--quality must be a number from 1 to 100.".to_owned()
        }
        ErrorKind::ValueValidation | ErrorKind::InvalidValue
            if option() == "--settle"
                && string(ContextKind::InvalidValue).is_some_and(|v| !v.is_empty()) =>
        {
            "--settle must be a number of seconds.".to_owned()
        }
        ErrorKind::InvalidValue if string(ContextKind::InvalidValue).is_none_or(str::is_empty) => {
            format!("{} needs a value.", option())
        }
        ErrorKind::ArgumentConflict => match string(ContextKind::PriorArg) {
            Some(prior) => {
                let prior = prior.split_whitespace().next().unwrap_or(prior);
                format!("Use either {prior} or {}, not both.", option())
            }
            None => format!("{} can only be given once.", option()),
        },
        _ => fallback_message(e),
    }
}

/// clap's own first line without its "error: " prefix, for the rare kinds we
/// don't rephrase (invalid UTF-8 and friends).
fn fallback_message(e: &clap::Error) -> String {
    let text = e.to_string();
    let first = text.lines().next().unwrap_or_default();
    let first = first.trim_start_matches("error: ");
    let mut message = first.to_owned();
    if let Some(c) = message.get(..1) {
        message.replace_range(..1, &c.to_ascii_uppercase());
    }
    if !message.ends_with('.') {
        message.push('.');
    }
    message
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
                config_dir: None,
            };
            run_with(args.iter().map(OsString::from), &mut ctx)
        };
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    /// Asserts a usage error: exit 2, nothing on stdout, the message and the hint on stderr.
    fn assert_usage_error(args: &[&str], message: &str) {
        let (code, out, err) = run_args(args);
        assert_eq!(code, 2, "{args:?}");
        assert_eq!(out, "", "{args:?}");
        assert_eq!(
            err,
            format!("morphring: {message}\nRun 'morphring help' for usage.\n"),
            "{args:?}"
        );
    }

    #[test]
    fn version_flags_and_command_print_the_version() {
        // Read from Cargo, so a version bump doesn't need a test edit. I learned
        // this at 0.1.1, which is about as early as one can learn it.
        let expected = format!("morphring {}\n", env!("CARGO_PKG_VERSION"));
        for args in [
            &["--version"][..],
            &["-v"],
            &["version"],
            &["--verbose", "version"],
            &["version", "--verbose"],
        ] {
            assert_eq!(
                run_args(args),
                (0, expected.clone(), String::new()),
                "{args:?}"
            );
        }
    }

    #[test]
    fn capital_v_is_not_a_version_flag() {
        assert_usage_error(&["-V"], "Unknown option '-V'.");
    }

    #[test]
    fn no_arguments_and_help_print_the_general_help() {
        for args in [&[][..], &["help"], &["--help"], &["-h"], &["help", "nope"]] {
            let (code, out, err) = run_args(args);
            assert_eq!(code, 0, "{args:?}");
            assert_eq!(out, args::GENERAL_HELP, "{args:?}");
            assert_eq!(err, "", "{args:?}");
        }
    }

    #[test]
    fn help_for_a_command_matches_its_dash_dash_help() {
        for name in ["convert", "watch", "formats", "presets", "doctor"] {
            let (code, via_help, _) = run_args(&["help", name]);
            assert_eq!(code, 0);
            let (code, via_flag, _) = run_args(&[name, "--help"]);
            assert_eq!(code, 0);
            assert_eq!(via_help, via_flag, "{name}");
            assert_eq!(via_help, args::help_text(Some(name)));
        }
        // Filee lowercases the command name for help.
        assert_eq!(run_args(&["help", "CONVERT"]).1, args::CONVERT_HELP);
    }

    #[test]
    fn convert_help_documents_quality_and_not_quiet() {
        let (_, out, _) = run_args(&["help", "convert"]);
        assert!(out.contains("-q, --quality <1-100>"));
        assert!(out.contains("-q is quality, not quiet"));
        for flag in [
            "-t, --to",
            "-o, --output",
            "-r, --recursive",
            "--overwrite",
            "--skip",
            "--json",
            "--quiet",
            "--verbose",
        ] {
            assert!(out.contains(flag), "{flag}");
        }
    }

    #[test]
    fn general_help_lists_every_command_and_exit_codes() {
        for needle in [
            "morphring convert",
            "--preset",
            "morphring watch",
            "morphring formats",
            "morphring presets",
            "morphring doctor",
            "morphring help",
            "morphring version",
            "Exit codes",
            "--verbose",
        ] {
            assert!(args::GENERAL_HELP.contains(needle), "{needle}");
        }
        // Linux examples, not D:\Scans.
        assert!(!args::GENERAL_HELP.contains('\\'));
    }

    #[test]
    fn unknown_commands_and_options_explain_themselves() {
        assert_usage_error(&["bogus"], "Unknown command 'bogus'.");
        assert_usage_error(&["--bogus"], "Unknown option '--bogus'.");
        assert_usage_error(
            &["convert", "a.png", "--to", "jpg", "--settle", "2"],
            "Unknown option '--settle'.",
        );
        assert_usage_error(&["version", "extra"], "Unexpected argument 'extra'.");
    }

    #[test]
    fn convert_needs_inputs_and_a_target() {
        assert_usage_error(&["convert"], "Give at least one file or folder to convert.");
        assert_usage_error(
            &["convert", "--to", "jpg"],
            "Give at least one file or folder to convert.",
        );
        assert_usage_error(
            &["convert", "a.png"],
            "Use either --to <format> or --preset <preset>.",
        );
        assert_usage_error(
            &["convert", "a.png", "--to", "jpg", "--preset", "to-png"],
            "Use either --to <format> or --preset <preset>.",
        );
        assert_usage_error(&["convert", "a.png", "-p"], "--preset needs a value.");
        assert_usage_error(&["convert", "a.png", "--to"], "--to needs a value.");
        assert_usage_error(&["convert", "a.png", "-t"], "--to needs a value.");
    }

    #[test]
    fn watch_needs_one_folder_and_a_target() {
        assert_usage_error(&["watch"], "Give exactly one folder to watch.");
        assert_usage_error(
            &["watch", "--to", "jpg"],
            "Give exactly one folder to watch.",
        );
        assert_usage_error(
            &["watch", "inbox", "other", "--to", "jpg"],
            "Unexpected argument 'other'.",
        );
        assert_usage_error(
            &["watch", "inbox"],
            "Use either --to <format> or --preset <preset>.",
        );
        assert_usage_error(
            &["watch", "inbox", "-t", "pdf", "-p", "to-pdf"],
            "Use either --to <format> or --preset <preset>.",
        );
        for settle in ["-1", "soon", "inf"] {
            assert_usage_error(
                &["watch", "inbox", "--to", "pdf", "--settle", settle],
                "--settle must be a number of seconds.",
            );
        }
        assert_usage_error(
            &["watch", "inbox", "--to", "pdf", "--settle"],
            "--settle needs a value.",
        );
        assert_usage_error(
            &["watch", "inbox", "--to", "pdf", "--skip", "--overwrite"],
            "Use either --overwrite or --skip, not both.",
        );
    }

    #[test]
    fn watch_wants_its_folder_and_presets_lists() {
        // No /inbox here (we hope), so the watch stops before it starts.
        let (code, out, err) = run_args(&["watch", "inbox", "--to", "pdf"]);
        assert_eq!((code, out.as_str()), (2, ""));
        assert!(err.starts_with("morphring: Folder not found: "), "{err}");
        assert!(err.ends_with("inbox\n"), "{err}");
        // No config dir here, so it is the built-ins and a clean exit.
        let (code, out, err) = run_args(&["presets"]);
        assert_eq!((code, err.as_str()), (0, ""));
        assert!(out.contains("to-png"), "{out}");
    }

    #[test]
    fn presets_actions_without_a_config_folder_refuse_to_save() {
        // Export is left out: with cwd "/" it would try to write to the root.
        for args in [
            &["presets", "import", "in.json"][..],
            &["presets", "reset", "--yes"],
            &["presets", "reset"],
        ] {
            let (code, out, err) = run_args(args);
            assert_eq!((code, out.as_str()), (1, ""), "{args:?}");
            assert_eq!(
                err, "morphring: no config folder to keep the presets in.\n",
                "{args:?}"
            );
        }
        assert_usage_error(&["presets", "export"], "Give the presets file.");
        assert_usage_error(&["presets", "bogus"], "Unknown command 'bogus'.");
    }

    #[test]
    fn presets_help_works_from_every_door() {
        for args in [
            &["presets", "--help"][..],
            &["presets", "export", "--help"],
            &["presets", "reset", "-h"],
            &["help", "presets"],
        ] {
            assert_eq!(
                run_args(args),
                (0, args::PRESETS_HELP.to_owned(), String::new()),
                "{args:?}"
            );
        }
    }

    #[test]
    fn slash_question_mark_is_help() {
        assert_eq!(run_args(&["/?"]).1, args::GENERAL_HELP);
        assert_eq!(run_args(&["convert", "/?"]).1, args::CONVERT_HELP);
        assert_eq!(run_args(&["presets", "/?"]).1, args::PRESETS_HELP);
        let rewritten = help_aliases(["convert", "/?", "--", "/?"].map(OsString::from));
        assert_eq!(rewritten, ["convert", "--help", "--", "/?"]);
    }

    #[test]
    fn quality_must_be_1_to_100() {
        for q in ["0", "101", "abc", "-5", "50.5"] {
            assert_usage_error(
                &["convert", "a.png", "--to", "jpg", "-q", q],
                "--quality must be a number from 1 to 100.",
            );
        }
        assert_usage_error(
            &["convert", "a.png", "--to", "jpg", "--quality"],
            "--quality needs a value.",
        );
    }

    #[test]
    fn overwrite_and_skip_are_exclusive() {
        assert_usage_error(
            &["convert", "a.png", "--to", "jpg", "--skip", "--overwrite"],
            "Use either --overwrite or --skip, not both.",
        );
    }

    #[test]
    fn repeated_options_are_fine_and_the_last_one_wins() {
        // Filee lets you say --json twice; clap would call that a conflict.
        let cli = Cli::try_parse_from([
            "morphring",
            "convert",
            "a",
            "--json",
            "--json",
            "-t",
            "png",
            "b",
            "-t",
            "jpg",
        ])
        .unwrap();
        let Some(Command::Convert(args)) = cli.command else {
            panic!("not convert")
        };
        assert_eq!(args.inputs, ["a", "b"]);
        assert_eq!(args.to.as_deref(), Some("jpg"));
        assert!(args.json);
    }
}
