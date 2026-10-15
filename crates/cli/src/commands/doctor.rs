//! `morphring doctor`.

use crate::args::DoctorArgs;
use crate::ctx::Ctx;

/// Runs `morphring doctor` and returns the exit code.
pub fn run(_args: &DoctorArgs, ctx: &mut Ctx) -> i32 {
    // TODO(T34): the real thing. Exit 2 keeps scripts from mistaking a stub for success.
    let _ = writeln!(ctx.err, "morphring: not implemented yet");
    2
}
