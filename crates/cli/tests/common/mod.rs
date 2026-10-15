//! Shared helpers for the CLI integration tests.
//!
//! The engines crate has its own `tests/common`, but every crate's tests are
//! islands, so the CLI keeps a small copy of the bits it needs.

// Each tests/*.rs file is its own crate and uses only a slice of this module;
// without the allowance the unused slice would light up as dead code.
#![allow(dead_code)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use morphring::ctx::Ctx;
use morphring_core::{CancelToken, Converter};

/// Skips the current test (by returning early) when `tool` is not on `PATH`.
///
/// Prints `SKIP: <tool> not found` so a skipped test is visible in the
/// `--nocapture` output instead of passing silently.
#[macro_export]
macro_rules! require_engine {
    ($tool:expr) => {
        if !$crate::common::engine_available($tool) {
            println!("SKIP: {} not found", $tool);
            return;
        }
    };
}

/// True when `tool` can be found on `PATH`.
pub fn engine_available(tool: &str) -> bool {
    which::which(tool).is_ok()
}

/// Absolute path of a committed fixture in `crates/engines/tests/fixtures`.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../engines/tests/fixtures/"
    ))
    .join(name)
}

/// What one in-process run of the CLI left behind.
#[derive(Debug)]
pub struct Outcome {
    pub code: i32,
    pub out: String,
    pub err: String,
}

/// Runs `morphring <args>` in-process with `cwd` as the current folder and
/// `engines` as the installed engines.
pub fn run_in(cwd: &Path, engines: Vec<Box<dyn Converter>>, args: &[&str]) -> Outcome {
    run_with_cancel(cwd, engines, args, CancelToken::new())
}

/// Like [`run_in`], with a cancel token the test controls.
pub fn run_with_cancel(
    cwd: &Path,
    engines: Vec<Box<dyn Converter>>,
    args: &[&str],
    cancel: CancelToken,
) -> Outcome {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = {
        let mut ctx = Ctx {
            out: &mut out,
            err: &mut err,
            cwd: cwd.to_path_buf(),
            cancel,
            engines,
        };
        morphring::run_with(args.iter().map(OsString::from), &mut ctx)
    };
    Outcome {
        code,
        out: String::from_utf8(out).unwrap_or_else(|e| panic!("stdout is not UTF-8: {e}")),
        err: String::from_utf8(err).unwrap_or_else(|e| panic!("stderr is not UTF-8: {e}")),
    }
}

/// Copies a fixture into `dir` and returns the copy's path.
pub fn copy_fixture(name: &str, dir: &Path) -> PathBuf {
    let to = dir.join(name);
    std::fs::copy(fixture(name), &to).unwrap_or_else(|e| panic!("cannot copy fixture {name}: {e}"));
    to
}
