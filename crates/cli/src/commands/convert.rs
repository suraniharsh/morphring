//! `morphring convert`.

use crate::args::ConvertArgs;
use crate::ctx::Ctx;

/// Runs `morphring convert` and returns the exit code.
pub fn run(_args: &ConvertArgs, ctx: &mut Ctx) -> i32 {
    // TODO(T32): the real thing. Exit 2 keeps scripts from mistaking a stub for success.
    let _ = writeln!(ctx.err, "morphring: not implemented yet");
    2
}
