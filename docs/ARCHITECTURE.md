# MorphRing Architecture

The code as it stands on `main` (v0.1.0 CLI released, v0.1.1 ring app working on Linux). PLAN.md says where the code is going and ENGINEERING.md sets the rules. This file describes what exists today. If this file and the code disagree, the code is right, so fix this file.

## 1. Workspaces and crates

There are two Cargo workspaces (PLAN.md D1):

| Workspace | Manifest | Members |
|---|---|---|
| Root | `Cargo.toml` (resolver 3, edition 2024, `rust-version = "1.85"`, version 0.1.0) | `crates/core`, `crates/engines`, `crates/cli`, `crates/platform`. `app` is excluded. |
| App | `app/src-tauri/Cargo.toml` (its own `[workspace]`, own `Cargo.lock`) | `morphring-app` only |

Dependency graph (arrows point at dependencies):

```
morphring (crates/cli, bin "morphring") ──► morphring-core
        │                               ──► morphring-engines ──► morphring-core
        └─────────────────────────────► morphring-platform   (no internal deps)

morphring-app (app/src-tauri, bin "morphring-app") ──► morphring-core, morphring-engines
```

- `morphring-core`: formats, the converter contract, the planner, output naming, the job runner, ring slices and ring geometry. It never starts a process. Its `test-support` feature exposes `testing::FakeConverter`.
- `morphring-engines`: external-tool engines (ImageMagick, FFmpeg, LibreOffice), tool discovery, the process runner and the registry.
- `morphring` (crate dir `crates/cli`): the clap CLI. `lib.rs` exposes `run` and `run_with`, and `main.rs` is a thin wrapper around them.
- `morphring-platform`: desktop integration. Today it only holds the Linux file manager context menus.
- `morphring-app`: Tauri 2 glue (drop zone, ring window, tray, single instance). It holds no conversion logic.

Workspace-wide lints: `unsafe_code = "deny"`, clippy `unwrap_used`/`expect_used = "warn"` (allowed in tests via `clippy.toml`). Toolchain: stable + rustfmt + clippy (`rust-toolchain.toml`). The app crate repeats these lints by hand.

## 2. Module map

### crates/core/src

| Module | Contents |
|---|---|
| `lib.rs` | Re-exports `CancelToken, ConvertOptions, Converter, Edge, EngineStatus, FileResult, FileState, FormatId, JobState, OutputAllocator, QualityLevel, Step, EngineError, ErrorKey`. |
| `contract.rs` | The converter contract (section 3). Ported from Filee `IConverter.cs` and `JobQueue.cs`. |
| `error.rs` | `ErrorKey` (stable `error.*` strings) and `EngineError`. |
| `formats.rs` | `Category` (14 families), `Format`, `static FORMATS: [Format; 172]` (Filee's order, includes the `folder` pseudo-format), plus `all, find_by_id, find_by_extension, detect, lookup_target, extension_of, name_without_extension` and `FOLDER`. Detection uses the extension only, and compound extensions (`tar.gz`) win. |
| `planner.rs` | `Planner`, `Route`, `Hop`, `MAX_HOPS = 3`, `MAX_PRIORITY_PENALTY = 9`. An exact search over all simple paths of up to 3 hops. |
| `output.rs` | `OutputRule`, `OutputLocation`, `ConflictPolicy`, `Tokens`, `resolve` (pure), `expand_pattern`, `sanitize`, `Reservations`, `JobAllocator`. |
| `job.rs` | `run_job`, `JobEvent`, internal `Job`, `Gate` (per-engine semaphore), `FinalAllocator`, `TempAllocator`. |
| `slices.rs` | Ring slice suggestions: `FavouriteList`, `default_favourites()`, `select_list`, `Reach`, `DisabledReason`, `Slice`, `RingSuggestion`, `suggest`, `MAX_SLICES = 12`. |
| `ring.rs` | Pure donut geometry (`Point`, `Hit`, `span`, `mid_angle`, `hit_test(_with_slack)`, `wedge_path`, `ring_path`, `GAP_PX`, `POP_OUT_PX`). Only its own tests use it today. The app draws with `app/ui/ring.math.js`, a JS mirror of this module that has its own tests. |
| `testing.rs` | `FakeConverter` (builder: `new(id).edge(from,to,cost).display_name().max_parallelism(n).missing(hint).behaviour(b)`), `Behaviour::{Copy, Fail, Panic, Sleep, Outputs(n)}`, `FakeCounters { calls(), max_concurrent() }`. Compiled under `cfg(any(test, feature = "test-support"))`. |

### crates/engines/src

| Module | Contents |
|---|---|
| `registry.rs` | `all() -> Vec<Box<dyn Converter>>` returns `[MagickEngine, FfmpegEngine, SofficeEngine]` in priority order. `statuses()` serves `doctor`. |
| `discovery.rs` | `Tool::{Magick, Ffmpeg, Soffice}` (`id`, `display_name`, `programs`), `Found { program, version }` + `companion("ffprobe")`, `find` (a process-wide `OnceLock<Mutex<HashMap>>` cache), `locate`, `status`, `forget`, `locate_in(tool, PATH, extra)` (pure), `system_folders`, `parse_version`, `install_hint(tool, os_release)` (Arch/Debian/download), `brew_hint`, `install_hint_here`. FFmpeg only counts as found when `ffprobe` sits in the same folder. |
| `process.rs` | `run(program, args, RunOptions, &CancelToken) -> Result<RunOutput, RunError>`. `RunOptions { timeout, cwd, env_remove, on_stdout_line }`, `RunOutput { stdout, stderr_tail }` (last 40 lines), `RunError::{Cancelled, TimedOut, Spawn, Failed{code, tail}}`. Kills the whole process group on cancel or timeout (Unix via `libc`). Also `summarize(tail)` and `move_into_place(from, to)` (rename, falling back to copy + remove). |
| `magick/mod.rs` + `magick/args.rs` | `MagickEngine` (id `magick`, parallelism 0 = one per core, 120 s timeout). `args.rs`: `READABLE` (14 rasters incl. `psd`, `raw`), `WRITABLE` (12), const `EDGES` (each readable to each writable, self edges included, plus each readable to `pdf`; still to `gif` costs 12), `FramePolicy`/`frame_policy`, `quality_value` (92/85/70), `args`, `identify_args`, `largest_frame`. |
| `ffmpeg/mod.rs` + `encoding.rs` + `probe.rs` | `FfmpegEngine` (id `ffmpeg`, parallelism 2, 30 min per pass, 60 s probe). `encoding.rs`: `VIDEO_FORMATS` (mp4 webm mkv mov avi), `AUDIO_FORMATS` (mp3 m4a wav flac ogg opus), static `EDGES` (video to video/audio, video to gif at cost 15, video to png poster frame, audio to audio, gif to mp4/webm), `plan(...) -> Vec<FfmpegPass { args, weight }>` (GIF uses two passes, 0.3 + 0.7), `video_args`, `audio_args`, `gif_args`, `poster_args`, `can_copy_video`. `probe.rs`: `MediaInfo`, `VideoStream`, `AudioStream`, `probe_args`, `parse_media_info`, `parse_progress_line`. |
| `soffice/mod.rs` + `filters.rs` | `SofficeEngine` (id `soffice`, `WORKERS = 2`, 180 s timeout, `new_in(data_dir)` for tests). It keeps a `ProfilePool` with one LibreOffice user profile per worker under `<XDG data>/morphring/libreoffice-profiles/worker-N`. `filters.rs`: format families (`WRITER`, `CALC`, `IMPRESS`, import-only lists, `DRAW`), `Family`, `family_of`, `filter_for`, `edges()` (`FALLBACK_COST = 25`, `TEXT_TO_DOCUMENT_COST = 28`, `SLIDES_TO_PDF_COST = 0`, no self edges), `profile_dir`, `args`. Success is judged by the output file, not the exit code. |

### crates/cli/src

| Module | Contents |
|---|---|
| `main.rs` | Sets up logging (stderr, `warn` by default, `RUST_LOG`, and `--verbose` raises `morphring*` crates to debug) and Ctrl+C (`Presses`: the first press cancels the token, the second exits with 130), then calls `morphring::run`. |
| `lib.rs` | `run(args, out, err, cancel) -> i32` reads the cwd and builds `Ctx` with `registry::all()`. `run_with(args, &mut Ctx) -> i32` is the entry point for tests. `parse` answers help and version before engines are looked up. `dispatch` matches on `Command`. Constants `VERSION`, `EXIT_USAGE = 2`. |
| `ctx.rs` | `Ctx { out: &mut dyn Write, err: &mut dyn Write, cwd, cancel, engines: Vec<Box<dyn Converter>> }`. Commands read nothing from the environment except through `Ctx` (or `Host` for integrate). |
| `args.rs` | clap tree: `Cli { version, verbose, command }`, `Command::{Convert, Formats, Doctor, Integrate, Help, Version}`, `ConvertArgs { inputs, to, output, recursive, quality (1..=100), overwrite, skip, json, quiet }`, `FormatsArgs`, `DoctorArgs`, `IntegrateArgs`, `HelpArgs`. Help texts are hand-written consts (`GENERAL_HELP`, `CONVERT_HELP`, ...) plus `help_text(cmd)`. The field names are a contract. |
| `inputs.rs` | `expand(inputs, recursive, cwd, is_known) -> Expanded { files, missing }`. Handles files, folders (known formats only, sorted) and wildcards (`*`, `?`, `[`). A literal file name wins over a wildcard reading of it. Deduplication is order-preserving (`IndexSet`). |
| `report.rs` | Human lines (`file_line`, `summary`, `Counts`), `message(ErrorKey)`, `JsonReport`/`JsonFile` + `write_json` (Filee key order). |
| `commands/convert.rs` | `run(&ConvertArgs, &mut Ctx) -> i32`. Exit codes: 0, 1 (some failed), 2 (usage), 3 (nothing to do). |
| `commands/formats.rs` | Lists the registry by category, or `formats <FMT>`: targets from two planners (installed, then all engines), with hints for missing engines. |
| `commands/doctor.rs` | Prints engine status rows plus `Folders { config, data, temp }`, human or `--json`. `run_in` takes injected folders. |
| `commands/integrate.rs` | `Action::{Install, Uninstall}`, `Host { home, release_build, app }` (`Host::from_process()` is called only in `lib.rs`), `APP_BINARY = "morphring-app"`, `--exe`, `--dry-run`, `--allow` (required outside release builds). Calls `morphring_platform::context_menu`. |

### crates/platform/src

`context_menu.rs`: `FileManager::{Dolphin, Nautilus, Nemo, Thunar}`, generators (`dolphin_service_menu`, `nautilus_script`, `nemo_action`, `thunar_sendto`), `menu_files(exe, home)`, `install(exe, home, dry_run) -> Vec<MenuFile>`, `uninstall(home, dry_run) -> Vec<PathBuf>`, `is_managed`, `MANAGED_MARKER = "X-MorphRing-Managed=true"`, `IntegrationError`. Writes are atomic. Only files that carry the marker are replaced or removed. Each entry runs `<app> --convert %F`.

### app/

| Path | Contents |
|---|---|
| `src-tauri/src/main.rs` | Logging, then `morphring_app::run()`. |
| `src-tauri/src/lib.rs` | `run()`: the single-instance plugin first, then `tauri-plugin-opener`, `invoke_handler![commands::convert, commands::ring_close, commands::ping]`, a setup hook (`tray::setup`, `instance::on_startup`) and `on_window_event(dropzone::on_window_event)`. This is the only place where plugins, commands and hooks are registered. |
| `src-tauri/src/commands.rs` | Event names `RING_OPEN = "ring-open"`, `RING_PROGRESS = "ring-progress"`, `RING_DONE = "ring-done"`, window `RING_WINDOW = "ring"`. Payloads `RingModel { files, title, slices: Vec<RingSlice { target, label, enabled, disabled_reason }> }`, `RingProgress { progress }`, `RingDone { outcome, message }`, `Outcome::{Succeeded, Failed, Cancelled}`, `ConvertSummary { outcome, outputs, done, failed, message }`. Pure `ring_model(files, engines)`. `Conversions` (managed state, created lazily by `conversions(app)`) has one worker thread that runs jobs FIFO: `new(engines, EventSink)`, `ring_model`, `submit`, `async convert`. Events from a job the ring no longer shows are dropped (`showing` id). Also `open_ring(app, files)`. |
| `src-tauri/src/instance.rs` | `CommandLine::parse(args, cwd)` (`--background`, `--settings`, `--quit`, `--convert <files>`), `Launch::of(cmd, first_launch)`, `Debouncer` (350 ms, the clock is injected), `on_startup`, `on_second_instance`, managed state `Pending`. |
| `src-tauri/src/dropzone.rs` | `accept_drop(paths) -> Accepted` keeps existing regular files with no duplicates. `on_window_event` emits `"files-dropped"` to `main` and calls `commands::open_ring`. |
| `src-tauri/src/tray.rs` | `TrayAction::{Open, Quit}`, `setup`, `show_main` (recreates `main` if it was closed). |
| `src-tauri/tauri.conf.json` | Windows `main` (index.html, drag-drop enabled) and `ring` (ring.html, 360x360, hidden, transparent, undecorated, always on top). `withGlobalTauri: true`, a strict CSP and `frontendDist: ../ui`. Bundles: deb and AppImage. |
| `src-tauri/capabilities/default.json` | `core:default`, `opener:default` for both windows. |
| `ui/` | No bundler and no framework; scripts use `window.__TAURI__`. `index.html` + `main.js` + `style.css` make the drop zone. `ring.html` + `ring.js` (DOM, animation, backend contract documented at the top) + `ring.math.js` (pure geometry, a UMD-style `window.RingMath`) + `ring.css` make the ring. `ring.test.mjs` runs with `node --test`. |
| `scripts/icons.sh` | Regenerates the icons from `src-tauri/icons/icon.svg`. |

Other: `scripts/make-fixtures.sh` regenerates `crates/engines/tests/fixtures` (needs magick, ffmpeg and soffice; the output is committed).

## 3. The contract (`morphring_core::contract`, `error`)

```rust
pub struct FormatId(&'static str);              // new(), as_str(), Display
pub struct Edge { pub from: FormatId, pub to: FormatId, pub cost: u32 }  // new() = cost 10, with_cost()
pub struct ConvertOptions { pub quality: Option<u8> }   // quality_level() -> QualityLevel
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
    fn convert(&self, step: &Step, out: &mut dyn OutputAllocator,
               progress: &dyn Fn(f32), cancel: &CancelToken) -> Result<Vec<PathBuf>, EngineError>;
}
pub enum FileState { Done, Skipped, Failed { key: ErrorKey, detail: String }, Cancelled }
pub struct FileResult { pub source: PathBuf, pub state: FileState, pub outputs: Vec<PathBuf> }
pub enum JobState { Completed, CompletedWithErrors, Failed, Cancelled }  // from_files(&[FileResult], cancelled)

pub enum ErrorKey { UnsupportedSource, NoRoute, EngineMissing, ConversionFailed } // as_str(): "error.*"
pub enum EngineError { Cancelled, NotInstalled { tool, hint }, Failed(String), Io(io::Error) } // key()
```

Engine obligations: never modify `step.input`. Get every output path from `out.allocate`, where `None` means skip (return `Ok(vec![])`). Write into a temp dir under `step.work_dir` first, then `process::move_into_place`. Check `cancel`. Report `progress` values that only rise. A multi-output engine allocates with suffixes `_p1`, `_p2`, ... and an empty `ext` asks for a folder.

## 4. How a conversion flows

### CLI: `morphring convert a.png b/ --to jpg -o out -q 80`

1. `main.rs`: logging, then a `CancelToken` wired to Ctrl+C, then `morphring::run(args, stdout, stderr, cancel)`.
2. `lib.rs::parse`: clap parsing. Help, version and usage errors exit here (code 0 or 2) without touching engines. Otherwise `Ctx { engines: registry::all(), cwd, ... }` is built and `dispatch` calls `commands::convert::run`.
3. `convert::run`:
   - `formats::lookup_target(&args.to)`. The target must exist and be `writable`, otherwise exit 2.
   - `inputs::expand(...)`: if any input is missing, exit 2; if nothing is left to convert, exit 3.
   - `output_rule(args, cwd)` gives `-o` as `CustomFolder { keep_folders_from: None }`, otherwise `SameFolder`. The conflict policy is Rename, or Overwrite/Skip from the flags. The pattern is `{name}`.
   - `ConvertOptions { quality }`.
   - Spawns a scoped thread that runs `job::run_job(...)`. `FileFinished` events go over an mpsc channel to the calling thread, which prints `report::file_line` in finishing order (suppressed by `--json`, and `--quiet` prints only failures).
   - Then prints the JSON report or `report::summary`, and exits 0 or 1.
4. `core::job::run_job(files, target, &options, &rule, &engines, &cancel, &on_event) -> Vec<FileResult>`:
   - Builds `Planner::new(engines, false)` (installed engines) and a "dream" `Planner::new(engines, true)` (every engine), one `Gate` per engine id (`max_parallelism`, where 0 means the core count), shared `Reservations`, and the work dir `$TMPDIR/morphring/job-XXXX/`.
   - Runs `min(cores, files)` scoped worker threads pulling indices from an `AtomicUsize`.
   - Per file (`convert_file`): stop if cancelled, then `formats::detect` (none gives `UnsupportedSource`), then `planner.plan(from, target)`. With no route, the dream planner decides between `EngineMissing` ("needs X: hint") and `NoRoute`.
   - `run_route`: for each hop, intermediate hops use `TempAllocator` (`<work>/<random>/<name><suffix>.<ext>`) and the last hop uses `FinalAllocator` (wrapping `JobAllocator` with `Tokens { source, preset: target name, index, now }`). The gate permit is held only for one step. Fan-out: every output of a hop is fed to the next hop. File progress is `(hop + p) / hops`, clamped so it only rises.
   - Panics are caught per file and per event handler. On failure or cancel, final outputs this file created are deleted. `Finished([])` becomes `Skipped`.
   - The results come back in input order. The work dir is removed on drop.
5. Engine `convert` (for example `MagickEngine`): `discovery::locate`, then optionally `identify` for frames, then `out.allocate`, then a tempdir in `work_dir`, then `process::run` (with cwd set to the tempdir), then validation of the output, then `move_into_place` to the allocated path.

### App: drop, ring, click, convert

1. Entry points: a drop on `main` (`dropzone::on_window_event` calls `accept_drop`, emits `files-dropped`, then calls `commands::open_ring`), or a launch with `morphring-app --convert files...` (`instance::on_startup` / `on_second_instance`, then `CommandLine::parse` and `Launch::of`, then the 350 ms `Debouncer`, then `commands::open_ring` on the main thread).
2. `commands::open_ring` (on a blocking thread) calls `conversions(app).ring_model(files)`. That runs `core::slices::suggest(files, &default_favourites(), reach)`, where `reach` uses an installed planner and an every-engine planner and returns `Ready`, `NeedsEngine{engine, hint}` or `NoRoute`. The result is a `RingModel`, emitted as `ring-open` to the `ring` window, which is then centred, shown and focused. It is centred because Wayland does not report the cursor position.
3. `ring.js`: a slice click invokes `convert { files, to }`.
4. `commands::convert` calls `Conversions::convert`, which calls `submit` (validates the target and sends a `Request` to the worker) and waits for the reply on `spawn_blocking`.
5. `Worker::run_job` calls `core::job::run_job` with `ConvertOptions::default()` and `OutputRule::default()`, so outputs go next to the source with Rename. It gets a fresh `CancelToken`, and the UI has no way to fire it yet. `Progress` averages per-file progress and emits `ring-progress` in steps of at least 1%. At the end it emits `ring-done` and replies with a `ConvertSummary`.
6. `ring_close` hides the ring. A running job keeps going.

## 5. Route planning details

- Edge cost: `edge.cost + min(engine_index, 9)` (priority penalty, so registry order breaks ties). Parallel edges for the same `from -> to` collapse to the cheapest.
- `plan(from, to)`: if `from == to`, only an explicit self edge counts. Otherwise the search walks every simple path of 3 hops or fewer and keeps the cheapest; on a tie, fewer hops win, then registration order. The search is deterministic.
- `targets_from(from)` gives `(FormatId, cost)` sorted by cost, then by the order the target first appears among edges. `formats <FMT>` uses it.
- Hand-tuned costs and the routes they exist for are pinned in `crates/engines/tests/routes.rs`: mp4 to jpg goes through png, still to gif costs 12, video to gif costs 15, pptx to pdf costs 0, the soffice fallback costs 25 (+2 priority), there is no docx to png route in v0.1.0, and there is no txt to mp3 route.

## 6. Tests

| Where | What | Helpers |
|---|---|---|
| `#[cfg(test)] mod tests` in every module | Pure logic: formats, planner (including a comparison against Filee's Dijkstra), output resolver (16-thread uniqueness), job runner (parallelism caps, cancel, panic isolation, fan-out), slices, ring geometry, arg builders, discovery with fake tool scripts, process runner (Unix process-group kill). | `core::testing::FakeConverter`. `job.rs` has a local `FnConverter`. |
| `crates/engines/tests/{magick,ffmpeg,soffice}.rs` | Golden tests that run the real tools on fixtures and check format, size or duration, not bytes. | `tests/common/mod.rs`: `require_engine!(tool)` (prints `SKIP:` and returns), `engine_available`, `fixture(name)`, `image_size`, `frame_count`, `duration_secs`. Each test file defines its own small `OutputAllocator`. |
| `crates/engines/tests/routes.rs` | Route costs over `registry::all()` with `assume_all_available = true`. Starts no programs. | |
| `crates/engines/tests/fixtures_check.rs` + `fixtures/` | Fixtures: sample.{png,jpg,gif,wav,mp4,txt,docx,odt}, notes.unknownext. Each is under 100 KB. | |
| `crates/cli/tests/{cli,convert}.rs` | End to end in-process through `run_with` with real or fake engines. Help and version run the built binary with an empty `PATH`. | `tests/common/mod.rs`: `run_in(cwd, engines, args) -> Outcome { code, out, err }`, `run_with_cancel`, `copy_fixture`, `fixture`, `require_engine!`. |
| `app/src-tauri/tests/commands.rs` | `Conversions` and `ring_model` with real engines, without a window. | Local `require_tool!`, `fixture`, recorder `EventSink`. |
| `app/src-tauri/tests/config.rs` | Checks `tauri.conf.json` windows and the files it points at. | |
| `app/ui/ring.test.mjs` | `node --test app/ui/ring.test.mjs` for `ring.math.js`. | |

Rules (ENGINEERING.md section 8): no `std::env::set_var` in tests. Inject the cwd, engines, home folder and clock instead. A missing engine skips a test and never fails it.

## 7. CI and release

`.github/workflows/ci.yml` runs on every push and PR, with concurrency cancelling older runs per ref:

| Job | Runs | Blocking |
|---|---|---|
| `fmt` | `cargo fmt --all --check` | yes |
| `clippy` | `cargo clippy --workspace --all-targets --locked -- -D warnings` | yes |
| `test (ubuntu/windows/macos)` | `cargo test --workspace --locked`. Installs ffmpeg on Ubuntu only, so the magick and soffice goldens skip in CI. | Ubuntu only; Windows and macOS are `continue-on-error` |
| `deny-licenses` | `cargo-deny check licenses bans sources` (`deny.toml`) | yes |
| `deny-advisories` | `cargo-deny check advisories` | no |
| `app` | Installs libwebkit2gtk-4.1-dev, then `cargo check --locked --manifest-path app/src-tauri/Cargo.toml` | no. App tests and clippy do not run in CI. |

`.github/workflows/release.yml` runs on a `v*` tag. Its single `linux-cli` job on ubuntu-22.04 runs the tests (`-p morphring-core -p morphring-engines -p morphring`), does a release build, smoke-tests `--version` (which must match the tag) and a wav to mp3 conversion, packages a tarball with `SHA256SUMS.txt`, and creates a **draft** GitHub release from the tag's `CHANGELOG.md` section. The release does not build the app or anything for Windows or macOS.

## 8. Extension points

### A new engine
1. Add a module in `crates/engines/src/<name>/` with a struct that implements `Converter`. Put pure argument building and edges in a submodule such as `args.rs` and unit-test it there.
2. For an external tool, add a `Tool` variant in `discovery.rs` (`id`, `display_name`, `programs`, `version_args`, version regex, `install_hint`/`brew_hint`/`download_hint` arms) and call `discovery::status`/`locate`. Run the tool through `process::run` with a timeout and the cancel token. A native Rust engine returns `EngineStatus::Available` directly; per PLAN v0.2 those go behind cargo features.
3. Add `pub mod` in `engines/src/lib.rs` and insert the engine into `registry::all()`. **The position is the priority**: earlier engines win ties, and PLAN says native engines go ahead of magick. Update `engines_are_in_filee_priority_order` and `routes.rs` (penalties shift when the order changes).
4. Any new format id must exist in `core/src/formats.rs` `FORMATS` (one line, and bump the array length). Pin every hand-tuned cost in `tests/routes.rs`.
5. Add golden tests in `crates/engines/tests/<name>.rs` with `mod common;` and `require_engine!`. Add fixtures via `scripts/make-fixtures.sh` and list them in `fixtures_check.rs`.
6. The CLI (`doctor`, `formats`, `convert`) and the app pick the engine up automatically through `registry::all()`. `doctor` rows come from `ctx.engines`.

### A new CLI command
1. Add a variant to `args::Command` and an `XxxArgs` struct, a `XXX_HELP` const, and update `help_text` and `GENERAL_HELP`.
2. Create `commands/xxx.rs` exposing `pub fn run(&XxxArgs, &mut Ctx) -> i32` and add it to `commands/mod.rs`.
3. Add a match arm in `lib.rs::dispatch`. Read the environment only in `lib.rs`, in the style of `integrate::Host::from_process`, and pass it in.
4. Exit codes: 0 ok, 1 some failed, 2 usage (`EXIT_USAGE`), 3 nothing to do.
5. Write tests through `tests/common::run_in` with `FakeConverter` or real engines.

### A new setting or preset field
No settings store exists yet. `doctor` only prints where it will live (`ProjectDirs` config dir, `~/.config/morphring`). ENGINEERING.md section 7 fixes the format: camelCase JSON, `#[serde(default)]`, `schemaVersion: 1`, atomic writes, a `.broken` copy on corruption, and it must work with no config dir. Today's knobs are:
- Conversion options: `ConvertOptions` (only `quality`). New per-conversion options go here and are mapped inside each engine.
- Output placement: `OutputRule { location, pattern, conflict }`. The CLI builds it in `convert::output_rule`, and the app hard-codes `OutputRule::default()` in `commands::Worker::run_job`.
- Ring contents: `slices::default_favourites()` (replaced by presets in v0.2, D2). The app builds the ring in `commands::ring_model`.
- Engine cost toggles, such as Filee's "prefer LibreOffice", are commented in `soffice/filters.rs::edges()`. Edges are currently static, so a toggle needs either engine construction with config or a planner-level override.

A settings module most naturally lives in `morphring-core` (shared by the CLI and the app). Deserialization needs `serde` derive, and `serde` is already a dependency of core. The CLI then loads it in `lib.rs::run` into `Ctx`, and the app keeps it in Tauri managed state.

### A new app page or window, Tauri command or event
1. A window is a `windows[]` entry in `tauri.conf.json` with its HTML in `app/ui/` (plain JS, `window.__TAURI__`, CSP `script-src 'self'`, so no inline scripts). Add its label to `capabilities/default.json` `windows` and extend `tests/config.rs`.
2. For a command, write a `#[tauri::command]` fn in a module, keep testable logic in plain functions or structs (as `Conversions` does), and register it in the single `generate_handler!` in `lib.rs`. New plugins are also registered only in `lib.rs`.
3. For events, define a `pub const NAME: &str` next to its payload struct (`#[serde(rename_all = "camelCase")]`) and `emit_to(window_label, NAME, payload)`. Document it in the contract comment at the top of the consuming JS file.
4. Shared state uses the lazy `try_state`/`manage` pattern (see `commands::conversions`, `instance::enqueue`).
5. Tray entries go in `TrayAction` (`ALL`, `id`, `label`, `from_id`) in `tray.rs`.

### Desktop integration
Add new file managers or OS integrations in `morphring-platform` (pure generators plus `install`/`uninstall` taking `home`, with the managed marker), and wire them through `cli/commands/integrate.rs`. Windows and macOS backends (PLAN v0.4) belong here behind `cfg(target_os)`.

## 9. Known gaps visible in the code

- The app cannot cancel a job: each job gets a new `CancelToken` that nothing fires, and the Escape key in `ring.js` only hides the ring.
- The app has no output settings or quality setting (it uses the defaults).
- `core::ring` geometry is duplicated in `app/ui/ring.math.js`, and only the JS copy is used at runtime. The two are kept in sync by their tests.
- The `app` CI job only runs `cargo check`. App tests, app clippy and `node --test` do not run in CI.
- `rust-version = "1.85"`, but `crates/cli/src/inputs.rs` uses `if let ... && let` chains (stable since 1.88). `app/src-tauri/src/instance.rs` deliberately avoids them for the 1.85 promise. The CI uses stable, so the gap goes undetected.
- `JobEvent::FileStarted` is emitted but no caller uses it. The CLI only listens to `FileFinished`.
