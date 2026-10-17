# MorphRing Engineering Guide

These are the rules for how MorphRing is built. They are opinionated on purpose. If a rule gets in the way of shipping the current release (v0.2), raise it and change this file in its own commit. Don't quietly ignore it.

Background reading: `docs/reverse-engineering/` (what Filee does) and `docs/research/` (what Rust offers). Filee paths below are relative to the Filee repo root. `docs/PLAN.md` holds the roadmap and the decisions D1-D12 that refined this file; where this file cites a "Dn", that is where the reasoning lives. `docs/PLAN-v0.2.md` section 2 holds the v0.2 decisions, and `docs/ARCHITECTURE.md` describes the code as it is.

## 1. Principles

1. **YAGNI for the current release.** Build what the release needs and nothing more. No plugin system, no async runtime in the core, no config option for a value nobody has asked to change. Write a TODO pointing at an issue rather than an abstraction "for later".
2. **Pure core, dirty edges.** Format detection, route planning, output naming, ring geometry and gesture logic are pure functions that are easy to test. Processes, file systems, windows and clocks are passed in from outside. Filee works the same way: `Filee.Core` never launches a tool. Everything external sits behind `IConverter` (`src/Filee.Core/Conversion/IConverter.cs`).
3. **Port behaviour, not structure.** Copy Filee's rules (edge costs, naming, exit codes, settle times) exactly, with tests. Write the code as idiomatic Rust, not as translated C#. Where Filee has a known bug, fix it and add a test (for example the Overwrite reservation bug, and the inexact hop-capped Dijkstra).
4. **Never lose user data.** We never overwrite the source. We write to a temp file and rename it into place. We delete partial output when a step fails or is cancelled. Drops are always treated as copies.
5. **Offline and private.** No telemetry, no analytics, no crash upload, no network access in a default build. The engine downloader (the `download` cargo feature) is off by default, and a future update check will be opt-in; those are the only network calls we plan to allow.

## 2. Workspace layout

```
morphring/
  Cargo.toml            # workspace, resolver 3, edition 2024, rust-version 1.93, shared [workspace.dependencies], exclude = ["app"]
  Cargo.lock            # committed: we ship a binary
  rust-toolchain.toml   # channel stable, rustfmt + clippy
  .cargo/config.toml    # Windows: static C runtime (+crt-static) for both workspaces
  crates/
    core/               # morphring-core: formats, Converter trait, options, presets, profiles, settings, history, planner, output paths, job runner, watch settling, ring geometry
    engines/            # morphring-engines: native image/svg/pdf/archive (cargo features), magick, poppler, ffmpeg, pandoc, ghostscript, soffice, process runner, tool discovery
    cli/                # morphring (binary): clap parsing, presets, input expansion, watch, human/JSON output, exit codes
    platform/           # morphring-platform: menu entries for Linux file managers, Explorer and Finder; the gesture state machine. No Tauri.
  app/src-tauri/        # Tauri v2 desktop app: ring, drop zone, Ctrl+drag (Linux), settings/history/toast windows, tray. Thin glue only. Its own cargo workspace.
```

Rules:

- `members` and `default-members` are `core`, `engines`, `cli` and `platform`. `app/src-tauri` is **its own cargo workspace** (the root `exclude` lists `app`), with its own `Cargo.lock`, and it depends on `morphring-core` and `morphring-engines` by path (D1). So `cargo build`, `cargo test`, `cargo clippy --workspace` and `cargo deny` never build webkit2gtk or check Tauri's licence tree. The app is built separately (`cargo tauri build`). A broken GUI build must never block a CLI release.
- Dependencies only point downward: `cli`/`app` -> `engines` -> `core`, and `platform` -> `core`. `core` depends on no other MorphRing crate, and nothing below `app` depends on Tauri.
- Native converters (`image`, `resvg`, `krilla`/`lopdf`, `zip`/`tar`) live in `engines` behind cargo features (`img`, `svg`, `pdf`, `archive`, all on by default), using the layout in `docs/research/rust-conversion-ecosystem.md` and `docs/research/v02-engines-and-crates.md`. External tools need no feature: they are found at runtime. Every dependency is declared once in the root `[workspace.dependencies]` by a skeleton task, so feature tasks never touch a manifest.
- Don't add a new crate until a module grows past roughly 2k lines or needs a dependency that others shouldn't pull in.

### The dev machine (checked 2026-10-15)

Arch Linux, desktop **Hyprland on Wayland**. Installed: `ffmpeg` n9.0.2 (with `ffprobe`), `magick` 7.1.2-32 Q16-HDRI, `soffice` 26.2.6.3, `pandoc`, `gs` (Ghostscript), `pdftoppm`, `bsdtar` and `webkit2gtk-4.1` 2.52.6. Missing: calibre, and the dev tools `cargo-deny`, `shellcheck` and `actionlint` (tasks use their stated fallbacks; CI runs `cargo deny` either way). Check with `command -v` before deciding a tool is missing, and never skip work on a guess.

On Wayland an app can't read the global cursor position or place a window at it. The ring opens centred, except for Ctrl+drag on Hyprland, where the app asks Hyprland's IPC socket for the cursor and moves the ring under it. The Ctrl+drag watcher reads `/dev/input` through evdev, so it needs the user in the `input` group; for testing, `MORPHRING_NO_GESTURE=1` turns it off and `MORPHRING_GESTURE_ANYWHERE=1` drops the "only from a file manager" check (ARCHITECTURE.md section 4.3).

## 3. Engines and route planning

### The Converter trait (in `core`)

```rust
pub struct Edge { pub from: FormatId, pub to: FormatId, pub cost: u32, pub flags: EdgeFlags } // cost defaults to 10

pub trait Converter: Send + Sync {
    fn id(&self) -> &'static str;               // "magick", "ffmpeg", "soffice"
    fn display_name(&self) -> &'static str;     // "ImageMagick", "FFmpeg", "LibreOffice" (used in install hints)
    fn edges(&self) -> &[Edge];                 // direct conversions; X->X self edges allowed
    fn max_parallelism(&self) -> usize { 0 }    // 0 = available_parallelism()
    fn status(&self) -> EngineStatus;           // cheap and cached: Available { path, version } | Missing { hint }
    fn accepts(&self, options: &ConvertOptions) -> bool { true } // false = sit this job out
    fn merger(&self) -> Option<&dyn Merger> { None }             // plus combiner() and tiff_merger()
    fn convert(
        &self,
        step: &Step,                            // input path, from, to, options, work dir
        out: &mut dyn OutputAllocator,          // the only way to get an output path
        progress: &dyn Fn(f32),                 // 0.0..=1.0, only goes up
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError>;     // the files actually written
}
```

Contract (taken from Filee and not negotiable):

- An engine **never picks its own output path.** It calls `out.allocate(ext, suffix)`. `None` means "skip this output" (the Skip policy), not an error. An empty `ext` means a folder. Multi-output steps use the suffix `_p{n}`, starting at 1.
- An engine never modifies its input, must be thread-safe and must check `cancel`.
- A missing tool means `status()` returns `Missing { hint: "pacman -S ghostscript" }` and the planner leaves its edges out. It is never a crash. When no route exists with the installed engines but one exists with all engines, the file fails with `error.engine_missing` and a detail naming the program and its hint, such as `needs LibreOffice: pacman -S libreoffice-fresh` (D10).
- An engine that can't honour a preset's options returns `false` from `accepts`, and the job plans without it. It never ignores an option silently (the native image engine benches itself this way and ImageMagick takes over).
- **ImageMagick never reads PDF, PS or EPS** (that needs Ghostscript and a permissive policy.xml). PDF input goes to Poppler (`pdftoppm`, `pdftotext`), the PostScript family and the PDF fallback to Ghostscript, and the built-in `pdf` engine does page ranges, splitting and merging (D4).

### Route planning

- The graph is built **once per job** from available engines only.
- Step cost is `edge.cost + min(priority_index, 9)`, so engine priority only breaks ties (`src/Filee.Core/Conversion/RoutePlanner.cs`).
- Routes have **at most 3 hops**. With a graph this small, enumerate every path of up to 3 hops and take the cheapest. That is exact (Filee's hop-capped Dijkstra isn't) and has no clever bugs. Ties go to the first path found in registration order, so results are deterministic.
- Same-format targets (resize, re-encode) need an explicit self edge.
- The registry order is the priority and is set once per release: image, resvg, pdf, magick, poppler, archive, ffmpeg, pandoc, ghostscript, soffice (native engines first, LibreOffice last). Changing it shifts every penalty after the change.
- Hand-tuned costs are copied from Filee and kept next to their engine: built-in 8-10, svg<->svgz 5, still->gif 12, video->gif 15, png->video 11, pandoc 10-14, calibre 20, LibreOffice fallback 25. Every cost has a test that names the route it is meant to produce, in that engine's own `routes_<engine>.rs` (shared pins stay in `routes.rs`).
- Edge flags keep image tracing to routes that start from a raster image, and keep vector-only edges (pdf->svg through LibreOffice) away from them.

### Running external tools

All external tools go through one `ProcessRunner` in `engines`. No other production code calls `Command` directly. Test helpers may (for example to run `magick identify` or `ffprobe` on a golden output), since checking results needs the tools themselves (D6).

- Pass an argument list (`.args()`) and never use a shell.
- Read stdout and stderr on their own threads to avoid pipe deadlocks. Keep the last 40 stderr lines for error messages.
- Kill the whole process tree on cancel, timeout or stall, because `soffice` forks `soffice.bin`. On Unix each tool starts in its own process group (`process_group(0)`) and we `killpg` it; on Windows see section 9.1.
- `RunOptions` takes a `timeout` and a `stall` watchdog (kill when the tool prints nothing for that long), both optional, plus cancel (D8). Timeouts: soffice 180 s, magick, poppler, ghostscript and pandoc 120 s, version probes 5 s, and a generous 30 min per ffmpeg pass. FFmpeg follows Filee's full remux fit matrix with hvc1 tagging. Tools that report no progress get an estimate from `progress::Estimate`.
- Judge success by the result: a non-empty output file must exist. Exit codes from soffice, pandoc and rhwp are unreliable.
- Write output into a fresh temp subdir, then move it into place. Fall back to copy and remove when `rename` crosses filesystems, since `/tmp` is tmpfs on Arch.
- Argument builders (for example `ffmpeg_args(...) -> Vec<String>`) are pure functions with unit tests that pin the exact argument list.

## 4. Concurrency model

The core and engines are **synchronous: std threads, no tokio in `core` or `engines`.** The work is almost all waiting on child processes, and threads keep cancel and kill logic easy to follow.

- **Jobs** run one at a time (FIFO). The CLI runs one job (`watch` runs one per settled file). The app keeps a single worker thread with a channel queue, and every job has a `jobId` and its own `CancelToken`, so the ring can cancel it.
- **Files inside a job** run in parallel with `std::thread::scope`, capped at `available_parallelism()`.
- **Per-engine limit:** one counting semaphore per engine id (a `Mutex<usize>` plus a `Condvar`, about 30 lines), **held around a single step, not the whole file**. Limits: magick = cores, ffmpeg = 2, soffice = 2 (each worker gets its own `-env:UserInstallation` profile, or runs fail silently).
- **Output reservations:** one `Mutex<HashSet<PathBuf>>` per job, held across resolve, exists-check and insert. A reserved name always gets a number, **under every policy, Overwrite included**.
- **Failure isolation:** each file catches its own errors and ends as `Done | Skipped | Failed { key, detail } | Cancelled`. The job state is computed from the file states using Filee's rules. One bad file never fails the job.
- **Cancel:** a `CancelToken` (an `Arc<AtomicBool>`). In the CLI the first Ctrl+C cancels cooperatively (kill children, delete partial output, exit 1) and the second calls `exit(130)`.
- **Merge mode** (Merge PDF) converts the non-PDF inputs in parallel under the same gates, then hands them to one engine's `Merger` in input order.
- **Tauri:** commands wait for the worker on `spawn_blocking`, and progress leaves the worker as events, in order. Nothing blocks the UI thread.

## 5. Errors

- **Libraries** (`core`, `engines`, `platform`) use `thiserror` enums. Each variant maps to a stable key that the CLI and UI show, plus a human detail. There are four keys: `error.unsupported_source`, `error.no_route`, `error.engine_missing` (D10: a route exists, but only through an engine that isn't installed) and `error.conversion_failed`.
- **Binaries** (`cli`, `app`) use `anyhow` with `.context("while converting {path}")`.
- No `unwrap()` or `expect()` outside tests, except for true invariants, and then `expect("why this cannot fail")`.
- Errors shown to users say what happened and what to do next ("ffmpeg not found: install it with `pacman -S ffmpeg`").

## 6. Logging

- Use `tracing` with `tracing-subscriber`. Logs go to stderr and default to `warn`. `RUST_LOG` or `--verbose` raise the level. `--verbose` is long-only, because `-v` means `--version` (Filee parity, D5).
- Log every external command line at `debug`. That is how we debug engines.
- stdout belongs to command output (`--json` especially). Never log to stdout.
- The app logs to stderr the same way today. A log file under `$XDG_STATE_HOME/morphring/`, capped and rotated at 1 MB, is planned. Logs never leave the machine.

## 7. Config and data locations

Use the `directories` crate (`ProjectDirs::from("", "", "morphring")`):

| What | Linux | Notes |
|---|---|---|
| `settings.json`, `presets.json`, `profiles.json`, `history.json` | `$XDG_CONFIG_HOME/morphring/` (`~/.config/morphring`) | shared by CLI and app; on Windows and macOS the same `ProjectDirs` config dir (`morphring doctor` prints it) |
| LibreOffice worker profiles | `$XDG_DATA_HOME/morphring/libreoffice-profiles/worker-N` | |
| Logs | `$XDG_STATE_HOME/morphring/` | app only |
| Job temp dirs | `$TMPDIR/morphring/<job-id>/` | deleted when the job ends |

- JSON is camelCase (`#[serde(rename_all = "camelCase")]`), with `#[serde(default)]` on every struct and a `schemaVersion` field from day one (starting at 1).
- Writes are atomic (`NamedTempFile::new_in(dir)`, then `sync_all`, then `persist`). If a file is corrupt, copy it to `<name>.broken`, log a warning and use defaults. Never crash on bad config.
- The config file is optional. MorphRing must run with no config dir at all.

## 8. Testing

- **Unit tests** sit next to the code (`#[cfg(test)] mod tests`). They are required for all pure logic: format registry (every extension unique, compound extensions like `tar.gz` matched first), planner routes and costs, output path resolver (port Filee's 10 `OutputPathResolverTests`), argument builders, page ranges, ring geometry, gesture detector.
- **Golden-file integration tests** live in `crates/*/tests/` with small fixtures in `tests/fixtures/` (each under 100 KB, and we must have the rights to ship it). They run real conversions into a `tempfile::TempDir` and check the result's format, dimensions or duration, not its bytes, since engine versions change bytes.
- **They skip when an engine is missing.** Use a `require_engine!("ffmpeg")` macro that prints `SKIP: ffmpeg not found` and returns. A missing engine must never fail CI, and a present one must never be skipped.
- **CI golden tests run every engine but LibreOffice on all three OSes.** Ubuntu's apt `imagemagick` is 6.x with no `magick`, so Linux CI installs ImageMagick 7 from a pinned, checksummed AppImage, and a follow-up step fails the job if any magick golden printed `SKIP:`. LibreOffice is too slow to install for the main job, so its goldens run in the separate, non-blocking `goldens-office` job (supersedes D7). Run the soffice goldens on the dev machine before tagging anyway.
- **Tests never call `std::env::set_var`** (D9). It is `unsafe` in edition 2024 and races between parallel test threads. Pass values like cwd, engine lists, home folders and allow flags in as parameters (the CLI's `Ctx`), and put environment changes on a child `Command` with `.env()`.
- **CLI tests** call `cli::run(args, &mut out, &mut err, cancel) -> i32` in-process with byte buffers, following Filee's `CliTests`. They check exit codes 0/1/2/3, the human lines and the `--json` shape.
- Inject the clock (`now: impl Fn() -> Instant`) wherever time matters (watch-folder settle, debounce, toast timing), so tests never sleep.
- Tests that touch paths compare them in a form that holds on Windows too (no hard-coded `/`, no assumptions about `\\?\` prefixes or drive letters). The suite must pass on all three OSes.
- A bug fix lands with a test that fails without it.

## 9. Code style

- `cargo fmt` with the default config. `cargo clippy --all-targets -- -D warnings` must pass. A narrow `#[allow(...)]` with a reason comment is fine. A crate-wide allow is not.
- **Reproducible builds** (D12): `rust-version = "1.93"` in both workspaces, `rust-toolchain.toml` on stable, and committed lockfiles. CI and release cargo calls use `--locked`. The MSRV was the edition 2024 minimum for v0.1, became 1.92 for v0.2 (krilla and krilla-svg need it) and 1.93 for the parity dependencies (sevenz-rust2). The blocking `msrv` CI job checks the root workspace with exactly that toolchain, so raising it is always a deliberate commit.
- Prefer plain data and functions over traits. Add a trait only when there are two implementations today (Converter yes, `Platform` yes, `FileSystem` no).
- Keep public APIs small. Use `pub(crate)` by default.
- Use `PathBuf`/`OsString` everywhere and convert to strings only for display, so Hangul, spaces and quotes in names survive. Use `std::path::absolute`, not `canonicalize`.
- Detect formats by our own `extension_of` (longest compound match, case-insensitive), never by `Path::extension()`.
- **Licences:** only MIT, Apache-2.0 (also WITH LLVM-exception), BSD-2/3-Clause, ISC, Zlib, MPL-2.0, Unicode, CC0-1.0 (`notify`), CDLA-Permissive-2.0 (`webpki-roots`, for the opt-in downloader) or bzip2-1.0.6 (`libbz2-rs-sys`) crates may be linked, enforced by `cargo deny` with `deny.toml`. A new licence goes into `deny.toml` with a comment saying which crate needs it. GPL/AGPL tools (GPL ffmpeg builds, Poppler, Ghostscript, pandoc, calibre) run as separate processes only and are never bundled. Credit Filee (MIT) in the README and THIRD-PARTY-NOTICES, and add every new linked crate family and external tool to THIRD-PARTY-NOTICES.

### 9.1 Windows and macOS rules

Windows and macOS are only reachable through GitHub Actions, so these rules exist to keep CI the only place we learn about them:

- **Compile everywhere.** Platform code sits behind `cfg(windows)`, `cfg(target_os = "macos")` or `cfg(target_os = "linux")`, and platform-only crates are target dependencies (`windows-sys`, `winreg`, `mslnk`, `evdev`, ...). Pure generators (registry entries, plists, menu files) compile on every OS so Linux CI type-checks them.
- **Kill trees with a Job Object.** Every child is assigned to a Job Object right after spawn, and cancel, timeout or stall calls `TerminateJobObject`, so `soffice.bin` and other grandchildren die with the tool. All the Win32 FFI lives in `win_job`, the one module with an `unsafe` allow.
- **Use `dunce`, not raw `canonicalize` output.** Paths shown to users or handed to tools go through `dunce::simplified`, so they never carry the `\\?\` verbatim prefix that many tools reject.
- **Retry locked renames.** `move_into_place` retries a rename that failed with `ERROR_SHARING_VIOLATION` or `ERROR_ACCESS_DENIED` three times, 100 ms apart (antivirus and the search indexer like to sniff fresh files), before falling back to copy and remove. The retry is Windows-only: on Unix those numbers are EIO and EPIPE.
- **Discovery knows each OS:** `gswin64c`/`gswin32c` for Ghostscript, `soffice.com` before `soffice`, Program Files, `/opt/homebrew/bin` and LibreOffice.app, with winget, scoop or brew install hints.
- The C runtime is linked statically on Windows (`.cargo/config.toml`), so a fresh PC needs no Visual C++ Redistributable. Release builds of the app use the Windows GUI subsystem, so no console pops up.
- The ring window on Windows re-asserts topmost on every show and never takes the focus; on macOS a transparent ring needs `macos-private-api` (PLAN-v0.2.md section 2).

### Comments: surani's voice

Code comments are written as **surani**, first person, with light humor. The joke is the garnish; the information is the meal. Every comment must still say *why*, which a reader can't get from the code itself.

```rust
// soffice forks soffice.bin and then wanders off like a cat. Kill the whole
// process group, or we leak a headless office suite per cancelled job. Ask me how I know.
let _ = killpg(pgid, Signal::SIGKILL);

// Filee caps routes at 3 hops. I tried 4 once; it found a way to turn a PNG into
// an MP3 via a PDF. Technically correct, spiritually wrong.
const MAX_HOPS: usize = 3;
```

Rules: jokes live in code comments only. Never in CLI output, `--json`, error messages, `///` docs or README command blocks and their shown output. Nothing mean about people, other projects or users. If the humor makes a comment unclear, drop the humor.

## 10. Commits

- Use [Conventional Commits](https://www.conventionalcommits.org/): `feat(core): add route planner`, `fix(engines): kill soffice process group on cancel`, `docs: add engineering guide`, `test(cli): port exit-code tests`, `chore(ci): add clippy job`. Scopes are crate names, plus `ci`, `docs` and `app`.
- **One logical change per commit.** Each commit builds and passes `fmt`, `clippy` and `test` on its own. Don't mix refactoring with behaviour changes.
- **One worktree per developer.** Each task gets its own `git worktree` and branch off `main`, with its own `target/`. The developer runs the `-p <crate>` checks for the crates they touched (`cargo test -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt --all --check`). After each merge the orchestrator runs the full `--workspace --locked` fmt, clippy and test on `main`. A red merge is reverted, not patched on `main`.
- The subject is imperative and at most 72 characters. The body explains why and mentions the Filee file being ported when there is one.
- Release tags are `vMAJOR.MINOR.PATCH`. `CHANGELOG.md` is updated in the release commit, and its notes become the GitHub release text.

## 11. CI

`.github/workflows/ci.yml`, on every push and PR. Every cargo call uses `--locked` (D12). **Since v0.2 every OS blocks** (XP-14, superseding D11): a red Windows or macOS run is a red run. Only `goldens-office` and `deny-advisories` may fail.

| Job | Runners | Command | Blocking |
|---|---|---|---|
| fmt | ubuntu-latest | `cargo fmt --all --check` | yes |
| clippy | ubuntu-latest | `cargo clippy --workspace --all-targets --locked -- -D warnings` (the app is excluded, D1) | yes |
| test | ubuntu-latest, windows-latest, macos-latest | `cargo test --workspace --locked --no-fail-fast`, with engines installed per OS, then the magick goldens again on Linux, red if any skipped | yes |
| goldens-office | ubuntu-latest | installs LibreOffice, runs the soffice goldens and the CLI convert tests | no: LibreOffice is slow and its start-up flaky on runners |
| msrv | ubuntu-latest | reads `rust-version` from `Cargo.toml`, then `cargo +<ver> check --workspace --locked --all-targets` | yes |
| deny-licenses | ubuntu-latest | `cargo deny check licenses bans sources` | yes |
| deny-advisories | ubuntu-latest | `cargo deny check advisories` | no: a RUSTSEC notice published this afternoon must not turn a tag red |
| app | ubuntu-latest, windows-latest, macos-latest | app clippy with `-D warnings` and app tests via `--manifest-path app/src-tauri/Cargo.toml`, then `node --test` over the UI tests (WebKitGTK installed on Linux) | yes |

- Engines in the test job: Linux gets ffmpeg, poppler-utils, ghostscript and pandoc from apt and **ImageMagick 7** as the AppImage from a pinned ImageMagick GitHub release with its SHA-256 (apt only has 6.x, and imagemagick.org keeps one build only). macOS gets ffmpeg, imagemagick, poppler, ghostscript and pandoc from Homebrew. Windows gets ffmpeg and pandoc from Chocolatey, ImageMagick from the runner image, and Ghostscript unpacked from its installer by 7-Zip (its installers hang on runners). Every step prints the tool versions.
- Use `Swatinem/rust-cache` for caching. Timeouts are 15 minutes per job (25 on Windows); if a job creeps towards them, trim it.
- `release.yml` runs on tags `v*` (and by `workflow_dispatch` on a branch, without the release). Build jobs per OS: `linux` (tests, the x86_64 CLI tarball, the app as .deb and .AppImage), `windows-cli` (zip), `windows-app` (NSIS current-user installer, with `app/src-tauri/windows/hooks.nsh` checked against `crates/platform/src/explorer.rs`), `macos-cli` (arm64 tarball, ad-hoc signed) and `macos-app` (ad-hoc signed .dmg). Each smoke-tests what it builds (`morphring --version` must match the tag, one wav to mp3 conversion). The Tauri CLI is pinned once, as `TAURI_CLI_VERSION`, and checked against the tauri crate in the app's lockfile. Then `publish` needs every build job, writes one `SHA256SUMS.txt` and creates a **draft** release from the tag's CHANGELOG section. Nobody publishes or tags from a task; the orchestrator does. See `docs/research/packaging-integration-ops.md`.

## 12. v0.2 scope guard

v0.2 is "presets and the real app", with Windows and macOS pulled forward from v0.4 (PLAN.md section 9, `docs/PLAN-v0.2.md` section 4). The Filee parity tasks of `docs/PLAN-v0.2-parity.md` extend it engine by engine; each of them still follows these rules.

In:

- `morphring-core`: Filee's built-in presets (94 today, Filee v1.11.0's set), `Preset` with per-family options, toolbar profiles and the profile selector, the settings, preset library and history stores (atomic, `.broken` copies; settings.json at schemaVersion 2, the others at 1), the default output location, `--keep-dates`, page ranges, the watch-folder settler, `Target::SameAsSource`, `accepts`, merge, combine and multi-page TIFF modes, rename steps in output rules, retry plans, ring slices from presets.
- `morphring-engines`: native image, SVG, PDF (krilla, krilla-svg, lopdf) and archive engines behind features; Poppler, Ghostscript and Pandoc engines; the stall watchdog, estimated progress, the FFmpeg remux matrix and `MediaOptions`, ImageMagick `ImageOptions`; Windows Job Objects and rename retries; discovery and hints for Windows and macOS.
- CLI: `convert --preset`, `--keep-dates`, `presets`, `watch`, and `integrate` for Linux file managers, Explorer (verb plus Send To) and Finder (Quick Action).
- App: ring from presets with the preset's options and the default output, cancel from the ring, settings page, history view, toast for long jobs, Ctrl+drag on Linux (evdev, ring under the cursor on Hyprland), the ink, cream and tomato palette, Windows topmost fixes, the macOS transparent ring, tray mode and Finder's Opened event.
- Release: all-OS blocking CI with engines, the multi-OS draft release (CLI for three OSes, .deb, .AppImage, NSIS, .dmg), the AUR `morphring-bin` PKGBUILD (not published), README and CHANGELOG with a "not yet verified on real hardware" list.

Out until later (PLAN-v0.2.md section 5 has the reasons): the Windows 11 top-level menu, MSI, code signing and notarization, the Homebrew tap, the engine downloader turned on by default, the pure-Rust PDF renderers on by default, gesture backends for Windows and macOS and for Wayland compositors other than Hyprland, the update check, the engines that would offer combine-into-one archive and multi-page TIFF (the job modes are in), read-only archives through bsdtar, preset, profile and watch-folder editors in the app, engine priority settings, universal or Intel macOS and Windows ARM64 builds. If it isn't in the "In" list or a parity task, it waits.
