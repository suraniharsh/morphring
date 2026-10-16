//! The FFmpeg engine: video and audio conversion, video to animated GIF and
//! video to a still PNG frame. Ported from Filee's
//! `src/Filee.Engines/Media/FfmpegConverter.cs` and `FfmpegRunner.cs`.
//!
//! Container and codec choices live in [`encoding`], ffprobe and progress
//! parsing in [`probe`]; this module runs the passes and keeps the output tidy.

pub mod encoding;
pub mod probe;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

use morphring_core::{
    CancelToken, Converter, Edge, EngineError, EngineStatus, OutputAllocator, Step,
};

use crate::discovery::{self, Tool};
use crate::process::{self, RunError, RunOptions};

use encoding::{AudioSource, FfmpegPass, Source, VideoSource};
use probe::MediaInfo;

/// How long one ffmpeg pass may go without printing a single line before we
/// call it stuck. There is deliberately no overall limit: a feature film can
/// encode for an hour on a slow laptop, and that's fine as long as it keeps
/// muttering progress at us. `-progress pipe:1` reports every half second,
/// so two minutes of silence means ffmpeg is wedged, not thinking.
const PASS_STALL: Duration = Duration::from_secs(120);

/// How long ffprobe may take to read a file's header (Filee's `ProbeTimeout`).
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// Progress reported once the probe is done; the passes share the range up to
/// `PROBED + PASSES_SPAN`, and 1.0 comes after the output is in place.
const PROBED: f32 = 0.02;
const PASSES_SPAN: f32 = 0.97;

/// How often the calling thread forwards ffmpeg's progress.
const PROGRESS_POLL: Duration = Duration::from_millis(50);

/// Converts video and audio files with the system's `ffmpeg` and `ffprobe`.
#[derive(Debug, Default, Clone, Copy)]
pub struct FfmpegEngine;

impl FfmpegEngine {
    /// The engine. Tools are looked up (and cached) on first use.
    pub fn new() -> Self {
        Self
    }
}

impl Converter for FfmpegEngine {
    fn id(&self) -> &'static str {
        Tool::Ffmpeg.id()
    }

    fn display_name(&self) -> &'static str {
        Tool::Ffmpeg.display_name()
    }

    fn edges(&self) -> &[Edge] {
        &encoding::EDGES
    }

    fn max_parallelism(&self) -> usize {
        // x264 and libvpx already use every core; two runs keep the CPU busy
        // between files without turning the laptop into a space heater.
        2
    }

    fn status(&self) -> EngineStatus {
        // discovery only reports ffmpeg as found when ffprobe sits next to it.
        discovery::status(Tool::Ffmpeg)
    }

    fn convert(
        &self,
        step: &Step,
        out: &mut dyn OutputAllocator,
        progress: &dyn Fn(f32),
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError> {
        let Some(found) = discovery::find(Tool::Ffmpeg) else {
            return Err(EngineError::NotInstalled {
                tool: Tool::Ffmpeg.display_name(),
                hint: discovery::install_hint_here(Tool::Ffmpeg),
            });
        };
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        let ffprobe = found.companion("ffprobe");
        let input = std::path::absolute(&step.input)?;

        let info = probe_file(&ffprobe, &input, cancel)?;
        progress(PROBED);

        let to = step.to.as_str();
        let Some(target) = out.allocate(to, "") else {
            return Ok(Vec::new());
        };

        // Everything is written into a private folder first, so a failed or
        // cancelled run never leaves half a video under the user's chosen name.
        // Dropping the TempDir is the cleanup, whichever way we leave.
        std::fs::create_dir_all(&step.work_dir)?;
        let scratch = tempfile::Builder::new()
            .prefix("ffmpeg-")
            .tempdir_in(&step.work_dir)?;
        let staged = scratch.path().join(format!("output.{to}"));
        let palette = scratch.path().join("palette.png");

        let source = source_of(&info);
        // Fails for a missing stream before anything runs (no video for a GIF, say).
        let passes = encoding::plan(&input, &staged, to, &step.options, &source, &palette)?;

        let reporter = Reporter::new(progress);
        let mut done = 0.0_f32;
        for pass in &passes {
            run_pass(&found.program, pass, info.duration, done, &reporter, cancel)?;
            done += pass.weight;
        }

        let written = std::fs::metadata(&staged).map(|m| m.len()).unwrap_or(0);
        if written == 0 {
            return Err(EngineError::Failed(
                "FFmpeg did not write any output.".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        process::move_into_place(&staged, &target)?;
        reporter.report(1.0);
        Ok(vec![target])
    }
}

/// Runs ffprobe on `input` and parses what it says.
fn probe_file(
    ffprobe: &Path,
    input: &Path,
    cancel: &CancelToken,
) -> Result<MediaInfo, EngineError> {
    let opts = RunOptions {
        timeout: Some(PROBE_TIMEOUT),
        ..RunOptions::default()
    };
    let output = match process::run(ffprobe, &probe::probe_args(input), opts, cancel) {
        Ok(output) => output,
        Err(RunError::Failed { tail, .. }) => {
            return Err(EngineError::Failed(with_reason(
                "FFmpeg cannot read this file",
                &process::summarize(&tail),
            )));
        }
        Err(err) => return Err(run_error(err, "ffprobe")),
    };
    probe::parse_media_info(&output.stdout)
        .map_err(|err| EngineError::Failed(format!("FFmpeg cannot read this file: {err}")))
}

/// The probe's findings as the plain values the argument builder wants.
fn source_of(info: &MediaInfo) -> Source<'_> {
    Source {
        video: info.video.as_ref().map(|v| VideoSource {
            codec: v.codec.as_deref(),
            pix_fmt: v.pix_fmt.as_deref(),
            codec_tag: v.codec_tag.as_deref(),
            width: v.width,
            height: v.height,
            display_height: v.display_height(),
            fps: (v.frame_rate > 0.0).then_some(v.frame_rate),
        }),
        audio: info.audio.as_ref().map(|a| AudioSource {
            codec: a.codec.as_deref(),
            codec_tag: a.codec_tag.as_deref(),
            channels: a.channels,
        }),
        duration: info.duration,
    }
}

/// Forwards progress to the caller, only ever upwards.
struct Reporter<'a> {
    progress: &'a dyn Fn(f32),
    last: std::cell::Cell<f32>,
}

impl<'a> Reporter<'a> {
    fn new(progress: &'a dyn Fn(f32)) -> Self {
        Self {
            progress,
            last: std::cell::Cell::new(PROBED),
        }
    }

    fn report(&self, value: f32) {
        if value > self.last.get() {
            self.last.set(value);
            (self.progress)(value);
        }
    }
}

/// Runs one pass and maps ffmpeg's position onto this pass's share of the bar.
fn run_pass(
    ffmpeg: &Path,
    pass: &FfmpegPass,
    duration: Option<f64>,
    done: f32,
    reporter: &Reporter<'_>,
    cancel: &CancelToken,
) -> Result<(), EngineError> {
    // The stdout callback runs on the runner's reader thread, but the caller's
    // progress closure isn't Sync. So the reader drops the latest fraction into
    // an atomic and this thread plays postman. f32 bits of non-negative numbers
    // sort like the numbers themselves, which makes fetch_max a free "max".
    let latest = AtomicU32::new(0);
    let on_line = |line: &str| {
        if let Some(fraction) = duration.and_then(|d| probe::parse_progress_line(line, d)) {
            latest.fetch_max(fraction.to_bits(), Ordering::Relaxed);
        }
    };
    let opts = RunOptions {
        stall: Some(PASS_STALL),
        on_stdout_line: Some(&on_line),
        ..RunOptions::default()
    };
    let forward = || {
        let fraction = f32::from_bits(latest.load(Ordering::Relaxed));
        reporter.report(PROBED + PASSES_SPAN * (done + pass.weight * fraction));
    };

    // TODO(v0.2): Filee shows a timed estimate when the duration is unknown
    // (raw streams); here the bar just waits at the pass start for those.
    let result = thread::scope(|scope| {
        let handle = scope.spawn(|| process::run(ffmpeg, &pass.args, opts, cancel));
        while !handle.is_finished() {
            forward();
            thread::sleep(PROGRESS_POLL);
        }
        let result = handle
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        forward();
        result
    });

    match result {
        Ok(_) => Ok(()),
        Err(err) => Err(run_error(err, "ffmpeg")),
    }
}

/// Turns a runner error into the message a user sees.
fn run_error(err: RunError, program: &str) -> EngineError {
    match err {
        RunError::Cancelled => EngineError::Cancelled,
        // Only ffprobe still runs against a clock; passes have the watchdog.
        RunError::TimedOut => EngineError::Failed(format!(
            "{program} did not finish within {} s and was stopped.",
            PROBE_TIMEOUT.as_secs()
        )),
        RunError::Stalled { after } => EngineError::Failed(format!(
            "FFmpeg stopped making progress for {} s",
            after.as_secs()
        )),
        RunError::Spawn { program, source } => {
            EngineError::Failed(format!("Could not start {program}: {source}"))
        }
        RunError::Failed { code, tail } => {
            let head = match code {
                Some(code) => format!("FFmpeg failed (exit code {code})"),
                None => "FFmpeg was stopped by a signal".to_owned(),
            };
            EngineError::Failed(with_reason(&head, &process::summarize(&tail)))
        }
    }
}

/// "head: reason", or "head." when there is no reason (Filee's wording).
fn with_reason(head: &str, reason: &str) -> String {
    if reason.is_empty() {
        format!("{head}.")
    } else {
        format!("{head}: {reason}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_identity() {
        let engine = FfmpegEngine::new();
        assert_eq!(engine.id(), "ffmpeg");
        assert_eq!(engine.display_name(), "FFmpeg");
        assert_eq!(engine.max_parallelism(), 2);
        assert_eq!(engine.edges(), &encoding::EDGES[..]);
    }

    #[test]
    fn failed_runs_carry_the_stderr_summary() {
        let err = run_error(
            RunError::Failed {
                code: Some(1),
                tail: vec![
                    "[libx264 @ 0x55d0] width not divisible by 2".into(),
                    "Conversion failed!".into(),
                ],
            },
            "ffmpeg",
        );
        assert_eq!(
            err.to_string(),
            "FFmpeg failed (exit code 1): width not divisible by 2 Conversion failed!"
        );
        let bare = run_error(
            RunError::Failed {
                code: Some(8),
                tail: Vec::new(),
            },
            "ffmpeg",
        );
        assert_eq!(bare.to_string(), "FFmpeg failed (exit code 8).");
    }

    #[test]
    fn cancel_and_timeout_map_to_engine_errors() {
        assert!(matches!(
            run_error(RunError::Cancelled, "ffmpeg"),
            EngineError::Cancelled
        ));
        assert_eq!(
            run_error(RunError::TimedOut, "ffprobe").to_string(),
            "ffprobe did not finish within 60 s and was stopped."
        );
        assert_eq!(
            run_error(RunError::Stalled { after: PASS_STALL }, "ffmpeg").to_string(),
            "FFmpeg stopped making progress for 120 s"
        );
    }

    #[test]
    fn reporter_only_goes_up() {
        let seen = std::cell::RefCell::new(Vec::new());
        let progress = |v: f32| seen.borrow_mut().push(v);
        let reporter = Reporter::new(&progress);
        for v in [0.01, 0.5, 0.4, 0.5, 0.9, 1.0] {
            reporter.report(v);
        }
        assert_eq!(*seen.borrow(), vec![0.5, 0.9, 1.0]);
    }
}
