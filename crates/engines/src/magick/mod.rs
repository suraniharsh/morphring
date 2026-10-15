//! The ImageMagick engine: raster images to raster images (and to PDF) by
//! running the system `magick`.
//!
//! Ported from Filee's `src/Filee.Engines/Magick/MagickImageConverter.cs` and
//! `ImageEncoder.Write`. Filee calls Magick.NET in-process; we start `magick`
//! through [`crate::process::run`] and build its arguments with [`args`].

pub mod args;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use morphring_core::{
    CancelToken, Converter, Edge, EngineError, EngineStatus, OutputAllocator, Step,
};

use crate::discovery::{self, Tool};
use crate::process::{self, RunError, RunOptions};
use args::FramePolicy;

/// How long one `magick` run may take before its process group is killed.
const TIMEOUT: Duration = Duration::from_secs(120);

/// Converts images with ImageMagick 7 (`magick`).
#[derive(Clone, Copy, Debug, Default)]
pub struct MagickEngine;

impl MagickEngine {
    /// The engine. It holds no state: the program is looked up (and cached) by
    /// [`crate::discovery`].
    pub const fn new() -> Self {
        Self
    }
}

impl Converter for MagickEngine {
    fn id(&self) -> &'static str {
        Tool::Magick.id()
    }

    fn display_name(&self) -> &'static str {
        Tool::Magick.display_name()
    }

    fn edges(&self) -> &[Edge] {
        args::EDGES
    }

    fn max_parallelism(&self) -> usize {
        0
    }

    fn status(&self) -> EngineStatus {
        discovery::status(Tool::Magick)
    }

    fn convert(
        &self,
        step: &Step,
        out: &mut dyn OutputAllocator,
        progress: &dyn Fn(f32),
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError> {
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        let program = discovery::locate(Tool::Magick).ok_or_else(|| EngineError::NotInstalled {
            tool: Tool::Magick.display_name(),
            hint: discovery::install_hint_here(Tool::Magick),
        })?;
        progress(0.0);

        // magick runs inside the temp dir (see below), so a relative input would
        // suddenly point at nothing. Pin it down first.
        let input = std::path::absolute(&step.input)?;
        let ext = step.to.as_str();

        // ICO sources keep their largest icon, and per-frame targets need the frame
        // count before we can ask for names. Everything else skips the extra run.
        let policy = args::frame_policy(step.from, step.to);
        let needs_identify = policy == FramePolicy::PerFrame || step.from.as_str() == "ico";
        let (frames, frame) = if needs_identify {
            let info = process::run(
                &program,
                &args::identify_args(&input),
                RunOptions {
                    timeout: Some(TIMEOUT),
                    ..RunOptions::default()
                },
                cancel,
            )
            .map_err(run_error)?;
            let frames = frame_count(&info.stdout);
            if frames == 0 {
                return Err(EngineError::Failed(format!(
                    "ImageMagick could not read {}",
                    step.input.display()
                )));
            }
            let pick = (step.from.as_str() == "ico")
                .then(|| args::largest_frame(&info.stdout))
                .flatten();
            (frames, pick)
        } else {
            (1, None)
        };
        let split = policy == FramePolicy::PerFrame && frames > 1;

        // Ask for every name up front. A `None` is the skip policy talking, not an
        // error: that frame is converted into the temp dir and quietly dropped.
        let targets: Vec<Option<PathBuf>> = if split {
            (1..=frames)
                .map(|n| out.allocate(ext, &format!("_p{n}")))
                .collect()
        } else {
            vec![out.allocate(ext, "")]
        };
        if targets.iter().all(Option::is_none) {
            return Ok(Vec::new());
        }

        // A private temp subdir per run: partial output never lands next to the
        // user's files, and the TempDir's Drop sweeps up after failures and cancels.
        std::fs::create_dir_all(&step.work_dir)?;
        let temp = tempfile::Builder::new()
            .prefix("magick-")
            .tempdir_in(&step.work_dir)?;

        // Output names are relative to the temp dir on purpose: magick treats '%'
        // in an output path as a format escape, and I'd rather not find out what a
        // TMPDIR of "/tmp/100%" does to it.
        let name = if split {
            format!("out-%d.{ext}")
        } else {
            format!("out.{ext}")
        };
        let argv: Vec<OsString> = args::args(
            &input,
            Path::new(&name),
            step.from,
            step.to,
            &step.options,
            frame,
        );
        let run = process::run(
            &program,
            &argv,
            RunOptions {
                timeout: Some(TIMEOUT),
                cwd: Some(temp.path().to_path_buf()),
                ..RunOptions::default()
            },
            cancel,
        );
        let tail = match run {
            Ok(output) => output.stderr_tail,
            Err(err) => return Err(run_error(err)),
        };

        let produced = if split {
            numbered_outputs(temp.path(), ext)?
        } else {
            vec![temp.path().join(&name)]
        };
        if produced.len() < targets.len() || produced.iter().any(|p| !non_empty(p)) {
            return Err(EngineError::Failed(no_output_message(&tail)));
        }

        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        let mut written = Vec::new();
        for (from, to) in produced.iter().zip(&targets) {
            let Some(to) = to else { continue };
            if let Err(err) = process::move_into_place(from, to) {
                // Half a set of frames is worse than none: take back what already
                // moved so a retry starts clean.
                for path in &written {
                    let _ = std::fs::remove_file(path);
                }
                return Err(err.into());
            }
            written.push(to.clone());
        }
        progress(1.0);
        Ok(written)
    }
}

/// Maps a process failure onto the engine error the job runner reports.
fn run_error(err: RunError) -> EngineError {
    match err {
        RunError::Cancelled => EngineError::Cancelled,
        RunError::TimedOut => EngineError::Failed(format!(
            "ImageMagick took longer than {} s and was stopped",
            TIMEOUT.as_secs()
        )),
        other => EngineError::Failed(format!("ImageMagick {other}")),
    }
}

fn no_output_message(tail: &[String]) -> String {
    let summary = process::summarize(tail);
    if summary.is_empty() {
        "ImageMagick wrote no output".to_string()
    } else {
        format!("ImageMagick wrote no output: {summary}")
    }
}

fn non_empty(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0)
}

/// Number of frames in `magick identify -format "%w %h\n"` output.
fn frame_count(identify_output: &str) -> usize {
    identify_output
        .lines()
        .filter(|line| {
            let mut parts = line.split_whitespace().map(str::parse::<u64>);
            matches!((parts.next(), parts.next()), (Some(Ok(_)), Some(Ok(_))))
        })
        .count()
}

/// The `out-<n>.<ext>` files magick wrote for a split, in frame order.
///
/// Sorted by the number rather than trusting it to start at 0: `%d` is the
/// scene number, and some files start counting wherever they please.
fn numbered_outputs(dir: &Path, ext: &str) -> std::io::Result<Vec<PathBuf>> {
    let suffix = format!(".{ext}");
    let mut numbered = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(n) = parse_frame_number(name, &suffix) {
            numbered.push((n, path));
        }
    }
    numbered.sort_by_key(|(n, _)| *n);
    Ok(numbered.into_iter().map(|(_, path)| path).collect())
}

/// `out-12.png` -> `Some(12)` for suffix `.png`.
fn parse_frame_number(name: &str, suffix: &str) -> Option<u64> {
    name.strip_prefix("out-")?
        .strip_suffix(suffix)?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_count_counts_size_lines_only() {
        assert_eq!(frame_count("32 32\n32 32\n32 32\n"), 3);
        assert_eq!(frame_count("64 48"), 1);
        assert_eq!(frame_count("\nwarning: odd\n16 16\n"), 1);
        assert_eq!(frame_count(""), 0);
    }

    #[test]
    fn frame_numbers_parse_from_temp_names() {
        assert_eq!(parse_frame_number("out-0.png", ".png"), Some(0));
        assert_eq!(parse_frame_number("out-12.png", ".png"), Some(12));
        assert_eq!(parse_frame_number("out-1.jpg", ".png"), None);
        assert_eq!(parse_frame_number("out.png", ".png"), None);
        assert_eq!(parse_frame_number("out-x.png", ".png"), None);
    }

    #[test]
    fn numbered_outputs_sort_numerically() {
        let dir = tempfile::tempdir().expect("temp dir");
        for name in ["out-10.png", "out-2.png", "out-1.png", "stray.png"] {
            std::fs::write(dir.path().join(name), b"x").expect("write");
        }
        let names: Vec<String> = numbered_outputs(dir.path(), "png")
            .expect("list")
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["out-1.png", "out-2.png", "out-10.png"]);
    }

    #[test]
    fn engine_identity() {
        let engine = MagickEngine::new();
        assert_eq!(engine.id(), "magick");
        assert_eq!(engine.display_name(), "ImageMagick");
        assert_eq!(engine.max_parallelism(), 0);
        assert_eq!(engine.edges(), args::EDGES);
    }
}
