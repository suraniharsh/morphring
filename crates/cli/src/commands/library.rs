//! `morphring presets export|import|reset`: the preset library as a whole.
//!
//! These go beyond Filee, whose command line only lists presets; export,
//! import and reset live on its GUI General page. For now they are signposts
//! with nothing behind them yet, so a script that calls one gets a clear
//! refusal instead of a quiet success that did nothing.

use crate::EXIT_USAGE;
use crate::args::{PresetsAction, PresetsFileArgs, PresetsResetArgs};
use crate::ctx::Ctx;

/// Runs one `morphring presets <action>` and returns the exit code.
pub fn run(action: &PresetsAction, ctx: &mut Ctx) -> i32 {
    match action {
        PresetsAction::Export(args) => export(args, ctx),
        PresetsAction::Import(args) => import(args, ctx),
        PresetsAction::Reset(args) => reset(args, ctx),
    }
}

/// `morphring presets export <file>`.
pub fn export(_args: &PresetsFileArgs, ctx: &mut Ctx) -> i32 {
    not_available_yet("export", ctx)
}

/// `morphring presets import <file>`.
pub fn import(_args: &PresetsFileArgs, ctx: &mut Ctx) -> i32 {
    not_available_yet("import", ctx)
}

/// `morphring presets reset [--yes]`.
pub fn reset(_args: &PresetsResetArgs, ctx: &mut Ctx) -> i32 {
    not_available_yet("reset", ctx)
}

/// The placeholder answer: one line on stderr and the usage exit code. It
/// will not be around long; it has the job security of a scaffold.
fn not_available_yet(action: &str, ctx: &mut Ctx) -> i32 {
    let _ = writeln!(
        ctx.err,
        "morphring: 'presets {action}' is not available yet."
    );
    EXIT_USAGE
}
