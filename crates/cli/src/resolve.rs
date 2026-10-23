//! Turns the `--to`/`--preset` arguments of `convert` and `watch` into a job,
//! ported from `ResolvePreset` in Filee's `src/Filee.Cli/Cli.cs`
//! (docs/reverse-engineering/07-cli-tests-docs.md section 6).
//!
//! The recipe: find the preset (or invent one for `--to`), clone it so the
//! saved ones stay pristine, then pile the overrides on in Filee's order. The
//! order matters more than it looks: `-o` comes last so it beats both the
//! preset and the settings, which is what anyone typing `-o` expects.

use std::fmt;
use std::path::Path;

use morphring_core::formats::Format;
use morphring_core::output::{ConflictPolicy, OutputLocation, OutputRule};
use morphring_core::planner::Planner;
use morphring_core::preset::{self, Preset};
use morphring_core::profiles::Library;
use morphring_core::settings::AppSettings;
use morphring_core::{ConvertOptions, Converter, Target, formats};

use crate::commands::presets;

/// Everything `run_job` needs besides the files and the engines.
#[derive(Clone, Debug, PartialEq)]
pub struct JobSpec {
    /// What the files become.
    pub target: Target,
    /// The preset's options with `-q` applied.
    pub options: ConvertOptions,
    /// Where outputs go, after the settings and the flags had their say.
    pub rule: OutputRule,
    /// The preset's target as stored (`*` for same-as-source presets), for
    /// the `target` field of the JSON report.
    pub label: String,
}

/// A problem with the arguments. The message comes without the
/// `morphring: ` prefix; the caller prints it and exits 2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageError(pub String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

/// Builds the job for `--to <to>` or `--preset <preset>` (exactly one should
/// be set) plus the override flags.
///
/// Overrides go on in Filee's order: the settings' default output (and its
/// keep-dates switch), `quality`, `overwrite`/`skip`, `keep_dates`, then
/// `output`, which sends every file flat into that folder (relative to
/// `cwd`).
///
/// `engines` are every registered engine, installed or not: a target none of
/// them writes is refused here, before a single file is touched.
// Eleven arguments is a lot, but they are exactly the flags convert and watch
// share, and a struct holding them would just be ConvertArgs wearing a hat.
#[allow(clippy::too_many_arguments)]
pub fn job_from_args(
    to: Option<&str>,
    preset: Option<&str>,
    quality: Option<u8>,
    overwrite: bool,
    skip: bool,
    keep_dates: bool,
    output: Option<&Path>,
    cwd: &Path,
    library: &Library,
    settings: &AppSettings,
    engines: &[Box<dyn Converter>],
) -> Result<JobSpec, UsageError> {
    // The dream planner: every engine counts, installed or not. A missing
    // engine gets its own friendly "needs LibreOffice" later on; a format
    // with no engine at all should not get as far as a job.
    let engines: Vec<&dyn Converter> = engines.iter().map(Box::as_ref).collect();
    let planner = Planner::new(&engines, true);
    let mut chosen = choose_preset(to, preset, library, &planner)?;
    let target = job_target(&chosen, &planner)?;

    // 1. The settings fill in a "Default" location and may force keep-dates.
    chosen.output = settings.with_default_output(&chosen.output);

    // 2. -q. Filee sets it whatever the target; an engine that has no use for
    // quality simply ignores it, so no harm done.
    let mut options = chosen.options();
    if let Some(q) = quality {
        options.quality = Some(q);
        options.image.quality = q;
    }

    let mut rule = chosen.output;
    // 3. The conflict policy. clap already made sure we don't get both.
    if overwrite {
        rule.conflict = ConflictPolicy::Overwrite;
    } else if skip {
        rule.conflict = ConflictPolicy::Skip;
    }
    // 4. --keep-dates can only turn dates on, never off.
    rule.keep_dates |= keep_dates;
    // 5. -o: flat into one folder. join keeps an absolute -o as is and anchors
    // a relative one at cwd, never at whatever folder the process sits in.
    if let Some(dir) = output {
        rule.location = OutputLocation::CustomFolder {
            path: cwd.join(dir),
            keep_folders_from: None,
        };
    }

    Ok(JobSpec {
        target,
        options,
        rule,
        label: chosen.target,
    })
}

/// A fresh copy of the preset to run: the one `--preset` names, or a default
/// one for `--to`.
fn choose_preset(
    to: Option<&str>,
    preset: Option<&str>,
    library: &Library,
    planner: &Planner,
) -> Result<Preset, UsageError> {
    match (to, preset) {
        (Some(to), None) => {
            let Some(format) = formats::lookup_target(to) else {
                return Err(UsageError(format!(
                    "unknown format '{to}'. Run 'morphring formats' for the list."
                )));
            };
            refuse_unwritable(format, planner)?;
            Ok(preset::for_target(format))
        }
        (None, Some(key)) => {
            // Conversions first, so a "Rotate" group can't hide your own
            // "Rotate" preset; a group is only found to say what it holds.
            if let Some(found) = preset::find_conversion(&library.presets, key) {
                // Same rule as the ring's greyed slice: better a clear "not
                // yet" than an H.264 file wearing a `_hevc` name tag.
                if let Some(reason) = found.unsupported_mode() {
                    return Err(UsageError(format!(
                        "Preset '{key}' is not supported yet ({reason})."
                    )));
                }
                return Ok(found.clone());
            }
            match preset::find(&library.presets, key) {
                Some(group) => Err(UsageError(presets::group_refusal(key, group, library))),
                None => Err(UsageError(format!(
                    "no preset '{key}'. Run 'morphring presets' for the list."
                ))),
            }
        }
        // lib.rs checks this before we get here; saying it again costs nothing
        // and keeps this function honest when someone calls it directly.
        _ => Err(UsageError(
            "Use either --to <format> or --preset <preset>.".to_owned(),
        )),
    }
}

/// The job target of a preset, refusing targets we can't produce. Only a
/// hand-edited presets.json can get here, but hand-editing is a hobby.
fn job_target(chosen: &Preset, planner: &Planner) -> Result<Target, UsageError> {
    match chosen.job_target() {
        Some(Target::Format(id)) => match formats::find_by_id(id.as_str()) {
            Some(format) => {
                refuse_unwritable(format, planner)?;
                Ok(Target::Format(id))
            }
            None => Ok(Target::Format(id)),
        },
        Some(target) => Ok(target),
        None => Err(UsageError(format!(
            "preset '{}' converts to '{}', which is not a known format.",
            chosen.display_name(),
            chosen.target
        ))),
    }
}

/// Refuses `format` as a target unless the catalog calls it writable *and*
/// some engine actually writes it. The catalog came from Filee, who had a few
/// more engines than we do, so its word alone used to send `--to aac` all the
/// way to the job runner just to hear "No engine can do this conversion".
fn refuse_unwritable(format: &Format, planner: &Planner) -> Result<(), UsageError> {
    if format.writable && planner.writable_targets().contains(&format.id) {
        Ok(())
    } else if planner.readable_sources().contains(&format.id) {
        Err(cannot_write(format.name))
    } else {
        Err(UsageError(format!(
            "MorphRing can't write {} yet.",
            format.name
        )))
    }
}

fn cannot_write(name: &str) -> UsageError {
    UsageError(format!("MorphRing reads {name} but can't write it."))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use morphring_core::FormatId;
    use morphring_core::options::Resize;
    use morphring_core::settings::{DefaultLocation, DefaultOutput};

    use super::*;

    fn resolve(to: Option<&str>, preset: Option<&str>) -> Result<JobSpec, UsageError> {
        job_from_args(
            to,
            preset,
            None,
            false,
            false,
            false,
            None,
            Path::new("/work"),
            &Library::builtin(),
            &AppSettings::default(),
            &morphring_engines::registry::all(),
        )
    }

    fn fmt(id: &str) -> Target {
        Target::Format(formats::find_by_id(id).unwrap().id)
    }

    #[test]
    fn to_gives_a_plain_job_next_to_the_source() {
        let spec = resolve(Some(".JPEG"), None).unwrap();
        assert_eq!(spec.target, fmt("jpg"));
        assert_eq!(spec.label, "jpg");
        assert_eq!(spec.options.quality, None);
        assert_eq!(spec.rule, OutputRule::default());
    }

    #[test]
    fn to_keeps_the_v01_messages() {
        assert_eq!(
            resolve(Some("nope"), None).unwrap_err().0,
            "unknown format 'nope'. Run 'morphring formats' for the list."
        );
        let err = resolve(Some("xcf"), None).unwrap_err().0;
        assert!(err.starts_with("MorphRing reads ") && err.ends_with(" but can't write it."));
    }

    #[test]
    fn to_refuses_what_no_engine_writes() {
        // FX-08: the catalog called these writable, the engines disagreed,
        // and the user found out one "No engine can do this conversion" later.
        for (to, name) in [
            ("aac", "AAC"),
            ("tsv", "TSV"),
            (".m4v", "M4V"),
            ("docm", "DOCM"),
        ] {
            assert_eq!(
                resolve(Some(to), None).unwrap_err().0,
                format!("MorphRing reads {name} but can't write it."),
                "{to}"
            );
        }
        // Nothing reads a 7z either, so "reads" would be a fib.
        assert_eq!(
            resolve(Some("7z"), None).unwrap_err().0,
            "MorphRing can't write 7Z yet."
        );
        // Without any engine, nothing is writable, not even good old PNG.
        let err = job_from_args(
            Some("png"),
            None,
            None,
            false,
            false,
            false,
            None,
            Path::new("/"),
            &Library::builtin(),
            &AppSettings::default(),
            &[],
        )
        .unwrap_err();
        assert_eq!(err.0, "MorphRing can't write PNG yet.");
    }

    #[test]
    fn a_preset_whose_target_no_engine_writes_is_refused() {
        let mut library = Library::builtin();
        library.presets.push(Preset {
            id: "to-aac".into(),
            target: "aac".into(),
            ..Preset::default()
        });
        let err = job_from_args(
            None,
            Some("to-aac"),
            None,
            false,
            false,
            false,
            None,
            Path::new("/"),
            &library,
            &AppSettings::default(),
            &morphring_engines::registry::all(),
        )
        .unwrap_err();
        assert_eq!(err.0, "MorphRing reads AAC but can't write it.");
    }

    #[test]
    fn preset_is_found_by_id_or_name() {
        let spec = resolve(None, Some("JPG 70%")).unwrap();
        assert_eq!(spec.target, fmt("jpg"));
        assert_eq!(spec.options.quality, Some(70));
        assert_eq!(spec.rule.pattern, "{name}_70");
        assert_eq!(spec.rule.location, OutputLocation::SameFolder);

        let spec = resolve(None, Some("half-size")).unwrap();
        assert_eq!(spec.target, Target::SameAsSource);
        assert_eq!(spec.label, "*");
        assert_eq!(spec.options.image.resize, Resize::Percent);
        assert_eq!(spec.options.image.resize_percent, 50);
    }

    #[test]
    fn unknown_preset_points_at_the_list() {
        assert_eq!(
            resolve(None, Some("nope")).unwrap_err().0,
            "no preset 'nope'. Run 'morphring presets' for the list."
        );
    }

    #[test]
    fn neither_or_both_is_an_error() {
        assert!(resolve(None, None).is_err());
        assert!(resolve(Some("jpg"), Some("to-png")).is_err());
    }

    #[test]
    fn a_preset_with_a_bogus_target_is_refused() {
        let mut library = Library::builtin();
        library.presets.push(Preset {
            id: "weird".into(),
            target: "floppy".into(),
            ..Preset::default()
        });
        let err = job_from_args(
            None,
            Some("weird"),
            None,
            false,
            false,
            false,
            None,
            Path::new("/"),
            &library,
            &AppSettings::default(),
            &morphring_engines::registry::all(),
        )
        .unwrap_err();
        assert!(err.0.contains("'floppy'"), "{err}");
    }

    #[test]
    fn overrides_apply_in_filee_order() {
        let settings = AppSettings {
            keep_file_dates: false,
            default_output: DefaultOutput {
                location: DefaultLocation::Subfolder,
                subfolder_name: "done".into(),
                custom_folder: String::new(),
            },
            ..AppSettings::default()
        };
        let library = Library::builtin();
        let run = |q, overwrite, skip, keep, out: Option<&Path>| {
            job_from_args(
                None,
                Some("jpg-70"),
                q,
                overwrite,
                skip,
                keep,
                out,
                Path::new("/work"),
                &library,
                &settings,
                &morphring_engines::registry::all(),
            )
            .unwrap()
        };

        let spec = run(None, false, false, false, None);
        assert_eq!(spec.rule.location, OutputLocation::Subfolder("done".into()));
        assert_eq!(spec.rule.conflict, ConflictPolicy::Rename);
        assert!(!spec.rule.keep_dates);

        let spec = run(Some(33), true, false, true, Some(Path::new("out")));
        assert_eq!(spec.options.quality, Some(33));
        assert_eq!(spec.options.image.quality, 33);
        assert_eq!(spec.rule.conflict, ConflictPolicy::Overwrite);
        assert!(spec.rule.keep_dates);
        // -o wins over the settings, and the pattern survives it.
        assert_eq!(
            spec.rule.location,
            OutputLocation::CustomFolder {
                path: PathBuf::from("/work/out"),
                keep_folders_from: None
            }
        );
        assert_eq!(spec.rule.pattern, "{name}_70");

        assert_eq!(
            run(None, false, true, false, None).rule.conflict,
            ConflictPolicy::Skip
        );
    }

    #[test]
    fn settings_keep_file_dates_turns_dates_on() {
        let settings = AppSettings {
            keep_file_dates: true,
            ..AppSettings::default()
        };
        let spec = job_from_args(
            Some("png"),
            None,
            None,
            false,
            false,
            false,
            None,
            Path::new("/"),
            &Library::builtin(),
            &settings,
            &morphring_engines::registry::all(),
        )
        .unwrap();
        assert!(spec.rule.keep_dates);
        assert_eq!(spec.target, Target::Format(FormatId::new("png")));
    }
}
