//! `morphring presets export|import|reset`: the preset library as a whole.
//!
//! These go beyond Filee, whose command line only lists presets; export,
//! import and reset live on its GUI General page (`UserDataStore.cs`). The
//! real work is in `morphring_core::bundle`; this file is the polite front
//! desk that reads files, writes files and asks before it shreds anything.

use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use morphring_core::bundle;
use morphring_core::profiles::Library;
use morphring_core::settings::Store;

use crate::EXIT_USAGE;
use crate::args::{PresetsAction, PresetsFileArgs, PresetsResetArgs};
use crate::ctx::Ctx;

/// Exit code when the command ran but did not get its way (Filee's
/// general failure code).
const EXIT_FAILED: i32 = 1;

/// The question `reset` asks, Filee's `general.reset_confirm` with a hint.
const RESET_PROMPT: &str = "Replace all presets and toolbar profiles with the defaults? [y/N] ";

/// Runs one `morphring presets <action>` and returns the exit code.
pub fn run(action: &PresetsAction, ctx: &mut Ctx) -> i32 {
    match action {
        PresetsAction::Export(args) => export(args, ctx),
        PresetsAction::Import(args) => import(args, ctx),
        PresetsAction::Reset(args) => reset(args, ctx),
    }
}

/// `morphring presets export <file>`: writes the library as a bundle.
pub fn export(args: &PresetsFileArgs, ctx: &mut Ctx) -> i32 {
    let library = Library::load(&store(ctx));
    let path = resolve(&ctx.cwd, &args.file);
    let mut json = match bundle::export(&library) {
        Ok(json) => json,
        Err(e) => return fail(ctx, &format!("cannot export the presets: {e}")),
    };
    json.push('\n');
    if let Err(e) = fs::write(&path, json) {
        return fail(ctx, &format!("cannot write {}: {e}", path.display()));
    }
    let count = library.presets.len() + library.profiles.len();
    let _ = writeln!(ctx.out, "Exported {count} items to {}", path.display());
    0
}

/// `morphring presets import <file>`: merges a bundle into the library and
/// saves it. Prints Filee's `general.imported` text.
pub fn import(args: &PresetsFileArgs, ctx: &mut Ctx) -> i32 {
    let store = store(ctx);
    if store.dir.is_none() {
        return no_config_folder(ctx);
    }
    let path = resolve(&ctx.cwd, &args.file);
    let json = match fs::read_to_string(&path) {
        Ok(json) => json,
        Err(e) => return fail(ctx, &format!("cannot read {}: {e}", path.display())),
    };
    let mut library = Library::load(&store);
    // bundle::import parses everything before touching the library, so an
    // error here leaves both memory and disk alone. Atomicity, the cheap kind.
    let report = match bundle::import(&mut library, &json) {
        Ok(report) => report,
        Err(e) => return fail(ctx, &format!("cannot import {}: {e}", path.display())),
    };
    if let Err(e) = library.save(&store) {
        return fail(ctx, &format!("cannot save the presets: {e}"));
    }
    let _ = writeln!(ctx.out, "Imported {} items", report.total());
    0
}

/// `morphring presets reset [--yes]`: back to the factory library. Asks first
/// on a terminal; refuses (exit 2) when nobody is there to ask and `--yes`
/// was not given, because a script that resets by accident deserves a no.
pub fn reset(args: &PresetsResetArgs, ctx: &mut Ctx) -> i32 {
    let store = store(ctx);
    if store.dir.is_none() {
        return no_config_folder(ctx);
    }
    if !args.yes {
        let stdin = io::stdin();
        if !stdin.is_terminal() {
            let _ = writeln!(
                ctx.err,
                "morphring: 'presets reset' needs --yes when not run from a terminal."
            );
            return EXIT_USAGE;
        }
        if !confirm(&mut stdin.lock(), ctx.err) {
            let _ = writeln!(ctx.err, "morphring: reset cancelled; nothing changed.");
            return EXIT_FAILED;
        }
    }
    if let Err(e) = bundle::reset(&store) {
        return fail(ctx, &format!("cannot save the presets: {e}"));
    }
    let _ = writeln!(
        ctx.out,
        "Presets and toolbar profiles are back to the defaults."
    );
    0
}

/// Asks [`RESET_PROMPT`] on `prompt` and reads one answer from `input`.
/// Only `y` or `yes` (any case) count; silence, end of input and a cat on
/// the keyboard all mean no.
fn confirm(input: &mut dyn BufRead, prompt: &mut dyn Write) -> bool {
    let _ = write!(prompt, "{RESET_PROMPT}");
    let _ = prompt.flush();
    let mut answer = String::new();
    if input.read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn store(ctx: &Ctx) -> Store {
    Store::new(ctx.config_dir.clone())
}

/// `file` against the command's current folder, unless it is absolute.
fn resolve(cwd: &Path, file: &Path) -> PathBuf {
    if file.is_absolute() {
        file.to_path_buf()
    } else {
        cwd.join(file)
    }
}

/// Without a config folder a save would quietly write nothing, and a quiet
/// success that did nothing is the one outcome worse than an error.
fn no_config_folder(ctx: &mut Ctx) -> i32 {
    fail(ctx, "no config folder to keep the presets in.")
}

fn fail(ctx: &mut Ctx, message: &str) -> i32 {
    let _ = writeln!(ctx.err, "morphring: {message}");
    EXIT_FAILED
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(text: &str) -> (bool, String) {
        let mut prompt = Vec::new();
        let yes = confirm(&mut text.as_bytes(), &mut prompt);
        (yes, String::from_utf8(prompt).unwrap())
    }

    #[test]
    fn only_yes_confirms() {
        for text in ["y\n", "Y\n", "yes\n", " YES \r\n"] {
            assert!(answer(text).0, "{text:?}");
        }
        for text in ["", "\n", "n\n", "no\n", "yep\n", "maybe\n"] {
            assert!(!answer(text).0, "{text:?}");
        }
    }

    #[test]
    fn confirm_asks_the_filee_question() {
        assert_eq!(answer("n\n").1, RESET_PROMPT);
    }

    #[test]
    fn relative_files_follow_the_cwd() {
        let cwd = Path::new("/work");
        assert_eq!(resolve(cwd, Path::new("a.json")), cwd.join("a.json"));
        let absolute = std::env::temp_dir().join("b.json");
        assert_eq!(resolve(cwd, &absolute), absolute);
    }
}
