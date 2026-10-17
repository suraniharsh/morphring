//! `morphring watch`: converts the files that arrive in a folder, ported
//! from `WatchAsync` in Filee's `src/Filee.Cli/Cli.cs`
//! (docs/reverse-engineering/07-cli-tests-docs.md section 5.3).
//!
//! The division of labour: `notify` only tells us *which* paths moved, the
//! pure [`Settler`] in core decides *when* they are done moving, and this file
//! owns the clock, the channel and the printing. Filee did the same with a
//! FileSystemWatcher and a 1 s timer; we just have fewer events to apologise
//! for.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

use morphring_core::job::{self, JobEvent};
use morphring_core::output::OutputLocation;
use morphring_core::preset;
use morphring_core::profiles::Library;
use morphring_core::settings::Store;
use morphring_core::watch::{self, AfterConversion, Settler, Stat, WatchRule};
use morphring_core::{CancelToken, Converter, FileResult, FileState, formats};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use walkdir::WalkDir;

use crate::EXIT_USAGE;
use crate::args::WatchArgs;
use crate::ctx::Ctx;
use crate::report;
use crate::resolve::{self, JobSpec};

/// How often the settler is asked what is ready, like Filee's 1 s timer.
const TICK: Duration = Duration::from_secs(1);

/// The longest we sit in `recv_timeout` before peeking at the cancel token.
/// Ctrl+C should feel instant, not "within the next second, probably".
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// How many handled files the settler remembers (the LRU cap).
const HANDLED_CAP: usize = 10_000;

/// Runs `morphring watch` and returns the exit code.
pub fn run(args: &WatchArgs, ctx: &mut Ctx) -> i32 {
    let store = Store::new(ctx.config_dir.clone());
    let library = Library::load(&store);
    let settings = store.load_settings();
    // -o is handled below through the watch rule, so it goes in as None: the
    // location gets replaced wholesale anyway.
    let spec = match resolve::job_from_args(
        args.to.as_deref(),
        args.preset.as_deref(),
        args.quality,
        args.overwrite,
        args.skip,
        args.keep_dates,
        None,
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

    let rule = WatchRule {
        folder: normalize(&ctx.cwd.join(&args.folder)),
        include_subfolders: args.recursive,
        output_folder: args.output.as_ref().map(|o| normalize(&ctx.cwd.join(o))),
        originals: if args.move_originals {
            AfterConversion::MoveToOriginals
        } else {
            AfterConversion::Keep
        },
        ..WatchRule::default()
    };
    if !rule.folder.is_dir() {
        let _ = writeln!(
            ctx.err,
            "morphring: Folder not found: {}",
            rule.folder.display()
        );
        return EXIT_USAGE;
    }

    let JobSpec {
        target,
        options,
        rule: mut output_rule,
        label,
    } = spec;
    // Inbox/2025/a.jpg lands in converted/2025/a.pdf, whatever the preset or
    // the settings thought about output folders.
    output_rule.location = OutputLocation::CustomFolder {
        path: rule.resolved_output_folder(),
        keep_folders_from: Some(rule.folder.clone()),
    };

    // FSEvents reports real paths, so on macOS a watch on /var/folders/...
    // hears about /private/var/folders/... and nothing would ever match the
    // rule. We remember the canonical spelling and translate back.
    let canonical = fs::canonicalize(&rule.folder).unwrap_or_else(|_| rule.folder.clone());

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = match RecommendedWatcher::new(
        move |event| {
            // The receiver only hangs up when we are shutting down, at which
            // point nobody cares about one more event.
            let _ = tx.send(event);
        },
        notify::Config::default(),
    ) {
        Ok(watcher) => watcher,
        Err(e) => {
            let _ = writeln!(ctx.err, "morphring: {}", watch_error(&rule.folder, &e));
            return EXIT_USAGE;
        }
    };
    let mode = if rule.include_subfolders {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    if let Err(e) = watcher.watch(&rule.folder, mode) {
        let _ = writeln!(ctx.err, "morphring: {}", watch_error(&rule.folder, &e));
        return EXIT_USAGE;
    }

    if !args.quiet {
        let name = preset_name(args, &library).unwrap_or_else(|| label.clone());
        let _ = writeln!(
            ctx.out,
            "Watching {} -> {name}, saving to {}. Press Ctrl+C to stop.",
            rule.folder.display(),
            rule.resolved_output_folder().display()
        );
        let _ = ctx.out.flush();
    }

    let settle = args
        .settle
        .map_or_else(watch::default_settle, Duration::from_secs_f64);
    let mut settler = Settler::new(settle, HANDLED_CAP);
    // Keep leaves what was already there alone; an inbox that moves its
    // originals away has no such qualms and takes everything.
    if rule.originals == AfterConversion::MoveToOriginals {
        settler.rescan(listing(&rule), Instant::now());
    }

    let engines: Vec<&dyn Converter> = ctx.engines.iter().map(|e| e.as_ref()).collect();
    // Ctrl+C stops the watching, not the batch in flight: that one finishes
    // and moves its originals, like Filee's Dispose. A second Ctrl+C is
    // main.rs's business and ends everything at once.
    let batch_cancel = CancelToken::new();
    let mut told_about_limit = false;
    let mut next_tick = Instant::now() + TICK;

    while !ctx.cancel.is_cancelled() {
        let wait = next_tick
            .saturating_duration_since(Instant::now())
            .min(CANCEL_POLL);
        match rx.recv_timeout(wait) {
            Ok(Ok(event)) => {
                if event.need_rescan() {
                    // The kernel dropped events on the floor; Filee's answer
                    // to an overflow is to look at everything again.
                    settler.rescan(listing(&rule), Instant::now());
                } else if wants_kind(&event.kind) {
                    for path in &event.paths {
                        let path = rebase(path, &canonical, &rule.folder);
                        notice(&mut settler, &rule, path);
                    }
                }
            }
            Ok(Err(e)) => {
                if matches!(e.kind, notify::ErrorKind::MaxFilesWatch) {
                    // A new subfolder under -r can hit the limit mid-watch.
                    // Say it once; repeating it per folder helps nobody.
                    if !told_about_limit {
                        let _ = writeln!(ctx.err, "morphring: {}", watch_error(&rule.folder, &e));
                        told_about_limit = true;
                    }
                } else {
                    tracing::warn!("watch error: {e}");
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            // The watcher lives until the end of this function, so this
            // cannot happen; if it somehow does, there is nothing left to hear.
            Err(RecvTimeoutError::Disconnected) => break,
        }

        let now = Instant::now();
        if now < next_tick {
            continue;
        }
        next_tick = now + TICK;
        let ready = settler.tick(now, stat);
        if ready.is_empty() {
            continue;
        }
        let results = thread::scope(|scope| {
            let (tx, rx) = mpsc::channel::<FileResult>();
            let (engines, output_rule, options, ready, cancel) =
                (&engines, &output_rule, &options, &ready, &batch_cancel);
            let worker = scope.spawn(move || {
                let on_event = move |event: JobEvent| {
                    if let JobEvent::FileFinished { result, .. } = event {
                        let _ = tx.send(result);
                    }
                };
                job::run_job(
                    ready,
                    target,
                    options,
                    output_rule,
                    engines,
                    cancel,
                    &on_event,
                )
            });
            // Same lines as convert, as each file finishes. --json is ignored
            // here: a report that never ends is not a JSON document.
            for result in rx {
                let bad = matches!(
                    result.state,
                    FileState::Failed { .. } | FileState::Cancelled
                );
                if !args.quiet || bad {
                    let _ = writeln!(ctx.out, "{}", report::file_line(&result, &ctx.cwd));
                    let _ = ctx.out.flush();
                }
            }
            worker.join()
        });
        let Ok(results) = results else {
            // run_job catches engine panics per file; this is the runner
            // itself falling over. The watch goes on, the next file may fare
            // better.
            let _ = writeln!(ctx.err, "morphring: a conversion stopped unexpectedly.");
            continue;
        };
        if rule.originals == AfterConversion::MoveToOriginals {
            move_originals(&rule, &results, ctx.err);
        }
    }

    // Failed files do not change the exit code: the user asked us to stop,
    // and stopping is what went well.
    drop(watcher);
    0
}

/// The display name of the preset the job runs, for the "Watching" line.
fn preset_name(args: &WatchArgs, library: &Library) -> Option<String> {
    match (&args.to, &args.preset) {
        (Some(to), _) => formats::lookup_target(to).map(|f| preset::for_target(f).display_name()),
        (None, Some(key)) => {
            preset::find_conversion(&library.presets, key).map(preset::Preset::display_name)
        }
        (None, None) => None,
    }
}

/// True for the event kinds that may mean "a file is new or growing".
/// Removals and reads tell us nothing a stat at tick time won't.
fn wants_kind(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any | EventKind::Other
    )
}

/// Hands `path` to the settler unless the rule ignores it.
fn notice(settler: &mut Settler, rule: &WatchRule, path: PathBuf) {
    let meta = fs::metadata(&path).ok();
    let is_dir = meta.as_ref().is_some_and(fs::Metadata::is_dir);
    if watch::should_ignore(rule, &path, is_dir) {
        return;
    }
    let stat = meta.filter(fs::Metadata::is_file).map(|m| stat_of(&m));
    settler.notice(path, stat, Instant::now());
}

/// Every file in the folder the rule would convert: the folder itself, or
/// the whole tree with `-r`.
fn listing(rule: &WatchRule) -> Vec<PathBuf> {
    let depth = if rule.include_subfolders {
        usize::MAX
    } else {
        1
    };
    WalkDir::new(&rule.folder)
        .max_depth(depth)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|path| !watch::should_ignore(rule, path, false))
        .collect()
}

/// Size and mtime of a file that is still there, for the settler.
fn stat(path: &Path) -> Option<Stat> {
    fs::metadata(path)
        .ok()
        .filter(fs::Metadata::is_file)
        .map(|m| stat_of(&m))
}

fn stat_of(meta: &fs::Metadata) -> Stat {
    // A file system without mtimes still has sizes; the epoch is as good a
    // "never changes" as any.
    (meta.len(), meta.modified().unwrap_or(UNIX_EPOCH))
}

/// Moves each converted source into `<folder>/originals`, keeping its
/// subfolders and numbering it on a clash.
fn move_originals(rule: &WatchRule, results: &[FileResult], err: &mut dyn std::io::Write) {
    for result in results.iter().filter(|r| r.state == FileState::Done) {
        let Some(target) = watch::originals_target(rule, &result.source, Path::exists) else {
            continue;
        };
        let moved = target
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::rename(&result.source, &target));
        if let Err(e) = moved {
            let _ = writeln!(
                err,
                "morphring: cannot move {} to {}: {e}",
                result.source.display(),
                target.display()
            );
        }
    }
}

/// `path` spelled under `folder` when it came in under `canonical` (the
/// macOS /private/var detour); anything else is returned as is.
fn rebase(path: &Path, canonical: &Path, folder: &Path) -> PathBuf {
    match path.strip_prefix(canonical) {
        Ok(rest) if canonical != folder => folder.join(rest),
        _ => path.to_path_buf(),
    }
}

/// Removes `.` and folds `..` without touching the disk, so the folder we
/// print and match against is the one the user meant, minus the dots.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    out.push(component);
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// One sentence about why the folder can't be watched. The inotify limit gets
/// its own, because "No space left on device" for a folder of three files is
/// the kind of message that ruins an afternoon.
fn watch_error(folder: &Path, e: &notify::Error) -> String {
    let limit = match &e.kind {
        notify::ErrorKind::MaxFilesWatch => true,
        // Some notify backends hand ENOSPC through as a plain io error.
        notify::ErrorKind::Io(io) => io.raw_os_error() == Some(28) && cfg!(target_os = "linux"),
        _ => false,
    };
    if limit {
        format!(
            "cannot watch {}: the system limit on watched folders was reached. \
             Raise it with 'sysctl fs.inotify.max_user_watches=524288' or watch fewer subfolders.",
            folder.display()
        )
    } else if let notify::ErrorKind::PathNotFound = e.kind {
        format!("Folder not found: {}", folder.display())
    } else {
        let detail = match &e.kind {
            notify::ErrorKind::Io(io) => io.to_string(),
            _ => e.to_string(),
        };
        format!("cannot watch {}: {detail}", folder.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_drops_dots_and_folds_parents() {
        let base = std::env::temp_dir();
        assert_eq!(normalize(&base.join(".")), normalize(&base));
        assert_eq!(
            normalize(&base.join("a").join("..").join("b")),
            base.join("b")
        );
    }

    #[test]
    fn rebase_translates_canonical_paths_only() {
        let folder = Path::new("/var/inbox");
        let canonical = Path::new("/private/var/inbox");
        assert_eq!(
            rebase(&canonical.join("a.png"), canonical, folder),
            folder.join("a.png")
        );
        assert_eq!(
            rebase(Path::new("/elsewhere/a.png"), canonical, folder),
            PathBuf::from("/elsewhere/a.png")
        );
        assert_eq!(
            rebase(&folder.join("a.png"), folder, folder),
            folder.join("a.png")
        );
    }

    #[test]
    fn the_watch_limit_gets_a_helpful_message() {
        let e = notify::Error::new(notify::ErrorKind::MaxFilesWatch);
        let message = watch_error(Path::new("/inbox"), &e);
        assert!(message.contains("max_user_watches"), "{message}");
    }

    #[test]
    fn plain_io_errors_keep_their_detail() {
        let e = notify::Error::io(std::io::Error::other("disk on fire"));
        let message = watch_error(Path::new("/inbox"), &e);
        assert!(message.contains("disk on fire"), "{message}");
    }
}
