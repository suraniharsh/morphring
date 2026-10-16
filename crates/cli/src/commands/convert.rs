//! `morphring convert`, ported from `ConvertAsync`, `RunJobAsync` and
//! `ResolvePreset` in Filee's `src/Filee.Cli/Cli.cs`. Turning the flags into a
//! job lives in [`crate::resolve`]; this file runs it and reports.

use std::path::Path;
use std::sync::mpsc;
use std::thread;

use morphring_core::job::{self, JobEvent};
use morphring_core::profiles::Library;
use morphring_core::settings::Store;
use morphring_core::{Converter, FileResult, FileState, formats};

use crate::args::ConvertArgs;
use crate::ctx::Ctx;
use crate::resolve::{self, JobSpec};
use crate::{EXIT_USAGE, inputs, report};

/// Exit code when some file failed or was cancelled (Filee's `SomeFailed`).
const EXIT_SOME_FAILED: i32 = 1;

/// Exit code when the inputs held nothing to convert (Filee's `NothingToDo`).
const EXIT_NOTHING_TO_DO: i32 = 3;

/// Runs `morphring convert` and returns the exit code.
pub fn run(args: &ConvertArgs, ctx: &mut Ctx) -> i32 {
    // Settings and presets come from the same folder the app uses, so a
    // preset made in the ring is one `--preset` away. No folder, no problem:
    // the built-ins and factory settings step in.
    let store = Store::new(ctx.config_dir.clone());
    let library = Library::load(&store);
    let settings = store.load_settings();
    let spec = match resolve::job_from_args(
        args.to.as_deref(),
        args.preset.as_deref(),
        args.quality,
        args.overwrite,
        args.skip,
        args.keep_dates,
        args.output.as_deref(),
        &ctx.cwd,
        &library,
        &settings,
    ) {
        Ok(spec) => spec,
        Err(e) => {
            let _ = writeln!(ctx.err, "morphring: {e}");
            return EXIT_USAGE;
        }
    };

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

    let JobSpec {
        target,
        options,
        rule,
        label,
    } = spec;
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
                target,
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
        if let Err(e) = report::write_json(ctx.out, &label, &results, &ctx.cwd) {
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
