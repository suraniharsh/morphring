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
    /// Engines in priority order.
    pub engines: Vec<Box<dyn Converter>>,
}
