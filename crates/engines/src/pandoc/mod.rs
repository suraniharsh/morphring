//! The Pandoc engine: Markdown, HTML, DOCX, ODT, RTF, reStructuredText and
//! LaTeX through the system `pandoc`.
//!
//! Ported from Filee's `src/Filee.Engines/Office/PandocConverter.cs`. Pandoc is
//! GPL, so like Filee we only ever run it as a separate program.

pub mod args;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use morphring_core::{
    CancelToken, Converter, Edge, EngineError, EngineStatus, OutputAllocator, Step,
};

use crate::discovery::{self, Tool};
use crate::process::{self, RunError, RunOptions};
use crate::progress::Estimate;

/// How long one `pandoc` run may take before its process group is killed.
const TIMEOUT: Duration = Duration::from_secs(120);

// Pandoc prints nothing while it works, so the ring gets an estimate. Most
// documents are done in well under this; a 400-page thesis is not most documents.
const TYPICAL: Duration = Duration::from_secs(2);

// A note on the offline principle: a Markdown or HTML input can point at
// `https://...` images, and the docx/odt/epub writers will happily download
// them to embed them. That is a network request MorphRing never asked for.
// `--sandbox` would stop it, but Arch's pandoc build fails docx output under
// the sandbox, so for now the risk is documented and revisited in v0.4.
// TODO(v0.4): offline guarantee for pandoc (sandbox, or strip remote images).

/// Converts documents with Pandoc.
#[derive(Clone, Copy, Debug, Default)]
pub struct PandocEngine;

impl PandocEngine {
    /// A new engine. It holds no state: the program is looked up (and cached)
    /// by [`crate::discovery`].
    pub fn new() -> Self {
        Self
    }
}

fn edges() -> &'static [Edge] {
    static EDGES: OnceLock<Vec<Edge>> = OnceLock::new();
    EDGES.get_or_init(args::edges)
}

impl Converter for PandocEngine {
    fn id(&self) -> &'static str {
        Tool::Pandoc.id()
    }

    fn display_name(&self) -> &'static str {
        Tool::Pandoc.display_name()
    }

    fn edges(&self) -> &[Edge] {
        edges()
    }

    fn max_parallelism(&self) -> usize {
        0
    }

    fn status(&self) -> EngineStatus {
        discovery::status(Tool::Pandoc)
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
        let program = discovery::locate(Tool::Pandoc).ok_or_else(|| EngineError::NotInstalled {
            tool: Tool::Pandoc.display_name(),
            hint: discovery::install_hint_here(Tool::Pandoc),
        })?;
        progress(0.0);

        let Some(target) = out.allocate(step.to.as_str(), "") else {
            return Ok(Vec::new());
        };

        // pandoc runs in the input's folder so relative image paths resolve,
        // which means every other path had better be absolute.
        let input = std::path::absolute(&step.input)?;
        let target = std::path::absolute(&target)?;
        let cwd = input
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        // Pandoc writes into a private temp dir first, so a half-written file
        // never shows up under the user's chosen name. The TempDir cleans up
        // after failures and cancels on Drop.
        std::fs::create_dir_all(&step.work_dir)?;
        let temp = tempfile::Builder::new()
            .prefix("pandoc-")
            .tempdir_in(&step.work_dir)?;
        let temp_dir = std::path::absolute(temp.path())?;
        let temp_out = temp_dir.join(format!("out.{}", step.to));

        // The media folder sits next to the final file, not the temp one: the
        // converted document links to it by that path.
        let argv = args::args(
            &input,
            step.from,
            step.to,
            &temp_out,
            &args::media_dir(&target),
        )
        .ok_or_else(|| {
            EngineError::Failed(format!(
                "Pandoc cannot convert {} to {}",
                step.from, step.to
            ))
        })?;

        let run = Estimate::during(progress, TYPICAL, || {
            process::run(
                &program,
                &argv,
                RunOptions {
                    timeout: Some(TIMEOUT),
                    cwd: Some(cwd),
                    ..RunOptions::default()
                },
                cancel,
            )
        });
        // Filee judges success by the output file alone, exit code be damned.
        // Pandoc's warnings go to stderr with exit 0, and that's fine by us.
        let (code, tail) = match run {
            Ok(output) => (Some(0), output.stderr_tail),
            Err(RunError::Failed { code, tail }) => (code, tail),
            Err(RunError::Cancelled) => return Err(EngineError::Cancelled),
            Err(RunError::TimedOut) => {
                return Err(EngineError::Failed(format!(
                    "Pandoc took longer than {} s and was stopped",
                    TIMEOUT.as_secs()
                )));
            }
            Err(other) => return Err(EngineError::Failed(format!("Pandoc {other}"))),
        };
        if !temp_out.is_file() {
            return Err(EngineError::Failed(failed_message(code, &tail)));
        }
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        process::move_into_place(&temp_out, &target)?;
        progress(1.0);
        Ok(vec![target])
    }
}

/// `Pandoc failed (exit N). <stderr summary>`, Filee's wording.
fn failed_message(code: Option<i32>, tail: &[String]) -> String {
    let code = code.map_or_else(|| "signal".to_string(), |c| c.to_string());
    let summary = process::summarize(tail);
    if summary.is_empty() {
        format!("Pandoc failed (exit {code}).")
    } else {
        format!("Pandoc failed (exit {code}). {summary}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_identity() {
        let engine = PandocEngine::new();
        assert_eq!(engine.id(), "pandoc");
        assert_eq!(engine.display_name(), "Pandoc");
        assert_eq!(engine.max_parallelism(), 0);
        assert_eq!(engine.edges(), args::edges().as_slice());
    }

    #[test]
    fn status_comes_from_discovery() {
        assert_eq!(
            PandocEngine::new().status(),
            discovery::status(Tool::Pandoc)
        );
    }

    #[test]
    fn failure_message_matches_filee() {
        assert_eq!(
            failed_message(Some(64), &["pandoc: unknown reader".to_string()]),
            "Pandoc failed (exit 64). pandoc: unknown reader"
        );
        assert_eq!(failed_message(Some(1), &[]), "Pandoc failed (exit 1).");
        assert_eq!(failed_message(None, &[]), "Pandoc failed (exit signal).");
    }
}
