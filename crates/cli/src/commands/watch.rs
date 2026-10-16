//! `morphring watch`: converts the files that arrive in a folder, ported
//! from `WatchAsync` in Filee's `src/Filee.Cli/Cli.cs`.
//!
//! A stub for now: the arguments are parsed and checked, but the watching
//! itself arrives with P2-42.

use crate::EXIT_USAGE;
use crate::args::WatchArgs;
use crate::ctx::Ctx;

/// Runs `morphring watch` and returns the exit code.
pub fn run(_args: &WatchArgs, ctx: &mut Ctx) -> i32 {
    let _ = writeln!(ctx.err, "morphring: watch is not available yet");
    EXIT_USAGE
}
