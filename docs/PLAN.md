# MorphRing Plan

This is the roadmap from v0.1.0 (today, 2026-10-15) to Filee parity, plus the task list for v0.1.0. The task list is written for developer agents that work **in parallel, each in its own git worktree**, one commit per task.

Read first: `docs/ENGINEERING.md` (the rules), `docs/reverse-engineering/01..07` (what Filee does), `docs/research/` (what Rust and Linux give us). Filee paths are relative to the Filee repo root.

**Today's plan is the critical path only.** v0.1.0 is the Linux CLI with three engines. Everything that does not sit on the path to the `v0.1.0` tag (the desktop app, ring geometry, context menus) moved to v0.1.1 (section 7), so no reviewer or tester spends time on it before the tag.

## 1. How to use this plan

- **Waves.** Every task depends only on tasks in earlier waves, and tasks in the same wave **own disjoint files**. When a wave has merged, start every task of the next wave at once.
- **One worktree per developer.** Each task gets its own `git worktree` branched from `main` after the previous wave merged, with its own `target/` directory. A half-written file in one task can then never break another task's build, and nobody waits on someone else's cargo lock.
- **Checks per task, then per merge.** A developer runs only `-p <crate>` checks (`cargo test -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt --all --check`). After merging each task, the orchestrator runs the full `--workspace --locked` fmt, clippy and test on `main`. A red merge is reverted, not patched on `main`.
- **One task, one commit** (Conventional Commits, ENGINEERING.md section 10), small enough for one developer in under an hour. The orchestrator makes the commit with the task's message and the attribution trailer. Developers never commit, tag or push.
- **Owned files only.** A task touches only the files in its "Owns" list. If it needs anything else, it stops and tells the orchestrator.
- **Cargo.toml, Cargo.lock and rust-toolchain.toml belong to T01.** T01 declares every v0.1.0 dependency up front (section 5.1). If a later task really needs another crate, the orchestrator adds it in a tiny `chore(deps)` commit before that wave starts.
- **Shared files exist before anyone needs them.** T01 creates every module file with its `pub mod` lines already in place (section 5.2), and gives every stub that another task calls its real signature (section 5.3). No task ever edits a `mod.rs` or `lib.rs` that another task in its wave also needs.
- **Never mutate the environment in tests** (D9). `std::env::set_var` is `unsafe` in edition 2024 and races between test threads. Pass values in as parameters, or set them on a child process with `Command::env`.
- **Humour rule** (from the user): code comments are written as surani, with light humour. Jokes never appear in CLI output, `--json`, error messages, `///` docs or README command blocks.

## 2. Starting point

- `morphring/` holds only `docs/` and no commits yet. The scaffold at `scratchpad/scaffold-v0` (about 425 lines) is **reference only**. Its `Format` enum and engine trait don't match ENGINEERING.md (no `OutputAllocator`, no edge costs, no status), so T01 starts fresh. Reusing its `.gitignore` is fine.
- **Machine state, corrected and checked today** (the brief was out of date): `ffmpeg` n9.0.2, `magick` 7.1.2-32 Q16-HDRI, `soffice` 26.2.6.3, `pandoc`, `gs` (Ghostscript), `pdftoppm`, `bsdtar` and `webkit2gtk-4.1` 2.52.6 **are installed**. Only calibre is missing. `cargo-deny`, `shellcheck` and `actionlint` are **not** installed, so tasks use their stated fallbacks. The desktop is **Hyprland on Wayland**, where an app cannot read the global cursor position or place a window at it. No agent should skip work because it thinks a tool is missing; check with `command -v` first.
- Optional before wave 2: the orchestrator may run `cargo install cargo-deny --locked` so T19's licence check can run locally. CI runs it either way.

## 3. Decisions made in this plan

These refine ENGINEERING.md. T03 writes them into ENGINEERING.md in its own commit, as that file asks.

| # | Decision | Why |
|---|---|---|
| D1 | `app/src-tauri` is **its own cargo workspace**, listed in the root `exclude`, with its own `Cargo.lock`. It depends on `morphring-core` and `morphring-engines` by path. | `cargo clippy --workspace` and `cargo deny` would otherwise build and check Tauri. Excluding it keeps the CLI release free of WebKit and of Tauri's licence tree. |
| D2 | In v0.1.1 the ring's slices are **target formats** from a favourites list (T26), not Filee's preset profiles. | Presets (68 built-ins, profiles, ProfileSelector) are v0.2. The ring still works, and presets slot in later without a UI change. |
| D3 | No `--preset` in v0.1.0. `convert` requires `--to`. | Same reason as D2. |
| D4 | **No PDF/PS/EPS input** in v0.1.0. ImageMagick never reads PDF. | ImageMagick needs Ghostscript and a permissive policy for that. A pdftoppm/Ghostscript engine lands in v0.2 (docs/reverse-engineering/02, "Do not route PDF/EPS through ImageMagick"). |
| D5 | `-v` means `--version` (Filee parity). Verbose logging is `--verbose`, long only. | ENGINEERING.md section 6 said `-v`/`--verbose`, which clashes with Filee's `-v`. |
| D6 | Only **production** code must go through the process runner. Test helpers may call `std::process::Command` (to run `magick identify`, `ffprobe`). | Checking golden outputs needs the tools themselves. |
| D7 | CI golden tests cover **ffmpeg only**. magick and soffice golden tests skip in CI and run on the dev machine before the tag. | Ubuntu's `imagemagick` is 6.x with no `magick` binary, and installing LibreOffice would blow the CI time budget. ImageMagick 7 in CI is v0.2. |
| D8 | The process runner has **two modes: a timeout or none**, plus cancel. No stall watchdog. FFmpeg remux only for h264/yuv420p into mp4, mkv or mov. | Filee's progress-stall watchdog, hvc1 tagging and full remux matrix are more than a first release needs, and would make T24 the bottleneck of wave 3. They move to v0.2. FFmpeg gets a generous 30-minute timeout instead. |
| D9 | **Tests never call `std::env::set_var`.** Values like cwd, engine lists, home folders and allow flags are parameters (`Ctx`); env changes go on a child `Command`. | `set_var` is `unsafe` in edition 2024 and races between parallel test threads. |
| D10 | A fourth stable error key, **`error.engine_missing`**. When no route exists with installed engines but one exists with all engines, the file fails with "A required program is not installed" and a detail like `needs LibreOffice: pacman -S libreoffice-fresh`. | Without it, `docx → pdf` on a machine without soffice says only "No engine can do this conversion", which tells the user nothing. |
| D11 | **Linux blocks, other OSes inform.** Windows and macOS CI test jobs are `continue-on-error` for v0.1.0. `cargo deny check licenses bans sources` blocks; `check advisories` does not. | A Linux-first tag must not wait on untested platforms or on a RUSTSEC notice published this afternoon. |
| D12 | **Reproducible builds:** a `rust-version` (the edition 2024 minimum for v0.1; raised to 1.92 and then 1.93 in v0.2, see ENGINEERING.md section 9), `rust-toolchain.toml` on stable, committed `Cargo.lock`, `--locked` in every CI and release cargo call. | We ship a binary. |

## 4. v0.1.0 scope

**In (ships today):**

- `morphring-core`: format registry (all 171 Filee entries), `Converter` contract, exact 3-hop route planner, output path resolver (Filee's Overwrite bug fixed), parallel job runner with per-engine limits, cancel and missing-engine hints (D10).
- `morphring-engines`: process runner with process-group kill, tool discovery with install hints, ImageMagick, FFmpeg and LibreOffice engines.
- CLI `morphring`: `convert` (`--to`, `-o`, `-r`, `-q/--quality`, `--overwrite`, `--skip`, `--json`, `--quiet`), `formats [<fmt>]`, `doctor`, `help`, `version`, Filee's exit codes 0/1/2/3, two-stage Ctrl+C.
- Release: README, CHANGELOG, LICENSE, THIRD-PARTY-NOTICES, CI, draft-release workflow, tag `v0.1.0`, Linux x86_64 CLI tarball with `SHA256SUMS.txt`.

**v0.1.1 (after the tag, section 7):** Tauri app with drop zone and click-mode ring (centred on Wayland), tray, single instance, `--convert <files>`, `morphring integrate install|uninstall` (Dolphin first).

**Out (later releases, section 9):** presets and `--preset`, settings file, `--keep-dates`, watch folders, history, PDF input, page ranges, native Rust engines, pandoc and calibre, live gestures, cursor-placed ring, Windows and macOS integration, engine downloads, updater, fonts, CAD, e-books, i18n beyond English.

## 5. Shared foundations created by T01

### 5.1 Dependencies declared up front

Root `[workspace.dependencies]` (caret majors; T01 resolves and commits the lock):

| Crate | Used by | For |
|---|---|---|
| `thiserror` 2 | core, engines | library errors |
| `anyhow` 1 | cli | binary errors |
| `tracing` 0.1, `tracing-subscriber` 0.3 (`env-filter`) | all / cli | logs to stderr |
| `serde` 1 (`derive`), `serde_json` 1 | core, engines, cli | ffprobe JSON, `--json` |
| `tempfile` 3 | core, engines; dev-dep of cli | work dirs, temp outdirs, test dirs |
| `chrono` 0.4 (`clock` only) | core | `{date}` / `{time}` tokens |
| `which` 8 | engines; dev-dep of cli | PATH lookup, test skips |
| `regex` 1 | engines | version parsing |
| `url` 2 | engines | LibreOffice profile `file://` URI |
| `directories` 6 | engines, cli | XDG data dir for profiles, doctor's folder list |
| `libc` 0.2 (unix only) | engines | `killpg` |
| `clap` 4 (`derive`) | cli | arguments |
| `ctrlc` 3 (`termination`) | cli | two-stage Ctrl+C |
| `glob` 0.3, `walkdir` 2, `indexmap` 2, `pathdiff` 0.2 | cli | input expansion and display |

Features and dev-dependencies, also fixed by T01 so nobody edits a manifest later:

- `morphring-core` has a feature `test-support` (no extra deps) that exposes `core::testing`.
- `morphring-engines` and `morphring` (cli) list `morphring-core = { path = "../core", features = ["test-support"] }` under `[dev-dependencies]`.
- `crates/platform` exists with no dependencies; it is filled in v0.1.1.
- `[workspace.lints]`: clippy `unwrap_used`/`expect_used` = warn (errors under `-D warnings`), rust `unsafe_code` = deny. `clippy.toml`: `allow-unwrap-in-tests = true`, `allow-expect-in-tests = true`.

The app's dependencies (`tauri` 2, `tauri-build` 2, `tauri-plugin-single-instance` 2, `tauri-plugin-opener` 2) live only in `app/src-tauri/Cargo.toml`, created in v0.1.1 by T21.

### 5.2 Module map and owners

| File | Created by | Filled by |
|---|---|---|
| `crates/core/src/contract.rs`, `error.rs`, `testing.rs` | T01 | T01 |
| `crates/core/src/formats.rs` | T01 stub | T05 |
| `crates/core/src/planner.rs` | T01 stub | T06 |
| `crates/core/src/output.rs` | T01 stub | T07 |
| `crates/core/src/job.rs` | T01 stub | T22 |
| `crates/engines/src/process.rs` | T01 stub | T09 |
| `crates/engines/src/discovery.rs` | T01 stub | T10 |
| `crates/engines/src/magick/mod.rs` (has `pub mod args;`), `magick/args.rs` | T01 | T23, T11 |
| `crates/engines/src/ffmpeg/mod.rs` (has `pub mod encoding; pub mod probe;`), `encoding.rs`, `probe.rs` | T01 | T24, T12, T13 |
| `crates/engines/src/soffice/mod.rs` (has `pub mod filters;`), `soffice/filters.rs` | T01 | T25, T14 |
| `crates/engines/src/registry.rs` (stub with real signatures) | T01 | T30 |
| `crates/cli/src/ctx.rs` | T01 | T01 (never edited after) |
| `crates/cli/src/args.rs`, `lib.rs`, `main.rs` | T01 | T15 |
| `crates/cli/src/commands/mod.rs` (has `pub mod convert; pub mod doctor; pub mod formats;`) | T01 | T01 (never edited in v0.1.0) |
| `crates/cli/src/commands/convert.rs`, `formats.rs`, `doctor.rs` (stubs with real signatures) | T01 | T32, T33, T34 |
| `crates/cli/src/inputs.rs`, `report.rs` | T01 stub | T16, T17 |
| `crates/cli/tests/common/mod.rs`, `tests/convert.rs` | T32 | T32 |
| `crates/cli/tests/cli.rs` | T36 | T36 |
| `crates/engines/tests/common/mod.rs`, `tests/fixtures/**` | T04 | T04 |
| `crates/platform/src/lib.rs` (doc comment only) | T01 | v0.1.1 |

Fixture paths from the CLI crate: `concat!(env!("CARGO_MANIFEST_DIR"), "/../engines/tests/fixtures/")`. The engines crate's `tests/common` is not visible from `crates/cli`, so T32 writes the CLI's own small `require_engine!`.

### 5.3 The contract

T01 writes the types every wave-2 task codes against, so nobody waits for anybody.

**`crates/core/src/contract.rs`**

- `FormatId(&'static str)` with `const fn new`. Engines name formats by id string (`FormatId::new("png")`), so they never wait for the registry.
- `Edge { from, to, cost }` (cost 10 by default), `Step { input, from, to, options, work_dir }`, `ConvertOptions { quality: Option<u8> }`, `QualityLevel { High, Normal, Small }` with `from_percent` (85 and up High, 60 to 84 Normal, below 60 Small, `None` = Normal).
- `trait OutputAllocator { fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf>; }`
- `EngineStatus { Available { path, version } | Missing { hint } }`, `CancelToken(Arc<AtomicBool>)`, and the `Converter` trait exactly as in ENGINEERING.md section 3, plus `fn display_name(&self) -> &'static str` (used in the D10 hint).
- `FileState { Done, Skipped, Failed { key, detail }, Cancelled }`, `FileResult { source, state, outputs }`, `JobState::from_files` (Filee's rule: Cancelled if cancel fired, Completed if all Done/Skipped, CompletedWithErrors if any Done, else Failed; `src/Filee.Core/Conversion/JobQueue.cs`).

**`crates/core/src/error.rs`:** `ErrorKey { UnsupportedSource, NoRoute, EngineMissing, ConversionFailed }` with `as_str()` giving `error.unsupported_source`, `error.no_route`, `error.engine_missing`, `error.conversion_failed`; `EngineError` via thiserror.

**`crates/core/src/testing.rs`** (behind `#[cfg(any(test, feature = "test-support"))]`): the one `FakeConverter` that T06, T22, T32, T33 and T34 all use. Builder: `FakeConverter::new("fake").edge("png", "jpg", 10).max_parallelism(2).missing("pacman -S fake").behaviour(...)` with behaviours `Copy` (writes a small file through the allocator), `Fail(msg)`, `Panic`, `Sleep(Duration)` (checks cancel every 10 ms) and `Outputs(n)` (fan-out with `_p{n}`). It records the high-water mark of concurrent `convert()` calls.

**`crates/engines/src/registry.rs`** stub: `pub fn all() -> Vec<Box<dyn Converter>>` and `pub fn statuses() -> Vec<(&'static str, EngineStatus)>`, both returning an empty `Vec` until T30.

**`crates/cli/src/ctx.rs`:** `pub struct Ctx<'a> { pub out: &'a mut dyn Write, pub err: &'a mut dyn Write, pub cwd: PathBuf, pub cancel: CancelToken, pub engines: Vec<Box<dyn Converter>> }`. Commands read nothing from the environment; tests inject everything (D9).

**`crates/cli/src/lib.rs`:** `pub fn run(args: impl IntoIterator<Item = OsString>, out, err, cancel: CancelToken) -> i32` (builds `Ctx` from `current_dir()` and `registry::all()`) and `pub fn run_with(args, ctx: &mut Ctx) -> i32`.

**Command signature** (every file in `commands/`): `pub fn run(args: &<Name>Args, ctx: &mut Ctx) -> i32`.

### 5.4 The CLI surface (in `crates/cli/src/args.rs`, from T01)

```
morphring convert <INPUTS>... --to/-t <FMT> [-o/--output <DIR>] [-r/--recursive]
                  [-q/--quality <1-100>] [--overwrite | --skip] [--json] [--quiet]
morphring formats [<FMT>]
morphring doctor [--json]
morphring help [<COMMAND>]
morphring version            (also -v / --version)
global: --verbose (long only)
```

`-q` is quality, not quiet (Filee parity). Field names in `ConvertArgs`: `inputs`, `to`, `output`, `recursive`, `quality`, `overwrite`, `skip`, `json`, `quiet`. T15 may polish help text but not rename these.

## 6. v0.1.0 task breakdown

### 6.1 Waves at a glance

| Wave | Tasks | Unblocks |
|---|---|---|
| 1 | T01 skeleton + contracts, T02 licence, T03 eng-guide update, T04 fixtures | everything |
| 2 | T05 formats, T06 planner, T07 output, T09 process, T10 discovery, T11 magick args, T12 ffmpeg args, T13 ffprobe, T14 soffice filters, T15 CLI dispatch, T16 inputs, T17 report, T19 CI, T20 release workflow | engines, job runner |
| 3 | T22 job runner, T23 magick, T24 ffmpeg, T25 soffice | registry |
| 4 | T30 registry + route tests | commands |
| 5 | T32 convert, T33 formats, T34 doctor | tests, README |
| 6 | T36 CLI integration tests, T31 README | release |
| 7 | T37 changelog + verification | tag |
| 8 | T38 tag v0.1.0 (orchestrator) | done |

Critical path: T01 → {T05, T06, T07, T09, T10, T11, T12, T13, T14, T15, T16, T17} → {T22, T23, T24, T25} → T30 → {T32, T33, T34} → {T36, T31} → T37 → T38. T02, T03, T04, T19 and T20 run alongside and are all done by wave 2. Task ids keep their numbers from the first draft; the gaps (T08, T18, T21, T26-T29, T35) are the v0.1.1 tasks in section 7.

### 6.2 Tasks

#### T01: Workspace skeleton, shared contracts and test fake

- **Wave:** 1  
- **Depends on:** nothing  
- **Owns:** `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `.gitignore`, `clippy.toml`, `crates/core/**`, `crates/engines/Cargo.toml`, `crates/engines/src/**`, `crates/cli/**`, `crates/platform/**`  
- **Commit:** `chore: add cargo workspace skeleton and core contracts`

Create the cargo workspace: resolver 3, edition 2024, rust-version per D12 (the edition 2024 minimum), version 0.1.0, license MIT, repository github.com/suraniharsh/morphring; members and default-members crates/core, crates/engines, crates/cli, crates/platform; `exclude = ["app/src-tauri"]` (D1). rust-toolchain.toml: channel stable, components rustfmt and clippy. Cargo.lock is committed (we ship a binary). Declare EVERY v0.1.0 dependency up front (PLAN.md section 5.1), including the core feature `test-support` and the dev-dependencies that enable it in engines and cli, so no later task edits a Cargo.toml or Cargo.lock. [workspace.lints]: clippy unwrap_used/expect_used = warn, rust unsafe_code = deny (T09's one killpg call carries a narrow #[allow] with a reason). clippy.toml: allow-unwrap-in-tests, allow-expect-in-tests. Write the contracts every wave-2 task codes against (PLAN.md section 5.3): crates/core/src/contract.rs (FormatId, Edge, Step, ConvertOptions, QualityLevel::from_percent, OutputAllocator, EngineStatus, CancelToken, the Converter trait from ENGINEERING.md section 3, FileState, FileResult, JobState::from_files), crates/core/src/error.rs (ErrorKey with four stable keys: error.unsupported_source, error.no_route, error.engine_missing, error.conversion_failed; EngineError via thiserror), crates/core/src/testing.rs (the ONE FakeConverter, behind #[cfg(any(test, feature = "test-support"))], described in section 5.3). Create every module file of section 5.2 with its `pub mod` lines already in place: crates/engines/src/magick/mod.rs contains `pub mod args;`, ffmpeg/mod.rs contains `pub mod encoding; pub mod probe;`, soffice/mod.rs contains `pub mod filters;`, crates/cli/src/commands/mod.rs contains `pub mod convert; pub mod doctor; pub mod formats;`. Stubs that other tasks call carry their real signature and a trivial body: registry::all() returns an empty Vec, registry::statuses() returns an empty Vec, each commands::<name>::run(args: &<Name>Args, ctx: &mut Ctx) -> i32 writes 'morphring: not implemented yet' to ctx.err and returns 2. crates/cli/src/ctx.rs defines Ctx { out: &mut dyn Write, err: &mut dyn Write, cwd: PathBuf, cancel: CancelToken, engines: Vec<Box<dyn Converter>> } (no env lookups inside commands; tests inject everything). crates/cli/src/args.rs holds the clap derive tree for the whole v0.1.0 surface (ConvertArgs, FormatsArgs, DoctorArgs, help, version; section 5.4) so T15 and T32-T34 agree on field names. crates/cli/src/lib.rs exposes `pub fn run(args, out, err, cancel) -> i32` (builds Ctx from std::env::current_dir and registry::all) and `pub fn run_with(args, ctx: &mut Ctx) -> i32`; for now it handles --version and dispatches to the stubs. main.rs only calls run with stdout/stderr. crates/platform/src/lib.rs is a doc comment only (filled in v0.1.1). Every remaining module is a one-line stub `//! TODO(Txx): ...`. No logic beyond contracts; target under 450 lines of Rust. Port the trait shape from src/Filee.Core/Conversion/IConverter.cs and the job-state rule from src/Filee.Core/Conversion/JobQueue.cs.

**Acceptance:**

- [ ] cargo build --locked succeeds
- [ ] cargo test --locked -p morphring-core passes (JobState::from_files covers Completed, CompletedWithErrors, Failed, Cancelled; QualityLevel::from_percent boundaries 1/59/60/84/85/100; ErrorKey::as_str for all four keys; FakeConverter writes its output through the allocator and records a concurrency high-water mark)
- [ ] cargo clippy --workspace --all-targets --locked -- -D warnings is clean
- [ ] cargo fmt --all --check is clean
- [ ] cargo tree -p morphring-core shows no other morphring crate and no tauri
- [ ] cargo run -q -p morphring -- --version prints 'morphring 0.1.0'
- [ ] cargo run -q -p morphring -- convert x.png --to jpg exits 2 and stderr says 'not implemented yet'
- [ ] grep -q 'pub mod args;' crates/engines/src/magick/mod.rs && grep -q 'pub mod encoding;' crates/engines/src/ffmpeg/mod.rs && grep -q 'pub mod probe;' crates/engines/src/ffmpeg/mod.rs && grep -q 'pub mod filters;' crates/engines/src/soffice/mod.rs
- [ ] Cargo.toml sets rust-version per D12 and rust-toolchain.toml says stable
- [ ] grep -R 'tauri' Cargo.toml crates/ returns nothing
- [ ] grep -R 'set_var' crates/ returns nothing

#### T02: LICENSE and third-party notices

- **Wave:** 1  
- **Depends on:** nothing  
- **Owns:** `LICENSE`, `THIRD-PARTY-NOTICES.md`  
- **Commit:** `docs: add MIT license and third-party notices`

MIT LICENSE for MorphRing (Copyright (c) 2026 MorphRing contributors). THIRD-PARTY-NOTICES.md: credit Filee (MIT, github.com/KnifeLemon/Filee) with its full licence text copied from the Filee repo's LICENSE, stating that MorphRing ports Filee's behaviour (format table, edge costs, naming rules, CLI contract). Section 'External programs (not bundled)': FFmpeg (LGPL/GPL), ImageMagick (ImageMagick License), LibreOffice (MPL-2.0) run as separate processes, found on PATH, never shipped. Placeholder section for Rust crate licences: generated with `cargo about` in a later release; `cargo deny` enforces the allow-list today.

**Acceptance:**

- [ ] LICENSE starts with 'MIT License' and contains the full MIT text
- [ ] THIRD-PARTY-NOTICES.md contains 'KnifeLemon/Filee' and the Filee MIT text
- [ ] THIRD-PARTY-NOTICES.md lists FFmpeg, ImageMagick and LibreOffice as external, not bundled

#### T03: Align ENGINEERING.md with plan decisions

- **Wave:** 1  
- **Depends on:** nothing  
- **Owns:** `docs/ENGINEERING.md`  
- **Commit:** `docs: align engineering guide with v0.1.0 plan decisions`

ENGINEERING.md says to change it in its own commit when a rule moves. Write in decisions D1-D12 from PLAN.md section 3, in the file's existing style and with small edits: app/src-tauri excluded from the root workspace (D1); -v is --version and --verbose is long-only (D5); test helpers may call Command directly (D6); CI golden tests cover ffmpeg only (D7); the process runner has two modes for v0.1.0, Timeout(d) or None, plus cancel, and the stall watchdog moves to v0.2 (D8); tests never call std::env::set_var (unsafe in edition 2024 and racy across test threads): pass values as parameters or set them on a child Command (D9); the fourth error key error.engine_missing (D10). Section 11: Windows and macOS test jobs are continue-on-error for v0.1.0, `cargo deny check licenses bans sources` blocks and `check advisories` does not, every cargo call uses --locked, and the app job only runs when app/src-tauri/Cargo.toml exists. Section 9/10: rust-version per D12, committed Cargo.lock, each developer works in their own git worktree and runs `-p <crate>` checks while the orchestrator runs `--workspace` checks after each merge. Comments rule: jokes live in code comments only, never in CLI output, --json, error messages, `///` docs or README command blocks. Record the corrected machine state (webkit2gtk-4.1 2.52.6, pandoc, gs, pdftoppm, bsdtar installed; calibre missing; desktop Hyprland on Wayland).

**Acceptance:**

- [ ] git diff shows only docs/ENGINEERING.md changed
- [ ] the file mentions 'exclude' for app/src-tauri, 'Hyprland', 'set_var', '--locked', 'continue-on-error' and 'error.engine_missing'
- [ ] the 'Running external tools' section no longer lists a stall watchdog for v0.1.0
- [ ] section 12 scope guard matches PLAN.md section 4

#### T04: Test fixtures, engine-skip helper and .gitattributes

- **Wave:** 1  
- **Depends on:** nothing  
- **Owns:** `scripts/make-fixtures.sh`, `crates/engines/tests/fixtures/**`, `crates/engines/tests/common/mod.rs`, `.gitattributes`  
- **Commit:** `test: add generated fixtures and require_engine helper`

scripts/make-fixtures.sh creates tiny, licence-free fixtures with the installed tools (we own them because we generate them): sample.png (64x48 RGBA gradient, magick), sample.jpg (64x48), sample.gif (3-frame 32x32 animation), sample.wav (1 s 440 Hz sine, ffmpeg lavfi, 8 kHz mono to stay small), sample.mp4 (1 s 64x64 testsrc, libx264 yuv420p + aac), sample.txt (two lines incl. Hangul and an emoji), sample.docx and sample.odt (soffice --convert-to from sample.txt), notes.unknownext (for unsupported-type tests). The script is 'safe to re-run' (it overwrites, never appends), NOT byte-for-byte reproducible: magick and soffice embed timestamps. Fixtures are generated once on the dev machine and committed; CI never regenerates them. .gitattributes marks `crates/engines/tests/fixtures/** binary`. crates/engines/tests/common/mod.rs provides `require_engine!("ffmpeg")` (prints 'SKIP: ffmpeg not found' and returns when which lookup fails), `fixture(name) -> PathBuf` via env!("CARGO_MANIFEST_DIR"), `image_size(path) -> (u32, u32)` via `magick identify -format '%w %h'`, `frame_count(path)`, and `duration_secs(path) -> f64` via ffprobe. Test helpers may call Command directly (D6).

**Acceptance:**

- [ ] bash scripts/make-fixtures.sh runs cleanly twice in a row (safe to re-run)
- [ ] find crates/engines/tests/fixtures -size +100k returns nothing
- [ ] magick identify crates/engines/tests/fixtures/sample.png reports 64x48
- [ ] ffprobe -v error -show_entries format=duration -of csv=p=0 crates/engines/tests/fixtures/sample.mp4 prints about 1.0
- [ ] git check-attr binary crates/engines/tests/fixtures/sample.png reports 'set'
- [ ] bash -n scripts/make-fixtures.sh is clean (shellcheck is not installed here; run it too if it ever is)

#### T05: Format registry

- **Wave:** 2  
- **Depends on:** T01  
- **Owns:** `crates/core/src/formats.rs`  
- **Commit:** `feat(core): port Filee format registry`

Port src/Filee.Core/Formats/FormatRegistry.cs:52-255 verbatim as a static ordered table of all 171 entries (170 formats plus the folder pseudo-format): id, display name, category, extensions (first is canonical), writable flag. Port ExtensionOf (FormatRegistry.cs:131-152): longest compound extension first (tar.gz, tar.bz2 ...), else last dot, eq_ignore_ascii_case, original casing kept; never use Path::extension(). Provide extension_of, name_without_extension, detect(path) -> Option<&Format>, find_by_id, find_by_extension (leading dot stripped, case-insensitive; jpeg -> jpg, tar.gz -> tgz), lookup_target(text) used by --to (trim, strip '.', id first, then extension). All camera RAW extensions map to the one 'raw' id. The full list is in docs/reverse-engineering/07-cli-tests-docs.md.

**Acceptance:**

- [ ] cargo test -p morphring-core formats passes
- [ ] test: every extension in the table is unique (case-insensitive)
- [ ] test: the table has 171 entries and 56 are read-only (writable == false)
- [ ] test: extension_of("Backup.TAR.GZ") == "TAR.GZ"; extension_of(".bashrc") is empty; detect("a.JPEG") is jpg
- [ ] test: find_by_extension(".tar.gz") is tgz; lookup_target("JPEG") is jpg; lookup_target("folder") is the folder format
- [ ] cargo clippy -p morphring-core --all-targets -- -D warnings is clean

#### T06: Route planner (exact, max 3 hops)

- **Wave:** 2  
- **Depends on:** T01  
- **Owns:** `crates/core/src/planner.rs`  
- **Commit:** `feat(core): add exact 3-hop route planner`

Planner::new(&[&dyn Converter], assume_all_available: bool) builds the edge list once from engines whose status() is Available (or all engines when assume_all_available; used by 'formats' and by the job runner's missing-engine hint). plan(from, to) -> Option<Route> enumerates every path of 1..=3 hops (MAX_HOPS = 3) and returns the cheapest by the sum of edge.cost + min(priority_index, 9), where priority_index is the engine's position in the slice. Ties go to the first path found in registration order. from == to only uses an explicit self edge. Route records the engine index per hop so the caller can name the engines it needs. Also can_convert(from, to) and targets_from(from) -> Vec<(FormatId, cost)> sorted by cost then registry order. This fixes Filee's inexact hop-capped Dijkstra (src/Filee.Core/Conversion/RoutePlanner.cs:36-93). Port the 9 tests of tests/Filee.Core.Tests/RoutePlannerTests.cs using core::testing::FakeConverter (do not write another fake).

**Acceptance:**

- [ ] cargo test -p morphring-core planner passes
- [ ] ported tests: direct edge used; two-step route found; cheaper route wins; priority breaks ties; same format requires self edge; impossible conversion is None; 4-hop routes rejected; unavailable engines excluded; assume_all_available includes Missing engines
- [ ] regression test: a graph where a cheap 3-hop path to an intermediate node blocks a hop-capped Dijkstra from a valid 3-hop route to the target; our planner finds it
- [ ] test: identical inputs always give the identical route (run 100 times)
- [ ] cargo clippy -p morphring-core --all-targets -- -D warnings is clean

#### T07: Output path resolver and job allocator

- **Wave:** 2  
- **Depends on:** T01  
- **Owns:** `crates/core/src/output.rs`  
- **Commit:** `feat(core): add output path resolver with shared reservations`

Port src/Filee.Core/Presets/OutputPathResolver.cs (144 lines) as a pure fn resolve(rule, tokens, target_ext, suffix, exists: &dyn Fn(&Path) -> bool, reserved: &HashSet<PathBuf>) -> Option<PathBuf>. OutputRule { location: SameFolder | Subfolder(name, default 'converted') | CustomFolder { path, keep_folders_from: Option<PathBuf> }, pattern (default '{name}'), conflict: Rename | Overwrite | Skip }. Tokens {name} {ext} {preset} {date yyyyMMdd} {time HHmmss} {index}; Windows-strict sanitize on every OS; suffix added after the pattern; numbering ' (2)', ' (3)'; the source is never overwritten (case-insensitive compare on every OS); compound extensions count as one. JobAllocator implements OutputAllocator over Arc<Mutex<HashSet<PathBuf>>>, holding the lock across resolve + exists + insert + create_dir_all. FIX Filee's bug: a reserved name is always numbered, under Overwrite too.

**Acceptance:**

- [ ] cargo test -p morphring-core output passes
- [ ] the 10 cases of tests/Filee.Core.Tests/OutputPathResolverTests.cs are ported by name (e.g. default_pattern_keeps_the_source_name, two_part_extensions_are_one_extension)
- [ ] test: Overwrite policy with two sources resolving to the same name gives 'a.jpg' and 'a (2).jpg' (Filee bug fixed)
- [ ] test: a target equal to the source path in different case is numbered, never returned as-is
- [ ] test: Skip with an existing file returns None
- [ ] test: 16 threads allocating the same name get 16 distinct paths
- [ ] cargo clippy -p morphring-core --all-targets -- -D warnings is clean

#### T09: Process runner

- **Wave:** 2  
- **Depends on:** T01  
- **Owns:** `crates/engines/src/process.rs`  
- **Commit:** `feat(engines): add process runner with group kill`

The only place production code calls std::process::Command. run(program, args: &[OsString], opts: RunOptions { timeout: Option<Duration>, cwd: Option<PathBuf>, env_remove: Vec<&'static str>, on_stdout_line: Option<&(dyn Fn(&str) + Sync)> }, cancel: &CancelToken) -> Result<RunOutput, RunError>. No shell, argument list only. stdout and stderr read on their own threads; keep the last 40 stderr lines (VecDeque). Unix: process_group(0) and libc::killpg(SIGKILL) on cancel or timeout (narrow #[allow(unsafe_code)] with a reason comment); Windows: child.kill() (TODO Job Objects, v0.4). Poll try_wait every 50 ms against cancel and the deadline. RunError::{Cancelled, TimedOut, Spawn{program, source}, Failed{code, tail}}. summarize(tail) strips '[x @ 0x...]' prefixes and keeps the last 3 distinct lines (src/Filee.Engines/Media/FfmpegRunner.cs). move_into_place(from, to): rename, falling back to copy + remove across filesystems (/tmp is tmpfs on Arch). Log the full command line at debug. Port src/Filee.Engines/Infrastructure/ProcessRunner.cs. No stall watchdog in v0.1.0 (D8). Tests that use sleep or sh are #[cfg(unix)].

**Acceptance:**

- [ ] cargo test -p morphring-engines process passes
- [ ] #[cfg(unix)] test: running 'sleep 30' then cancelling returns Cancelled within 1 s and the child pid no longer exists
- [ ] #[cfg(unix)] test: a child that spawns a grandchild ('sh -c "sleep 30 & wait"') has both killed (grandchild pid written to a temp file and checked with kill -0)
- [ ] #[cfg(unix)] test: timeout Some(200ms) on 'sleep 5' returns TimedOut
- [ ] test: the stderr tail keeps only the last 40 lines
- [ ] test: summarize strips '[libx264 @ 0x55d...]' prefixes
- [ ] #[cfg(unix)] test: the stdout line callback sees each line of printf 'a\nb\n'
- [ ] test: move_into_place moves a file between two TempDirs
- [ ] cargo clippy -p morphring-engines --all-targets -- -D warnings is clean

#### T10: Tool discovery and install hints

- **Wave:** 2  
- **Depends on:** T01  
- **Owns:** `crates/engines/src/discovery.rs`  
- **Commit:** `feat(engines): add tool discovery with install hints`

locate(Tool) for Magick, Ffmpeg (needs ffmpeg AND ffprobe in the same folder) and Soffice: PATH via the which crate, then fixed folders /usr/bin, /usr/local/bin, /opt/homebrew/bin, /usr/lib/libreoffice/program, /opt/libreoffice*/program. Version via 'ffmpeg -hide_banner -version', 'magick -version', 'soffice --version' run through process::run with a 5 s timeout; parse_version(tool, stdout) is a pure fn. Cache per tool in a OnceLock<Mutex<HashMap>>. install_hint(tool, os_release: &str) is pure: 'pacman -S ffmpeg' / 'pacman -S imagemagick' / 'pacman -S libreoffice-fresh' when ID or ID_LIKE is arch, 'sudo apt install ...' for debian/ubuntu, 'brew install ...' on macOS, a download URL otherwise; a thin wrapper reads /etc/os-release. display_name(tool) gives 'FFmpeg', 'ImageMagick', 'LibreOffice'. Mirrors src/Filee.Engines/Infrastructure/EngineEnvironment.cs minus downloads.

**Acceptance:**

- [ ] cargo test -p morphring-engines discovery passes
- [ ] test: parse_version on the real strings from this machine: 'ffmpeg version n9.0.2 Copyright ...' gives 9.0.2; 'Version: ImageMagick 7.1.2-32 Q16-HDRI x86_64 ...' gives 7.1.2-32; 'LibreOffice 26.2.6.3 620(Build:3)' gives 26.2.6.3; a git build 'ffmpeg version N-12345-gabcdef Copyright' gives N-12345-gabcdef
- [ ] test: install_hint with 'ID=arch' gives 'pacman -S ffmpeg'; with 'ID=ubuntu\nID_LIKE=debian' gives 'sudo apt install ffmpeg'; soffice on arch gives 'pacman -S libreoffice-fresh'
- [ ] test (skips if ffmpeg missing): locate(Ffmpeg) is Some and its folder also contains ffprobe
- [ ] cargo clippy -p morphring-engines --all-targets -- -D warnings is clean

#### T11: ImageMagick argument builder and edges

- **Wave:** 2  
- **Depends on:** T01  
- **Owns:** `crates/engines/src/magick/args.rs`  
- **Commit:** `feat(engines): add ImageMagick argument builder`

Pure fns only (magick/mod.rs already declares `pub mod args;`, from T01). EDGES: raster set {png, jpg, webp, gif, bmp, tiff, ico, avif, heic, jxl, tga, ppm, psd (read), raw (read)} to the writable raster set at cost 10, self edges for every writable raster (re-encode), still -> gif cost 12, raster -> pdf cost 10. No PDF/EPS/PS input in v0.1.0 (D4). args(input, output, from, to, opts) -> Vec<OsString> following Filee's ApplyOptions order (docs/reverse-engineering/02-engines-media-image.md; src/Filee.Engines/Magick/ImageEncoder.cs): input[0] for psd, -coalesce only for gif<->webp, -auto-orient, per-format max side (jpg 65500, webp 16383, avif 16384) as -resize 'NxN>', +profile '!icc,*', -background white -alpha remove -alpha off for jpg/ppm/heic, -quality N (from QualityLevel: 92/85/70, default Normal), -define icon:auto-resize=256,128,64,48,32,16 for ico, -compress LZW for tiff; output written as 'FMT:path' so the extension never decides. frame_policy(from, to) says single file vs one file per frame with suffix _p{n}.

**Acceptance:**

- [ ] cargo test -p morphring-engines magick::args passes
- [ ] test pins the exact arg list for png -> jpg at quality 80
- [ ] test: ico args include icon:auto-resize
- [ ] test: a jpg target always includes '-background white' and '-alpha remove'
- [ ] test: EDGES contains a png->png self edge, png->gif at cost 12, png->pdf, and no pdf->* edge
- [ ] test: frame_policy(gif, png) is one file per frame; frame_policy(gif, webp) is single file
- [ ] cargo clippy -p morphring-engines --all-targets -- -D warnings is clean

#### T12: FFmpeg encoding argument builder and edges

- **Wave:** 2  
- **Depends on:** T01  
- **Owns:** `crates/engines/src/ffmpeg/encoding.rs`  
- **Commit:** `feat(engines): add ffmpeg argument builder`

Port the v0.1.0 subset of src/Filee.Engines/Media/MediaEncoding.cs (612 lines) as pure fns returning Vec<String>. EDGES: video {mp4, webm, mkv, mov, avi} <-> video (cost 10, self edges), video -> audio {mp3, m4a, wav, flac, ogg, opus} (10), audio <-> audio (10, self edges), video -> gif (15), gif -> mp4/webm (10), video -> png poster frame (10). Common args: -hide_banner -nostdin -nostats -v error -y -i IN ... -f <explicit muxer> -progress pipe:1 OUT. Remux (-c copy) ONLY when quality != Small and the source is h264 + yuv420p going into mp4, mkv or mov; everything else re-encodes (no hvc1 tag, no general fit matrix in v0.1.0, D8). Encode: libx264 -preset medium -crf 18/23/28 -pix_fmt yuv420p; VP9 -b:v 0 -crf 24/32/38 -row-mt 1 -deadline good -cpu-used 4; even-size scale filter; -movflags +faststart for mp4/mov/m4a; mp3 VBR -q:a 0/2/5 with -id3v2_version 3; aac 256/192/128 k per stereo pair; opus 160/128/64 k; -rf64 auto for wav. GIF: two passes, palettegen then paletteuse, fps 15/12/10, width 640/480/320, dither sierra2_4a. Poster: -ss duration/10 -frames:v 1. Floats formatted with '.', up to 3 decimals. Every flag on the wav -> mp3 path must exist in ffmpeg 4.4 (Ubuntu 22.04's apt version, used by the release smoke test). Inputs are plain parameters (codec, pix_fmt, width, height, duration, channels), not T13's MediaInfo, so the two tasks stay independent.

**Acceptance:**

- [ ] cargo test -p morphring-engines ffmpeg::encoding passes
- [ ] test pins the full arg list for wav -> mp3 at Normal (-q:a 2, -id3v2_version 3, -f mp3, -progress pipe:1)
- [ ] test: mp4(h264/yuv420p) -> mkv at High is a stream copy (-c copy); the same at Small re-encodes; hevc -> mp4 re-encodes
- [ ] test: mp4 -> webm at Small uses libvpx-vp9 -crf 38
- [ ] test: GIF pass 1 contains 'palettegen' and pass 2 contains 'paletteuse' with fps=12 at Normal
- [ ] test: the scale filter rounds odd sizes to even
- [ ] test: EDGES has mp4->gif at cost 15 and mp4->png
- [ ] cargo clippy -p morphring-engines --all-targets -- -D warnings is clean

#### T13: ffprobe and progress parsing

- **Wave:** 2  
- **Depends on:** T01  
- **Owns:** `crates/engines/src/ffmpeg/probe.rs`  
- **Commit:** `feat(engines): parse ffprobe output and ffmpeg progress`

Pure parsing of 'ffprobe -v error -print_format json -show_format -show_streams' via serde_json::Value into MediaInfo { duration, video: Option<VideoStream { codec, pix_fmt, width, height, rotation }>, audio: Option<AudioStream { codec, channels, bit_rate }> }. Accept numbers as JSON numbers or numeric strings; attached_pic streams are covers, not video; rotation from side_data_list or tags.rotate; ratios 'a/b' and 'a:b' parse to f64. probe_args(input) -> Vec<String>. parse_progress_line(line, duration) -> Option<f32> from out_time_us= (clamped 0..=1). Port src/Filee.Engines/Media/MediaInfo.cs. Test JSON is embedded as string literals in the test module.

**Acceptance:**

- [ ] cargo test -p morphring-engines ffmpeg::probe passes
- [ ] test: an ffprobe JSON for an mp3 with cover art reports no video stream
- [ ] test: duration given as the string "1.023" parses to 1.023
- [ ] test: rotation -90 is read from side_data_list
- [ ] test: 'out_time_us=500000' with duration 1.0 gives 0.5; garbage gives None
- [ ] cargo clippy -p morphring-engines --all-targets -- -D warnings is clean

#### T14: LibreOffice filter table, edges and profiles

- **Wave:** 2  
- **Depends on:** T01  
- **Owns:** `crates/engines/src/soffice/filters.rs`  
- **Commit:** `feat(engines): add LibreOffice filter table`

Port FilterFor and BuildEdges from src/Filee.Engines/Office/LibreOfficeConverter.cs as static tables. Families: writer {docx, doc, odt, rtf, txt, html}, calc {xlsx, xls, ods, csv}, impress {pptx, ppt, odp}, plus the read-only legacy inputs listed in docs/reverse-engineering/03-engines-office-docs.md. Filter strings: pdf -> {writer|calc|impress|draw}_pdf_Export, docx 'MS Word 2007 XML', doc 'MS Word 97', rtf 'Rich Text Format', txt 'Text (encoded):UTF8', xlsx 'Calc MS Excel 2007 XML', xls 'MS Excel 97', csv 'Text - txt - csv (StarCalc):44,34,76', pptx 'Impress MS PowerPoint 2007 XML', ppt 'MS PowerPoint 97', html 'HTML (StarWriter)' / 'HTML (StarCalc)'. Costs: fallback 25, txt/html -> docx/odt/pdf 28, impress -> pdf 0 (Filee's LibreOfficeForSlides default). PDF/A is out. args(input, filter, outdir, profile_dir) -> Vec<OsString> with --headless --norestore --nologo --nodefault --nolockcheck --nofirststartwizard -env:UserInstallation=<url::Url::from_directory_path(profile)> --convert-to <filter> --outdir <dir> <input>. profile_dir(data_dir: &Path, worker) = <data_dir>/libreoffice-profiles/worker-N (the caller passes the directories data dir, so the fn stays pure).

**Acceptance:**

- [ ] cargo test -p morphring-engines soffice::filters passes
- [ ] test: filter_for(docx, pdf) == 'pdf:writer_pdf_Export'; filter_for(xlsx, csv) is the 44,34,76 string
- [ ] test: pptx->pdf edge cost 0; docx->pdf 25; txt->docx 28
- [ ] test: the -env:UserInstallation value starts with 'file:///' and percent-encodes a space in the profile path
- [ ] test: no edge targets a read-only format
- [ ] cargo clippy -p morphring-engines --all-targets -- -D warnings is clean

#### T15: CLI dispatch, help, version, logging and Ctrl+C

- **Wave:** 2  
- **Depends on:** T01  
- **Owns:** `crates/cli/src/main.rs`, `crates/cli/src/lib.rs`, `crates/cli/src/args.rs`  
- **Commit:** `feat(cli): add dispatch, help, version and Ctrl+C`

Fill in run() / run_with() in lib.rs over the clap tree T01 put in args.rs. T15 may improve help text, about strings and after_help examples in args.rs but must not rename T01's types or fields (T32-T34 code against them). -v/--version and the 'version' command print 'morphring 0.1.0'; clap's -V is disabled. 'help [<cmd>]' prints help for that command. Usage errors print 'morphring: <msg>' and "Run 'morphring help' for usage." to err and return 2 (Filee src/Filee.Cli/CliOptions.cs, Help.cs; Linux examples, and the help says -q means quality, not quiet). help and version return before touching engines. run() builds Ctx with std::env::current_dir and registry::all(); run_with() is what tests call. main.rs: tracing-subscriber to stderr (default warn, RUST_LOG respected, --verbose long-only raises to debug, D5), two-stage Ctrl+C with ctrlc (first press sets the CancelToken, second std::process::exit(130)), ExitCode::from(run(...) as u8). No env mutation anywhere (D9).

**Acceptance:**

- [ ] cargo test -p morphring passes (in-process tests of run_with() with Vec<u8> buffers)
- [ ] cargo run -q -p morphring -- --version prints 'morphring 0.1.0' and exits 0; so do '-v' and 'version'
- [ ] cargo run -q -p morphring -- bogus exits 2 and stderr contains "Run 'morphring help' for usage."
- [ ] cargo run -q -p morphring -- convert a.png exits 2 (missing --to)
- [ ] cargo run -q -p morphring -- convert a.png --to jpg -q 0 exits 2
- [ ] cargo run -q -p morphring -- help convert mentions '-q, --quality'
- [ ] stdout is empty on usage errors
- [ ] cargo clippy -p morphring --all-targets -- -D warnings is clean

#### T16: CLI input expansion

- **Wave:** 2  
- **Depends on:** T01  
- **Owns:** `crates/cli/src/inputs.rs`  
- **Commit:** `feat(cli): expand files, folders and wildcards`

expand(inputs: &[OsString], recursive: bool, cwd: &Path, is_known: &dyn Fn(&Path) -> bool) -> Expanded { files: Vec<PathBuf>, missing: Vec<OsString> }. Explicit files are taken as-is; folders keep only is_known files, walked with walkdir sorted by file name (deeper only with -r); wildcards (*, ?, [) only in the last segment, expanded with glob relative to cwd and not filtered by format (with -r they recurse); a wildcard with no match counts as missing. std::path::absolute-style joining against the cwd parameter (never the process cwd, never canonicalize). Order-preserving dedupe with IndexSet, exact compare (case-distinct names stay distinct on Linux). PathBuf/OsString throughout. Port the expansion rules in docs/reverse-engineering/07-cli-tests-docs.md (src/Filee.Cli/Cli.cs).

**Acceptance:**

- [ ] cargo test -p morphring inputs passes
- [ ] test (TempDir): a folder with a.png, b.txt, c.unknownext and is_known accepting png/txt returns a.png, b.txt in sorted order
- [ ] test: '*.png' with no match is reported in missing
- [ ] #[cfg(target_os = "linux")] test: 'A.png' and 'a.png' both kept (case-insensitive file systems on macOS and Windows cannot even create both)
- [ ] test: the same path given twice is kept once
- [ ] test: a name with Hangul, spaces and a single quote survives unchanged
- [ ] test: -r descends into subfolders, non -r does not
- [ ] cargo clippy -p morphring --all-targets -- -D warnings is clean

#### T17: CLI human and JSON report

- **Wave:** 2  
- **Depends on:** T01  
- **Owns:** `crates/cli/src/report.rs`  
- **Commit:** `feat(cli): format human and JSON reports`

Pure formatting over core::contract::FileResult. Human line per file in completion order: 'ok    src -> a, b', 'skip  src (the output file already exists)', 'stop  src (cancelled)', 'fail  src: message (detail)'; paths shown relative to cwd when inside it (pathdiff). Summary 'Converted D of N file(s)[, F failed].'. Messages per error key: error.unsupported_source "This file type isn't supported", error.no_route 'No engine can do this conversion', error.engine_missing 'A required program is not installed' (detail e.g. 'needs LibreOffice: pacman -S libreoffice-fresh'), error.conversion_failed 'The engine reported an error'. JSON (serde, to_writer_pretty, 2-space indent): { preset, target, converted, skipped, failed, files: [{ source (absolute), state (lowercase), outputs, error (only for failed/cancelled) }] } in input order; failed counts cancelled too; preset is null in v0.1.0. Field names exactly as Filee's (src/Filee.Cli/Cli.cs, EnglishTexts.cs). No jokes in any string here.

**Acceptance:**

- [ ] cargo test -p morphring report passes
- [ ] test pins the four human line shapes and the summary with and without failures
- [ ] test: an engine_missing failure renders 'fail  x.docx: A required program is not installed (needs LibreOffice: pacman -S libreoffice-fresh)'
- [ ] test: JSON for one done, one skipped and one cancelled file has converted 1, skipped 1, failed 1 and 'error' only on the cancelled entry
- [ ] test: the JSON round-trips through serde_json::from_slice and keys appear in Filee's order
- [ ] cargo clippy -p morphring --all-targets -- -D warnings is clean

#### T19: CI workflow and cargo-deny config

- **Wave:** 2  
- **Depends on:** T01  
- **Owns:** `.github/workflows/ci.yml`, `deny.toml`  
- **Commit:** `chore(ci): add fmt, clippy, test and deny workflow`

ci.yml on push and pull_request, every cargo call with --locked. Jobs: fmt (cargo fmt --all --check); clippy (cargo clippy --workspace --all-targets --locked -- -D warnings; safe because the app is excluded, D1); test, a matrix over ubuntu-latest, windows-latest, macos-latest with `continue-on-error: ${{ matrix.os != 'ubuntu-latest' }}` so only Linux blocks for v0.1.0 (cargo test --workspace --locked; Ubuntu apt-installs ffmpeg so the ffmpeg golden tests run; magick and soffice golden tests skip in CI, D7); deny-licenses (EmbarkStudios/cargo-deny-action@v2, `check licenses bans sources`, blocking); deny-advisories (`check advisories`, continue-on-error, because a new RUSTSEC notice must not turn a tag red without a code change); app (`if: hashFiles('app/src-tauri/Cargo.toml') != ''`, continue-on-error, installs libwebkit2gtk-4.1-dev and runs cargo check --manifest-path app/src-tauri/Cargo.toml) so v0.1.1 needs no CI edit. dtolnay/rust-toolchain@stable (honours rust-toolchain.toml), Swatinem/rust-cache@v2, timeout-minutes 15. deny.toml allow-list: MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, MPL-2.0, Unicode-3.0, Unicode-DFS-2016. actionlint and shellcheck are not installed here, so use the YAML fallback check.

**Acceptance:**

- [ ] python -I -c 'import yaml,sys; yaml.safe_load(open(sys.argv[1]))' .github/workflows/ci.yml succeeds (actionlint if installed)
- [ ] the workflow has jobs fmt, clippy, test, deny-licenses, deny-advisories, app; test has the 3-OS matrix with continue-on-error for non-Ubuntu; deny-advisories and app are continue-on-error; app has the hashFiles guard
- [ ] grep -c -- '--locked' .github/workflows/ci.yml counts every cargo build/test/clippy line
- [ ] if cargo-deny is installed: cargo deny check licenses bans sources passes locally; otherwise the deny-licenses job passes after merge

#### T20: Release workflow

- **Wave:** 2  
- **Depends on:** T01, T04  
- **Owns:** `.github/workflows/release.yml`  
- **Commit:** `chore(ci): add draft release workflow for v* tags`

On push of tags 'v*', one job on ubuntu-22.04 (a tag push does not wait for ci.yml, so this job tests too): apt install ffmpeg (4.4 there); cargo test --locked -p morphring-core -p morphring-engines -p morphring; cargo build --release --locked -p morphring; smoke test: target/release/morphring --version must equal 'morphring ${GITHUB_REF_NAME#v}', then convert crates/engines/tests/fixtures/sample.wav --to mp3 -o $T and check exit 0 and a non-empty $T/sample.mp3 (only this one conversion); package morphring-<tag>-x86_64-linux.tar.gz with the binary, README.md, LICENSE, THIRD-PARTY-NOTICES.md, CHANGELOG.md; write SHA256SUMS.txt; extract the CHANGELOG section for the tag (awk between '## [x.y.z]' headings) as the notes; gh release create --draft --verify-tag with both files. No Tauri job in v0.1.0 (a half-finished bundle job can leave a draft with half its assets). permissions: contents: write.

**Acceptance:**

- [ ] python -I -c 'import yaml,sys; yaml.safe_load(open(sys.argv[1]))' .github/workflows/release.yml succeeds (actionlint if installed)
- [ ] the workflow triggers only on push tags 'v*', runs cargo test before packaging, uses --locked and --draft --verify-tag
- [ ] grep -i tauri .github/workflows/release.yml returns nothing
- [ ] the awk command from the workflow, run locally against a sample CHANGELOG with '## [0.1.0]' and '## [0.0.1]' sections, prints only the 0.1.0 section

#### T22: Parallel job runner

- **Wave:** 3  
- **Depends on:** T05, T06, T07  
- **Owns:** `crates/core/src/job.rs`  
- **Commit:** `feat(core): add parallel job runner with per-engine limits`

run_job(files, target, options, rule, engines: &[&dyn Converter], cancel, on_event: &(dyn Fn(JobEvent) + Sync)) -> Vec<FileResult> (input order). Builds one Planner per job (available engines only). Files run in parallel with std::thread::scope, at most available_parallelism() at a time (work queue over an AtomicUsize index). Per-engine counting semaphore (Mutex<usize> + Condvar, about 30 lines, in this file) keyed by engine id with limit max_parallelism() (0 = cores), held around a single convert() step only. Per file: detect source (unknown -> Failed error.unsupported_source, detail '.ext'); plan; when there is no route, plan again with assume_all_available: if that finds a route, Failed error.engine_missing with detail 'needs <DisplayName>: <hint>' built from the Missing status of the engines on that route, else Failed error.no_route with detail 'txt → mp3'. Run each hop: intermediate outputs go to $TMPDIR/morphring/<job-id>/<random>/ via a temp allocator, the final hop uses the shared JobAllocator (T07); fan-out (a step producing many files runs the next step per output). Progress per file = (step + p)/steps and only rises; JobEvent::{FileStarted, FileProgress, FileFinished(FileResult)}. Any error or panic in a file is caught (catch_unwind) and becomes Failed error.conversion_failed with the message; cancellation gives Cancelled and deletes partial outputs. The work dir is removed at the end. Port src/Filee.Core/Conversion/JobQueue.cs (487 lines) minus the queue (the CLI runs one job). Tests use core::testing::FakeConverter.

**Acceptance:**

- [ ] cargo test -p morphring-core job passes (relevant cases of tests/Filee.Core.Tests/JobQueueTests.cs ported)
- [ ] test: 8 files with a fake engine of max_parallelism 2 never exceed 2 concurrent convert() calls (FakeConverter high-water mark)
- [ ] test: one failing file among 5 gives 4 Done + 1 Failed and job state CompletedWithErrors
- [ ] test: unknown extension gives error.unsupported_source; no route at all gives error.no_route
- [ ] test: a route that only a Missing fake engine (hint 'pacman -S libreoffice-fresh') can serve gives error.engine_missing with that hint in the detail
- [ ] test: a two-hop route writes the intermediate into the job temp dir and the temp dir is gone afterwards
- [ ] test: cancel during a slow fake step gives Cancelled and no file at the reserved output path
- [ ] test: progress events per file are monotonic
- [ ] test: a panicking fake engine gives Failed, not a crashed job
- [ ] cargo clippy -p morphring-core --all-targets -- -D warnings is clean

#### T23: ImageMagick engine

- **Wave:** 3  
- **Depends on:** T04, T09, T10, T11  
- **Owns:** `crates/engines/src/magick/mod.rs`, `crates/engines/tests/magick.rs`  
- **Commit:** `feat(engines): add ImageMagick engine`

Keep T01's `pub mod args;` line. MagickEngine implements Converter: id 'magick', edges from args::EDGES, max_parallelism 0 (cores), status from discovery (cached). convert(): allocate the output(s) (None -> skip, return Ok(empty)), write into a fresh temp subdir of step.work_dir, run magick via process::run with timeout Some(120 s), honour cancel, move_into_place, treat a missing or empty output as EngineError with the stderr summary. Multi-frame inputs follow args::frame_policy with suffix _p{n}. Progress: report 0.0 then 1.0 (fake progress is v0.2). Golden tests use tests/common (require_engine!("magick")).

**Acceptance:**

- [ ] cargo test -p morphring-engines --test magick passes (prints SKIP when magick is missing)
- [ ] golden: sample.png -> jpg gives a 64x48 JPEG (magick identify -format %m reports JPEG)
- [ ] golden: sample.png -> webp and -> ico succeed
- [ ] golden: sample.gif -> png writes 3 files named sample_p1.png .. sample_p3.png
- [ ] golden: sample.png -> pdf writes a non-empty file starting with '%PDF'
- [ ] golden: an allocator returning None writes nothing and returns Ok(empty)
- [ ] cargo clippy -p morphring-engines --all-targets -- -D warnings is clean

#### T24: FFmpeg engine

- **Wave:** 3  
- **Depends on:** T04, T09, T10, T12, T13  
- **Owns:** `crates/engines/src/ffmpeg/mod.rs`, `crates/engines/tests/ffmpeg.rs`  
- **Commit:** `feat(engines): add ffmpeg engine`

Keep T01's `pub mod encoding; pub mod probe;` lines. FfmpegEngine implements Converter: id 'ffmpeg', edges from encoding::EDGES, max_parallelism 2, status requires ffmpeg and ffprobe together. convert(): ffprobe the input (probe.rs), build args (encoding.rs), run with timeout Some(30 min) (no stall watchdog in v0.1.0, D8) and on_stdout_line feeding parse_progress_line into progress; GIF runs two passes weighted 0.3/0.7; output is written into a temp subdir, then moved; partial output deleted on any failure; a 0-byte output is a failure. Errors carry summarize(stderr tail). Port src/Filee.Engines/Media/FfmpegConverter.cs and FfmpegRunner.cs.

**Acceptance:**

- [ ] cargo test -p morphring-engines --test ffmpeg passes (SKIP when ffmpeg is missing)
- [ ] golden: sample.wav -> mp3 has a duration within 0.1 s of 1.0
- [ ] golden: sample.mp4 -> webm and -> mkv succeed; mkv is a stream copy (ffprobe reports h264)
- [ ] golden: sample.mp4 -> gif produces a GIF with more than 1 frame
- [ ] golden: sample.mp4 -> png produces one 64x64 PNG
- [ ] golden: progress values are monotonic and end at 1.0
- [ ] golden: cancelling mid-run leaves no output file
- [ ] cargo clippy -p morphring-engines --all-targets -- -D warnings is clean

#### T25: LibreOffice engine

- **Wave:** 3  
- **Depends on:** T04, T09, T10, T14  
- **Owns:** `crates/engines/src/soffice/mod.rs`, `crates/engines/tests/soffice.rs`  
- **Commit:** `feat(engines): add LibreOffice engine with worker profiles`

Keep T01's `pub mod filters;` line. SofficeEngine implements Converter: id 'soffice', edges from filters::EDGES, max_parallelism 2. A profile pool (Mutex<Vec<usize>> + Condvar pre-filled with 0 and 1, slot returned by a Drop guard) gives each running conversion its own -env:UserInstallation profile under the directories data dir; without it concurrent runs exit 1 silently. Each run gets a private tempfile::TempDir outdir; success = the first non-empty file in it (exit code ignored); then move_into_place under the allocated name. Timeout 180 s with process-group kill (soffice forks soffice.bin). Port src/Filee.Engines/Office/LibreOfficeConverter.cs (323 lines); warm-up is deferred. The engine takes its data dir in a constructor (new_in(data_dir)) so tests use a TempDir and never touch ~/.local/share.

**Acceptance:**

- [ ] cargo test -p morphring-engines --test soffice passes (SKIP when soffice is missing)
- [ ] golden: sample.docx -> pdf gives a file starting with '%PDF'
- [ ] golden: sample.odt -> docx gives a valid zip (first bytes 'PK')
- [ ] golden: two conversions started at once both succeed (the profile pool works)
- [ ] golden: cancelling a conversion returns within 2 s and leaves no process whose command line contains the test's TempDir profile path (checked by scanning /proc/*/cmdline)
- [ ] cargo clippy -p morphring-engines --all-targets -- -D warnings is clean

#### T30: Engine registry and route cost tests

- **Wave:** 4  
- **Depends on:** T06, T23, T24, T25  
- **Owns:** `crates/engines/src/registry.rs`, `crates/engines/tests/routes.rs`  
- **Commit:** `feat(engines): add engine registry and route tests`

Replace T01's stub bodies, keeping the signatures: registry::all() -> Vec<Box<dyn Converter>> in priority order magick, ffmpeg, soffice (Filee's order from src/Filee.Engines/EngineRegistry.cs:32-54; native engines join ahead of these in v0.2), and registry::statuses() -> Vec<(&'static str, EngineStatus)> for doctor. tests/routes.rs builds a Planner with assume_all_available = true and names the route every hand-tuned cost is meant to produce.

**Acceptance:**

- [ ] cargo test -p morphring-engines --test routes passes without any tool installed
- [ ] route test: mp4 -> jpg is ffmpeg(mp4->png) then magick(png->jpg) (poster frame, Filee's documented route)
- [ ] route test: png -> gif is magick at cost 12; mp4 -> gif is ffmpeg at cost 15
- [ ] route test: pptx -> pdf uses soffice at cost 0; docx -> pdf at 25 plus the priority penalty
- [ ] route test: png -> png is a single magick self edge; docx -> png is None in v0.1.0 (no PDF input, D4) and the test says so
- [ ] route test: txt -> mp3 is None
- [ ] cargo clippy -p morphring-engines --all-targets -- -D warnings is clean

#### T32: convert command

- **Wave:** 5  
- **Depends on:** T15, T16, T17, T22, T30  
- **Owns:** `crates/cli/src/commands/convert.rs`, `crates/cli/tests/common/mod.rs`, `crates/cli/tests/convert.rs`  
- **Commit:** `feat(cli): add convert command`

commands::convert::run(args, ctx). Resolve --to with formats::lookup_target (unknown -> "morphring: unknown format 'x'." exit 2; read-only -> "MorphRing reads X but can't write it." exit 2). Expand inputs (T16) against ctx.cwd with is_known = formats::detect; report every missing input as 'morphring: not found: <x>' and exit 2 before converting; zero files -> 'morphring: no files to convert.' exit 3. OutputRule: SameFolder by default, -o -> CustomFolder (flat); --overwrite/--skip set the policy; -q sets ConvertOptions.quality. Run core::job::run_job with ctx.engines and ctx.cancel; print human lines to ctx.out as files finish (suppressed by --quiet), then the summary; --json prints only the JSON document (beats --quiet). Exit 0 when nothing failed or was cancelled, else 1. crates/cli/tests/common/mod.rs: the CLI's own small require_engine! helper (the engines crate's tests/common is not visible here) and fixture(name) via concat!(env!("CARGO_MANIFEST_DIR"), "/../engines/tests/fixtures/"). Tests call run_with() with a TempDir cwd and injected engines.

**Acceptance:**

- [ ] cargo test -p morphring --test convert passes (engine-dependent cases skip when the tool is missing)
- [ ] cargo run -q -p morphring -- convert crates/engines/tests/fixtures/sample.png --to jpg -o $T exits 0 and $T/sample.jpg exists
- [ ] running the same command again writes '$T/sample (2).jpg'; with --skip it prints a 'skip' line and exits 0; with --overwrite it replaces sample.jpg
- [ ] cargo run -q -p morphring -- convert crates/engines/tests/fixtures/sample.mp4 --to jpg -o $T --json | jq -e '.converted == 1' (two-hop route)
- [ ] convert missing.png --to jpg exits 2 with 'not found: missing.png'
- [ ] convert on an empty folder exits 3
- [ ] convert notes.unknownext --to pdf exits 1 with a 'fail' line naming the unsupported type
- [ ] convert sample.png --to raw exits 2 with 'reads' and "can't write"
- [ ] test: with ctx.engines = [FakeConverter docx->pdf with status Missing { hint: 'pacman -S libreoffice-fresh' }], converting a .docx to pdf exits 1 and the fail line contains 'needs' and 'pacman -S libreoffice-fresh'
- [ ] cargo clippy -p morphring --all-targets -- -D warnings is clean

#### T33: formats command

- **Wave:** 5  
- **Depends on:** T15, T30  
- **Owns:** `crates/cli/src/commands/formats.rs`  
- **Commit:** `feat(cli): add formats command`

commands::formats::run(args, ctx). 'morphring formats' lists the registry grouped by category: '  {id:<10} {Name}[ (read only)]  .ext ...', folder excluded. 'morphring formats <fmt>' lists targets reachable with installed engines (Planner over ctx.engines), then 'With optional engines' targets that need a missing engine (planner with assume_all_available) plus that engine's install hint. Unknown format -> exit 2. Port the formats output of src/Filee.Cli/Cli.cs. Unit tests in the file use run_with-style Ctx with FakeConverter engines.

**Acceptance:**

- [ ] cargo test -p morphring formats passes
- [ ] cargo run -q -p morphring -- formats exits 0 and lists 'png' and 'docx', with '(read only)' next to raw
- [ ] cargo run -q -p morphring -- formats png lists jpg and webp when magick is installed
- [ ] test: with a Missing fake engine, 'formats docx' shows pdf under 'With optional engines' with the hint
- [ ] cargo run -q -p morphring -- formats nope exits 2
- [ ] cargo clippy -p morphring --all-targets -- -D warnings is clean

#### T34: doctor command

- **Wave:** 5  
- **Depends on:** T15, T30  
- **Owns:** `crates/cli/src/commands/doctor.rs`  
- **Commit:** `feat(cli): add doctor command`

commands::doctor::run(args, ctx), reading statuses from ctx.engines. One line per engine: 'ok       magick   7.1.2-32  /usr/bin/magick' or 'missing  soffice  install: pacman -S libreoffice-fresh', then the config and temp folders MorphRing would use, then 'N of 3 engines available.'. --json prints [{id, status, path, version, hint}]. Exit 0 even when engines are missing (doctor is informational); exit 1 only on an internal error. No network.

**Acceptance:**

- [ ] cargo test -p morphring doctor passes
- [ ] cargo run -q -p morphring -- doctor exits 0 and shows 'ok' for magick, ffmpeg and soffice on the dev machine
- [ ] unit test: three Missing fake engines print three 'missing' lines with their install hints and return 0
- [ ] cargo run -q -p morphring -- doctor --json | jq -e 'length == 3'
- [ ] cargo clippy -p morphring --all-targets -- -D warnings is clean

#### T36: CLI integration tests

- **Wave:** 6  
- **Depends on:** T32, T33, T34  
- **Owns:** `crates/cli/tests/cli.rs`  
- **Commit:** `test(cli): port Filee CLI integration tests`

Port the tests/Filee.Cli.Tests/CliTests.cs cases that fit v0.1.0 against morphring::run_with() in-process with Vec<u8> buffers and a TempDir cwd, reusing crates/cli/tests/common (from T32): 1 converts files and wildcards into an output folder; 3 json output lists every file; 4 a folder converts the files it knows; 5 a file that cannot be converted fails with exit 1; 6 wrong arguments explain themselves with exit 2 (minus preset rows); 7 a missing input is reported; 10 formats listed (minus presets); 12 case-distinct inputs are both converted and exact duplicates removed (#[cfg(target_os = "linux")]); 13 unicode, spaces and quotes stay in a single input argument. 11 help and version need no engines: run as a subprocess, Command::new(env!("CARGO_BIN_EXE_morphring")).env("PATH", <empty TempDir>), never std::env::set_var (D9). Engine-dependent tests skip with 'SKIP: magick not found'. Keep-dates, watch and preset tests are deferred to v0.2 and listed in a comment.

**Acceptance:**

- [ ] cargo test -p morphring --test cli passes on the dev machine with zero SKIP lines
- [ ] the help/version test passes with PATH set to an empty temp dir, via a subprocess
- [ ] grep -R 'set_var' crates/ returns nothing
- [ ] cargo clippy -p morphring --all-targets -- -D warnings is clean

#### T31: README

- **Wave:** 6  
- **Depends on:** T32, T33, T34  
- **Owns:** `README.md`  
- **Commit:** `docs: add README`

Written after the commands exist, so every example is copied from real output. What MorphRing is (drag-to-convert, offline, ring menu; Rust port of Filee with credit and link), status (v0.1.0 is the CLI; the desktop ring follows in v0.1.1), install (build from source with cargo --locked; release tarball with sha256sum -c; required tools with pacman/apt commands: ffmpeg, imagemagick, libreoffice-fresh), usage examples (png to jpg, a folder of wavs to mp3 into -o, --json piped to jq, formats, doctor), exit code table 0/1/2/3, note that -q means quality, privacy (no network), development (fmt/clippy/test, links to docs/), licence. surani's voice is fine in prose; command blocks and their shown output stay exact and joke-free.

**Acceptance:**

- [ ] every command in README code blocks runs against target/debug/morphring with the documented exit code (checked by hand or a throwaway script, not committed)
- [ ] README links docs/PLAN.md, docs/ENGINEERING.md, LICENSE and THIRD-PARTY-NOTICES.md and they exist
- [ ] README contains the exit code table and credits KnifeLemon/Filee

#### T37: Release v0.1.0: changelog and verification

- **Wave:** 7  
- **Depends on:** T02, T03, T19, T20, T31, T36  
- **Owns:** `CHANGELOG.md`  
- **Commit:** `chore(release): prepare v0.1.0`

Write CHANGELOG.md (Keep a Changelog) with '## [0.1.0] - 2026-10-15' listing Added items actually merged (CLI convert/formats/doctor/help/version, engines magick/ffmpeg/soffice, exact planner, output naming with the Overwrite fix, missing-engine hints, parallel runner, CI, release workflow), 'Known limitations' (Linux-first; Windows/macOS untested; no presets, watch, PDF input or desktop app yet; v0.1.1 brings the ring preview) and 'Credits' (Filee). Run the full verification list in PLAN.md section 8, re-run every README example against the release binary and fix README in a follow-up docs commit if anything differs. Results go in the PR body, not in a file.

**Acceptance:**

- [ ] cargo fmt --all --check, cargo clippy --workspace --all-targets --locked -- -D warnings and cargo test --workspace --locked all pass
- [ ] cargo build --release --locked -p morphring and target/release/morphring --version prints 'morphring 0.1.0'
- [ ] the release verification script in PLAN.md section 8 passes
- [ ] every README command matches the release binary's behaviour
- [ ] CHANGELOG.md has a '## [0.1.0]' heading that the release.yml awk extraction prints non-empty

#### T38: Tag v0.1.0 and publish draft release

- **Wave:** 8  
- **Depends on:** T37  
- **Owns:** no files (orchestrator only)  
- **Commit:** `chore(release): tag v0.1.0`

Orchestrator only, no file changes: create the annotated tag v0.1.0 on the T37 commit, push main and the tag (with the user's approval), watch release.yml, download the tarball from the draft release, verify SHA256SUMS.txt and run the smoke test on it, then publish the draft (user approval required before publishing).

**Acceptance:**

- [ ] git tag -l v0.1.0 shows the tag on the release commit
- [ ] gh run list --workflow release.yml shows success for the tag
- [ ] sha256sum -c SHA256SUMS.txt passes on the downloaded tarball
- [ ] the extracted morphring --version prints 'morphring 0.1.0'

## 7. v0.1.1: the desktop preview (after the tag)

These tasks were in the first draft as "stretch". They are off today's critical path, so nobody reviews or tests them before `v0.1.0` is tagged. They start once T38 is done, in this order. Each keeps the same rules (worktrees, owned files, one commit).

| Wave | Task | Owns | Notes |
|---|---|---|---|
| A | T08 Ring geometry (pure) | `crates/core/src/ring.rs` + `pub mod ring;` line in `crates/core/src/lib.rs` | Port `DonutGeometry` (docs/reverse-engineering/05): angles clockwise from 12 o'clock, `index_at`, `hit_test` with 7 px slack, wedge path with a constant 3 px gap. Port `tests/Filee.App.Tests/DonutGeometryTests.cs`. |
| A | T18 Context menu builders (Dolphin first) | `crates/platform/**` (incl. its `Cargo.toml`) | Pure string builders plus atomic writes for the Dolphin service menu (mode 755), Nautilus script, Nemo action and Thunar sendto file, all running `<app> --convert %F`, each with an `X-MorphRing-Managed=true` marker. `install(exe, home, dry_run)`: `home` is a parameter, never read from the env in tests (D9). |
| A | T21 Tauri app scaffold with drop zone | `app/**` | Own cargo workspace (D1). **Pre-registers every plugin and command as a stub** in `app/src-tauri/src/lib.rs`: `tauri-plugin-single-instance`, `tauri-plugin-opener`, and `invoke_handler![convert, ring_close, ping]`, plus empty `tray.rs`, `instance.rs` and `commands.rs` modules, so T27, T28 and T35 never edit `lib.rs`. Acceptance tolerates "SKIP: webkit2gtk-4.1 missing" on machines without it (it is installed here). |
| B | T26 Ring slice suggestions | `crates/core/src/slices.rs` + its `pub mod` line | Target formats from a favourites list (D2); disabled slices carry the D10 reason. |
| B | T27 Ring window UI (click mode) | `app/ui/ring.*` | SVG donut, spring hover, easeOutBack open, reduced motion, progress ring. Centred on Wayland. |
| B | T28 Tray, single instance, `--convert` | `app/src-tauri/src/tray.rs`, `instance.rs` | 350 ms debounce with an injected clock. |
| B | T29 `integrate` command | `crates/cli/src/commands/integrate.rs`, `commands/mod.rs`, `args.rs` | Adds the subcommand. `home` and the allow flag (release build or `--allow`) are passed into `run()` as parameters, never read with `set_var`/`var` in tests (D9). |
| C | T35 App commands: ring model, convert, progress | `app/src-tauri/src/commands.rs` | Fills the stubs T21 registered. One FIFO worker thread; `spawn_blocking`; never blocks the UI. |

v0.1.1 also adds the Tauri `.deb`/`.AppImage` build to `release.yml` only once it builds green in CI's `app` job three times in a row.

## 8. Release verification (run in T37, again on the tarball in T38)

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked              # on the dev machine: zero SKIP lines expected
cargo build --release --locked -p morphring
M=target/release/morphring; F=crates/engines/tests/fixtures; T=$(mktemp -d)
$M --version                                 # morphring 0.1.0
$M doctor                                    # ok magick / ffmpeg / soffice
$M convert $F/sample.png --to jpg -o $T                 # exit 0
$M convert $F/sample.wav --to mp3 -o $T                 # exit 0
$M convert $F/sample.mp4 --to gif -o $T                 # exit 0 (ffmpeg, cost 15)
$M convert $F/sample.mp4 --to jpg -o $T --json | jq -e '.converted == 1'   # two hops
$M convert $F/sample.docx --to pdf -o $T                # exit 0 (soffice)
$M convert $F/notes.unknownext --to pdf -o $T; test $? -eq 1
$M convert $F/missing.png --to jpg; test $? -eq 2
$M convert "$(mktemp -d)" --to jpg; test $? -eq 3
$M formats png | grep -q webp
env PATH=/nonexistent $M --version           # help/version need no engines
```

Then re-run every command in README.md against `$M` and compare the output with what the README shows.

## 9. Roadmap to Filee parity

Each release follows the same rules: waves, worktrees, one commit per task, tests first for anything pure.

### v0.1.0 (released): "It converts"
Linux CLI with ImageMagick, FFmpeg and LibreOffice, the exact planner, safe output naming, missing-engine hints, parallel jobs, CI and a draft release.

### v0.1.1 (released): "The ring, as a preview"
Section 7: Tauri drop zone, click-mode ring centred on Wayland, tray, single instance, `--convert`, `morphring integrate` for Dolphin (plus Nautilus, Nemo, Thunar files, untested here).

### v0.2: "Presets and the real app" (done on main; R-03 cuts the v0.2.0 release)
Planned and tracked task by task in [PLAN-v0.2.md](PLAN-v0.2.md); every task in its section 7 up to R-02 has landed. What it delivered:
- [x] Presets: Filee's built-ins (94 by now, after the v1.11.0 additions), `Preset` with image/pdf/document/media/archive options and output rule, `--preset`, `morphring presets`, toolbar profiles and the profile selector, ring slices from presets (replaces D2).
- [x] Settings store: settings.json, presets.json, profiles.json and history.json in the config dir, atomic writes, `.broken` copies, `schemaVersion` 1 (settings.json moved to 2 with the rest of Filee's settings). Default output location, `--keep-dates` (creation time too on Windows and macOS).
- [x] Process runner stall watchdog, FFmpeg's full remux fit matrix and hvc1 tagging, estimated progress for silent tools (D8), FFmpeg `MediaOptions`, ImageMagick `ImageOptions`.
- [x] PDF: Poppler (pdftoppm, pdftotext) and Ghostscript engines with page ranges, images and SVG to PDF with krilla, page ranges, split and merge with lopdf, the job-level merge mode.
- [x] Native engines behind cargo features, ahead of magick in the registry: `image`, `resvg` (plus svg<->svgz), zip/tar/tgz extract and pack with Filee's archive safety rules.
- [x] Pandoc engine with Filee's reader/writer tables and costs 10-14.
- [x] `watch` command (notify, 1 s tick, 5 s settle, 2 s on Windows); history capped at 200.
- [x] App: ring from presets with the preset's options and default output, cancel from the ring, settings page, toast for long jobs, history view. AUR `morphring-bin` PKGBUILD (not published). ImageMagick 7 and LibreOffice in CI, and Windows, macOS and MSRV CI jobs blocking.
- [x] `GestureDetector` ported as a pure module with its tests.
- [x] Job modes: combine-into-one archive and multi-page TIFF run in the job runner (an engine offering a `Combiner` or `TiffMerger` still has to arrive before their presets light up).
- [x] Filee parity in core: the rename steps in output rules, retry plans from history entries, the rest of Filee's app settings (settings.json `schemaVersion` 2), the subtitles, data and 3D toolbar profiles. CLI: `--json`'s `preset` field, `--quiet` on stderr, "/?" for help, and `presets export/import/reset` declared (not available until the library bundle lands).

Pulled in from v0.3:
- [x] Ctrl+drag on Linux: an evdev watcher (needs the `input` group; `MORPHRING_NO_GESTURE` turns it off, `MORPHRING_GESTURE_ANYWHERE` drops the file-manager check), touchpad tap-and-drag, and the ring under the cursor through Hyprland's IPC. Not the libinput/XInput2 backends or the layer-shell overlay below; other compositors get a centred ring.
- [x] Theming: the app's own palette (ink #0e0e0e, cream #f1ecdf, tomato #f04a35) as CSS tokens, light and dark pages, and a ring icon. Accent-derived user themes stay in v0.3.

Pulled in from v0.4 (see PLAN-v0.2.md section 4):
- [x] Windows: Job Object kill in the process runner, rename retries on sharing violations, path and test fixes, a static C runtime, gswin64c and soffice.com discovery with winget/scoop hints, the HKCU Explorer verb plus Send To, ring topmost fixes, a current-user NSIS installer and a CLI zip.
- [x] macOS: the transparent ring (macos-private-api), Accessory tray mode, Finder's Opened event with a cold-start queue, a template tray icon, a Finder Quick Action, an ad-hoc signed .dmg and an arm64 CLI tarball, brew hints and LibreOffice.app discovery.
- [x] `release.yml`: build jobs for all three OSes and one publish job that makes the draft.

Deferred from v0.2 (reasons in PLAN-v0.2.md section 5): the Windows 11 top-level menu (IExplorerCommand), the MSI, code signing and notarization, the Homebrew tap, the engine downloader, the pure-Rust PDF renderers for machines without Poppler, low-level mouse hook / event tap gesture backends, the update check and updater, the engines behind combine-into-one archive and multi-page TIFF, CombinePages, read-only archives through bsdtar, Pandoc's `--sandbox`, preset/profile/watch-folder editors and history retry in the app, engine priority settings, universal or Intel macOS and Windows ARM64 builds, AUR publishing.

Filee parity work continues in [PLAN-v0.2-parity.md](PLAN-v0.2-parity.md) (PX-01 to PX-196, measured against Filee v1.11.0): it fills the remaining core placeholders (tuning, library bundle and migrations; rename and retry are in) and adds the engines whose dependencies are already declared, which moves several v0.3 and v0.5 items below forward.

### v0.3: "The ring at the cursor"
- Linux gestures: an input-backend trait; libinput backend for Wayland (opt-in, needs the `input` group), x11rb XInput2 backend for X11; ring on a gtk-layer-shell overlay surface (docs/research/ring-overlay-and-gesture.md), Hyprland `cursorpos` IPC for placement; GlobalShortcuts portal hotkey via `ashpd`.
- Drag mode: invisible drop-target window at the cursor, spring animations, progress ring hand-off to toasts (docs/reverse-engineering/05).
- More converters: spreadsheets (calamine + rust_xlsxwriter + Filee's CSV rules), OOXML variant rewrite with macro stripping, EML to HTML/TXT/ZIP, archives via bsdtar for 7z/rar/iso/cab/deb/rpm, merge and multi-page modes.
- i18n plumbing (English plus Filee's ko/zh-CN keys), theming with accent-derived tokens, reduced motion.

### v0.4: "Windows and macOS"
- Windows: Job Object kill in the process runner, low-level hook backend, registry verb + Send To, Windows 11 menu via a Rust `IExplorerCommand` cdylib, NSIS/MSI installer, engine downloader with pinned SHA-256 (`src/Filee.Engines/Infrastructure/EngineInstaller.cs`).
- macOS: event-tap backend, Services provider via objc2, `.dmg`, notarization if a certificate is available.
- Tauri bundles for all three OSes in `release.yml`. Opt-in update check (GitHub releases), then `tauri-plugin-updater` once builds are signed.

### v0.5: "Long tail"
Fonts (TTF/OTF/WOFF/WOFF2/EOT, CFF to TTF), e-books (EPUB reader/writer, calibre for Kindle formats), HWP/HWPX via rhwp, HEIC writer for platforms without libheif, ICNS, CAD (DXF first; DWG only via an external converter).

### v1.0: parity
Every Filee format edge and preset covered or explicitly declined in a parity table, all of Filee's tests ported, signed builds on three OSes.

## 10. Risks for today

| Risk | Mitigation |
|---|---|
| Wave 2 is wide (14 tasks) and the T01 contract turns out wrong | The pragmatic team reviews T01 before wave 2 starts. A contract change after that is one orchestrator commit merged before the next wave. |
| Parallel developers break each other's builds | One worktree and one `target/` per developer, `-p <crate>` checks per task, full `--workspace --locked` checks by the orchestrator after each merge (section 1). |
| soffice hangs or fails silently when two run at once | Per-worker profiles (T25), 180 s timeout with group kill (T09), success judged by the output file. |
| Ubuntu CI has ImageMagick 6 only; Ubuntu 22.04 has ffmpeg 4.4 | D7: magick and soffice tests skip in CI and run locally before tagging. The release smoke test is the single wav → mp3 conversion, whose flags exist in 4.4 (T12). |
| `cargo deny` rejects a licence, or a fresh advisory turns CI red | Licences block with the allow-list in T19; advisories don't (D11). The app is outside the root workspace (D1). |
| Windows or macOS tests fail | Non-blocking for v0.1.0 (D11). Unix-only tests are `#[cfg(unix)]`, case-sensitivity tests `#[cfg(target_os = "linux")]`. |
| A tag push releases untested code | `release.yml` runs `cargo test --locked` before packaging (T20). |
| Running out of day | Section 11 cut line. |

## 11. Cut line and process rules

- **If we are running late,** cut inside tasks rather than whole tasks, in this order: the "With optional engines" list in `formats <fmt>` (T33), `doctor --json` (T34), then the LibreOffice engine (T25: drop it from `registry::all()` and say so in the CHANGELOG; `docx → pdf` then reports "No engine can do this conversion"). Everything else is the minimum for a useful, honest release.
- Merge order inside a wave: by task id. A red task does not block the others in its wave; it blocks only its dependants.
- The orchestrator commits each merged task with the task's commit message and the attribution trailer; developers do not commit, tag or push. Pushing and publishing the release need the user's approval.
- After every wave the tester runs the whole suite on `main`, not just the new tasks.
