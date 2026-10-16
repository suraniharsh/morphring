//! `morphring presets`: lists the presets, ported from `ListPresets` in
//! Filee's `src/Filee.Cli/Cli.cs`.
//!
//! One line per preset in the user's `presets.json` order, so the list reads
//! the same way the ring does. Nothing here writes to disk, unless you count
//! the `.broken` copy `Library::load` leaves behind for a mangled file.

use std::fmt::Write as _;

use morphring_core::profiles::Library;
use morphring_core::settings::Store;

use crate::ctx::Ctx;

/// Runs `morphring presets` and returns the exit code.
pub fn run(ctx: &mut Ctx) -> i32 {
    let library = Library::load(&Store::new(ctx.config_dir.clone()));

    // Built up in one String and written once: a table that arrives in
    // instalments is how you end up with half a header in a broken pipe.
    let mut text = format!("{:<24} {:<28} TARGET\n", "ID", "NAME");
    for preset in &library.presets {
        let _ = writeln!(
            text,
            "{:<24} {:<28} {}",
            preset.id,
            preset.display_name(),
            preset.target
        );
    }
    text.push_str("\nUse one with: morphring convert <files> --preset <id or name>\n");

    let _ = ctx.out.write_all(text.as_bytes());
    0
}
