# MorphRing Architecture

The code as it stands on `main` during v0.2 (v0.1.0 CLI and v0.1.1 ring app released; the v0.2 tasks of PLAN-v0.2.md have landed, the Filee parity tasks of PLAN-v0.2-parity.md are in progress). PLAN.md and `docs/PLAN-v0.2.md` say where the code is going and ENGINEERING.md sets the rules. This file describes what exists today. If this file and the code disagree, the code is right, so fix this file.

Paths in backticks are relative to the repo root. Module tables name Rust modules (`job::merge` is `crates/core/src/job/merge.rs`).

## 1. Workspaces and crates

There are two Cargo workspaces (PLAN.md D1):

| Workspace | Manifest | Members |
|---|---|---|
| Root | `Cargo.toml` (resolver 3, edition 2024, `rust-version = "1.93"`, version 0.1.1) | `crates/core`, `crates/engines`, `crates/cli`, `crates/platform`. `app` is excluded. |
| App | `app/src-tauri/Cargo.toml` (its own `[workspace]`, own `app/src-tauri/Cargo.lock`, also `rust-version = "1.93"`) | `morphring-app` only |

Dependency graph (arrows point at dependencies):

```
morphring (crates/cli, bin "morphring") ──► morphring-core
        │                               ──► morphring-engines ──► morphring-core
        └─────────────────────────────► morphring-platform   (no internal deps)

morphring-app (app/src-tauri, bin "morphring-app") ──► morphring-core, morphring-engines
```

- `morphring-core`: formats, the converter contract and per-family options, presets, toolbar profiles, settings and history stores, the planner, output naming, the job runner and its modes (merge, combine, multi-page TIFF), watch-folder settling, page ranges, `--keep-dates`, ring slices and ring geometry. It never starts a process. Its `test-support` feature exposes `testing::FakeConverter`.
- `morphring-engines`: ten engines (four built-in Rust ones behind cargo features, six that drive external tools), tool discovery with install hints for Linux, Windows and macOS, the process runner, estimated progress and the registry.
- `morphring` (crate dir `crates/cli`): the clap CLI. Its `lib` exposes `run` and `run_with`, and `main` is a thin wrapper around them.
- `morphring-platform`: desktop integration (Linux file manager menus, the Windows Explorer verb and Send To entry, the macOS Finder Quick Action) behind one `integration` front door, plus the pure gesture state machine. The app does not depend on it yet.
- `morphring-app`: Tauri 2 glue (drop zone, ring window, Ctrl+drag on Linux, settings, history and toast windows, tray, single instance, macOS lifecycle). It holds no conversion logic.

Workspace-wide lints: `unsafe_code = "deny"`, clippy `unwrap_used`/`expect_used = "warn"` (allowed in tests via `clippy.toml`). The only `unsafe` lives in narrowly allowed spots: the Unix `killpg` call in `process` and the Win32 calls in `win_job`. Toolchain: stable + rustfmt + clippy (`rust-toolchain.toml`). The app crate repeats these lints by hand. `.cargo/config.toml` links the C runtime statically on Windows (both workspaces), so the binaries need no Visual C++ Redistributable.

### Engine cargo features (`crates/engines/Cargo.toml`)

Everything is on by default except `download`. The ones with code behind them today:

| Feature | Engine module | Crates |
|---|---|---|
| `img` | `native_image` | `image` |
| `svg` | `svg` | `resvg`, `usvg`, `flate2` |
| `pdf` | `pdf` | `lopdf`, `krilla`, `krilla-svg`, `usvg`, `image` |
| `archive` | `archive` | `zip`, `tar`, `flate2` (plus `chrono` for ZIP's DOS dates) |

The other default features (`subtitles`, `data`, `model3d`, `fonts`, `cad`, `doc`, `ebook`, `email`, `sheets`, `markdown`, `icns`, `trace`, `pdf-native-render`) and the opt-in `download` only declare dependencies for the parity tasks (PX-05); no module uses them yet. Building with `--no-default-features` gives the v0.1 build: external tools only.

## 2. Module map

### crates/core/src

| Module | Contents |
|---|---|
| `lib` | Re-exports `CancelToken, Combiner, ConvertOptions, Converter, Edge, EdgeFlags, EngineStatus, FileResult, FileState, FormatId, JobState, Merger, OutputAllocator, QualityLevel, Step, TiffMerger`, `EngineError, ErrorKey`, `job::Target`, `planner::EngineTuning` and everything in `options`. |
| `contract` | The converter contract (section 3). Ported from Filee IConverter.cs and JobQueue.cs. |
| `error` | `ErrorKey` (stable `error.*` strings) and `EngineError`. |
| `options` | Per-family conversion options ported from Filee's Preset.cs: `ImageOptions` (with `Resize`, `TiffCompression`), `PdfOptions` (`PageSize`, page range), `DocumentOptions`, `MediaOptions` (`MediaQuality`, `WavFormat`, `VideoCodec`, `AudioCodec` and the per-target codec tables `video_codecs_for`, `effective_video_codec`, ...), `ArchiveOptions` (`ArchiveLevel`), `TracingOptions` (`TracingMode`). Ranged numbers are clamped on load. Core only stores them; engines decide what they mean. |
| `formats` | `Category` (17 families, including Subtitle, Data and Model3D), `Format`, a static table of 245 formats in Filee's order (includes the `folder` pseudo-format), plus `all, find_by_id, find_by_extension, detect, lookup_target, extension_of, name_without_extension` and `FOLDER`. Detection uses the extension only, and compound extensions (tar.gz) win. |
| `preset` | `Preset` (id, name, target, `ConvertOptions`, `OutputRule`), `SAME_AS_SOURCE = "*"`, `for_target` (the preset `--to` invents), `find` (by id or name). |
| `builtin` | `presets()`: Filee's 94 built-in presets (BuiltInData.cs, v1.11.0), with stable ids in Filee's order. |
| `profiles` | `ToolbarProfile` (one ring layout per set of extensions), `builtin_profiles`, `select` (Filee's `ProfileSelector`), `catch_all`, `extensions_of`, `MAX_SLICES = 12`, and `Library` (presets plus profiles, loaded from and saved to presets.json and profiles.json, `schemaVersion` 1). |
| `settings` | `Store` (atomic JSON writes, a `.broken` copy of a corrupt file, defaults on any error), `AppSettings` (settings.json, `SETTINGS_SCHEMA_VERSION = 2`) with every field of Filee's AppSettings: `DefaultOutput`/`DefaultLocation` (the default output location presets fall back to), `language` (`LANGUAGES`), start with system, context menu, update checks, `paused`, `ThemeSettings` (`ThemeMode`, accent `DEFAULT_ACCENT`, corner radius, `ReduceMotion`), `DonutSettings` (outer radius, hole ratio, opacity), `TriggerGesture` triggers (`TriggerKind`, `TriggerButton`, `TriggerScope`, `DEFAULT_HOLD_MS = 450`), excluded processes, `engine_priority`, `LibreOfficeFor`, and the first-run, feedback, engine-copy and notification bookkeeping. Ranged numbers are clamped on load. Most fields are stored for parity; nothing reads them yet. |
| `history` | `History` (history.json, newest first, capped at `MAX_HISTORY = 200`), `HistoryEntry`, `HistoryFailure`, and `watch_results` for watch-folder summaries. |
| `watch` | The pure watch-folder state machine: `WatchRule`, `AfterConversion`, `should_ignore`, `originals_target`, `Settler` (fed `Stat`s and `Instant`s by the caller), `default_settle` (5 s, 2 s on Windows). Ported from Filee's FolderWatcher.cs. |
| `page_range` | `parse("1-3,5,8-", page_count)` (forgiving: junk means every page) and `runs`. Ported from Filee's PageRange.cs. |
| `dates` | `copy_dates(source, outputs)` for `--keep-dates`: access and modification times via `filetime`, creation time on Windows and macOS only. Failures are warnings. |
| `planner` | `Planner`, `Route`, `Hop`, `EngineTuning` (per-engine priority and cost overrides), `MAX_HOPS = 3`, `MAX_PRIORITY_PENALTY = 9`. An exact search over all simple paths of up to 3 hops, honouring `EdgeFlags`. |
| `output` | `OutputRule` (with `renames`, the name rules applied after the pattern and before `sanitize`), `OutputLocation`, `ConflictPolicy`, `Tokens`, `resolve` (pure), `expand_pattern`, `sanitize`, `Reservations`, `JobAllocator`, `DEFAULT_SUBFOLDER`, `DEFAULT_PATTERN`. |
| `job` | `run_job`, `Target::{Format, SameAsSource}`, `JobEvent`, internal `Job`, `Gate` (per-engine semaphore), `FinalAllocator`, `TempAllocator`. |
| `job::merge` | Merge mode (Merge PDF): converts non-PDF inputs to PDF in parallel, then hands them in order to an engine's `Merger`. The output goes to the first file that made it (Filee pinned it on file 1 even when file 1 failed). |
| `job::tiff` | Multi-page TIFF mode (Filee's `RunMultiPageTiffAsync`): every input goes through the planner to PNG in the work dir (a PDF fans out into one PNG per page; a TIFF passes through as is), then the whole stack, in input order, goes to the first engine with a `TiffMerger`. One TIFF comes out, named after the first file that made it; one bad file fails alone. |
| `job::combine` | Combine-into-one-archive mode (Filee's `RunCombinedAsync`): every input, file or folder, goes into one archive as it is, named `{first name}_files.<ext>`, through the first engine with a `Combiner`. Clashing entry names are staged as `notes (2).txt` (case-insensitively); a cancel removes the half-written archive; keep dates is not applied. |
| `rename` | `RenameStep` (Filee's name rules: `Replace`, `Remove`, `Prefix`, `Suffix`, `SpacesToUnderscores`, `RemoveCopyNumber`, `RemoveBrackets`, `RemoveLeadingNumber`, `RemoveSymbols`, `LowerCase`, `UpperCase`, `Regex`), `apply_all`, `regex_replace`, `regex_error`. The JSON matches Filee's, so its preset files load as they are. |
| `retry` | `plan(entry, exists)` and `plan_watch` turn a `HistoryEntry` into a `RetryPlan { preset, files, skipped_missing, watch_rule_id }` and a `RetryRoute::{Nothing, Job, Watch}`. Pure: the caller says which paths still exist. Ported from Filee's `ConversionService.Retry`. Nothing calls it yet. |
| `slices` | Ring slices from presets: `suggest(files, &Library, &ReachCtx)` picks the profile, then marks each preset `Reach::{Ready, NeedsEngine, NoRoute}` with a `DisabledReason`; `ReachCtx { reach, can_merge }`. Ported from Filee's RadialViewModel.cs and PresetAvailability.cs. |
| `ring` | Pure donut geometry (`Point`, `Hit`, `span`, `mid_angle`, `hit_test(_with_slack)`, `wedge_path`, `ring_path`, `GAP_PX`, `POP_OUT_PX`). Only its own tests use it. The app draws with `app/ui/ring.math.js`, a JS mirror with its own tests. |
| `testing` | `FakeConverter` (builder: `new(id).edge(from,to,cost).display_name().max_parallelism(n).missing(hint).behaviour(b)`), `Behaviour::{Copy, Fail, Panic, Sleep, Outputs(n)}`, `FakeCounters`. Compiled under `cfg(any(test, feature = "test-support"))`. |
| `bundle`, `migrate`, `tuning` | Doc-comment-only placeholders for parity tasks (library export and import, library migrations, settings to `EngineTuning`). |

### crates/engines/src

| Module | Contents |
|---|---|
| `registry` | `all() -> Vec<Box<dyn Converter>>` in priority order (section 5), `statuses()` for `doctor`. |
| `discovery` | `Tool::{Magick, Ffmpeg, Soffice, Pdftoppm, Ghostscript, Pandoc}`, `Os::{Linux, Windows, MacOs}`, `Found`, `find` (process-wide cache), `locate`, `status`, `builtin_status` (for native engines), `forget`, `locate_in(tool, os, PATH, extra)` (pure; results go through `dunce::simplified` so Windows paths lose their `\\?\` prefix), `system_folders(_in)` (Program Files, LibreOffice.app, Homebrew, ...), `parse_version`, `install_hint` (Arch/Debian), `brew_hint`, `winget_hint`, `install_hint_for`, `install_hint_here`. FFmpeg only counts as found with `ffprobe` beside it, Poppler with `pdftotext` and `pdfinfo`; Ghostscript is `gswin64c`/`gswin32c` on Windows and LibreOffice prefers soffice.com there. |
| `process` | `run(program, args, RunOptions, &CancelToken) -> Result<RunOutput, RunError>`. `RunOptions { timeout, stall, stall_poll, cwd, env_remove, on_stdout_line }`, `RunError::{Cancelled, TimedOut, Stalled, Spawn, Failed}`, `STDERR_TAIL_LINES = 40`. Kills the whole tree on cancel, timeout or stall: a process group on Unix, a Job Object on Windows. Also `summarize(tail)` and `move_into_place(from, to)`: rename, retried 3 times 100 ms apart on Windows sharing violations or access denied (antivirus, indexer), then copy + remove. |
| `win_job` | Windows only: the Job Object wrapper (`windows-sys`) that `process` assigns every child to, so `TerminateJobObject` takes soffice.bin and friends down too. |
| `progress` | `Estimate::during`: estimated progress (from 0.1 rising towards 0.9) for tools that report none. Ported from Filee's ProgressEstimate.cs. |
| `native_image` (feature `img`) | `ImageEngine` (id `image`): png, jpg, gif, bmp, tiff, ico, qoi, ppm, tga each to each with the `image` crate, cost 8, still to gif 12. `ops::accepts` declines presets it cannot honour, so magick takes over. |
| `svg` (feature `svg`) | `SvgEngine` (id `resvg`): svg/svgz to png with resvg, svg <-> svgz repack (cost 5). |
| `pdf` (feature `pdf`) | `PdfEngine` (id `pdf`) built from parts: `image_to_pdf` (krilla, JPEGs embedded as is), `svg_pdf` (krilla-svg, text stays text), `pages` (lopdf page range and split), and `merge` (lopdf merger, the engine's `Merger`). |
| `magick` | `MagickEngine` (id `magick`, one per core, 120 s). `magick::args`: `READABLE`, `WRITABLE`, `EDGES`, `FramePolicy`, `quality_for`, `args` (applies `ImageOptions`), `identify_args`, `largest_frame`. |
| `poppler` | `PopplerEngine` (id `poppler`): `render` (pdftoppm, pdf to png/jpg/tiff at cost 10, page ranges, `_p{n}` suffixes, DPI clamped like Filee) and `text` (pdftotext, UTF-8 with BOM, a clear error for PDFs without a text layer). |
| `archive` (feature `archive`) | `ArchiveEngine` (id `archive`): `extract` (zip, tar, tar.gz to `folder`), `pack` (file or folder to zip, tar, tar.gz; UTF-8 names, clamped DOS dates, PAX for long names), `safety` (Filee's ArchiveSafety.cs: no absolute paths, `..` or escaping links, entry and size caps, a free-space check). |
| `ffmpeg` | `FfmpegEngine` (id `ffmpeg`, parallelism 2, 30 min per pass with a stall watchdog, 60 s probe). `ffmpeg::encoding`: formats, `EDGES`, `Source`, `plan(...) -> Vec<FfmpegPass>` with Filee's full remux fit matrix, hvc1 tagging and `MediaOptions` (quality, codecs, WAV format). `ffmpeg::probe`: ffprobe JSON and `-progress` parsing. |
| `pandoc` | `PandocEngine` (id `pandoc`, 120 s): Markdown, HTML, DOCX, ODT, RTF, reStructuredText, LaTeX. `pandoc::args`: Filee's reader/writer tables, `COST = 10`, `OVERLAP_COST = 14`, media extraction. |
| `ghostscript` | `GhostscriptEngine` (id `ghostscript`, 120 s): PDF, EPS, PS and AI to png/jpg/tiff, `pngalpha` for the PostScript family, PDF on white and priced above Poppler. `ghostscript::args` is pure. |
| `soffice` | `SofficeEngine` (id `soffice`, `WORKERS = 2`, 180 s, `new_in(data_dir)` for tests) with one LibreOffice profile per worker. `soffice::filters`: families, `filter_for`, `edges()` (`FALLBACK_COST = 25`, `TEXT_TO_DOCUMENT_COST = 28`, `SLIDES_TO_PDF_COST = 0`), `profile_dir`, `args`. Success is judged by the output file, not the exit code. |
| `parts` (in `lib`) | `Part`/`Parts`: an engine made of several parts lists each part's edges and routes a step to the first part that has the pair. Used by `pdf`, `poppler` and `archive`. |

### crates/cli/src

| Module | Contents |
|---|---|
| `main` | Logging (stderr, `warn` by default, `RUST_LOG`, `--verbose` raises `morphring*` to debug), Ctrl+C (`Presses`: the first press cancels, the second exits with 130), then `morphring::run`. |
| `lib` | `run(args, out, err, cancel) -> i32` builds `Ctx` from the cwd, `registry::all()` and the `ProjectDirs` config dir. `run_with(args, &mut Ctx) -> i32` is the entry point for tests. `parse` answers help and version before engines are looked up; `help_aliases` rewrites "/?" to `--help` first (up to a `--`), so "/?" works wherever `-h` does. Constants `VERSION`, `EXIT_USAGE = 2`. |
| `ctx` | `Ctx { out, err, cwd, cancel, engines, config_dir: Option<PathBuf> }`. `None` means built-in presets and default settings only. Commands read nothing from the environment except through `Ctx` (or `Host` for integrate). |
| `args` | clap tree: `Command::{Convert, Watch, Formats, Presets, Doctor, Integrate, Help, Version}`, `ConvertArgs { inputs, to, preset, output, recursive, quality, overwrite, skip, keep_dates, json, quiet }`, `PresetsArgs { action }` with `PresetsAction::{Export, Import, Reset}` (`PresetsFileArgs { file }`, `PresetsResetArgs { yes }`), `WatchArgs`, `FormatsArgs`, `DoctorArgs`, `IntegrateArgs`, `HelpArgs`, hand-written help consts. The field names are a contract. |
| `resolve` | `job_from_args` turns `--to`/`--preset` plus overrides into a `JobSpec` in Filee's order (`-o` last, so it beats the preset and the settings). `UsageError` for exit 2. |
| `inputs` | `expand(inputs, recursive, cwd, is_known) -> Expanded { files, missing }`: files, folders (known formats only, sorted) and wildcards, deduplicated in order. |
| `report` | Human lines (`file_line`, `summary`, `Counts`), `message(ErrorKey)`, `error_text`, `JsonReport`/`JsonFile` + `write_json` (Filee key order, with the preset's display name in `preset`, `null` for `--to`). |
| `commands::convert` | `run(&ConvertArgs, &mut Ctx) -> i32`, also the merge mode for the Merge PDF preset. `--json` writes only the JSON document to stdout; `--quiet` prints only the failed and cancelled lines, on stderr. Exit codes 0, 1 (some failed), 2 (usage), 3 (nothing to do). |
| `commands::watch` | `morphring watch <folder>`: a `notify` watcher plus a 1 s tick feeding `core::watch::Settler`, conversions per settled file, history entries, Ctrl+C to stop. |
| `commands::presets` | `morphring presets` lists the presets in presets.json order. |
| `commands::library` | `presets export <file>`, `presets import <file>` and `presets reset [-y]`: declared (MorphRing extras; Filee has them only in its GUI) but they report that they are not available yet and exit 2, until `core::bundle` lands. |
| `commands::formats` | The registry by category, or `formats <FMT>`: targets from two planners (installed, then all engines), with hints for missing engines. |
| `commands::doctor` | Engine status rows plus `Folders { config, data, temp }`, human or `--json`. `run_in` takes injected folders. |
| `commands::integrate` | `Action::{Install, Uninstall}`, `Host { home, release_build, app }`, `APP_BINARY = "morphring-app"`, `find_app`, `--exe`, `--dry-run`, `--allow` (required outside release builds). Calls `morphring_platform::integration`. |

### crates/platform/src

| Module | Contents |
|---|---|
| `integration` | The front door: `install(exe, home, dry_run)`, `uninstall`, `Change`, `IntegrationError` (incl. `Unsupported`), `LABEL = "Convert with MorphRing"`, `surfaces`. Picks the backend at compile time. |
| `context_menu` | Linux: Dolphin service menu, Nautilus script, Nemo action, Thunar Send To, each running `<app> --convert %F`. Only files carrying `MANAGED_MARKER` are replaced or removed. |
| `explorer` | Windows: the per-user verb under `HKCU\Software\Classes\*\shell\MorphRing` and `Directory\shell\MorphRing` (`winreg`), plus a Send To shortcut (`mslnk`). Pure `verb_entries` generators; the registry calls are `cfg(windows)`. `app/src-tauri/windows/hooks.nsh` writes the same keys from the installer, and release.yml checks the two agree. |
| `finder` | macOS: the Quick Action bundle "Convert with MorphRing.workflow" in the user's Services folder, from the templates in `crates/platform/resources/quick-action/Info.plist` and `crates/platform/resources/quick-action/document.wflow`. |
| `gesture` | The pure `Detector` port of Filee's `GestureDetector` (drag, hold, modifiers, scopes) with Filee's tests. Not wired to any input source yet; the Linux watcher in the app has its own small state machine. |

Every backend compiles on every OS, so a Linux build still type-checks the Windows and macOS generators.

### app/

| Path | Contents |
|---|---|
| `app/src-tauri/src/main.rs` | Logging, then `morphring_app::run()`. |
| `app/src-tauri/src/lib.rs` | `run()`: the single-instance plugin first, then `tauri-plugin-opener` and `tauri-plugin-dialog`, managed `PendingOpen` and `GestureActive`, one `generate_handler!` with every command (`convert`, `ring_close`, `ping`, `cancel_job`, `settings_get`, `settings_set`, `presets_list`, `history_list`, `history_clear`, `toast_dismiss`, `toast_check`, `toast_dismiss_delay`, `toast_layout`), the setup hook (tray, Ctrl+drag watcher, `instance::on_startup`, `lifecycle::setup`) and both window-event handlers. The only place where plugins, commands and hooks are registered. |
| `app/src-tauri/src/commands.rs` | `RingModel { files, title, slices: Vec<RingSlice { preset_id, target, label, enabled, disabled_reason }> }`, events `ring-open`, `ring-progress`, `ring-done` (ring only) and `job-started`, `job-progress`, `job-finished` (app-wide, carrying `jobId`). `Conversions` (lazy managed state) runs jobs FIFO on one worker: the preset's target, options and output rule, with the settings' default output filled in, and merge mode for Merge PDF. `cancel_job(jobId)` cancels a running or queued job. Finished jobs go to `history::record`. |
| `app/src-tauri/src/instance.rs` | `CommandLine::parse` (`--background`, `--settings`, `--quit`, `--convert <files>`), `Launch::of`, the 350 ms `Debouncer`, `on_startup`, `on_second_instance`, `open_ring`. |
| `app/src-tauri/src/lifecycle.rs` | macOS manners: `RunEvent::Opened` file URLs (queued in `PendingOpen` until setup), Accessory mode for `--background`, Dock reopen. A no-op elsewhere. |
| `app/src-tauri/src/ring_window.rs` | `show`: centre, show and focus on Linux and macOS; on Windows re-assert topmost and never steal focus. The ring is hidden when unused, never left as a click-through overlay. |
| `app/src-tauri/src/dropzone.rs` | `accept_drop(paths)` keeps existing regular files without duplicates; `on_window_event` emits `files-dropped` to `main` and opens the ring. |
| `app/src-tauri/src/gesture_linux.rs` | Linux only (`evdev`): the Ctrl+drag watcher (section 4.3). |
| `app/src-tauri/src/hyprland.rs` | Linux only: a tiny Hyprland IPC client (`cursor`, `active_class`, `move_window`). Every call is `None` off Hyprland. |
| `app/src-tauri/src/drag_ring.rs` | The ring's side of Ctrl+drag: `arm` opens the ring empty under the cursor, the window's drag events become `ring-drag-enter`, `ring-drag-over`, `ring-drag-leave`, `ring-drag-drop`, and `ring-drag-dismiss` tidies up when the drop lands elsewhere. `GestureActive` says whether the watcher runs (then the main window starts hidden). |
| `app/src-tauri/src/settings.rs` | The settings window (`settings_get`, `settings_set`, `presets_list`) and the lazily managed `Prefs` (settings plus preset library) every other module reads; `validate*` for the default output. |
| `app/src-tauri/src/history.rs` | The history window (`history_list`, `history_clear`), `record` after each job, `history-changed`. Plain `*_in(dir)` functions for tests. |
| `app/src-tauri/src/toast.rs` | The toast window for jobs that outlive the ring: shown after 1.5 s, dismissed after 5 s (15 s on errors), bottom-right of the work area. |
| `app/src-tauri/src/tray.rs` | `TrayAction` (open, settings, history, quit), `show_main`, `show_window`; a template icon and Dock-hiding on macOS. |
| `app/src-tauri/tauri.conf.json` | Windows `main` (drop zone), `ring` (360x360, hidden, transparent, undecorated, always on top), `settings`, `history`, `toast` (transparent, always on top). `withGlobalTauri: true`, a strict CSP, `frontendDist` pointing at `app/ui`. Linux bundles: deb and AppImage. |
| `app/src-tauri/tauri.windows.conf.json` | NSIS, current-user install, `app/src-tauri/windows/hooks.nsh` for the Explorer verb and Send To. |
| `app/src-tauri/tauri.macos.conf.json` | `macOSPrivateApi` (the transparent ring), app + dmg, ad-hoc signing, macOS 13+, file associations for "Open With". |
| `app/src-tauri/capabilities/default.json` | `core:default`, `opener:default`, `dialog:default` for all five windows. |
| `app/ui/` | No bundler and no framework; scripts use `window.__TAURI__`. Drop zone (`app/ui/index.html`, `app/ui/main.js`, `app/ui/style.css`), ring (`app/ui/ring.html`, `app/ui/ring.js` with its backend contract at the top, `app/ui/ring.math.js`, `app/ui/ring.css`), settings, history and toast pages, and `app/ui/ring.test.mjs` for `node --test`. |
| `app/scripts/icons.sh` | Regenerates the icons from `app/src-tauri/icons/icon.svg` (the ring itself, in the palette below). |

The palette ("ink, cream and one tomato slice") is defined as CSS custom properties at the top of each stylesheet: ink `#0e0e0e`, cream `#f1ecdf` and tomato `#f04a35` as `--accent`, with every other colour a shade of those three (`--surface`, `--text-secondary`, `--accent-strong`, ...). Pages follow the system light/dark preference; the donut does not: cream slices, an ink hole and gaps, and the hovered slice turns solid tomato.

Other: `scripts/make-fixtures.sh` regenerates `crates/engines/tests/fixtures` (the output is committed). `packaging/aur/morphring-bin/PKGBUILD` is the AUR package for the CLI tarball (not published).

## 3. The contract (`morphring_core::contract`, `error`, `options`)

```rust
pub struct FormatId(&'static str);              // new(), as_str(), Display
pub struct EdgeFlags { pub image_sources_only: bool, pub not_for_image_sources: bool }
pub struct Edge { pub from: FormatId, pub to: FormatId, pub cost: u32, pub flags: EdgeFlags }
                                                // new() = cost 10, with_cost()
pub struct ConvertOptions { pub quality: Option<u8>,          // the CLI's -q override
    pub image: ImageOptions, pub pdf: PdfOptions, pub document: DocumentOptions,
    pub media: MediaOptions, pub archive: ArchiveOptions, pub trace: TracingOptions }
pub enum QualityLevel { High /* >=85 */, Normal /* 60-84 or None */, Small /* <60 */ }
pub struct Step { pub input: PathBuf, pub from: FormatId, pub to: FormatId,
                  pub options: ConvertOptions, pub work_dir: PathBuf }
pub trait OutputAllocator { fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf>; }
pub enum EngineStatus { Available { path: PathBuf, version: String }, Missing { hint: String } }
pub struct CancelToken(Arc<AtomicBool>);        // new(), cancel(), is_cancelled(); clones share
pub trait Converter: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn edges(&self) -> &[Edge];
    fn max_parallelism(&self) -> usize { 0 }   // 0 = one per core
    fn status(&self) -> EngineStatus;
    fn accepts(&self, options: &ConvertOptions) -> bool { true }
    fn merger(&self) -> Option<&dyn Merger> { None }
    fn combiner(&self) -> Option<&dyn Combiner> { None }
    fn tiff_merger(&self) -> Option<&dyn TiffMerger> { None }
    fn convert(&self, step: &Step, out: &mut dyn OutputAllocator,
               progress: &dyn Fn(f32), cancel: &CancelToken) -> Result<Vec<PathBuf>, EngineError>;
}
pub trait Merger: Send + Sync {                 // Combiner and TiffMerger have the same shape
    fn merge(&self, inputs: &[PathBuf], out: &mut dyn OutputAllocator,
             progress: &dyn Fn(f32), cancel: &CancelToken) -> Result<Vec<PathBuf>, EngineError>;
}
pub enum FileState { Done, Skipped, Failed { key: ErrorKey, detail: String }, Cancelled }
pub struct FileResult { pub source: PathBuf, pub state: FileState, pub outputs: Vec<PathBuf> }
pub enum JobState { Completed, CompletedWithErrors, Failed, Cancelled }  // from_files(&[FileResult], cancelled)

pub enum ErrorKey { UnsupportedSource, NoRoute, EngineMissing, ConversionFailed } // as_str(): "error.*"
pub enum EngineError { Cancelled, NotInstalled { tool, hint }, Failed(String), Io(io::Error) } // key()

// morphring_core::job
pub enum Target { Format(FormatId), SameAsSource }   // SameAsSource uses the engine's self edge
```

Engine obligations: never modify the step's `input`. Get every output path from `out.allocate`, where `None` means skip (return `Ok(vec![])`). Write into a temp dir under `step.work_dir` first, then `process::move_into_place`. Check `cancel`. Report `progress` values that only rise. A multi-output engine allocates with suffixes `_p1`, `_p2`, ... and an empty `ext` asks for a folder. An engine that cannot honour the options returns `false` from `accepts`, and the job plans without it (the native image engine does this, so magick takes over); it never ignores an option silently.

## 4. How a conversion flows

### 4.1 CLI: `morphring convert a.png photos --to jpg -o out -q 80` (or `--preset half-size`)

1. `main`: logging, then a `CancelToken` wired to Ctrl+C, then `morphring::run(args, stdout, stderr, cancel)`.
2. `lib::parse`: clap parsing. Help, version and usage errors exit here (code 0 or 2) without touching engines. Otherwise `Ctx` is built with `registry::all()` and the config dir, and `dispatch` calls `commands::convert::run`.
3. `convert::run`:
   - `resolve::job_from_args`: loads the `Library` and `AppSettings` from `ctx.config_dir`, takes `--preset` (by id or name) or invents one with `preset::for_target` for `--to` (the target must exist and be writable), clones it, then applies the settings' default output, `-q`, `--overwrite`/`--skip`, `--keep-dates` and finally `-o`. Errors exit 2.
   - `inputs::expand`: missing inputs exit 2, nothing left exits 3.
   - Runs the job on a scoped thread (merge mode for a merge preset, `job::run_job` otherwise). `FileFinished` events come back over a channel and are printed in finishing order (`--json` prints nothing until the end, `--quiet` only failures, on stderr).
   - Prints the JSON report or `report::summary`, exits 0 or 1.
4. `core::job::run_job(files, target, &options, &rule, &engines, &cancel, &on_event) -> Vec<FileResult>`:
   - Drops engines whose `accepts(&options)` is false, then builds `Planner::new(engines, false)` (installed engines) and a "dream" planner over every engine, one `Gate` per engine id (`max_parallelism`, 0 = core count), shared `Reservations`, and a work dir under the system temp folder.
   - Runs `min(cores, files)` scoped workers pulling indices from an `AtomicUsize`.
   - Per file: stop if cancelled, `formats::detect` (none gives `UnsupportedSource`), the target per file (`SameAsSource` means the file's own format), `planner.plan(from, target)`. With no route, the dream planner decides between `EngineMissing` ("needs X: hint") and `NoRoute`.
   - `run_route`: intermediate hops use `TempAllocator`, the last hop `FinalAllocator` (wrapping `JobAllocator` with `Tokens`). The gate permit is held for one step only. Every output of a hop feeds the next hop. File progress is (hop + p) divided by hops, clamped so it only rises.
   - When `rule.keep_dates` is set (`--keep-dates`), `dates::copy_dates` gives each done file's outputs its dates.
   - Panics are caught per file and per event handler. On failure or cancel, final outputs this file created are deleted. `Finished([])` becomes `Skipped`.
   - Results come back in input order. The work dir is removed on drop.
5. Engine `convert` (for example `MagickEngine`): `discovery::locate`, maybe `identify` for frames, `out.allocate`, a tempdir in `work_dir`, `process::run` (cwd = the tempdir, timeout, stall watchdog for ffmpeg), validation of the output, `move_into_place`.

### 4.2 App: drop or launch, ring, click, convert

1. Entry points: a drop on `main` (`dropzone::on_window_event`), `morphring-app --convert files...` from a file manager menu, Explorer verb, Send To or the Quick Action (`instance::on_startup` / `on_second_instance`, then `CommandLine::parse`, `Launch::of` and the 350 ms `Debouncer`), Finder's "Open With" on macOS (`lifecycle::on_run_event`, `RunEvent::Opened`), or Ctrl+drag on Linux (4.3).
2. `commands::open_ring` builds the `RingModel` with `slices::suggest(files, &library, ReachCtx { reach, can_merge })` against the user's library from `settings::Prefs`. Slices are the presets of the selected toolbar profile, each enabled or carrying a reason (needs an engine, no route, merge not available). It is emitted as `ring-open` and `ring_window::show` shows the ring, centred (Wayland does not tell apps where the cursor is).
3. `app/ui/ring.js`: a slice click invokes `convert { files, presetId }`.
4. `Conversions::convert` queues a request; the worker resolves the preset, fills in the default output from the settings, and runs `job::run_job` (or merge mode) with a `CancelToken` kept per `jobId`. Progress goes out as `ring-progress` (for the job the ring shows) and `job-progress` (for the toast), in steps of at least 1 %, in order even when files convert in parallel.
5. The ring's hover cancel button calls `cancel_job(jobId)`. At the end `ring-done` and `job-finished` go out, the job is recorded in history.json and the command answers with a `ConvertSummary`.
6. `ring_close` hides the ring; a running job keeps going and, after 1.5 s, gets a toast card.

### 4.3 Linux: Ctrl+drag

`gesture_linux::start` runs at setup unless `MORPHRING_NO_GESTURE` is set. It opens every evdev device under `/dev/input` that has Ctrl or a left button, and rescans every 2 s so keyboards and mice plugged in later (or reset by a suspend) are picked up too; a device that goes away is forgotten until it comes back. Reading the devices only works for users in the `input` group (Wayland gives apps no global input, on purpose). Without readable devices it logs one info line and returns false: the main window and its drop zone then stay the way in. When it runs, `GestureActive` is set and the main window starts hidden (the tray brings it back).

One thread per device feeds a small state machine: Ctrl (either side) plus the left button, in any order, arms it; on a touchpad a tap followed by a second touch within 350 ms counts as the button (libinput invents tap buttons, the kernel never reports them). On arm it asks Hyprland which window has focus and rejects the press unless the class looks like a file manager (Dolphin, Nautilus, Nemo, Thunar, PCManFM, Caja, ...), so Ctrl+dragging text in a browser stays quiet. `MORPHRING_GESTURE_ANYWHERE` skips that check; without Hyprland the check passes. It then polls the Hyprland cursor every 20 ms, and once it moves 12 px the press is a drag: `drag_ring::arm` shows the ring empty, without focus, with its hole under the cursor (`hyprland::move_window`, or centred when the cursor is unknown). The drag's hover and drop events become `ring-drag-*` events for `app/ui/ring.js`; dropping on a slice converts with that preset, and releasing anywhere else dismisses the ring. Windows hooks and the macOS event tap are not written yet, so there `GestureActive` is false.

## 5. Registry order and route planning

`registry::all()` returns, in this order (the order is the priority): `image`, `resvg`, `pdf`, `magick`, `poppler`, `archive`, `ffmpeg`, `pandoc`, `ghostscript`, `soffice`. The built-in four are present only when their feature is on; external engines are always present and report `Missing` when their program is not found. Native engines go first (no process to start), Poppler beats Ghostscript for PDFs, LibreOffice stays the last resort. The order is set once for v0.2 (PLAN-v0.2.md section 2) and pinned in `registry`'s own tests.

- Edge cost: `edge.cost + min(engine_index, 9)` (priority penalty, so registry order breaks ties), adjusted by `EngineTuning` when given. Parallel edges for the same `from -> to` collapse to the cheapest.
- `EdgeFlags`: `image_sources_only` edges (image tracing) are used only in routes that start from a raster image; `not_for_image_sources` edges are never used in those. Ported from Filee's RoutePlanner.cs.
- `plan(from, to)`: if `from == to`, only an explicit self edge counts. Otherwise the search walks every simple path of 3 hops or fewer and keeps the cheapest; on a tie, fewer hops win, then registration order. Deterministic.
- `targets_from(from)` gives `(FormatId, cost)` sorted by cost, then by first appearance. `formats <FMT>` uses it.
- Route pins: `crates/engines/tests/routes.rs` keeps the pins that hold for all of v0.2 (mp4 to jpg via png, video to gif 15, pptx to pdf 0, the soffice fallback 25, no txt to mp3). Each engine pins its own in `crates/engines/tests/routes_images.rs`, `crates/engines/tests/routes_poppler.rs`, `crates/engines/tests/routes_ghostscript.rs` and `crates/engines/tests/routes_pandoc.rs`.

## 6. Tests

| Where | What | Helpers |
|---|---|---|
| `#[cfg(test)] mod tests` in every module | Pure logic: formats, options, presets and profiles (Filee's ProfileSelectorTests), settings and history stores, watch settling, page ranges, planner, output resolver, job runner and merge mode, slices, ring geometry, gesture detector, every argument builder, archive safety, discovery with fake tool scripts, the process runner (process-group and Job Object kill, stall watchdog, rename retry). | `core::testing::FakeConverter` |
| `crates/engines/tests/{magick,ffmpeg,soffice,ghostscript,pandoc,svg,native_image,poppler_render,poppler_text,pdf_images,pdf_pages,pdf_svg,pdf_merge,archive_extract,archive_pack}.rs` | Golden tests that run real tools or the native engines on fixtures and check format, size, page count or duration, not bytes. | `crates/engines/tests/common/mod.rs`: `require_engine!(tool)` (prints `SKIP:` and returns), `fixture(name)`, `image_size`, `frame_count`, `duration_secs` |
| `crates/engines/tests/routes.rs` and the per-engine route files | Route costs over `registry::all()` with every engine assumed present. Start no programs. | |
| `crates/engines/tests/fixtures_check.rs` | Every fixture in `crates/engines/tests/fixtures` exists and stays under 100 KB. | |
| `crates/cli/tests/{cli,convert,convert_presets,convert_merge,presets,watch}.rs` | End to end in-process through `run_with` with real or fake engines; help and version run the built binary with an empty `PATH`. | `crates/cli/tests/common/mod.rs`: `run_in`, `run_with_cancel`, `copy_fixture`, `require_engine!` |
| `app/src-tauri/tests/commands.rs` | `Conversions`, `ring_model`, cancel and job events with real and fake engines, without a window. | Recorder `EventSink` |
| `app/src-tauri/tests/config.rs` | The Tauri config: every window and its page, capabilities, icons, hidden and focus-free windows, and the scripts each page loads. | |
| `app/ui/ring.test.mjs` | `node --test` for `app/ui/ring.math.js` (geometry, wedges, the progress arc). | |

Rules (ENGINEERING.md section 8): no `std::env::set_var` in tests. Inject the cwd, engines, config dir, home folder and clock instead. A missing engine skips a test and never fails it, and CI checks that the magick goldens did not skip.

## 7. CI and release

`.github/workflows/ci.yml` runs on every push and PR, cancelling older runs per ref. Every cargo call uses `--locked`. Since v0.2 (XP-14) every OS blocks:

| Job | Runs | Blocking |
|---|---|---|
| `fmt` | `cargo fmt --all --check` | yes |
| `clippy` | `cargo clippy --workspace --all-targets --locked -- -D warnings` | yes |
| `test (ubuntu, windows, macos)` | `cargo test --workspace --locked --no-fail-fast` with engines installed: Ubuntu gets ffmpeg, poppler-utils, ghostscript, pandoc from apt and ImageMagick 7 as a pinned, checksummed AppImage, then re-runs the magick goldens and fails if any printed `SKIP:`; macOS gets them from Homebrew; Windows from Chocolatey, with Ghostscript unpacked by 7-Zip. | yes |
| `goldens-office` | Installs LibreOffice and runs the soffice goldens and the CLI convert tests. | no (`continue-on-error`) |
| `msrv` | Reads `rust-version` from `Cargo.toml`, then `cargo +<ver> check --workspace --locked --all-targets` | yes |
| `deny-licenses` | `cargo-deny check licenses bans sources` (`deny.toml`) | yes |
| `deny-advisories` | `cargo-deny check advisories` | no |
| `app (ubuntu, windows, macos)` | WebKitGTK on Linux, then app clippy with `-D warnings`, app tests and `node --test` over the UI tests. | yes |

`.github/workflows/release.yml` runs on a `v*` tag (or by hand, without the release). Build jobs, each testing or smoke-testing what it makes:

| Job | Runner | Output |
|---|---|---|
| `linux` | ubuntu-22.04 | Tests core, engines and cli, builds the x86_64 CLI tarball (smoke: `--version` must match the tag, a wav to mp3 conversion), and the ring app as .deb and .AppImage with the pinned `TAURI_CLI_VERSION`, checked against the tauri crate in the app's lockfile. |
| `windows-cli` | windows-2025 | The x86_64 CLI as a zip, smoke-tested. |
| `windows-app` | windows-2025 | The per-user NSIS installer, after checking `app/src-tauri/windows/hooks.nsh` against `crates/platform/src/explorer.rs`. |
| `macos-cli` | macos-26 | The arm64 CLI, ad-hoc signed, as a tarball. |
| `macos-app` | macos-26 | The Apple Silicon app as an ad-hoc signed .dmg. |
| `publish` | ubuntu-latest | Needs every build job; writes one SHA256SUMS.txt and opens a **draft** release from the tag's `CHANGELOG.md` section. A human publishes it. |

## 8. Extension points

### A new engine
1. Add a module folder under `crates/engines/src` with a struct that implements `Converter`. Put pure argument building and edges in a submodule and unit-test it there. An engine with several independent jobs uses `parts::Parts`, one file per part.
2. For an external tool, add a `Tool` variant in `discovery` (programs per OS, version probe, `install_hint`, `brew_hint`, `winget_hint` arms, extra system folders) and call `discovery::status`/`locate`. Run it through `process::run` with a timeout and the cancel token, and use `progress::Estimate` if it reports no progress. A native Rust engine returns `discovery::builtin_status()` and goes behind a cargo feature.
3. Add `pub mod` in the engines `lib` and insert the engine into `registry::all()`. **The position is the priority**, so changing it shifts every penalty after it: update `registry`'s order tests and the route pins.
4. Any new format id must exist in `formats` (one line, bump the array length). Pin every hand-tuned cost in the engine's own routes test file (see section 5).
5. Add golden tests in a new file under `crates/engines/tests` with `mod common;` and `require_engine!`. Add fixtures via `scripts/make-fixtures.sh` and list them in `crates/engines/tests/fixtures_check.rs`. If the tool should run in CI, install it in every OS's engine step in ci.yml.
6. The CLI and the app pick the engine up through `registry::all()`. Add its program to THIRD-PARTY-NOTICES.md if it is an external tool, and its crates if it links new ones.

### A new CLI command
1. Add a variant to `args::Command`, an `XxxArgs` struct and a help const; update `help_text` and `GENERAL_HELP`.
2. Create `commands::xxx` exposing `pub fn run(&XxxArgs, &mut Ctx) -> i32` and add it to `commands::mod`.
3. Add a match arm in `lib::dispatch`. Read the environment only in `lib`, in the style of `integrate::Host::from_process`, and pass it in.
4. Exit codes: 0 ok, 1 some failed, 2 usage (`EXIT_USAGE`), 3 nothing to do.
5. Test through `run_in` from the CLI's test helpers with `FakeConverter` or real engines.

### A new setting or preset option
- Settings live in `settings::AppSettings` (settings.json). Add a field with a default; `#[serde(default)]` keeps old files loading. The CLI reads it in `resolve`, the app through `settings::Prefs` and shows it in `app/ui/settings.js`.
- Per-conversion options go in the family struct in `options` and are mapped inside each engine. An engine that cannot honour a new option must decline it in `accepts`.
- Presets and profiles live in `builtin` and `profiles`; the user's copies are presets.json and profiles.json. Built-in ids never change.
- Engine priority overrides go through `EngineTuning` (the `tuning` module will turn `AppSettings::engine_priority` and `libre_office_for` into one).
- Output name rules are `rename::RenameStep`s in `OutputRule::renames`; a new step is a variant there plus its case in `apply_all`.

### A new app page or window, Tauri command or event
1. A window is a `windows[]` entry in `app/src-tauri/tauri.conf.json` with its HTML in `app/ui/` (plain JS, `window.__TAURI__`, CSP `script-src 'self'`, so no inline scripts) and the palette tokens. Add its label to `app/src-tauri/capabilities/default.json` and extend `app/src-tauri/tests/config.rs`.
2. For a command, write a `#[tauri::command]` fn, keep testable logic in plain functions or structs (as `Conversions` and `history::record_in` do), and register it in the single `generate_handler!` in `app/src-tauri/src/lib.rs`. Plugins are registered only there too.
3. For events, define a `pub const NAME: &str` next to its camelCase payload struct and `emit_to` the window. Document it in the contract comment at the top of `commands` and of the consuming JS file.
4. Shared state uses the lazy `try_state`/`manage` pattern (`commands::conversions`, `settings::prefs`).
5. Tray entries go in `TrayAction` in `tray`.

### Desktop integration
New file managers or OS surfaces go in `morphring-platform`: pure generators plus `install`/`uninstall` taking `home`, with a managed marker, and a branch in `integration`. Keep generators compiling on every OS and gate only the OS calls with `cfg(target_os)`. Gesture backends for Windows (low-level mouse hook) and macOS (event tap) will feed `gesture::Detector`.

## 9. Known gaps visible in the code

- `core::ring` geometry is duplicated in `app/ui/ring.math.js`, and only the JS copy runs. Both copies are tested; removing one is deferred (PLAN-v0.2.md section 5).
- `platform::gesture` is not used: the Linux watcher in `gesture_linux` has its own small state machine, it needs Hyprland for the cursor and focus checks (other Wayland compositors get a centred ring and no file-manager filter), and Windows and macOS have no gesture backend yet.
- `job::combine` and `job::tiff` run their jobs, but no shipped engine offers a `Combiner` or `TiffMerger` yet (only the test fakes do), so those jobs fail with a reason and their presets stay disabled. The `bundle`, `migrate` and `tuning` core modules are empty placeholders, so `presets export/import/reset` are declared but not available.
- `retry` plans retries but no command or window calls it, and most `AppSettings` fields (theme, donut, triggers, engine priority, ...) are stored without anything reading them.
- Most of the parity dependencies (spreadsheets, documents, e-books, fonts, CAD, 3D, downloads) are declared in the manifests but not used by any module yet.
- `JobEvent::FileStarted` is sent but nobody listens; the CLI uses `FileFinished` and the app ignores it.
- Pandoc runs without `--sandbox` (it fails for docx on Arch's build), so a document that links remote images can make pandoc fetch them.
