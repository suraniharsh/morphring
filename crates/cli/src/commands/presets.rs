//! `morphring presets`: lists the presets, ported from `ListPresets` in
//! Filee's `src/Filee.Cli/Cli.cs`.
//!
//! One line per preset in the user's `presets.json` order, so the list reads
//! the same way the ring does. Groups are listed too, with the target
//! `group`, because they are on the ring and a list that hides them would
//! leave people wondering where "Rotate" went. `--json` also names what each
//! group holds. Nothing here writes to disk, unless you count the `.broken`
//! copy `Library::load` leaves behind for a mangled file.

use std::fmt::Write as _;

use morphring_core::preset::{self, Preset};
use morphring_core::profiles::Library;
use morphring_core::settings::Store;
use serde::Serialize;

use crate::ctx::Ctx;

/// Exit code when the JSON could not be written (same as convert's).
const EXIT_WRITE_FAILED: i32 = 1;

/// Runs `morphring presets` and returns the exit code.
pub fn run(json: bool, ctx: &mut Ctx) -> i32 {
    let library = Library::load(&Store::new(ctx.config_dir.clone()));

    if json {
        let rows: Vec<Row> = library
            .presets
            .iter()
            .map(|p| Row::of(p, &library))
            .collect();
        let mut text = match serde_json::to_string_pretty(&rows) {
            Ok(text) => text,
            Err(e) => {
                let _ = writeln!(ctx.err, "morphring: cannot write the list: {e}");
                return EXIT_WRITE_FAILED;
            }
        };
        text.push('\n');
        let _ = ctx.out.write_all(text.as_bytes());
        return 0;
    }

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

/// One preset as `presets --json` prints it. `members` is there for groups
/// only: a conversion holding an empty list would look like a sad group.
#[derive(Serialize)]
struct Row<'a> {
    id: &'a str,
    name: String,
    target: &'a str,
    #[serde(rename = "isGroup")]
    is_group: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    members: Option<Vec<&'a str>>,
}

impl<'a> Row<'a> {
    fn of(preset: &'a Preset, library: &'a Library) -> Self {
        Self {
            id: &preset.id,
            name: preset.display_name(),
            target: &preset.target,
            is_group: preset.is_group(),
            members: preset
                .is_group()
                .then(|| member_ids(preset, library).collect()),
        }
    }
}

/// The ids of the conversions `group` holds, in ring order. Like Filee, each
/// member goes through the conversion lookup, so a member id that has since
/// gone missing (or turned into a group) quietly drops out.
fn member_ids<'a>(group: &'a Preset, library: &'a Library) -> impl Iterator<Item = &'a str> {
    group
        .sub_preset_ids
        .iter()
        .filter_map(|id| preset::find_conversion(&library.presets, id))
        .map(|p| p.id.as_str())
}

/// The usage error for `--preset <key>` when `key` names a group (Filee's
/// `ResolvePreset`): a group converts nothing, so the message names what it
/// holds, or says that it holds nothing at all any more.
pub fn group_refusal(key: &str, group: &Preset, library: &Library) -> String {
    let members: Vec<&str> = member_ids(group, library).collect();
    if members.is_empty() {
        format!(
            "'{key}' is a group on the donut and has no presets left. \
             Run 'morphring presets' for the list."
        )
    } else {
        format!(
            "'{key}' is a group on the donut, not a conversion. Use one of: {}",
            members.join(", ")
        )
    }
}
