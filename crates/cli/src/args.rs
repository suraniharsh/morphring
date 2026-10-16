//! The clap tree for the whole command line (PLAN.md section 5.4, plus the
//! v0.2 additions from PLAN-v0.2.md P2-05).
//!
//! Field names here are a contract: the commands code against them.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Offline file converter.
#[derive(Debug, Parser)]
#[command(
    name = "morphring",
    about = "Convert files offline from the command line.",
    disable_version_flag = true,
    disable_help_subcommand = true,
    args_override_self = true,
    override_help = GENERAL_HELP
)]
pub struct Cli {
    /// Print the version and exit.
    #[arg(short = 'v', long = "version")]
    pub version: bool,

    /// Log what MorphRing is doing to stderr.
    #[arg(long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Convert files to another format.
    #[command(override_help = CONVERT_HELP)]
    Convert(ConvertArgs),
    /// Convert the files that arrive in a folder until Ctrl+C.
    #[command(override_help = WATCH_HELP)]
    Watch(WatchArgs),
    /// List formats, or the targets one format converts to.
    #[command(override_help = FORMATS_HELP)]
    Formats(FormatsArgs),
    /// List the presets.
    #[command(override_help = PRESETS_HELP)]
    Presets,
    /// Show which engines are installed.
    #[command(override_help = DOCTOR_HELP)]
    Doctor(DoctorArgs),
    /// Add or remove the file manager context menu entries.
    #[command(override_help = INTEGRATE_HELP)]
    Integrate(IntegrateArgs),
    /// Show help for MorphRing or one command.
    #[command(override_help = GENERAL_HELP)]
    Help(HelpArgs),
    /// Print the version.
    #[command(override_help = GENERAL_HELP)]
    Version,
}

/// Arguments of `morphring convert`.
#[derive(Debug, Args)]
pub struct ConvertArgs {
    /// Files, folders or wildcards to convert.
    #[arg(required = true, value_name = "INPUTS")]
    pub inputs: Vec<OsString>,

    /// Target format, as an id or extension (e.g. jpg, pdf, mp3). Exactly one
    /// of `to` and `preset` is set once `run` has validated the arguments.
    #[arg(short = 't', long = "to", value_name = "FMT")]
    pub to: Option<String>,

    /// Preset to convert with, by id or name.
    #[arg(short = 'p', long = "preset", value_name = "PRESET")]
    pub preset: Option<String>,

    /// Write outputs to this folder instead of next to each source.
    #[arg(short = 'o', long = "output", value_name = "DIR")]
    pub output: Option<PathBuf>,

    /// Descend into subfolders.
    #[arg(short = 'r', long = "recursive")]
    pub recursive: bool,

    /// Quality from 1 to 100 (-q is quality, not quiet).
    #[arg(short = 'q', long = "quality", value_name = "1-100",
          allow_hyphen_values = true,
          value_parser = clap::value_parser!(u8).range(1..=100))]
    pub quality: Option<u8>,

    /// Replace existing output files.
    #[arg(long, conflicts_with = "skip")]
    pub overwrite: bool,

    /// Leave existing output files alone and skip those conversions.
    #[arg(long)]
    pub skip: bool,

    /// Print a JSON report instead of human-readable lines.
    #[arg(long)]
    pub json: bool,

    /// Print only the summary and errors.
    #[arg(long)]
    pub quiet: bool,

    /// Give each output the dates of its source.
    #[arg(long)]
    pub keep_dates: bool,
}

/// Arguments of `morphring watch`.
#[derive(Debug, Args)]
pub struct WatchArgs {
    /// The folder to watch.
    #[arg(required = true, value_name = "FOLDER")]
    pub folder: PathBuf,

    /// Target format, as an id or extension. Exactly one of `to` and `preset`
    /// is set once `run` has validated the arguments.
    #[arg(short = 't', long = "to", value_name = "FMT")]
    pub to: Option<String>,

    /// Preset to convert with, by id or name.
    #[arg(short = 'p', long = "preset", value_name = "PRESET")]
    pub preset: Option<String>,

    /// Write outputs here instead of `<folder>/converted`.
    #[arg(short = 'o', long = "output", value_name = "DIR")]
    pub output: Option<PathBuf>,

    /// Watch subfolders too; their outputs keep the subfolder.
    #[arg(short = 'r', long = "recursive")]
    pub recursive: bool,

    /// Quality from 1 to 100 (-q is quality, not quiet).
    #[arg(short = 'q', long = "quality", value_name = "1-100",
          allow_hyphen_values = true,
          value_parser = clap::value_parser!(u8).range(1..=100))]
    pub quality: Option<u8>,

    /// Replace existing output files.
    #[arg(long, conflicts_with = "skip")]
    pub overwrite: bool,

    /// Leave existing output files alone and skip those conversions.
    #[arg(long)]
    pub skip: bool,

    /// Print only errors.
    #[arg(long)]
    pub quiet: bool,

    /// Give each output the dates of its source.
    #[arg(long)]
    pub keep_dates: bool,

    /// Move converted sources to `<folder>/originals`.
    #[arg(long)]
    pub move_originals: bool,

    /// Seconds a file must stay unchanged before it is converted.
    #[arg(long, value_name = "SECONDS", allow_hyphen_values = true,
          value_parser = parse_settle)]
    pub settle: Option<f64>,
}

/// `--settle`: a finite number of seconds, zero or more. The message itself
/// is replaced by `usage_message`, so it only has to be true, not pretty.
fn parse_settle(text: &str) -> Result<f64, String> {
    match text.trim().parse::<f64>() {
        // "inf" and "NaN" parse as f64 too; waiting forever for a file to
        // settle is a watch folder that never converts anything.
        Ok(seconds) if seconds.is_finite() && seconds >= 0.0 => Ok(seconds),
        _ => Err("must be a number of seconds".to_owned()),
    }
}

/// Arguments of `morphring formats`.
#[derive(Debug, Args)]
pub struct FormatsArgs {
    /// Show the targets this format converts to.
    #[arg(value_name = "FMT")]
    pub format: Option<String>,
}

/// Arguments of `morphring doctor`.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Print the engine list as JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments of `morphring integrate`.
#[derive(Debug, Args)]
pub struct IntegrateArgs {
    /// `install` or `uninstall`. Checked by the command, so a typo gets a
    /// sentence instead of clap's list of possible values.
    #[arg(value_name = "ACTION")]
    pub action: Option<String>,

    /// The program the menus start (default: the MorphRing app).
    #[arg(long, value_name = "PATH")]
    pub exe: Option<PathBuf>,

    /// Show what would change without changing anything.
    #[arg(long)]
    pub dry_run: bool,

    /// Change the desktop even though this is not a release build.
    #[arg(long)]
    pub allow: bool,
}

/// Arguments of `morphring help`.
#[derive(Debug, Args)]
pub struct HelpArgs {
    /// The command to show help for.
    #[arg(value_name = "COMMAND")]
    pub command: Option<String>,
}

// The help texts are written by hand, like Filee's Help.cs, because clap's
// generated help can't say "-q is quality, not quiet" with a straight face
// and we want `help convert` and `convert --help` to be the very same words.

/// `morphring help` (and `morphring --help`).
pub const GENERAL_HELP: &str = "\
MorphRing: convert files offline from the command line.

Usage:
  morphring convert <files or folders...> --to <format> [options]
  morphring convert <files or folders...> --preset <preset> [options]
  morphring watch <folder> --to <format> [options]
  morphring formats [<format>]
  morphring presets
  morphring doctor [--json]
  morphring integrate install|uninstall
  morphring help [<command>]
  morphring version

Examples:
  morphring convert photo.png --to jpg
  morphring convert *.png --to webp --quality 80 -o converted
  morphring convert ~/Scans --recursive --to pdf
  morphring convert clip.mov --to mp4 --skip
  morphring convert ~/Scans --recursive --preset to-pdf
  morphring watch ~/Inbox --to pdf --move-originals
  morphring formats docx

Run 'morphring help <command>' for the options of one command. Add --verbose to any
command to log what MorphRing runs to stderr (RUST_LOG works too).

Exit codes: 0 all converted, 1 some failed or were cancelled, 2 wrong arguments or a
missing input, 3 nothing to convert.
";

/// `morphring help convert` (and `morphring convert --help`).
pub const CONVERT_HELP: &str = "\
Usage: morphring convert <files or folders...> --to <format> [options]
       morphring convert <files or folders...> --preset <preset> [options]

Files can be wildcards ('*.heic'). Folders convert the files in them that MorphRing knows.

Options:
  -t, --to <format>      Target format, e.g. jpg, png, pdf, docx, mp4 (see 'morphring formats')
  -p, --preset <preset>  Preset id or name, e.g. to-pdf or 'Half size' (see 'morphring presets')
  -o, --output <folder>  Save there instead of next to the sources
  -r, --recursive        Include sub folders
  -q, --quality <1-100>  Quality for JPG, WebP, AVIF, MP3, ... (-q is quality, not quiet)
      --overwrite        Replace existing output files (default: add a number)
      --skip             Leave existing output files alone
      --keep-dates       Give each output the dates of its source
      --json             Print the result as JSON (for scripts and other programs)
      --quiet            Print only the summary and errors
      --verbose          Log what MorphRing runs to stderr
  -h, --help             Show this help

Ctrl+C stops the conversion and removes unfinished output; press it twice to quit at once.
";

// One text per OS, because the settle default is one number per OS
// (core::watch::default_settle). A macro, since concat! wants literals and a
// `const fn` that glues strings together is still a dream.
macro_rules! watch_help {
    ($settle:literal) => {
        concat!(
            "\
Usage: morphring watch <folder> --to <format> [options]
       morphring watch <folder> --preset <preset> [options]

Converts every file that arrives in the folder until you press Ctrl+C.

Options:
  -t, --to <format>      Target format, e.g. jpg, png, pdf, docx, mp4 (see 'morphring formats')
  -p, --preset <preset>  Preset id or name (see 'morphring presets')
  -o, --output <folder>  Save there (default: <folder>/converted)
  -r, --recursive        Include sub folders; their files keep the sub folder in the output
      --move-originals   Move converted sources to <folder>/originals; files already
                         waiting are converted at start
      --settle <seconds> Wait until a file has not changed for this long (default ",
            $settle,
            ")
      --keep-dates       Give each output the dates of its source
      --quiet            Print nothing but errors
"
        )
    };
}

/// `morphring help watch` (and `morphring watch --help`).
#[cfg(not(windows))]
pub const WATCH_HELP: &str = watch_help!("5");

/// `morphring help watch` (and `morphring watch --help`).
#[cfg(windows)]
pub const WATCH_HELP: &str = watch_help!("2");

/// `morphring help presets` (and `morphring presets --help`).
pub const PRESETS_HELP: &str = "\
Usage: morphring presets

Lists the presets with their id, name and target format. Use one with:
  morphring convert <files> --preset <id or name>
";

/// `morphring help formats` (and `morphring formats --help`).
pub const FORMATS_HELP: &str = "\
Usage: morphring formats [<format>]

Lists every format, or what one format converts to with the engines installed.
";

/// `morphring help doctor` (and `morphring doctor --help`).
pub const DOCTOR_HELP: &str = "\
Usage: morphring doctor [--json]

Shows which engines (ImageMagick, FFmpeg, LibreOffice) are installed, and how to
install the missing ones.

Options:
      --json             Print the engine list as JSON
";

/// `morphring help integrate` (and `morphring integrate --help`).
pub const INTEGRATE_HELP: &str = "\
Usage: morphring integrate install|uninstall [options]

Adds or removes 'Convert with MorphRing' in the file manager context menu
(Dolphin, Nautilus, Nemo and Thunar). The entry opens the MorphRing app.

Options:
      --exe <path>       Program the menu starts (default: morphring-app next to
                         this program or on PATH)
      --dry-run          Show what would change without changing anything
      --allow            Change the desktop from a development build
";

/// The help text for `morphring help [<command>]`. Unknown commands get the
/// general help, as in Filee.
pub fn help_text(command: Option<&str>) -> &'static str {
    match command.map(str::to_ascii_lowercase).as_deref() {
        Some("convert") => CONVERT_HELP,
        Some("watch") => WATCH_HELP,
        Some("presets") => PRESETS_HELP,
        Some("formats") => FORMATS_HELP,
        Some("doctor") => DOCTOR_HELP,
        Some("integrate") => INTEGRATE_HELP,
        _ => GENERAL_HELP,
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn clap_tree_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn convert_parses_every_flag() {
        let cli = Cli::try_parse_from([
            "morphring",
            "convert",
            "a.png",
            "b",
            "-t",
            "jpg",
            "-o",
            "out",
            "-r",
            "-q",
            "80",
            "--skip",
            "--json",
            "--quiet",
        ])
        .unwrap();
        let Some(Command::Convert(args)) = cli.command else {
            panic!("not convert")
        };
        assert_eq!(args.inputs, ["a.png", "b"]);
        assert_eq!(args.to.as_deref(), Some("jpg"));
        assert_eq!(args.preset, None);
        assert_eq!(args.output, Some(PathBuf::from("out")));
        assert!(args.recursive && args.skip && args.json && args.quiet && !args.overwrite);
        assert_eq!(args.quality, Some(80));
    }

    #[test]
    fn convert_rejects_bad_quality_and_conflicting_policies() {
        assert!(
            Cli::try_parse_from(["morphring", "convert", "a", "--to", "jpg", "-q", "0"]).is_err()
        );
        assert!(
            Cli::try_parse_from([
                "morphring",
                "convert",
                "a",
                "--to",
                "jpg",
                "--skip",
                "--overwrite"
            ])
            .is_err()
        );
        // A missing --to is no longer clap's business: run() checks --to
        // against --preset, so it can say one sentence about both.
        assert!(Cli::try_parse_from(["morphring", "convert", "a"]).is_ok());
    }

    #[test]
    fn convert_takes_a_preset_and_keep_dates() {
        let cli = Cli::try_parse_from([
            "morphring",
            "convert",
            "a.png",
            "-p",
            "to-pdf",
            "--keep-dates",
        ])
        .unwrap();
        let Some(Command::Convert(args)) = cli.command else {
            panic!("not convert")
        };
        assert_eq!(args.to, None);
        assert_eq!(args.preset.as_deref(), Some("to-pdf"));
        assert!(args.keep_dates);
    }

    #[test]
    fn watch_parses_every_flag() {
        let cli = Cli::try_parse_from([
            "morphring",
            "watch",
            "inbox",
            "--preset",
            "Half size",
            "-o",
            "out",
            "-r",
            "-q",
            "70",
            "--overwrite",
            "--quiet",
            "--keep-dates",
            "--move-originals",
            "--settle",
            "0.2",
        ])
        .unwrap();
        let Some(Command::Watch(args)) = cli.command else {
            panic!("not watch")
        };
        assert_eq!(args.folder, PathBuf::from("inbox"));
        assert_eq!(args.to, None);
        assert_eq!(args.preset.as_deref(), Some("Half size"));
        assert_eq!(args.output, Some(PathBuf::from("out")));
        assert_eq!(args.quality, Some(70));
        assert!(args.recursive && args.overwrite && args.quiet && args.keep_dates);
        assert!(args.move_originals && !args.skip);
        assert_eq!(args.settle, Some(0.2));
    }

    #[test]
    fn settle_is_a_finite_number_of_seconds() {
        assert_eq!(parse_settle("0"), Ok(0.0));
        assert_eq!(parse_settle(" 2.5 "), Ok(2.5));
        for bad in ["-1", "abc", "inf", "NaN", ""] {
            assert!(parse_settle(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn watch_help_states_this_os_settle_default() {
        let secs = morphring_core::watch::default_settle().as_secs();
        assert!(
            WATCH_HELP.contains(&format!("(default {secs})")),
            "{WATCH_HELP}"
        );
    }
}
