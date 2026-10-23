//! Everything a command may touch, passed in from outside so tests can inject it.

use std::io::Write;
use std::path::PathBuf;

use morphring_core::{CancelToken, Converter};

/// The world as a command sees it. Commands read nothing from the environment.
pub struct Ctx<'a> {
    /// Command output (human lines or `--json`).
    pub out: &'a mut dyn Write,
    /// Error messages.
    pub err: &'a mut dyn Write,
    /// The directory relative inputs are resolved against.
    pub cwd: PathBuf,
    /// Fired by Ctrl+C.
    pub cancel: CancelToken,
    /// Fired by a second Ctrl+C: stop everything now, including work that
    /// `cancel` lets finish (the batch `watch` is converting).
    pub quit: CancelToken,
    /// Engines in priority order.
    pub engines: Vec<Box<dyn Converter>>,
    /// Where settings and presets live. `None` means the built-in defaults
    /// only: tests, and machines without a home folder.
    pub config_dir: Option<PathBuf>,
}
