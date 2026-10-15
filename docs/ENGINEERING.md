# MorphRing Engineering Guide

These are the rules for how MorphRing is built. They are opinionated on purpose. If a rule gets in the way of shipping v0.1.0 today, raise it and change this file in its own commit. Don't quietly ignore it.

Background reading: `docs/reverse-engineering/` (what Filee does) and `docs/research/` (what Rust offers). Filee paths below are relative to the Filee repo root.

## 1. Principles

1. **YAGNI for v0.1.0.** Build what the release needs and nothing more. No plugin system, no async runtime in the core, no config option for a value nobody has asked to change. Write a TODO pointing at an issue rather than an abstraction "for later".
2. **Pure core, dirty edges.** Format detection, route planning, output naming, ring geometry and gesture logic are pure functions that are easy to test. Processes, file systems, windows and clocks are passed in from outside. Filee works the same way: `Filee.Core` never launches a tool. Everything external sits behind `IConverter` (`src/Filee.Core/Conversion/IConverter.cs`).
3. **Port behaviour, not structure.** Copy Filee's rules (edge costs, naming, exit codes, settle times) exactly, with tests. Write the code as idiomatic Rust, not as translated C#. Where Filee has a known bug, fix it and add a test (for example the Overwrite reservation bug, and the inexact hop-capped Dijkstra).
4. **Never lose user data.** We never overwrite the source. We write to a temp file and rename it into place. We delete partial output when a step fails or is cancelled. Drops are always treated as copies.
5. **Offline and private.** No telemetry, no analytics, no crash upload, no network access at all in v0.1.0. A future update check will be opt-in, and it is the only network call we plan to allow.

## 2. Workspace layout

```
morphring/
  Cargo.toml            # workspace, resolver 3, edition 2024, shared [workspace.dependencies]
  crates/
    core/               # morphring-core: formats, Converter trait, planner, output paths, job runner, settings, ring geometry
    engines/            # morphring-engines: magick, ffmpeg, soffice (+ native ones behind cargo features), process runner, tool discovery
    cli/                # morphring (binary): clap parsing, input expansion, human/JSON output, exit codes
    platform/           # morphring-platform: context-menu files, XDG paths, reveal-in-folder, gesture state machine. No Tauri.
  app/src-tauri/        # Tauri v2 desktop app: ring window, drop zone, tray. Thin glue only.
```

Rules:

- `default-members` is `core`, `engines`, `cli` and `platform`, so `cargo build` and `cargo test` never need webkit2gtk. The app is built separately (`cargo tauri build`). A broken GUI build must never block a CLI release.
- Dependencies only point downward: `cli`/`app` -> `engines` -> `core`, and `platform` -> `core`. `core` depends on no other MorphRing crate, and nothing below `app` depends on Tauri.
- Native converters (`image`, `resvg`, `zip`/`tar`, `calamine`) live in `engines` behind cargo features, using the layout in `docs/research/rust-conversion-ecosystem.md`. External tools need no feature: they are found at runtime.
- Don't add a new crate until a module grows past roughly 2k lines or needs a dependency that others shouldn't pull in.

## 3. Engines and route planning

### The Converter trait (in `core`)

```rust
pub struct Edge { pub from: FormatId, pub to: FormatId, pub cost: u32 } // cost defaults to 10

pub trait Converter: Send + Sync {
    fn id(&self) -> &'static str;               // "magick", "ffmpeg", "soffice"
    fn edges(&self) -> &[Edge];                 // direct conversions; X->X self edges allowed
    fn max_parallelism(&self) -> usize { 0 }    // 0 = available_parallelism()
    fn status(&self) -> EngineStatus;           // cheap and cached: Available { path, version } | Missing { hint }
    fn convert(
        &self,
        step: &Step,                            // input path, from, to, preset
        out: &mut dyn OutputAllocator,          // the only way to get an output path
        progress: &dyn Fn(f32),                 // 0.0..=1.0, only goes up
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError>;     // the files actually written
}
```

Contract (taken from Filee and not negotiable):

- An engine **never picks its own output path.** It calls `out.allocate(ext, suffix)`. `None` means "skip this output" (the Skip policy), not an error. An empty `ext` means a folder. Multi-output steps use the suffix `_p{n}`, starting at 1.
- An engine never modifies its input, must be thread-safe and must check `cancel`.
- A missing tool means `status()` returns `Missing { hint: "pacman -S ghostscript" }` and the planner leaves its edges out. It is never a crash.

### Route planning

- The graph is built **once per job** from available engines only.
- Step cost is `edge.cost + min(priority_index, 9)`, so engine priority only breaks ties (`src/Filee.Core/Conversion/RoutePlanner.cs`).
- Routes have **at most 3 hops**. With a graph this small, enumerate every path of up to 3 hops and take the cheapest. That is exact (Filee's hop-capped Dijkstra isn't) and has no clever bugs. Ties go to the first path found in registration order, so results are deterministic.
- Same-format targets (resize, re-encode) need an explicit self edge.
- Hand-tuned costs are copied from Filee and kept in one place: built-in 8-10, still->gif 12, video->gif 15, png->video 11, pandoc 10-14, calibre 20, LibreOffice fallback 25. Every cost has a test that names the route it is meant to produce.

### Running external tools

All external tools go through one `ProcessRunner` in `engines`. Nothing else calls `Command` directly.

- Pass an argument list (`.args()`) and never use a shell.
- Read stdout and stderr on their own threads to avoid pipe deadlocks. Keep the last 40 stderr lines for error messages.
- Start each tool in its own process group (`process_group(0)`) and kill the whole group on cancel or timeout, because `soffice` forks `soffice.bin`.
- Two modes: a hard timeout (soffice 180 s), or a stall watchdog (ffmpeg: no progress for 10 min).
- Judge success by the result: a non-empty output file must exist. Exit codes from soffice, pandoc and rhwp are unreliable.
- Write output into a fresh temp subdir, then move it into place. Fall back to copy and remove when `rename` crosses filesystems, since `/tmp` is tmpfs on Arch.
- Argument builders (for example `ffmpeg_args(...) -> Vec<String>`) are pure functions with unit tests that pin the exact argument list.

## 4. Concurrency model

v0.1.0 is **synchronous: std threads, no tokio in `core` or `engines`.** The work is almost all waiting on child processes, and threads keep cancel and kill logic easy to follow.

- **Jobs** run one at a time (FIFO). The CLI runs one job. The app keeps a single worker thread with a channel queue.
- **Files inside a job** run in parallel with `std::thread::scope`, capped at `available_parallelism()`.
- **Per-engine limit:** one counting semaphore per engine id (a `Mutex<usize>` plus a `Condvar`, about 30 lines), **held around a single step, not the whole file**. Limits: magick = cores, ffmpeg = 2, soffice = 2 (each worker gets its own `-env:UserInstallation` profile, or runs fail silently).
- **Output reservations:** one `Mutex<HashSet<PathBuf>>` per job, held across resolve, exists-check and insert. A reserved name always gets a number, **under every policy, Overwrite included**.
- **Failure isolation:** each file catches its own errors and ends as `Done | Skipped | Failed { key, detail } | Cancelled`. The job state is computed from the file states using Filee's rules. One bad file never fails the job.
- **Cancel:** a `CancelToken` (an `Arc<AtomicBool>`). In the CLI the first Ctrl+C cancels cooperatively (kill children, delete partial output, exit 1) and the second calls `exit(130)`.
- **Tauri:** the app calls the job runner from `spawn_blocking` and forwards progress over a channel. Nothing blocks the UI thread.

## 5. Errors

- **Libraries** (`core`, `engines`, `platform`) use `thiserror` enums. Each variant maps to a stable key that the CLI and UI show (`error.unsupported_source`, `error.no_route`, `error.conversion_failed`) plus a human detail.
- **Binaries** (`cli`, `app`) use `anyhow` with `.context("while converting {path}")`.
- No `unwrap()` or `expect()` outside tests, except for true invariants, and then `expect("why this cannot fail")`.
- Errors shown to users say what happened and what to do next ("ffmpeg not found: install it with `pacman -S ffmpeg`").

## 6. Logging

- Use `tracing` with `tracing-subscriber`. Logs go to stderr and default to `warn`. `RUST_LOG` or `-v`/`--verbose` raise the level.
- Log every external command line at `debug`. That is how we debug engines.
- stdout belongs to command output (`--json` especially). Never log to stdout.
- The app also writes to `$XDG_STATE_HOME/morphring/morphring.log`, capped and rotated at 1 MB. Logs never leave the machine.

## 7. Config and data locations

Use the `directories` crate (`ProjectDirs::from("", "", "morphring")`):

| What | Linux | Notes |
|---|---|---|
| `settings.json`, `presets.json` | `$XDG_CONFIG_HOME/morphring/` (`~/.config/morphring`) | shared by CLI and app |
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
- **CLI tests** call `cli::run(args, &mut out, &mut err, cancel) -> i32` in-process with byte buffers, following Filee's `CliTests`. They check exit codes 0/1/2/3, the human lines and the `--json` shape.
- Inject the clock (`now: impl Fn() -> Instant`) wherever time matters (watch-folder settle, debounce), so tests never sleep.
- A bug fix lands with a test that fails without it.

## 9. Code style

- `cargo fmt` with the default config. `cargo clippy --all-targets -- -D warnings` must pass. A narrow `#[allow(...)]` with a reason comment is fine. A crate-wide allow is not.
- Prefer plain data and functions over traits. Add a trait only when there are two implementations today (Converter yes, `Platform` yes, `FileSystem` no).
- Keep public APIs small. Use `pub(crate)` by default.
- Use `PathBuf`/`OsString` everywhere and convert to strings only for display, so Hangul, spaces and quotes in names survive. Use `std::path::absolute`, not `canonicalize`.
- Detect formats by our own `extension_of` (longest compound match, case-insensitive), never by `Path::extension()`.
- **Licences:** only MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0 or Unicode crates may be linked, enforced by `cargo deny`. GPL/AGPL tools (GPL ffmpeg builds, Ghostscript, pandoc, calibre) run as separate processes only and are never bundled. Credit Filee (MIT) in the README and THIRD-PARTY-NOTICES.

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

Rules: no jokes in error messages, user-facing strings or doc comments of public APIs (keep `///` docs straight). Nothing mean about people, other projects or users. If the humor makes a comment unclear, drop the humor.

## 10. Commits

- Use [Conventional Commits](https://www.conventionalcommits.org/): `feat(core): add route planner`, `fix(engines): kill soffice process group on cancel`, `docs: add engineering guide`, `test(cli): port exit-code tests`, `chore(ci): add clippy job`. Scopes are crate names, plus `ci`, `docs` and `app`.
- **One logical change per commit.** Each commit builds and passes `fmt`, `clippy` and `test` on its own. Don't mix refactoring with behaviour changes.
- The subject is imperative and at most 72 characters. The body explains why and mentions the Filee file being ported when there is one.
- Release tags are `vMAJOR.MINOR.PATCH`. `CHANGELOG.md` is updated in the release commit, and its notes become the GitHub release text.

## 11. CI

`.github/workflows/ci.yml`, on every push and PR:

| Job | Runners | Command |
|---|---|---|
| fmt | ubuntu-latest | `cargo fmt --all --check` |
| clippy | ubuntu-latest | `cargo clippy --workspace --all-targets -- -D warnings` (default members, no Tauri) |
| test | ubuntu-latest, windows-latest, macos-latest | `cargo test --workspace` |
| deny | ubuntu-latest | `cargo deny check licenses advisories` |

- Ubuntu installs `ffmpeg imagemagick` so the golden tests actually run on at least one OS. Elsewhere they skip.
- Use `Swatinem/rust-cache` for caching. CI must finish in under 10 minutes or we trim it.
- The Tauri app build is a separate, non-blocking job until it's stable.
- `release.yml` runs on tags `v*`: it builds the Linux CLI tarball plus `SHA256SUMS.txt` (and Tauri `.deb`/`.AppImage` on ubuntu-22.04 when they build), smoke-tests `morphring --version` and one real conversion, and creates a **draft** release. See `docs/research/packaging-integration-ops.md`.

## 12. v0.1.0 scope guard

In: format registry, Converter trait plus magick/ffmpeg/soffice, planner, output resolver, parallel job runner with cancel, `morphring convert|formats|help|version`, `--json` and exit codes, `morphring doctor`, CI. Stretch: Tauri drop zone with a click-mode ring, `morphring integrate` (Dolphin first).

Out until later: live gestures, watch folders, engine downloads, presets UI, Windows/macOS context menus, updater, fonts, CAD, e-books. If it isn't in the "In" list, it waits for v0.2.
