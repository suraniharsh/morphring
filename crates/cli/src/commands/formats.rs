//! `morphring formats`.

use crate::args::FormatsArgs;
use crate::ctx::Ctx;

/// Runs `morphring formats` and returns the exit code.
pub fn run(_args: &FormatsArgs, ctx: &mut Ctx) -> i32 {
    // TODO(T33): the real thing. Exit 2 keeps scripts from mistaking a stub for success.
    let _ = writeln!(ctx.err, "morphring: not implemented yet");
    2
}
