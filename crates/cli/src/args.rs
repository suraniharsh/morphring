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
    disable_version_flag = true,
    disable_help_subcommand = true
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
    Convert(ConvertArgs),
    /// List formats, or the targets one format converts to.
    Formats(FormatsArgs),
    /// Show which engines are installed.
    Doctor(DoctorArgs),
    /// Show help for MorphRing or one command.
    Help(HelpArgs),
    /// Print the version.
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

    /// Print only the summary.
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

/// Arguments of `morphring help`.
#[derive(Debug, Args)]
pub struct HelpArgs {
    /// The command to show help for.
    #[arg(value_name = "COMMAND")]
    pub command: Option<String>,
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
