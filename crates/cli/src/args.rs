//! The clap tree for the whole v0.1.0 command line (PLAN.md section 5.4).
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
    /// List formats, or the targets one format converts to.
    #[command(override_help = FORMATS_HELP)]
    Formats(FormatsArgs),
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

    /// Target format, as an id or extension (e.g. jpg, pdf, mp3).
    #[arg(short = 't', long = "to", value_name = "FMT")]
    pub to: String,

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
  morphring formats [<format>]
  morphring doctor [--json]
  morphring integrate install|uninstall
  morphring help [<command>]
  morphring version

Examples:
  morphring convert photo.png --to jpg
  morphring convert *.png --to webp --quality 80 -o converted
  morphring convert ~/Scans --recursive --to pdf
  morphring convert clip.mov --to mp4 --skip
  morphring formats docx

Run 'morphring help <command>' for the options of one command. Add --verbose to any
command to log what MorphRing runs to stderr (RUST_LOG works too).

Exit codes: 0 all converted, 1 some failed or were cancelled, 2 wrong arguments or a
missing input, 3 nothing to convert.
";

/// `morphring help convert` (and `morphring convert --help`).
pub const CONVERT_HELP: &str = "\
Usage: morphring convert <files or folders...> --to <format> [options]

Files can be wildcards ('*.heic'). Folders convert the files in them that MorphRing knows.

Options:
  -t, --to <format>      Target format, e.g. jpg, png, pdf, docx, mp4 (see 'morphring formats')
  -o, --output <folder>  Save there instead of next to the sources
  -r, --recursive        Include sub folders
  -q, --quality <1-100>  Quality for JPG, WebP, AVIF, MP3, ... (-q is quality, not quiet)
      --overwrite        Replace existing output files (default: add a number)
      --skip             Leave existing output files alone
      --json             Print the result as JSON (for scripts and other programs)
      --quiet            Print only the summary and errors
      --verbose          Log what MorphRing runs to stderr
  -h, --help             Show this help

Ctrl+C stops the conversion and removes unfinished output; press it twice to quit at once.
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
        assert_eq!(args.to, "jpg");
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
        assert!(Cli::try_parse_from(["morphring", "convert", "a"]).is_err());
    }
}
