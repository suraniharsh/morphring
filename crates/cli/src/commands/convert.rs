//! `morphring convert`, ported from `ConvertAsync`, `RunJobAsync` and
//! `ResolvePreset` in Filee's `src/Filee.Cli/Cli.cs`.

use std::path::Path;
use std::sync::mpsc;
use std::thread;

use morphring_core::job::{self, JobEvent};
use morphring_core::output::{ConflictPolicy, OutputLocation, OutputRule};
use morphring_core::{ConvertOptions, Converter, FileResult, FileState, formats};

use crate::args::ConvertArgs;
use crate::ctx::Ctx;
use crate::{EXIT_USAGE, inputs, report};

/// Exit code when some file failed or was cancelled (Filee's `SomeFailed`).
const EXIT_SOME_FAILED: i32 = 1;

/// Exit code when the inputs held nothing to convert (Filee's `NothingToDo`).
const EXIT_NOTHING_TO_DO: i32 = 3;

/// Runs `morphring convert` and returns the exit code.
pub fn run(args: &ConvertArgs, ctx: &mut Ctx) -> i32 {
    let Some(target) = formats::lookup_target(&args.to) else {
        let _ = writeln!(
            ctx.err,
            "morphring: unknown format '{}'. Run 'morphring formats' for the list.",
            args.to
        );
        return EXIT_USAGE;
    };
    if !target.writable {
        let _ = writeln!(
            ctx.err,
            "morphring: MorphRing reads {} but can't write it.",
            target.name
        );
        return EXIT_USAGE;
    }

    // Folders only pick up files we recognise; explicitly named files go in
    // regardless, so the job can tell the user *why* it can't convert them.
    let expanded = inputs::expand(&args.inputs, args.recursive, &ctx.cwd, &|p: &Path| {
        formats::detect(p).is_some()
    });
    // Filee reports every missing input before giving up, so one typo doesn't
    // hide the next. Nothing gets converted until the list is clean.
    for missing in &expanded.missing {
        let _ = writeln!(
            ctx.err,
            "morphring: not found: {}",
            Path::new(missing).display()
        );
    }
    if !expanded.missing.is_empty() {
        return EXIT_USAGE;
    }
    if expanded.files.is_empty() {
        let _ = writeln!(ctx.err, "morphring: no files to convert.");
        return EXIT_NOTHING_TO_DO;
    }

    let rule = output_rule(args, &ctx.cwd);
    let options = ConvertOptions {
        quality: args.quality,
    };
    let engines: Vec<&dyn Converter> = ctx.engines.iter().map(|e| e.as_ref()).collect();
    let cancel = ctx.cancel.clone();

    // Under --json, stdout is reserved for the one JSON document; under
    // --quiet only the bad news makes it out.
    let wants_line = |state: &FileState| {
        !args.json
            && (!args.quiet || matches!(state, FileState::Failed { .. } | FileState::Cancelled))
    };

    // run_job blocks and fires events from its worker threads, but ctx.out is a
    // plain `&mut dyn Write` that no thread may share. So the job runs on its own
    // thread and posts finished files to us here, where the writer lives. Lines
    // come out in finishing order, like Filee, not in input order.
    let results = thread::scope(|scope| {
        let (tx, rx) = mpsc::channel::<FileResult>();
        let worker = scope.spawn(move || {
            let on_event = move |event: JobEvent| {
                if let JobEvent::FileFinished { result, .. } = event {
                    // The receiver only hangs up once we're done, so a send
                    // error here would mean nobody is listening anyway.
                    let _ = tx.send(result);
                }
            };
            job::run_job(
                &expanded.files,
                target.id,
                &options,
                &rule,
                &engines,
                &cancel,
                &on_event,
            )
        });
        for result in rx {
            if wants_line(&result.state) {
                let _ = writeln!(ctx.out, "{}", report::file_line(&result, &ctx.cwd));
            }
        }
        worker.join()
    });
    let results = match results {
        Ok(results) => results,
        Err(_) => {
            // run_job catches engine panics per file, so getting here means the
            // runner itself fell over. Say so instead of pretending all is well.
            let _ = writeln!(ctx.err, "morphring: the conversion stopped unexpectedly.");
            return EXIT_SOME_FAILED;
        }
    };

    if args.json {
        if let Err(e) = report::write_json(ctx.out, target.id.as_str(), &results, &ctx.cwd) {
            let _ = writeln!(ctx.err, "morphring: cannot write the report: {e}");
            return EXIT_SOME_FAILED;
        }
    } else {
        let _ = writeln!(ctx.out, "{}", report::summary(&results));
    }

    if report::Counts::of(&results).failed == 0 {
        0
    } else {
        EXIT_SOME_FAILED
    }
}

/// Where outputs go and what happens when a name is taken. `-o` is flat: every
/// output lands straight in that folder, like Filee's `UseFolder`.
fn output_rule(args: &ConvertArgs, cwd: &Path) -> OutputRule {
    let location = match &args.output {
        // join keeps an absolute -o as is and anchors a relative one at cwd,
        // never at whatever folder the process happens to be sitting in.
        Some(dir) => OutputLocation::CustomFolder {
            path: cwd.join(dir),
            keep_folders_from: None,
        },
        None => OutputLocation::SameFolder,
    };
    let conflict = if args.overwrite {
        ConflictPolicy::Overwrite
    } else if args.skip {
        ConflictPolicy::Skip
    } else {
        ConflictPolicy::Rename
    };
    OutputRule {
        location,
        conflict,
        ..OutputRule::default()
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::*;

    fn args(extra: &[&str]) -> ConvertArgs {
        ConvertArgs {
            inputs: vec![OsString::from("a.png")],
            to: "jpg".into(),
            output: extra
                .iter()
                .position(|a| *a == "-o")
                .map(|i| PathBuf::from(extra[i + 1])),
            recursive: false,
            quality: None,
            overwrite: extra.contains(&"--overwrite"),
            skip: extra.contains(&"--skip"),
            json: false,
            quiet: false,
        }
    }

    #[test]
    fn default_rule_is_same_folder_with_rename() {
        let rule = output_rule(&args(&[]), Path::new("/work"));
        assert_eq!(rule.location, OutputLocation::SameFolder);
        assert_eq!(rule.conflict, ConflictPolicy::Rename);
        assert_eq!(rule.pattern, OutputRule::default().pattern);
    }

    #[test]
    fn output_folder_is_flat_and_anchored_at_cwd() {
        let rule = output_rule(&args(&["-o", "out"]), Path::new("/work"));
        assert_eq!(
            rule.location,
            OutputLocation::CustomFolder {
                path: PathBuf::from("/work/out"),
                keep_folders_from: None
            }
        );
        let rule = output_rule(&args(&["-o", "/abs"]), Path::new("/work"));
        assert_eq!(
            rule.location,
            OutputLocation::CustomFolder {
                path: PathBuf::from("/abs"),
                keep_folders_from: None
            }
        );
    }

    #[test]
    fn policy_flags_pick_the_conflict_policy() {
        let cwd = Path::new("/work");
        assert_eq!(
            output_rule(&args(&["--overwrite"]), cwd).conflict,
            ConflictPolicy::Overwrite
        );
        assert_eq!(
            output_rule(&args(&["--skip"]), cwd).conflict,
            ConflictPolicy::Skip
        );
    }
}
