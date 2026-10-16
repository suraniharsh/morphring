# MorphRing v0.2 plan: "Presets and the real app", with v0.4 Windows and macOS pulled forward

Status: plan, 2026-10-16, revised after the pragmatic review (section 8). Source of truth for the phase 2 task graph. The roadmap is in PLAN.md section 9 (v0.2 and v0.4), the rules in ENGINEERING.md, the code layout in ARCHITECTURE.md, and the research in docs/research/v02-engines-and-crates.md, windows-support.md and macos-support.md.

## 1. How this plan runs

- Every task is one Conventional Commit of under about an hour, made in its own git worktree off `main`. A task starts as soon as every task in its `deps` has landed on `main`.
- **Tasks that can run at the same time own disjoint files.** A script checked this over the whole graph: 0 conflicts. Shared files (`lib.rs` mod lines, Cargo manifests and lockfiles, CLI `args.rs`, the app's `lib.rs`, `tauri.conf.json`, the engine registry, `discovery.rs`, `process.rs`, `formats.rs`) are pre-filled by skeleton tasks: P2-01 (deps, MSRV, deny), P2-02a/P2-02b (core), P2-03a/P2-03b (engines), P2-05 (CLI), P2-06 (app) and XP-02 (platform). Later tasks only fill files they own.
- Each engine task pins its routes in its **own** `tests/routes_<engine>.rs` (or its own golden file). `routes.rs` keeps only the pins that stay true through v0.2, and `routes_images.rs` belongs to P2-31.
- Developer checks per task: `export CARGO_BUILD_JOBS=2`, then `cargo fmt --all --check`, `cargo clippy -p <crate> --all-targets -- -D warnings` and `cargo test -p <crate>`, plus the task's acceptance commands. App tasks run `cargo test/clippy --manifest-path app/src-tauri/Cargo.toml` and `node --test app/ui/`. After each merge the orchestrator runs the full `--workspace --locked` checks on `main` and reverts a red merge.
- **Windows and macOS** are reachable only through GitHub Actions. A task with `needs_ci_os` is verified by the tester on a pushed branch (`gh run watch`). Every such task depends on XP-01, so the jobs exist and have engines installed; release.yml tasks (XP-08, XP-09, XP-12) use their own runners. CI stays non-blocking until XP-14.
- **Checks an agent cannot run** (Hyprland ring behaviour without a reachable session, anything on a real Mac or Windows desktop) never stall a task: the tester records each as `Unverified: <check>` in the commit body, and R-03 turns those lines into the README's 'Not yet verified on real hardware' list (at least macOS ring transparency, the Quick Action, the Opened event, Windows topmost behaviour, the Explorer verb).
- Release tags: the orchestrator tags **v0.1.1** as soon as R-01 lands and **v0.2.0** after R-03; R-04 then bumps the AUR package. Both release workflows also run by `workflow_dispatch` on a branch, so the tester checks them before tagging. The Tauri CLI version lives in one place: `TAURI_CLI_VERSION` at the top of release.yml.
- P2-19 (gesture port) has no consumer until v0.3: schedule it only when a slot is free; nothing waits on it except XP-13.
- Machine: Arch/Hyprland, 8 shared CPUs. Installed: ffmpeg, magick 7, soffice, pandoc 3.11, gs 10.08, pdftoppm/pdftotext/pdfinfo 26.08, bsdtar, makepkg, node/npx, webkit2gtk-4.1. Missing: calibre, cargo-tauri (use `npx @tauri-apps/cli@$TAURI_CLI_VERSION`), cargo-deny, actionlint, plutil (CI checks these).

## 2. Key decisions

| Decision | Choice | Why |
|---|---|---|
| Registry order (penalty = min(index, 9)) | image, resvg, pdf, magick, poppler, archive, ffmpeg, pandoc, ghostscript, soffice | Native engines go ahead of magick (PLAN v0.2); poppler beats Ghostscript for PDF; LibreOffice stays last. Set once (P2-03b), so penalties never shift again in v0.2. |
| Preset options | `ConvertOptions { quality, image, pdf, document, media, archive }` (P2-02b `options.rs`) | Engines read one struct. `quality` stays the `-q` override. `--to` keeps v0.1 defaults. |
| Same-as-source presets (`*`) | `job::Target::{Format, SameAsSource}` | half-size and grayscale need the target per file. |
| Engine declines unsupported options | `Converter::accepts(&ConvertOptions)`; the job planner drops engines that decline | The native `image` engine never silently ignores a preset option; magick takes over. |
| Merge PDF | `Converter::merger()` + core `job/merge.rs` + lopdf merger; slices get `ReachCtx { reach, can_merge }` | Filee's IPdfMerger mode, with the 'output on a failed file 1' quirk fixed. `Reach` stays an enum. |
| Format ids | Add `qoi`; keep `ppm` as the one id for ppm/pgm/pbm/pnm (P2-02a) | Every engine edge must name an existing id; splitting ppm buys nothing. |
| Settings | `settings.json`, `presets.json`, `profiles.json`, `history.json` in the ProjectDirs config dir, schemaVersion 1, atomic writes, `.broken` copies | ENGINEERING.md section 7, ported from Filee's UserDataStore. |
| MSRV | 1.92 for both workspaces (krilla, krilla-svg), checked with `cargo +1.92` on root and app | Also fixes the let-chain gap (cli inputs.rs). An `msrv` CI job guards it. |
| PDF fixtures | sample.pdf made by LibreOffice from a flat ODT, Latin-1 text only | Embedded fonts with ToUnicode, so pdftotext gets the text back; no CJK font on CI. |
| PDF on Windows/macOS | poppler and Ghostscript as external tools, with install hints | pdfium-render and the engine downloader are deferred. |
| Windows menu and installer | HKCU verb + Send To, no admin, from `integrate` and the NSIS hooks; NSIS current-user only | The Win11 top-level menu needs signing; the MSI skips hooks.nsh (both deferred). |
| macOS | macos-private-api for the transparent ring, Accessory tray mode, `RunEvent::Opened` with a PendingOpen queue pre-registered by P2-06, a Quick Action, an ad-hoc dmg | macos-support.md. Notarization is deferred. |
| Release | build jobs per OS, then one publish job; pinned runner labels checked for arch, fallback macos-15 / windows-latest | A draft never gets half its assets. |
| CI ImageMagick on Linux | AppImage from ImageMagick/ImageMagick GitHub release assets at a fixed tag + SHA-256 | imagemagick.org/archive/binaries keeps only the latest build, so a pinned URL there would 404. |

## 3. Waves (earliest start, assuming every dependency lands quickly)

| Wave | Tasks |
|---|---|
| 1 | R-01, P2-00, XP-01, P2-02a, P2-04 |
| 2 | XP-02, P2-01, P2-60, XP-08, P2-61, P2-02b, P2-14, P2-15 |
| 3 | P2-16, P2-19, XP-06, XP-11, P2-05, P2-06, P2-10, P2-12, P2-17, P2-03a |
| 4 | P2-03b, P2-11, P2-13, P2-20, P2-21, P2-22, P2-24, P2-33, XP-03, XP-04, XP-07, XP-10, XP-09 |
| 5 | P2-18, P2-23, P2-25, P2-37, P2-27, P2-28, P2-30, P2-31, P2-32, P2-34, P2-36, P2-40, P2-41, P2-52, P2-53, XP-05, XP-12 |
| 6 | P2-26, P2-35, P2-29, P2-42, P2-50 |
| 7 | P2-51 |
| 8 | P2-54 |
| 9 | XP-13 |
| 10 | XP-14 |
| 11 | R-02 |
| 12 | R-03 |
| 13 | R-04 |

Critical path (13 deep): P2-02a -> P2-02b -> P2-10 -> P2-11 -> P2-52 -> P2-50 -> P2-51 -> P2-54 -> XP-13 -> XP-14 -> R-02 -> R-03 -> R-04 (P2-18 runs alongside P2-52). Splitting P2-02 and P2-03 lets the pure core tasks (P2-14, P2-15) start in wave 2 and the engine unit-level tasks (P2-20/21/22/24, P2-33, XP-03/04) in wave 4, before the registry re-pin. Waves 3 to 5 run 10 to 17 tasks each: throttle to the CPU budget.

## 4. Scope

- v0.1.1 release: the Tauri ring app bundled as .deb and .AppImage (Linux x86_64) in release.yml, a pinned Tauri CLI checked against the tauri crate, CHANGELOG 0.1.1, versions 0.1.1 (R-01).
- Cross-OS CI first: the root workspace and the Tauri app are built, linted and tested on ubuntu, windows and macos with engines installed (XP-01). Every task with a Windows/macOS CI check depends on XP-01. Windows and macOS stay non-blocking until XP-14.
- Skeleton tasks that pre-declare every shared registration: workspace deps, MSRV 1.92 and deny.toml (P2-01); core mod lines, stubs and the qoi format id (P2-02a); option types, Target, accepts/merger and job hooks (P2-02b); engine stubs, discovery tools and process hooks (P2-03a); registry order and the route-pin split (P2-03b); CLI args and command stubs (P2-05); app commands, windows, events and the lifecycle hook with PendingOpen state (P2-06); per-OS integrate dispatch in platform (XP-02).
- Presets: the 68 Filee built-ins, `Preset` with image/pdf/document/media/archive/output options, `--preset`, `morphring presets`, toolbar profiles with ProfileSelector, ring slices from presets via a ReachCtx with can_merge (replaces D2).
- Settings store: settings.json, presets.json, profiles.json, history.json in the config dir, atomic writes, `.broken` copies, schemaVersion 1; default output location; `--keep-dates` (mtime via filetime, creation time on Windows/macOS files).
- Process runner stall watchdog, fake progress for silent tools (D8), FFmpeg remux fit matrix with hvc1 tagging, FFmpeg MediaOptions, ImageMagick ImageOptions.
- PDF: PageRange, pdftoppm (PDF to image) and pdftotext (PDF to text, UTF-8 BOM), Ghostscript (PDF/EPS/PS/AI to image, fallback), krilla image to PDF, krilla-svg SVG to PDF, lopdf page range/split and merge, the job-level merge mode.
- Native engines behind cargo features: `image` (PNG/JPEG/GIF/BMP/TIFF/ICO/QOI/PPM-family/TGA ahead of magick), `resvg` for SVG, zip/tar/tgz archives with Filee's ArchiveSafety rules.
- Pandoc engine with Filee's reader/writer tables and costs 10/14.
- `morphring watch` with notify plus Filee's 1 s tick, 5 s settle (2 s on Windows), handled keys (bounded) and move-originals; history capped at 200.
- App: ring from presets with preset options and default output, cancel from the ring, settings page, history view, toast for long jobs.
- GestureDetector ported as a pure module with Filee's tests (platform crate), low priority, ready for v0.3.
- Packaging: AUR `morphring-bin` PKGBUILD with real checksums (files only; R-04 bumps it to 0.2.0 after the tag), ImageMagick 7 (GitHub release AppImage, pinned tag + SHA-256) and LibreOffice in Linux CI.
- v0.4 pulled forward, Windows: path and test fixes, Job Object kill, `move_into_place` retry on sharing violations, winget/scoop hints, gswin64c and soffice.com discovery, Explorer HKCU verb plus Send To, ring topmost fixes, NSIS current-user installer, Windows CLI zip.
- v0.4 pulled forward, macOS: transparent ring through macos-private-api, Accessory tray mode, RunEvent::Opened/Reopen with a cold-start queue, template tray icon, Finder Quick Action, ad-hoc signed .dmg, macOS arm64 CLI tarball, brew hints and LibreOffice.app discovery.
- Release: multi-OS release workflow (build jobs plus one publish job, runner labels checked), Windows/macOS/MSRV CI blocking, docs, README/CHANGELOG 0.2.0 with a 'not yet verified on real hardware' list, the version bump. The orchestrator tags v0.1.1 after R-01 and v0.2.0 after R-03.
- Plan document: /home/surani/Projects/surani/morphring/docs/PLAN-v0.2.md (65 tasks; file ownership checked by script: 0 conflicts between tasks that can run concurrently; critical path 13 deep).

## 5. Deferred (with reasons)

- Windows 11 top-level Explorer entry (IExplorerCommand cdylib in a sparse package): an unsigned build needs an all-users admin install, so it waits for a code-signing certificate.
- Windows MSI installer: Tauri's MSI is per-machine and does not run hooks.nsh, so an MSI install would have no Explorer verb or Send To entry, and it doubles the WiX surface. v0.2 ships NSIS current-user only.
- Code signing and notarization (SignPath or Azure Artifact Signing on Windows, Developer ID plus notarytool on macOS): needs paid accounts and secrets the owner has not set up. v0.2 ships unsigned or ad-hoc signed builds labelled preview.
- Homebrew tap formula and cask: needs a separate public tap repo, and Homebrew disables unsigned casks since 2026-09. The README documents the tarball and cargo install instead.
- Engine downloader with pinned SHA-256 (Filee EngineInstaller): network code plus a large surface. Missing-engine hints cover v0.2.
- pdfium-render, pdf-text-native (pdf-extract) and hayro: needed for PDF on Windows/macOS without poppler. v0.2 gives poppler/Ghostscript install hints there.
- Low-level mouse hook (Windows), event tap (macOS), libinput/XInput2 gesture backends and the cursor-placed ring: v0.3 gesture work. Only the pure GestureDetector lands now.
- Opt-in update check and tauri-plugin-updater: the updater needs signed builds.
- Combine-into-one archive (zip-all), multi-page TIFF and CombinePages (pdf-long-image): v0.3 'merge and multi-page modes'. Their presets exist but their slices stay disabled with a reason.
- 7z, rar, xz, bz2, cab and the other read-only archives through bsdtar: v0.3.
- Pandoc offline guarantee (`--sandbox`): fails for docx on Arch's pandoc build. The behaviour is documented and revisited in v0.4.
- App editors: preset editor, donut/profile editor, watch-folder UI, history retry. Users can edit presets.json and profiles.json by hand in v0.2.
- Engine priority and 'prefer LibreOffice' settings: edges are static, so this needs a planner override.
- Universal or Intel macOS builds, and Windows ARM64.
- Publishing to AUR: needs the owner's AUR account. P2-60/R-04 commit the PKGBUILD only.
- Click-through on the transparent Windows ring: Tauri #11461 was closed as not planned. The ring is hidden when not in use instead.
- CJK text in the PDF fixtures and goldens: no CJK font is guaranteed on CI runners, so the PDF-to-text golden checks Latin-1 text only.
- Removing the duplicate ring geometry (core::ring vs app/ui/ring.math.js): both copies are tested, and removing one has no user-visible value.

## 6. Risks

| Risk | Mitigation |
|---|---|
| Skeletons block most of the graph | They are split small (P2-02a/b, P2-03a/b) and mechanical. A stub that fails to compile is fixed in the skeleton, never in a dependant. |
| Hidden Windows/macOS failures behind fail-fast | XP-01 adds `--no-fail-fast`; XP-13 sweeps up everything left, owning ci.yml and all test code, before XP-14 blocks. |
| A new crate needs a newer Rust than 1.92 | P2-01 runs `cargo +1.92 check` on both workspaces and checks each crate's publish date itself. |
| Wrong Tauri CLI pin passes silently | R-01's Check Tauri CLI step compares `tauri info` with Cargo.lock and fails on a mismatch; one env var serves every job. |
| Pinned runner label missing or wrong arch | XP-08 prints sw_vers/uname and fails unless arm64; fallbacks macos-15 and windows-latest. |
| Native image engine changes golden-test routes | The magick goldens in cli/app assert output bytes, not the engine; P2-31 runs them and stops rather than editing files it does not own. |
| tauri-build rejects macos-private-api without the matching config | XP-10 changes the Cargo feature and `tauri.macos.conf.json` in one commit; CI on macOS proves it. |
| Ring is white in a bundled macOS .app (tauri#13415) | XP-12 builds the dmg in CI. Transparency stays in the README's 'not yet verified' list if nobody tests on a Mac. |
| Hand-written Finder .wflow rejected by macOS | `plutil -lint` in CI. Fallback: document 'Open With', which comes free with XP-10's fileAssociations. |
| lopdf merge is our own code | Tests on fixtures; encrypted input refused; a merge error fails cleanly. |
| Archive extraction security | Our own loop with Filee's safety rules, plus in-test hostile archives (P2-33, P2-34). |
| Pandoc may fetch network images (offline principle) | Documented; `--sandbox` revisited in v0.4. |
| Flaky timing tests on shared CI runners | The Windows lock test is `#[ignore]`; watch tests use generous timeouts; the Job Object test polls instead of sleeping. |
| CI time (brew installs, Windows builds) | Windows timeout 25 min, LibreOffice in its own non-blocking job, caches only when needed. |

## 7. Tasks

### R-01: Release 0.1.1: bundle the ring app (.deb, .AppImage) and bump versions

- **Deps:** none
- **Commit:** `chore(release): bundle the ring app and prepare v0.1.1`
- **CI OS check:** none (Linux only)
- **Owns:** `.github/workflows/release.yml`, `CHANGELOG.md`, `Cargo.toml`, `Cargo.lock`, `app/src-tauri/Cargo.toml`, `app/src-tauri/Cargo.lock`, `app/src-tauri/tauri.conf.json`

Goal: v0.1.1 ships the Tauri ring app next to the CLI. Do this first, because P2-01 also edits Cargo.toml and the lockfiles.
1. Versions: in the root Cargo.toml set `[workspace.package] version = "0.1.1"`. The internal path deps keep `version = "0.1.0"`, which ^0.1.0 still matches. Set `version = "0.1.1"` in app/src-tauri/Cargo.toml and `"version": "0.1.1"` in app/src-tauri/tauri.conf.json. Refresh both lockfiles with `cargo update -w` at the root and `cargo update -p morphring-app --manifest-path app/src-tauri/Cargo.toml`. Change nothing else in the lockfiles.
2. CHANGELOG.md: add `## [0.1.1] - <date of commit>` above 0.1.0 (keep `## [Unreleased]` empty above it). Added: the desktop ring app as a preview (drop zone, click-mode ring centred on Wayland, tray, single instance, `morphring-app --convert <files>`, file manager entries from `morphring integrate` now open it), plus Linux .deb and .AppImage downloads. Fixed: the ring staying open on focus-follows-mouse compositors such as Hyprland (commit 7af37b7). Known issues: the ring cannot cancel a job yet, uses default quality and output naming, and Windows/macOS are untested. Keep a Changelog style, no jokes.
3. Tauri CLI pin: add `env: TAURI_CLI_VERSION: "<x.y.z>"` at the top of release.yml, with a comment. Pick the newest @tauri-apps/cli 2.x that is at least two weeks old (`npm view @tauri-apps/cli time --json`; on 2026-10-16 that is 2.12.1, published 2026-09-30) and that `tauri build` accepts with the tauri 2.11.6 crate in app/src-tauri/Cargo.lock. The CLI numbers do not follow the crate's. If 2.12.1 complains, fall back to 2.11.5. Every later job (XP-08, XP-09, XP-12) uses this one variable.
4. Add a `Check Tauri CLI` step before the build: run `npx --yes @tauri-apps/cli@$TAURI_CLI_VERSION info` from `app/` and fail (exit 1, with a message) unless the reported `tauri` crate version equals the one in app/src-tauri/Cargo.lock (grep the lockfile) and the CLI's major is 2. A wrong pin then cannot pass silently.
5. release.yml: keep the single `linux-cli` job and its comments (one job on purpose until XP-08). After `Smoke test`, add steps that (a) apt-install `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf libfuse2`, and (b) build the app with `npx --yes @tauri-apps/cli@$TAURI_CLI_VERSION build --bundles deb,appimage --ci`, run from `app/`, with env `APPIMAGE_EXTRACT_AND_RUN=1` so linuxdeploy runs without FUSE. Copy `app/src-tauri/target/release/bundle/deb/*.deb` and `bundle/appimage/*.AppImage` into `dist/`, renamed `morphring-app_${version}_amd64.deb` and `MorphRing_${version}_x86_64.AppImage`. Add both to SHA256SUMS.txt (one `sha256sum` call over every asset) and to `gh release create`. Smoke step: `dpkg-deb -I` on the .deb; the AppImage is executable and over 10 MB.
6. Add a `workflow_dispatch:` trigger. When `github.ref_type != 'tag'`, skip the tag/version check and `Create draft release`, and upload `dist/*` with actions/upload-artifact@v4 instead. The tester runs it with `gh workflow run release.yml --ref <branch>`. Comment why, in surani's voice (ENGINEERING.md section 9).

**Acceptance:**

- cargo build --locked -p morphring && ./target/debug/morphring --version prints 'morphring 0.1.1'
- cargo check --locked --manifest-path app/src-tauri/Cargo.toml passes; cargo test --locked --manifest-path app/src-tauri/Cargo.toml passes
- cd app && npx --yes @tauri-apps/cli@<TAURI_CLI_VERSION> build --bundles deb --ci produces app/src-tauri/target/release/bundle/deb/*.deb
- The Check Tauri CLI step logic, run locally, passes with the pinned version and fails when TAURI_CLI_VERSION is set to 1.6.0
- awk extraction from release.yml run locally on CHANGELOG.md with ver=0.1.1 prints a non-empty section
- gh workflow run release.yml --ref <branch> succeeds and the run's artifact contains the CLI tarball, the .deb, the .AppImage and SHA256SUMS.txt

### P2-00: Commit the architecture, research and v0.2 plan docs

- **Deps:** none
- **Commit:** `docs: add architecture map, v0.2 research and the v0.2 plan`
- **CI OS check:** none (Linux only)
- **Owns:** `docs/ARCHITECTURE.md`, `docs/research/macos-support.md`, `docs/research/windows-support.md`, `docs/research/v02-engines-and-crates.md`, `docs/PLAN-v0.2.md`

These docs are untracked on main. Commit them unchanged, so every worktree sees the plan and the research. Run `git status` first and add only these five paths. Do not edit their content (R-02 updates ARCHITECTURE.md at the end).

**Acceptance:**

- git ls-files docs/ lists the five files
- git show --stat HEAD touches only these five files

### XP-01: CI: build and test the root workspace and the app on Linux, Windows and macOS, with engines

- **Deps:** none
- **Commit:** `chore(ci): test the app and engines on Linux, Windows and macOS`
- **CI OS check:** windows, macos
- **Owns:** `.github/workflows/ci.yml`

Goal: give every later XP task a real Windows/macOS signal. Windows and macOS stay non-blocking (`continue-on-error`) until XP-14.
1. `test` matrix (ubuntu-latest, windows-latest, macos-latest): `cargo test --workspace --locked --no-fail-fast`, so a failing crate does not hide the others. timeout-minutes 25 on Windows, 15 elsewhere, through a matrix include.
2. Engines per OS, with every version printed so a log shows what ran:
- Ubuntu: apt `ffmpeg poppler-utils ghostscript pandoc` (ImageMagick 7 comes in P2-61).
- macOS: `brew install ffmpeg imagemagick poppler ghostscript pandoc` with HOMEBREW_NO_AUTO_UPDATE=1, HOMEBREW_NO_INSTALL_CLEANUP=1, HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK=1. No brew cache yet.
- Windows: pwsh `choco install ffmpeg pandoc ghostscript -y --no-progress`, then `echo "C:\ProgramData\chocolatey\bin" >> $env:GITHUB_PATH`, then `magick -version` (ImageMagick 7 is preinstalled).
No LibreOffice in this job (D7).
3. Replace the `app` job with a matrix over the three OSes. Ubuntu installs `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev`. Every OS sets up Node 24 (actions/setup-node@v4) and runs `cargo clippy --locked --manifest-path app/src-tauri/Cargo.toml --all-targets -- -D warnings`, `cargo test --locked --manifest-path app/src-tauri/Cargo.toml` and `node --test app/ui/`. Remove the hashFiles guards. Ubuntu becomes blocking for the app; Windows and macOS do not.
4. A runner-info step in every matrix job prints the OS version (`sw_vers` + `uname -m` on macOS, `[Environment]::OSVersion` on Windows, `lsb_release -a` on Linux), so the logs record which image ran.
5. An `msrv` job on ubuntu: read `rust-version` from Cargo.toml with grep, install that toolchain with dtolnay/rust-toolchain, run `cargo +<ver> check --workspace --locked --all-targets`. continue-on-error with a comment: it fails on the let-chains in cli/src/inputs.rs until P2-01 bumps the MSRV to 1.92, and XP-14 makes it blocking.
6. Keep fmt, clippy and deny as they are, and `--locked` everywhere. Update the header comment. Comments in surani's voice.
7. The tester writes the list of failing Windows/macOS tests from the first run into the task's notes (the orchestrator passes it to XP-02 and XP-13).

**Acceptance:**

- python -c 'import yaml; yaml.safe_load(open(".github/workflows/ci.yml"))' parses (or actionlint if installed)
- Push the branch: test (ubuntu) and app (ubuntu) are green; test/app for windows-latest and macos-latest run to completion with engines installed (failures allowed and logged)
- The failing-test list from the run is recorded in the task notes

### P2-02a: Core skeleton part 1: module stubs, mod lines and the missing format ids

- **Deps:** none
- **Commit:** `feat(core): declare v0.2 core modules and the qoi format`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/core/src/lib.rs`, `crates/core/src/formats.rs`, `crates/core/src/options.rs`, `crates/core/src/preset.rs`, `crates/core/src/builtin.rs`, `crates/core/src/profiles.rs`, `crates/core/src/settings.rs`, `crates/core/src/history.rs`, `crates/core/src/watch.rs`, `crates/core/src/page_range.rs`, `crates/core/src/dates.rs`

Goal: a tiny commit, so the pure core tasks (P2-14, P2-15, P2-16) start at once. No behaviour change.
1. lib.rs: `pub mod builtin; pub mod dates; pub mod history; pub mod options; pub mod page_range; pub mod preset; pub mod profiles; pub mod settings; pub mod watch;` (keep the existing ones and their order alphabetical).
2. Stub files, each only a `//!` doc naming the task that fills it and the Filee source it ports: options.rs (P2-02b, Filee Presets/*Options.cs), preset.rs and builtin.rs (P2-10), profiles.rs (P2-11), settings.rs (P2-12), history.rs (P2-13), watch.rs (P2-15), page_range.rs (P2-14). dates.rs also holds the signature job.rs will call: `pub fn copy_dates(_source: &Path, _outputs: &[PathBuf]) {}` (P2-16 fills it). Stubs declare no other types: every type a later task needs comes from a task in its deps.
3. formats.rs: add the ids the v0.2 engines route to and that do not exist yet:
- `rw("qoi", "QOI", Image, &["qoi"])` after tga.
- Keep `ppm` as the single id for ppm/pgm/pbm/pnm (do not split it: no test or engine needs separate ids, and the image crate writes all of them through one PNM encoder). Document in a comment that `pnm` is an extension alias of `ppm`.
- Check, and pin in a formats test, that the ids the engine tasks use exist with the right access: eps, ps, svgz, tgz, rst, tex, md, txt, html, rtf, odt, docx, tar, zip, folder are rw; ai is read-only. Fix any that are wrong in this commit.
- Add `qoi` to the image category lists wherever formats.rs builds categories, and a detect test for `x.qoi`.

**Acceptance:**

- cargo test --workspace --locked passes (no behaviour change)
- cargo test -p morphring-core formats passes with the new qoi and access pins
- cargo clippy --workspace --all-targets --locked -- -D warnings passes
- cargo run -q -p morphring -- formats qoi exits 0

### P2-04: Engine test fixtures for PDF, EPS, SVG, Markdown and archives

- **Deps:** none
- **Commit:** `test(engines): add PDF, EPS, SVG, Markdown and archive fixtures`
- **CI OS check:** none (Linux only)
- **Owns:** `scripts/make-fixtures.sh`, `crates/engines/tests/fixtures/`, `crates/engines/tests/fixtures_check.rs`, `crates/engines/tests/common/mod.rs`

Add small committed fixtures (each under 100 KB, made by us) to crates/engines/tests/fixtures/, generated by scripts/make-fixtures.sh (sources are heredocs inside the script, so the fixtures folder holds only fixtures):
- sample.pdf: 3 pages with the lines 'Page 1', 'Page 2', 'Page 3' and a Latin-1 line 'Café déjà vu' on page 1. Generate it with LibreOffice from a flat ODT written by the script (`<text:p>` per page with a `fo:break-before="page"` paragraph style, font Liberation Sans or DejaVu Sans): `soffice --headless --convert-to pdf --outdir <tmp> sample-pdf.fodt`. LibreOffice embeds a subset font with a ToUnicode map, so `pdftotext` returns the text, which a hand-written PostScript file with base fonts cannot do. No Hangul: no CJK font is guaranteed on CI.
- sample.eps: one page, a coloured shape on a transparent background, with a %%BoundingBox (hand-written PostScript is fine here: only rendering is tested).
- sample.ps: one page, hand-written.
- sample.svg: 120x80 with a rect, a circle and a text element. sample.svgz: `gzip -9n` of sample.svg.
- sample.md: a heading, a list, a link and an emphasised word.
- sample.zip and sample.tar.gz: each holds `hello.txt` and `dir/inner.txt`, made with bsdtar from files touched to a fixed mtime (`touch -d @1700000000`), with `--uid 0 --gid 0 --uname '' --gname ''`, so regeneration is byte-stable except the PDF.
The PDF embeds a creation date, so the script normalises nothing there; it is generated once and committed. Malicious archives are not fixtures: P2-34 builds them in-test.
fixtures_check.rs: every fixture exists, stays under 100 KB and starts with the right magic bytes.
tests/common/mod.rs: add `pdf_page_count(path) -> Option<usize>` (runs `pdfinfo`, None if missing), `read_text(path) -> String` (strips a UTF-8 BOM, normalises CRLF) and `is_dir_with(path, names) -> bool`. `require_engine!` must accept plain program names `pdftoppm`, `pdftotext`, `pdfinfo`, `gs` and `pandoc`: if its lookup goes through `discovery::Tool`, fall back to `which` for unknown names.

**Acceptance:**

- bash scripts/make-fixtures.sh regenerates the files; a second run changes nothing but sample.pdf
- cargo test -p morphring-engines --test fixtures_check passes
- pdfinfo crates/engines/tests/fixtures/sample.pdf reports Pages: 3; pdftotext crates/engines/tests/fixtures/sample.pdf - prints 'Page 1'..'Page 3' and 'Café déjà vu'
- du -k crates/engines/tests/fixtures/* shows every file under 100 KB

### XP-02: Fix Windows path tests and add per-OS integrate dispatch in platform

- **Deps:** XP-01
- **Commit:** `fix(cli): make path tests portable and dispatch integrate per OS`
- **CI OS check:** windows
- **Owns:** `crates/cli/src/report.rs`, `crates/cli/src/commands/doctor.rs`, `crates/cli/src/commands/integrate.rs`, `crates/platform/src/lib.rs`, `crates/platform/src/integration.rs`, `crates/platform/src/explorer.rs`, `crates/platform/src/finder.rs`, `crates/platform/src/gesture.rs`, `crates/platform/src/context_menu.rs`

Two jobs that touch the same CLI files, so one commit. XP-01 has landed, so CI windows-latest/macos-latest test jobs exist and XP-01's notes list the failing tests.
Before changing anything, the developer saves a baseline from main: `cargo run -q -p morphring -- integrate install --dry-run --allow --exe /usr/bin/morphring-app > $SCRATCH/integrate-baseline.txt` (and the same for `integrate uninstall --dry-run`). The acceptance compares against these files.
A. The known Windows failures (docs/research/windows-support.md section 1; cross-check XP-01's list):
- report::tests (5): build fixture paths from a real absolute root with a `#[cfg(windows)] r"C:\work"` / `#[cfg(not(windows))] "/home/u/work"` const helper; compare `Path`s, or normalise `\` only inside assertions.
- doctor::tests::current_folders_end_in_morphring: on Windows assert a `morphring` component rather than the last one (ProjectDirs gives `%APPDATA%\morphring\config`).
- integrate::tests (2): `#[cfg(target_os = "linux")]`.
Never compare a canonicalized path with a non-canonical one.
B. Platform skeleton, so XP-06, XP-11 and P2-19 only fill their own files:
- integration.rs: `pub enum Change { File(PathBuf), Registry(String) }` with Display; `pub fn install(exe: &Path, home: &Path, dry_run: bool) -> Result<Vec<Change>, IntegrationError>` and `uninstall(home, dry_run)`, dispatching by cfg: Linux to context_menu (wrap its results), Windows to explorer, macOS to finder, other OSes `IntegrationError::Unsupported`. Move IntegrationError here if needed and re-export it from context_menu.
- explorer.rs, finder.rs: compile on every OS; `install`/`uninstall` stubs return `Ok(vec![])`; a `//!` doc on what XP-06 / XP-11 add.
- gesture.rs: an empty `//!` stub for P2-19.
- lib.rs: `pub mod context_menu; pub mod explorer; pub mod finder; pub mod gesture; pub mod integration;`.
- cli commands/integrate.rs: call `morphring_platform::integration` and print each Change, keeping today's Linux output byte for byte.

**Acceptance:**

- cargo test -p morphring -p morphring-platform passes on Linux
- cargo clippy -p morphring -p morphring-platform --all-targets -- -D warnings passes
- integrate install/uninstall --dry-run output diffs empty against the baseline files saved from main before the change
- On the pushed branch, CI test (windows-latest) shows no failures in report::, doctor:: or integrate:: tests

### P2-01: Workspace: MSRV 1.92, deny CC0-1.0, declare every v0.2 dependency and engine feature

- **Deps:** R-01
- **Commit:** `chore: raise MSRV to 1.92 and declare the v0.2 dependencies`
- **CI OS check:** none (Linux only)
- **Owns:** `Cargo.toml`, `Cargo.lock`, `deny.toml`, `crates/core/Cargo.toml`, `crates/engines/Cargo.toml`, `crates/cli/Cargo.toml`, `crates/platform/Cargo.toml`, `app/src-tauri/Cargo.toml`, `app/src-tauri/Cargo.lock`

Goal: no later task touches a manifest or lockfile, except XP-10 (one tauri feature on macOS) and the release bumps. Versions come from docs/research/v02-engines-and-crates.md section 1, but do not trust the doc: check each with `cargo info <crate>@<ver>` or the crates.io API (`curl -s https://crates.io/api/v1/crates/<name>/<ver>` -> `created_at`). Each must exist and be at least two weeks old; if not, take the newest older patch and say so in the commit body. (lopdf 0.45.0 exists on crates.io; confirm its date.)
1. Root Cargo.toml: `rust-version = "1.92"`. `[workspace.dependencies]`:
- image 0.25.10 (default-features=false; features png, jpeg, gif, bmp, tiff, ico, qoi, pnm, tga)
- resvg =0.47.0 and usvg =0.47.0; krilla 0.8.2; krilla-svg 0.8.1
- lopdf 0.45 (>=0.42 for RUSTSEC-2026-0187)
- zip 9.0.3 (default-features=false, features deflate, time); tar 0.4.46; flate2 1.1.10
- notify 8.2.0, filetime 0.2.29, dunce 1.0.5
- windows-sys 0.61.2 (features Win32_Foundation, Win32_System_JobObjects, Win32_System_Threading, Win32_Security, Win32_Storage_FileSystem)
- winreg and mslnk (current versions, MIT). If either fails cargo deny, drop it and note in the commit body that XP-06 uses the registry verb only (no Send To).
2. deny.toml: allow `CC0-1.0` (notify) with a comment.
3. crates/core/Cargo.toml: filetime.
4. crates/engines/Cargo.toml features:
```
default = ["img", "svg", "pdf", "archive"]
img = ["dep:image"]
svg = ["dep:resvg", "dep:usvg", "dep:flate2"]
pdf = ["dep:lopdf", "dep:krilla", "dep:krilla-svg", "dep:usvg", "dep:image"]
archive = ["dep:zip", "dep:tar", "dep:flate2"]
```
dunce as a normal dependency, windows-sys under `[target.'cfg(windows)'.dependencies]`.
5. crates/cli/Cargo.toml: notify; filetime as a dev-dependency.
6. crates/platform/Cargo.toml: winreg and mslnk under cfg(windows).
7. app/src-tauri/Cargo.toml: `rust-version = "1.92"`, `tauri-plugin-dialog = "2"` (folder picker for the settings page).
Regenerate both lockfiles with the smallest change (`cargo update -p` per new crate, never a blanket update). Nothing in src/ changes.

**Acceptance:**

- cargo build --workspace --locked and cargo build -p morphring-engines --no-default-features --locked both pass
- cargo test --workspace --locked passes (no behaviour change)
- cargo check --locked --manifest-path app/src-tauri/Cargo.toml passes
- rustup toolchain install 1.92 if missing; cargo +1.92 check --workspace --locked --all-targets passes
- cargo +1.92 check --locked --manifest-path app/src-tauri/Cargo.toml --all-targets passes
- The commit body lists each new crate with its version and publish date
- On the pushed branch, the deny-licenses job is green

### P2-60: AUR morphring-bin PKGBUILD

- **Deps:** R-01
- **Commit:** `chore(packaging): add the morphring-bin AUR package`
- **CI OS check:** none (Linux only)
- **Owns:** `packaging/aur/morphring-bin/PKGBUILD`, `packaging/aur/morphring-bin/.SRCINFO`, `packaging/aur/README.md`

A `morphring-bin` PKGBUILD that installs the release tarball's `morphring` to /usr/bin, and README, LICENSE, CHANGELOG and THIRD-PARTY-NOTICES to /usr/share/{doc,licenses}/morphring. pkgver=0.1.1, arch=x86_64, license=MIT, provides/conflicts=morphring.
- source: the GitHub release tarball URL. sha256sums: the real checksum. The orchestrator tags v0.1.1 when R-01 lands, so the assets exist (possibly as a draft): `gh release download v0.1.1 -p '*x86_64-linux*.tar.gz' -D $SCRATCH` and `sha256sum` it, cross-checked against the release's SHA256SUMS.txt. Only if the release has no assets yet, use 'SKIP' with a `# TODO(R-03): real checksum` comment; R-03 replaces it.
- optdepends: imagemagick, ffmpeg, libreoffice-fresh, poppler, ghostscript, pandoc, each with a reason.
- The app gets a separate `morphring-app-bin` package later; packaging/aur/README.md says so and documents the bump procedure: after a tag is published, set pkgver, take sha256 from the release's SHA256SUMS.txt (never from a local build), run `makepkg --printsrcinfo > .SRCINFO`.
- Do not publish to AUR (deferred: needs the owner's account).

**Acceptance:**

- cd packaging/aur/morphring-bin && makepkg --printsrcinfo | diff - .SRCINFO is empty
- bash -n PKGBUILD passes
- If the v0.1.1 assets exist: grep -c SKIP PKGBUILD is 0, and makepkg -f in a temp copy builds a package whose pacman -Qlp lists /usr/bin/morphring

### XP-08: Release workflow: multi-OS CLI builds and a single publish job

- **Deps:** R-01
- **Commit:** `chore(release): build CLI archives for Windows and macOS`
- **CI OS check:** windows, macos
- **Owns:** `.github/workflows/release.yml`

Restructure release.yml into build jobs plus one `publish` job, so a draft never gets half its assets. Keep the top-level `TAURI_CLI_VERSION` env and the Check Tauri CLI step from R-01.
- `linux` (ubuntu-22.04): today's steps: the CLI tarball, the app .deb and .AppImage.
- `windows-cli` (windows-2025): `cargo build --release --locked -p morphring`, smoke-test `--version` against the tag (on dispatch runs, against Cargo.toml's version), convert sample.wav to mp3 with ffmpeg from choco, zip `morphring-<tag>-x86_64-windows.zip` with morphring.exe and the docs.
- `macos-cli` (macos-26, arm64): the same plus `codesign -s - target/release/morphring`, then `morphring-<tag>-aarch64-macos.tar.gz`.
Runner check: the first step of each non-Linux job prints `sw_vers` and `uname -m` (macOS) or `[Environment]::OSVersion` (Windows), and the macOS job fails unless `uname -m` is arm64. If `macos-26` is not available to this private repo (the job never starts or errors on the label), switch to `macos-15` and say so in the commit body; likewise `windows-2025` falls back to `windows-latest`.
Each build job uploads its dist files with actions/upload-artifact@v4. `publish` (ubuntu, needs all builds) downloads them, writes one SHA256SUMS.txt, extracts the notes (same awk) and runs `gh release create --draft --verify-tag` with every asset. On workflow_dispatch it skips gh release and uploads the combined dist (R-01's behaviour). Update the header comment: a multi-job release is fine now, because publish runs only when every build succeeded. XP-09 and XP-12 add app jobs later, so keep job names stable and the asset collection generic (`dist/*`).

**Acceptance:**

- YAML parses (python yaml.safe_load or actionlint)
- gh workflow run release.yml --ref <branch> succeeds; the combined artifact holds the Linux tarball, .deb, .AppImage, the Windows zip, the macOS tarball and a SHA256SUMS.txt that passes sha256sum -c
- The macos-cli log shows uname -m = arm64 and the sw_vers output

### P2-61: CI: ImageMagick 7 on Linux and a LibreOffice golden job

- **Deps:** XP-01
- **Commit:** `chore(ci): run ImageMagick 7 and LibreOffice golden tests`
- **CI OS check:** none (Linux only)
- **Owns:** `.github/workflows/ci.yml`

PLAN v0.2: "ImageMagick 7 and LibreOffice in CI".
- In the ubuntu `test` job, install ImageMagick 7 from the GitHub release assets of ImageMagick/ImageMagick at a fixed tag (for example `https://github.com/ImageMagick/ImageMagick/releases/download/<tag>/ImageMagick-<...>-gcc-x86_64.AppImage`; pick the newest tag at least two weeks old). Those URLs stay valid, unlike imagemagick.org/archive/binaries, which keeps only the latest build. Check its SHA-256 against a value hard-coded in the workflow, put it at `$HOME/.local/bin/magick` (on GITHUB_PATH) with `APPIMAGE_EXTRACT_AND_RUN=1` in the job env, then `magick -version`. The magick golden tests now run on Linux CI.
- A `goldens-office` job (ubuntu-latest, timeout 20, continue-on-error: true, comment D7): apt-installs `libreoffice-writer-nogui libreoffice-calc-nogui libreoffice-impress-nogui`, then `cargo test -p morphring-engines --locked --test soffice` and `cargo test -p morphring --locked --test convert`.
- Keep the main test job under 10 minutes; if the AppImage adds more than 1 minute, cache it with actions/cache keyed on the tag.

**Acceptance:**

- YAML parses
- On the pushed branch, test (ubuntu-latest) logs show 'Version: ImageMagick 7' and the magick golden tests print no SKIP lines
- goldens-office runs the soffice golden tests with no SKIP lines

### P2-02b: Core skeleton part 2: option types, Target, accepts/merger, job hooks

- **Deps:** P2-02a
- **Commit:** `feat(core): add preset option types and job hooks for v0.2`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/core/src/lib.rs`, `crates/core/src/contract.rs`, `crates/core/src/options.rs`, `crates/core/src/job.rs`, `crates/core/src/job/merge.rs`, `crates/core/src/output.rs`, `crates/core/src/testing.rs`, `crates/cli/src/commands/convert.rs`, `crates/engines/src/magick/args.rs`, `app/src-tauri/src/commands.rs`

Goal: the option types and job hooks every engine and front end builds on. Port the shapes from docs/reverse-engineering/01-core.md sections 4.5 and 5.
1. options.rs (complete): ImageOptions, PdfOptions, DocumentOptions, MediaOptions, ArchiveOptions and their enums (Resize, TiffCompression, PageSize, MediaQuality, WavFormat, ArchiveLevel) with Filee's defaults. serde camelCase, `#[serde(default)]`, string enums, `margin_mm: f32`. A unit test pins every default.
2. contract.rs: `ConvertOptions { quality: Option<u8>, image, pdf, document, media, archive }` (drop `Eq`, keep PartialEq and Default; quality stays the CLI `-q` override). `trait Merger: Send + Sync { fn merge(&self, inputs: &[PathBuf], out: &mut dyn OutputAllocator, progress: &dyn Fn(f32), cancel: &CancelToken) -> Result<Vec<PathBuf>, EngineError>; }`. New Converter methods with defaults: `fn accepts(&self, _options: &ConvertOptions) -> bool { true }` and `fn merger(&self) -> Option<&dyn Merger> { None }`. lib.rs re-exports Merger and `pub use options::*` types.
3. output.rs: serde derives on OutputLocation, ConflictPolicy, OutputRule; `OutputLocation::Default` ("use the settings' default output"; resolve() treats it as SameFolder); `OutputRule.keep_dates: bool`, default false.
4. job.rs: `pub enum Target { Format(FormatId), SameAsSource }`; run_job takes `target: Target`; for SameAsSource the per-file target is the source format (needs a self edge). The installed planner is built only from engines where `accepts(options)` is true; the dream planner keeps every engine. After a file ends Done with `rule.keep_dates`, call `crate::dates::copy_dates(&source, &outputs)` (P2-02a's no-op). `mod merge;` (src/job/merge.rs) with `pub(super) fn try_run(...) -> Option<Vec<FileResult>>` returning None; run_job calls it first. Make the helpers merge needs (`plan_file`, temp allocator, cleanup) `pub(super)`.
5. Callers: cli convert.rs, app commands.rs and the magick/args.rs tests move to `Target::Format` and `..Default::default()`, with no behaviour change.
6. Tests: SameAsSource (a FakeConverter png->png self edge) and the accepts filter (an engine whose accepts returns false is never used; the missing route still reports NoRoute/EngineMissing as before). testing.rs gains `FakeConverter::declining(...)` if that is the easiest way to write it.

**Acceptance:**

- cargo test --workspace --locked passes; cargo test -p morphring-core shows the new SameAsSource and accepts tests
- cargo clippy --workspace --all-targets --locked -- -D warnings passes
- cargo test --locked --manifest-path app/src-tauri/Cargo.toml passes
- cargo run -q -p morphring -- convert crates/engines/tests/fixtures/sample.png --to jpg -o $(mktemp -d) still exits 0

### P2-14: PageRange parser

- **Deps:** P2-02a
- **Commit:** `feat(core): port the page range parser`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/core/src/page_range.rs`

Port src/Filee.Core/Conversion/PageRange.cs (docs/reverse-engineering/01-core.md section 5.1). `pub fn parse(expr: &str, page_count: usize) -> Vec<usize>` returns sorted, distinct, 0-based indices:
- page_count 0 gives []; a blank expression gives all pages.
- Comma-split, trimmed entries: N, A-B, A- and -B. Non-numeric parts are ignored, reversed ranges swapped, values clamped to 1..=count.
- Nothing valid gives all pages.
Also `pub fn runs(indices: &[usize]) -> Vec<(usize, usize)>`: contiguous 1-based (first, last) runs for pdftoppm and Ghostscript. Port every case in Filee's PageRangeTests and add runs tests.

**Acceptance:**

- cargo test -p morphring-core page_range passes
- cargo clippy -p morphring-core --all-targets -- -D warnings passes

### P2-15: Watch-folder settle state machine (pure)

- **Deps:** P2-02a
- **Commit:** `feat(core): add the watch-folder settle state machine`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/core/src/watch.rs`

Port src/Filee.Core/Watching/FolderWatcher.cs's decision logic (docs/reverse-engineering/01-core.md sections 10.3 to 10.5), with no OS watcher and no clock.
- `WatchRule { folder, include_subfolders, output_folder: Option<PathBuf>, originals: Keep|MoveToOriginals }` with `resolved_output_folder()` and `originals_folder()`.
- `should_ignore(rule, path, is_dir) -> bool`: ~$ and dot names, desktop.ini, thumbs.db, the temporary extensions list, inside the output or originals folder, subfolders unless recursive, unknown formats via formats::detect.
- `Settler::new(settle: Duration, handled_cap: usize)` (callers pass 10_000): `notice(path, Option<(u64, SystemTime)>, now: Instant)`; `tick(now, stat: impl Fn(&Path) -> Option<(u64, SystemTime)>) -> Vec<PathBuf>` returns ready files, sorted and deduplicated by the handled key `path|size|mtime`; the handled set is an LRU bounded at handled_cap (fixes Filee's leak); `rescan(paths)` for overflow.
- `default_settle()`: 2 s on Windows, 5 s elsewhere.
- `originals_target(rule, source, exists: impl Fn(&Path) -> bool) -> Option<PathBuf>`, with ' (2)' numbering.
Tests: port FolderWatcherTests' pure parts (default settle pin, ignore list, settle timing with an injected now, dedup, originals numbering, the LRU bound).

**Acceptance:**

- cargo test -p morphring-core watch passes
- cargo clippy -p morphring-core --all-targets -- -D warnings passes

### P2-16: --keep-dates: copy source times onto outputs

- **Deps:** P2-02a, P2-01, XP-01
- **Commit:** `feat(core): copy file dates onto outputs for --keep-dates`
- **CI OS check:** windows, macos
- **Owns:** `crates/core/src/dates.rs`

Fill `dates::copy_dates(source, outputs)` (docs/research/v02-engines-and-crates.md section 6; Filee 01-core line 321). Keep the signature P2-02a declared.
- Read the source metadata, then `filetime::set_file_times(out, atime, mtime)` on every output file or folder.
- On Windows and macOS, also set the creation time with `std::fs::File::set_times` and `FileTimesExt::set_created` (cfg-gated). On Windows skip folders for the creation time (opening a directory needs FILE_FLAG_BACKUP_SEMANTICS; not worth it now).
- Errors log a warn and never fail.
Tests: a file and a folder output get the source mtime (TempDir, filetime sets the source). A missing output does not panic.

**Acceptance:**

- cargo test -p morphring-core dates passes
- cargo clippy -p morphring-core --all-targets -- -D warnings passes
- CI windows-latest and macos-latest test jobs show the dates tests passing

### P2-19: Port GestureDetector as a pure state machine with Filee's tests

- **Deps:** XP-02
- **Commit:** `feat(platform): port the gesture detector state machine`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/platform/src/gesture.rs`

Low priority: nothing uses it until v0.3, so the orchestrator schedules it only when a developer slot is free. Port src/Filee.App/Services/Triggers/GestureDetector.cs and the settings model (docs/reverse-engineering/05-app-ui-ring.md sections 4.1 and 4.2).
- Types: `Modifiers` (hand-written bitflags), `Button`, `GestureKind {Drag, Hold, KeyChord}`, `TriggerGesture`, `Scope`.
- `Detector::new(gestures, system_drag_threshold, scope_filter: Box<dyn Fn(&TriggerGesture, i32, i32) -> bool + Send>)`.
- `mouse_down`, `mouse_move`, `mouse_up`, `hold_elapsed(token)`, `key_down`, each returning `Vec<GestureEvent>` (DragStarted, DragEnded, HoldTriggered, KeyChordTriggered, RequestHoldTimer{token, ms}). No threads, no clock.
- Covers exact modifier match, the drag threshold (`<=` means no move), Hold cancelled by moving, late-modifier detection with late_rejected, pause.
Port every test in Filee's GestureDetectorTests with the same names in snake_case.

**Acceptance:**

- cargo test -p morphring-platform gesture passes with at least as many tests as Filee's GestureDetectorTests
- cargo clippy -p morphring-platform --all-targets -- -D warnings passes

### XP-06: Windows Explorer menu: HKCU verb and Send To shortcut

- **Deps:** XP-02, P2-01, XP-01
- **Commit:** `feat(platform): add the Explorer verb and Send To entry on Windows`
- **CI OS check:** windows
- **Owns:** `crates/platform/src/explorer.rs`

Fill explorer.rs (docs/research/windows-support.md section 3, options 1 and 2).
1. Pure, tested on every OS: `verb_entries(exe: &Path) -> Vec<RegEntry { key, name, value }>` for `Software\Classes\*\shell\MorphRing` (default 'Convert with MorphRing', `Icon` = exe, `MultiSelectModel` = 'Player', marker `MorphRingManaged=1`) with `command` = `"<exe>" --convert "%1"`; the same under `Software\Classes\Directory\shell\MorphRing`. `sendto_path(appdata) -> PathBuf` (`Microsoft\Windows\SendTo\MorphRing.lnk`). Export the key paths as `pub const` so XP-09's hooks.nsh check can grep them.
2. `#[cfg(windows)]` install/uninstall write/remove these under HKCU with winreg, and create/remove the .lnk with mslnk (target exe, args `--convert`). Only keys carrying the marker are removed. dry_run lists the Changes without writing. The registry root is a parameter so tests can use `Software\Classes\MorphRingTest`.
The app already collects one-process-per-file launches through single-instance plus the 350 ms Debouncer.
3. Tests: verb_entries quoting (spaces, non-ASCII exe paths) on all OSes. `#[cfg(windows)]` round trip under the test root, leaving no keys. Never write the real verb in tests.

**Acceptance:**

- cargo test -p morphring-platform passes on Linux (the pure generator tests run)
- cargo clippy -p morphring-platform --all-targets -- -D warnings passes
- CI windows-latest: the registry round-trip test passes
- A cfg(windows) test calls integration::install with dry_run on a fake exe path and asserts two registry keys plus the SendTo .lnk are listed (runs on CI windows-latest)

### XP-11: macOS Finder Quick Action written by integrate

- **Deps:** XP-02, XP-01
- **Commit:** `feat(platform): add a Finder Quick Action on macOS`
- **CI OS check:** macos
- **Owns:** `crates/platform/src/finder.rs`, `crates/platform/resources/quick-action/`

Fill finder.rs (docs/research/macos-support.md section 6).
- `install(exe, home, dry_run)` writes `~/Library/Services/Convert with MorphRing.workflow/Contents/{Info.plist,document.wflow}`. Info.plist: NSServices with menu item 'Convert with MorphRing', NSSendFileTypes public.item, NSRequiredContext. document.wflow: one Run Shell Script action, input as arguments, /bin/sh, body `open -a MorphRing "$@"`.
- Templates live in crates/platform/resources/quick-action/, loaded with include_str!, written atomically, with a managed marker comment `<!-- X-MorphRing-Managed=true -->` so uninstall removes only ours.
- On macOS after a real install, run `/System/Library/CoreServices/pbs -update` (failure ignored; platform has no engines dep, so a direct Command is allowed and commented). Return a Change telling the user to enable it in System Settings > General > Login Items & Extensions > Finder.
- Pure generation and file writing compile and test on every OS against a TempDir home: files, marker, uninstall removes them.
- Risk note in code: the .wflow is hand-written. Whether Finder shows it can only be seen on a Mac: If no such machine is reachable, the tester records this check as UNVERIFIED in the commit body (line `Unverified: <check>`) and the task still lands; R-03 lists it in the README.

**Acceptance:**

- cargo test -p morphring-platform passes on Linux (writes the workflow into a TempDir home)
- CI macos-latest: a cfg(target_os = "macos") test runs `plutil -lint` on both generated files and passes
- cargo clippy -p morphring-platform --all-targets -- -D warnings passes
- Quick Action visible in Finder: recorded as UNVERIFIED in the commit body unless a Mac is available

### P2-05: CLI skeleton: --preset, --keep-dates, presets and watch commands, config dir in Ctx

- **Deps:** P2-02b
- **Commit:** `feat(cli): declare presets, watch and --preset/--keep-dates`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/cli/src/args.rs`, `crates/cli/src/lib.rs`, `crates/cli/src/ctx.rs`, `crates/cli/src/main.rs`, `crates/cli/src/commands/mod.rs`, `crates/cli/src/commands/presets.rs`, `crates/cli/src/commands/watch.rs`, `crates/cli/src/commands/convert.rs`, `crates/cli/src/resolve.rs`, `crates/cli/tests/common/mod.rs`, `crates/cli/tests/cli.rs`

Port the argument contract from docs/reverse-engineering/07-cli-tests-docs.md sections 3, 5.2, 5.3, 5.5 and 6.
1. args.rs: ConvertArgs gains `to: Option<String>`, `preset: Option<String>` (-p/--preset), `keep_dates: bool`. New `Command::Presets` and `Command::Watch(WatchArgs { folder, to, preset, output, recursive, quality, overwrite, skip, quiet, keep_dates, move_originals, settle: Option<f64> (>= 0) })`. PRESETS_HELP and WATCH_HELP (settle default 5 s on Linux/macOS, 2 s on Windows). Update GENERAL_HELP and help_text.
2. lib.rs: 'Use either --to <format> or --preset <preset>.' (exit 2) when both or neither are given, for convert and watch; keep the existing "at least one file" ordering. Dispatch arms for presets and watch. `mod resolve;`. Compute `config_dir` from `ProjectDirs::from("", "", "morphring")`.
3. ctx.rs: `config_dir: Option<PathBuf>` (None = defaults only).
4. convert.rs: only the minimal change for `to: Option<String>` (validated above, so take it with a `let Some(to) = ... else { unreachable usage path returning 2 }`). `--preset` is not wired yet (P2-41).
5. Stubs: commands/presets.rs `run(&mut Ctx) -> i32` prints nothing, returns 0 (P2-40). commands/watch.rs `run(&WatchArgs, &mut Ctx) -> i32` writes 'morphring: watch is not available yet', returns 2 (P2-42). resolve.rs: a `//!` doc and an empty module (P2-41 adds job_from_args).
6. tests/common/mod.rs: run_in builds Ctx with config_dir = None; new `run_in_with_config(cwd, config_dir, engines, args)`.
7. tests/cli.rs: the two usage errors, `help watch`, `help presets`, `presets` exits 0.

**Acceptance:**

- cargo test -p morphring passes
- cargo run -q -p morphring -- convert x.png exits 2 with 'Use either --to <format> or --preset <preset>.'
- cargo run -q -p morphring -- help watch prints the watch help with the 5 s settle default
- cargo clippy -p morphring --all-targets -- -D warnings passes

### P2-06: App skeleton: settings/history/toast windows, commands, job events, lifecycle hook

- **Deps:** P2-02b, P2-01
- **Commit:** `feat(app): add settings, history and toast windows and job events`
- **CI OS check:** none (Linux only)
- **Owns:** `app/src-tauri/src/lib.rs`, `app/src-tauri/src/commands.rs`, `app/src-tauri/src/settings.rs`, `app/src-tauri/src/history.rs`, `app/src-tauri/src/toast.rs`, `app/src-tauri/src/ring_window.rs`, `app/src-tauri/src/lifecycle.rs`, `app/src-tauri/src/tray.rs`, `app/src-tauri/tauri.conf.json`, `app/src-tauri/capabilities/default.json`, `app/src-tauri/tests/config.rs`, `app/ui/settings.html`, `app/ui/settings.js`, `app/ui/settings.css`, `app/ui/history.html`, `app/ui/history.js`, `app/ui/toast.html`, `app/ui/toast.js`, `app/ui/toast.css`

Goal: the app feature tasks never touch lib.rs, tauri.conf.json or capabilities.
1. tauri.conf.json: windows `settings` (settings.html, 640x560, hidden), `history` (history.html, 640x520, hidden), `toast` (toast.html, 360x120, hidden, undecorated, transparent, alwaysOnTop, skipTaskbar, focus false, shadow false). Their labels go in capabilities/default.json, plus `dialog:default` for settings. tests/config.rs checks each page exists and loads only same-origin scripts.
2. lib.rs: register tauri_plugin_dialog after the opener. generate_handler! gains `settings::settings_get`, `settings::settings_set`, `settings::presets_list`, `history::history_list`, `history::history_clear`, `commands::cancel_job`, `toast::toast_dismiss`, each a stub returning a default/empty value or Ok(()).
3. Lifecycle, pre-registered for XP-10: lifecycle.rs defines `pub struct PendingOpen(pub Mutex<Vec<PathBuf>>)` (Default), `pub fn setup(app: &mut App) -> Result<(), Box<dyn Error>>` (empty body) and `pub fn on_run_event(app: &AppHandle, event: &RunEvent)` (empty match). lib.rs calls `.manage(lifecycle::PendingOpen::default())`, calls `lifecycle::setup(app)?` at the end of the existing setup closure, and switches `.run(generate_context!())` to `.build(...)?.run(|app, event| lifecycle::on_run_event(app, &event))`. XP-10 then fills only lifecycle.rs and tray.rs.
4. settings.rs (real, small): `pub fn config_dir(app) -> Option<PathBuf>` using ProjectDirs, the same as the CLI.
5. commands.rs: each job gets a `job_id: u64`. Emit app-wide events `job-started` (JobStarted { jobId, files, label }), `job-progress` (JobProgress { jobId, progress }, coalesced at 1 % like ring-progress) and `job-finished` (JobFinished { jobId, summary }). On finish call `crate::history::record(app, &record)` (stub). Move the ring centring/showing from open_ring into `ring_window::show(&window)` with today's exact behaviour (center, show, focus). Document the events in a comment block in commands.rs and in toast.js.
6. tray.rs: TrayAction::{Settings, History} showing those windows (`settings::open(app)`, `history::open(app)`: `get_webview_window(..).show()` + `set_focus()`).
7. UI stubs: minimal HTML with the CSP-compatible external script and a heading, no inline scripts.
Keep every existing test green; add a unit test for the job_id sequence.
Manual Hyprland check (drop gives the same ring and conversion; tray shows Settings and History): the developer runs it on this machine if a Wayland session is reachable; otherwise If no such machine is reachable, the tester records this check as UNVERIFIED in the commit body (line `Unverified: <check>`) and the task still lands; R-03 lists it in the README.

**Acceptance:**

- cargo test --locked --manifest-path app/src-tauri/Cargo.toml passes (config.rs checks the 5 windows)
- cargo clippy --locked --manifest-path app/src-tauri/Cargo.toml --all-targets -- -D warnings passes
- node --test app/ui/ passes
- Manual Hyprland check run, or recorded as UNVERIFIED in the commit body

### P2-10: Presets: Preset model, the 68 built-ins, lookup and job building

- **Deps:** P2-02b
- **Commit:** `feat(core): port Filee's presets and the 68 built-ins`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/core/src/preset.rs`, `crates/core/src/builtin.rs`

Port src/Filee.Core/Presets/Preset.cs and BuiltInData.cs:15-161 (table in docs/reverse-engineering/01-core.md section 5.2). Read the Filee source for the exact 68 entries; keep their ids, order and non-default options.
1. `Preset { id, name, name_key: Option<String>, target: String, image, pdf, document, media, archive, output: OutputRule }`, serde camelCase and default. `display_name()`: name, else the English text for name_key (Half size, Grayscale, Merge PDF, Split PDF, Long image, Extract, One ZIP, from en.json:103-109), else target.to_uppercase().
2. builtin.rs: `pub fn presets() -> Vec<Preset>` with all 68 and their patterns (e.g. `{name}_70`). pdf-split uses `Subfolder("{name}")`. Every location is `OutputLocation::Default`.
3. preset.rs: `find(presets, key)` (id case-insensitive, else display name case-insensitive); `for_target(format) -> Preset` (the --to preset, all defaults); `Preset::job_target() -> Option<job::Target>` ('*' gives SameAsSource, else formats::lookup_target); `Preset::options() -> ConvertOptions` (quality = Some(image.quality) only when it differs from the default, so `--to` keeps v0.1 behaviour).
4. Tests: 68 presets with unique ids; every target is '*', 'folder' or a writable format id (this catches ids P2-02a missed); find by id, by name and by 'merge pdf'; display names; a JSON round trip.

**Acceptance:**

- cargo test -p morphring-core preset passes
- cargo clippy -p morphring-core --all-targets -- -D warnings passes

### P2-12: Settings store: atomic JSON files, .broken copies, AppSettings, default output

- **Deps:** P2-02b
- **Commit:** `feat(core): add the settings store with atomic writes`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/core/src/settings.rs`

Port src/Filee.Core/Settings/UserDataStore.cs and AppSettings.cs (docs/reverse-engineering/01-core.md section 7), following ENGINEERING.md section 7.
1. `Store { dir: Option<PathBuf> }`: `read_json<T: DeserializeOwned + Default>(&self, name) -> T` (missing file or dir None gives the default; corrupt JSON is copied to `<name>.broken`, warns, gives the default). `write_json<T: Serialize>(&self, name, value) -> io::Result<()>`: create the dir, `NamedTempFile::new_in(dir)`, pretty JSON, `sync_all`, `persist`; dir None writes nothing and returns Ok.
2. `AppSettings { schema_version: 1, keep_history: true, keep_file_dates: false, default_output: DefaultOutput { location: SameFolder|Subfolder|CustomFolder, subfolder_name: "converted", custom_folder: "" } }`: only the fields v0.2 uses, camelCase, default, unknown fields ignored. `load_settings()`/`save_settings()` on settings.json. `with_default_output(&self, rule) -> OutputRule`: Filee's WithDefaultOutput; maps `OutputLocation::Default` to the default location and ORs keep_file_dates into keep_dates.
3. Tests (TempDir, no env vars): missing gives defaults; round trip; corrupt gives defaults plus a .broken copy; dir None never writes; an atomic write leaves no temp files; with_default_output for each location.

**Acceptance:**

- cargo test -p morphring-core settings passes
- cargo clippy -p morphring-core --all-targets -- -D warnings passes

### P2-17: Job runner: merge-into-one-PDF mode

- **Deps:** P2-02b
- **Commit:** `feat(core): add the merge-into-one-PDF job mode`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/core/src/job/merge.rs`

Fill `merge::try_run` (Filee JobQueue.cs:116-154, docs/reverse-engineering/01-core.md section 4.5).
- Active when options.pdf.merge_into_single, the target is Format(pdf), files.len() > 1 and an installed engine's merger() is Some; else None.
- Convert every non-PDF file to PDF in parallel into the job work dir with the temp allocator, reusing job.rs's `pub(super)` helpers. PDF inputs are used as they are, in input order.
- One final output named after file 1 (Tokens index 1). If Skip returns None, every file is Skipped.
- Call merger.merge. The output is attached to file 1; every non-failed file becomes Done. Fix Filee's quirk: when file 1 failed, attach the output to the first successful file (with a test).
- A merge error fails every non-failed file with ConversionFailed (no panic). Cancel gives Cancelled. Progress: conversion 0 to 0.8, merge 0.8 to 1.
Tests with local fakes (an image->pdf FnConverter plus a fake Merger that concatenates bytes): ordering, Skip, a failing middle file, file 1 failing, cancel, a merge error.

**Acceptance:**

- cargo test -p morphring-core job::merge passes
- cargo clippy -p morphring-core --all-targets -- -D warnings passes

### P2-03a: Engines skeleton part 1: stub engines, discovery tools, process and progress hooks

- **Deps:** P2-02b, P2-01
- **Commit:** `feat(engines): add v0.2 engine stubs and process hooks`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/lib.rs`, `crates/engines/src/discovery.rs`, `crates/engines/src/process.rs`, `crates/engines/src/win_job.rs`, `crates/engines/src/progress.rs`, `crates/engines/src/native_image/mod.rs`, `crates/engines/src/svg/mod.rs`, `crates/engines/src/pdf/mod.rs`, `crates/engines/src/pdf/image_to_pdf.rs`, `crates/engines/src/pdf/pages.rs`, `crates/engines/src/pdf/merge.rs`, `crates/engines/src/pdf/svg_pdf.rs`, `crates/engines/src/poppler/mod.rs`, `crates/engines/src/poppler/render.rs`, `crates/engines/src/poppler/text.rs`, `crates/engines/src/ghostscript/mod.rs`, `crates/engines/src/pandoc/mod.rs`, `crates/engines/src/archive/mod.rs`, `crates/engines/src/archive/safety.rs`, `crates/engines/src/archive/extract.rs`, `crates/engines/src/archive/pack.rs`

Goal: every engine task owns only its own files. The new engines are NOT registered yet (P2-03b does that), so no route or cost changes in this commit.
1. lib.rs: `pub mod progress; pub mod poppler; pub mod ghostscript; pub mod pandoc;`, `#[cfg(feature="img")] pub mod native_image;`, `#[cfg(feature="svg")] pub mod svg;`, `#[cfg(feature="pdf")] pub mod pdf;`, `#[cfg(feature="archive")] pub mod archive;`, `#[cfg(windows)] mod win_job;`.
2. Engine structs and ids: ImageEngine 'image' ('Built-in images'), SvgEngine 'resvg' ('Built-in SVG'), PdfEngine 'pdf' ('Built-in PDF'), ArchiveEngine 'archive' ('Built-in archives'), status `Available { path: PathBuf::new(), version: env!("CARGO_PKG_VERSION") }`. PopplerEngine 'poppler', GhostscriptEngine 'ghostscript', PandocEngine 'pandoc', status from discovery. Multi-part engines build `edges()` from a `OnceLock<Vec<Edge>>` concatenating submodule `pub(crate) fn edges() -> Vec<Edge>` (pdf: image_to_pdf, pages, svg_pdf; poppler: render, text; archive: extract, pack); `convert` dispatches by (from, to) to the submodule's `pub(crate) fn convert(step, out, progress, cancel)`. Stubs return empty edges and `Err(EngineError::Failed("not implemented"))`. `PdfEngine::merger()` returns `merge::merger()` (stub: None). archive/safety.rs is an empty `//!` stub. Single-file engines (native_image, svg, ghostscript, pandoc) are one mod.rs each; their tasks may add sibling files.
3. discovery.rs: Tool variants Pdftoppm (programs pdftoppm; companions pdftotext and pdfinfo in the same folder; `pdftoppm -v` on stderr), Ghostscript (gs, plus gswin64c/gswin32c on Windows; `gs --version`), Pandoc (`^pandoc(?:\.exe)? (\d+)\.(\d+)`). Arch/Debian/brew hints (poppler / poppler-utils / poppler, ghostscript, pandoc). Windows hints come in XP-04.
4. process.rs: `RunOptions.stall: Option<Duration>` (ignored until P2-20); `RunError::Stalled { after: Duration }` with a message; on Windows after spawn call `win_job::attach(&child) -> Option<win_job::Job>`, and kill_group calls `job.terminate()` when Some, else `child.kill()`. win_job.rs stub: attach returns None.
5. progress.rs: `pub struct Estimate` with `pub fn start(progress: &(dyn Fn(f32) + Sync), typical: Duration) -> EstimateGuard` as a no-op (P2-21 fills it; the guard stops on drop). This signature is frozen: P2-21 must keep it.

**Acceptance:**

- cargo test --workspace --locked passes (no route changes)
- cargo test -p morphring-engines --no-default-features --locked passes
- cargo clippy --workspace --all-targets --locked -- -D warnings passes
- cargo run -q -p morphring -- doctor output is unchanged from main (the stubs are not registered yet)

### P2-03b: Engines skeleton part 2: final registry order and the route-pin split

- **Deps:** P2-03a
- **Commit:** `feat(engines): register v0.2 engines in their final order`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/registry.rs`, `crates/engines/tests/routes.rs`, `crates/engines/tests/routes_images.rs`

1. registry::all() final order (penalty = min(index, 9)): image, resvg, pdf, magick, poppler, archive, ffmpeg, pandoc, ghostscript, soffice; feature-gated entries use cfg. The stubs have no edges, so routes keep their steps; only costs move.
2. Update the order tests in registry.rs and tests/routes.rs, and re-pin every literal cost (e.g. png->gif becomes 12 + penalty(magick)).
3. Split routes.rs so later engine tasks never edit it. routes.rs keeps only pins that stay true through v0.2: registry order, mp4->gif direct via ffmpeg (15 + penalty; the detour assertion compares against `10 + penalty(ffmpeg) + 12 + penalty(image)` too), pptx->pdf via soffice, docx->pdf at the soffice fallback cost, no route for txt->mp3. Move the image pins (mp4->jpg through png, png->gif at the still-to-gif cost, the png->png self edge) to a new tests/routes_images.rs, which P2-31 owns. Delete `docx_to_png_has_no_route_in_v0_1_0` with a comment: P2-25 pins docx->pdf->png in routes_poppler.rs.

**Acceptance:**

- cargo test --workspace --locked passes (all route tests re-pinned, same steps)
- cargo clippy --workspace --all-targets --locked -- -D warnings passes
- cargo run -q -p morphring -- doctor lists image, resvg, pdf, magick, poppler, archive, ffmpeg, pandoc, ghostscript, soffice, with poppler, gs and pandoc found on this machine
- cargo run -q -p morphring -- convert crates/engines/tests/fixtures/sample.wav --to mp3 -o $(mktemp -d) exits 0

### P2-11: Toolbar profiles, ProfileSelector and the preset library

- **Deps:** P2-10, P2-12
- **Commit:** `feat(core): port toolbar profiles and the profile selector`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/core/src/profiles.rs`

Port src/Filee.Core/Profiles/ToolbarProfile.cs, ProfileSelector and the library parts of UserDataStore (docs/reverse-engineering/01-core.md sections 7.3, 7.4 and 9; BuiltInData.cs:164-273).
1. `ToolbarProfile { id, name, name_key, extensions, is_fallback, preset_ids }`, MAX_SLICES = 12. `builtin_profiles()`: the 15 built-ins in Filee order with Filee's preset lists. `select(profiles, files) -> Option<&ToolbarProfile>` follows Filee's 4 rules.
2. `Library { presets, profiles }`: `Library::builtin()`, `Library::load(store)` reads presets.json and profiles.json (missing gives built-ins; corrupt gives a .broken copy and built-ins). `ensure_consistency()`: drop unknown preset ids, dedupe, cap at 12, ensure a fallback. `save(store)`. Files are `{ schemaVersion: 1, presets: [...] }` and `{ schemaVersion: 1, profiles: [...] }`.
3. Tests: port ProfileSelectorTests case by case; every built-in profile references existing presets; consistency rules; load/save round trip in a TempDir.

**Acceptance:**

- cargo test -p morphring-core profiles passes
- cargo clippy -p morphring-core --all-targets -- -D warnings passes

### P2-13: History store capped at 200

- **Deps:** P2-12, P2-10
- **Commit:** `feat(core): add conversion history capped at 200 entries`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/core/src/history.rs`

Port src/Filee.Core/History/HistoryEntry.cs and WatchResults.cs (docs/reverse-engineering/01-core.md section 8).
- `HistoryEntry { job_id, finished_at (chrono local, RFC 3339), preset_id, preset_name, state: JobState as a string, sources, outputs, watch_rule_id, failures: [{ source, error_key, error_detail }], preset: Option<Preset> (kept only when something failed) }`; `HistoryEntry::from_results(...)`.
- `History::load(store)`, `add(store, settings, entry)` (no-op when keep_history is off; newest first; trimmed to MAX_HISTORY = 200; atomic), `clear`.
- `watch_results(history, rule_id) -> (converted, failures)`: the newest entry decides each source; paths compare case-insensitively on Windows.
Tests: cap at 200, order, keep_history off, round trip, watch_results.

**Acceptance:**

- cargo test -p morphring-core history passes
- cargo clippy -p morphring-core --all-targets -- -D warnings passes

### P2-18: Ring slices from presets and profiles (replaces D2 favourites)

- **Deps:** P2-11, P2-06
- **Commit:** `feat(core): build ring slices from presets and profiles`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/core/src/slices.rs`, `app/src-tauri/src/commands.rs`

Rewrite slices::suggest as `pub fn suggest<P, R>(files: &[P], library: &Library, ctx: &ReachCtx<R>) -> RingSuggestion` where `pub struct ReachCtx<R: Fn(FormatId, FormatId) -> Reach> { pub reach: R, pub can_merge: bool }` is defined in slices.rs (Reach stays the enum it is today; there is no trait). Callers set can_merge from `engines.iter().any(|e| e.status().is_available() && e.merger().is_some())`.
- Profile from profiles::select; slices are its presets in order, capped at MAX_SLICES. Each Slice carries `preset_id`, `label` = display_name and `target` (format id, '*' or 'folder').
- `disabled: Option<DisabledReason>`: '*' resolves to each file's own format for the reach check; presets whose mode is not in v0.2 (archive.combine_into_one, image.multi_page_tiff, pdf.combine_pages) get a new `DisabledReason::NotYetSupported`; merge presets are enabled only with 2 or more files and `ctx.can_merge`.
- Remove FavouriteList and default_favourites (D2 is replaced). Keep the RingSuggestion shape the app serialises, plus the new fields.
Port the slice tests to profiles; add tests for '*', the disabled reasons and merge (can_merge true/false).
app commands.rs: adapt the existing suggest call only as far as needed to compile, passing `Library::builtin()` and the can_merge computation; P2-50 owns the real wiring.

**Acceptance:**

- cargo test -p morphring-core slices passes
- cargo test --locked --manifest-path app/src-tauri/Cargo.toml passes
- cargo clippy -p morphring-core --all-targets -- -D warnings passes

### P2-20: Process runner stall watchdog

- **Deps:** P2-03a
- **Commit:** `feat(engines): kill tools that stop making progress`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/process.rs`, `crates/engines/src/ffmpeg/mod.rs`

Filee FfmpegRunner (docs/reverse-engineering/02-engines-media-image.md around line 370): no overall timeout for long media; a watchdog kills the tree when no output arrives for the stall period.
- Implement `RunOptions.stall` in process.rs: every stdout/stderr line resets a shared AtomicU64 of millis since start; a poll (interval injectable, default 1 s) kills the process group and returns `RunError::Stalled` when stall elapses. Timeout and stall coexist.
- ffmpeg/mod.rs: `stall: Some(120 s)` with `-progress pipe:1` (already parsed); remove the 30-minute timeout for transcodes (keep the 60 s probe timeout). Map Stalled to EngineError::Failed('FFmpeg stopped making progress for 120 s'). Touch nothing else in mod.rs (P2-23 edits the options path later).
Tests (unix): a script that prints then sleeps is killed as Stalled within stall + 1.5 s; a chatty script running longer than stall is not killed.

**Acceptance:**

- cargo test -p morphring-engines process passes
- cargo test -p morphring-engines --test ffmpeg passes
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-21: Fake progress for silent tools (D8)

- **Deps:** P2-03a
- **Commit:** `feat(engines): estimate progress for tools that report none`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/progress.rs`, `crates/engines/src/soffice/mod.rs`, `crates/engines/src/magick/mod.rs`

Port src/Filee.Engines/Infrastructure/ProgressEstimate.cs (docs/reverse-engineering/02-engines-media-image.md section 3.5), keeping P2-03a's `Estimate::start` signature.
- start spawns a thread ticking every 150 ms reporting `from + (to - from) * (1 - exp(-elapsed / typical))`, from 0.1 to 0.9; never decreases; stops when the guard drops (join on drop).
- The curve is a pure `fn estimate_at(elapsed, typical, from, to) -> f32` with tests (63 % at `typical`, monotonic, bounded).
- Use it in soffice (typical 4 s) and magick (typical 2 s) around process::run. The engines report 1.0 themselves at the end.

**Acceptance:**

- cargo test -p morphring-engines progress passes
- cargo test -p morphring-engines --test soffice --test magick pass (they skip without the tool)
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-22: FFmpeg full remux fit matrix and hvc1 tagging

- **Deps:** P2-03a
- **Commit:** `feat(engines): port FFmpeg's remux matrix and hvc1 tagging`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/ffmpeg/encoding.rs`, `crates/engines/src/ffmpeg/probe.rs`

Port Filee's remux rules (docs/reverse-engineering/02-engines-media-image.md lines 430-470, MediaEncoding.cs:181-213). Only encoding.rs and probe.rs change.
- `can_copy_video`: the full container-by-codec fit matrix (mp4/m4v/mov: h264, hevc, av1, mpeg4; mkv: almost anything; webm: vp8, vp9, av1; avi: limited), the pix_fmt allow-list, quality != Small.
- `can_copy_audio`: quality != Small, no bitrate override, unchanged channels, a codec that fits the container.
- `-tag:v hvc1` for hevc copied into mp4, m4v or mov.
- probe.rs keeps the extra fields the rules need (codec_tag, pix_fmt, channels).
Arg-list tests pin: iPhone hevc mov to mp4 (copy + hvc1), h264 yuv420p mkv to mp4 (copy), vp9 webm to mp4 (re-encode), aac into mkv (audio copy).

**Acceptance:**

- cargo test -p morphring-engines ffmpeg passes
- cargo test -p morphring-engines --test ffmpeg passes
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-23: FFmpeg: honour MediaOptions

- **Deps:** P2-20, P2-22
- **Commit:** `feat(engines): map media preset options to FFmpeg`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/ffmpeg/encoding.rs`, `crates/engines/src/ffmpeg/mod.rs`, `crates/engines/tests/ffmpeg.rs`

Map MediaOptions through ffmpeg (docs/reverse-engineering/02-engines-media-image.md section 5). Runs after P2-20 and P2-22, so it owns both mod.rs (where `step.options.quality_level()` and the run call site live) and encoding.rs. Pass the whole `ConvertOptions` from mod.rs into `plan`/`video_args`/`audio_args`.
- Quality: High, Balanced or Small; `options.quality` (-q) wins when set.
- max_height: scale=-2:min(ih,H), never upscaling. audio_bitrate_kbps: 0 means the format default. remove_audio: -an. audio_channels: -ac. audio_sample_rate: WAV only. wav_format: the pcm codec.
The mp4-720 and mp3-128 presets produce the right args. Pure tests pin the arg lists; golden tests in tests/ffmpeg.rs convert sample.mp4 with max_height below the source height (check with ffprobe) and sample.wav to mp3 at 128k (bitrate via ffprobe).

**Acceptance:**

- cargo test -p morphring-engines ffmpeg passes
- cargo test -p morphring-engines --test ffmpeg passes
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-24: ImageMagick: honour ImageOptions

- **Deps:** P2-03a
- **Commit:** `feat(engines): map image preset options to ImageMagick`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/magick/args.rs`, `crates/engines/tests/magick.rs`

Port ApplyOptions (docs/reverse-engineering/02-engines-media-image.md section 4.6, in order) into the magick arg builder in args.rs only:
- Resize: Percent `-resize N%`; LongEdge `-resize NxN>`; Width `-resize W>`. Never upscale.
- dpi `-units PixelsPerInch -density`; grayscale `-colorspace Gray`.
- keep_metadata false: `-strip`, but keep the ICC profile when keep_color_profile (`+profile '!icc,*'`).
- background: `-background C -alpha remove -alpha off` for targets without alpha.
- tiff_compression `-compress None|LZW|Zip`; webp_lossless `-define webp:lossless=true`; ico_sizes `-define icon:auto-resize=...`.
- Quality: `options.quality` wins, else image.quality when it differs from the default.
Pure tests pin each option's args. Golden tests in tests/magick.rs (call MagickEngine directly, so routing does not matter): half-size halves sample.png's width; grayscale gives a Gray colorspace (magick identify).

**Acceptance:**

- cargo test -p morphring-engines magick passes
- cargo test -p morphring-engines --test magick passes on this machine (magick 7 installed)
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-25: Poppler engine: PDF to PNG/JPEG/TIFF with page ranges

- **Deps:** P2-03b, P2-04, P2-14
- **Commit:** `feat(engines): render PDF pages to images with pdftoppm`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/poppler/render.rs`, `crates/engines/tests/poppler_render.rs`, `crates/engines/tests/routes_poppler.rs`

Fill poppler/render.rs (docs/research/v02-engines-and-crates.md section 2.3; Filee PdfiumConverter, reverse-engineering/02 section 9.2).
- Edges pdf->png, pdf->jpg, pdf->tiff at cost 10.
- Page count from `pdfinfo` (the companion in the same folder) via process::run; if that fails, all pages via `-f 1` and no `-l`.
- Indices from page_range::parse(options.pdf.page_range, count), grouped by page_range::runs.
- One `pdftoppm -r clamp(render_dpi,36,1200) -png|-jpeg|-tiff [-jpegopt quality=Q] -f A -l B in.pdf <tmp>/p` per run, 120 s timeout per run, cancel honoured.
- Rename the zero-padded outputs to allocations with suffix `_p{n}` (1-based page), or no suffix when exactly one page is selected (use -singlefile).
- Progress: pages done over total. Each output must be non-empty.
Pure tests for the arg builder and the rename mapping. Golden (require_engine pdftoppm): sample.pdf to png gives 3 files _p1.._p3; page range '2' gives one png with no suffix; dimensions at 150 dpi. routes_poppler.rs: pdf->png is poppler; pdf->webp goes poppler png then image/magick; docx->png is soffice(docx->pdf) then poppler(pdf->png).

**Acceptance:**

- cargo test -p morphring-engines poppler passes
- cargo test -p morphring-engines --test poppler_render --test routes_poppler passes
- T=$(mktemp -d); cargo run -q -p morphring -- convert crates/engines/tests/fixtures/sample.pdf --to png -o $T gives sample_p1.png sample_p2.png sample_p3.png
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-37: Poppler engine: PDF to text (UTF-8 with BOM)

- **Deps:** P2-03b, P2-04, P2-21
- **Commit:** `feat(engines): extract PDF text with pdftotext`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/poppler/text.rs`, `crates/engines/tests/poppler_text.rs`

Fill poppler/text.rs (docs/research/v02-engines-and-crates.md section 2.4; Filee PdfTextConverter, reverse-engineering/02 section 9.3).
- Edge pdf->txt at cost 10.
- `pdftotext -enc UTF-8 in.pdf <tmp>/out.txt` (no -layout), 120 s timeout.
- Post-process: form feeds become a blank line between pages; collapse 3+ newlines to 2. Write UTF-8 with a BOM.
- Empty or whitespace-only text fails with 'The PDF contains no text (it may be scanned or contain only images).'
- Fake progress (progress::Estimate, typical 1 s).
Pure tests for the post-processing. Golden: sample.pdf gives a txt starting with EF BB BF and containing 'Page 1' and 'Café déjà vu' (P2-04's fixture; no CJK).

**Acceptance:**

- cargo test -p morphring-engines --test poppler_text passes
- T=$(mktemp -d); cargo run -q -p morphring -- convert crates/engines/tests/fixtures/sample.pdf --to txt -o $T; head -c3 $T/sample.txt | xxd shows efbbbf
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-26: Ghostscript engine: PDF/EPS/PS/AI to image

- **Deps:** P2-03b, P2-04, P2-14, P2-21, P2-25
- **Commit:** `feat(engines): add the Ghostscript engine for PDF, EPS and PS`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/ghostscript/mod.rs`, `crates/engines/src/ghostscript/args.rs`, `crates/engines/tests/ghostscript.rs`, `crates/engines/tests/routes_ghostscript.rs`

Fill the ghostscript engine (Filee GhostscriptConverter, reverse-engineering/02 section 8.2).
- Edges: eps/ps/ai -> png/jpg/tiff at cost 10; pdf -> png/jpg/tiff at cost 12, so poppler wins (pinned in routes_ghostscript.rs, plus 'with poppler missing, pdf->png uses ghostscript' through the dream/installed planner split).
- Devices: pngalpha for EPS/PS/AI, png16m for PDF, jpeg with -dJPEGQ, tiff24nc.
- `-dSAFER -dBATCH -dNOPAUSE -dQUIET -r<dpi> -dFirstPage/-dLastPage` per page run, `-dEPSCrop` for EPS, `-sOutputFile=<tmp>/p%d.<ext>`; map outputs to `_p{n}` like poppler.
- Fake progress typical 2 s; timeout 120 s. AI files that are not PDF-based fail with Filee's message.
Pure arg tests. Golden (require_engine gs): sample.eps to png keeps alpha (PNG IHDR colour type 6); sample.ps to png; sample.pdf page range '1-2' to jpg gives 2 files (call the engine directly).

**Acceptance:**

- cargo test -p morphring-engines --test ghostscript --test routes_ghostscript passes
- T=$(mktemp -d); cargo run -q -p morphring -- convert crates/engines/tests/fixtures/sample.eps --to png -o $T produces $T/sample.png
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-27: Native PDF: images to PDF with krilla

- **Deps:** P2-03b, P2-04
- **Commit:** `feat(engines): write images to PDF natively with krilla`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/pdf/image_to_pdf.rs`, `crates/engines/tests/pdf_images.rs`

Fill pdf/image_to_pdf.rs (docs/research/v02-engines-and-crates.md section 2.1; Filee PdfSharpConverter, reverse-engineering/02 section 9.1).
- Edges {png, jpg, gif, bmp, tiff, webp} -> pdf at cost 10. The pdf engine's index 2 beats magick's 3; pin in pdf_images.rs that png->pdf routes to 'pdf'.
- A JPEG with no alpha is embedded byte for byte (Image::from_jpeg). Others are decoded with the image crate (PNG keeps alpha as an SMask). webp decodes only if the image crate has webp; otherwise drop the webp edge and let magick keep it (say which in the commit body).
- One page per frame: first frame only, unless a multi-frame TIFF or GIF.
- PageSize FitImage: page = image size at the source DPI, else 96; cap the long side at 14400 pt (the PDF limit). A4/Letter: landscape when the image is wider, margin_mm, scale to fit, centred.
- Progress (i+1)/frames. Write to a temp file and move it into place.
Tests: page size math (pure). Golden: sample.png to pdf has 1 page (via lopdf); sample.jpg to pdf contains the original JPEG bytes (DCTDecode stream); A4 gives a 595x842 MediaBox.

**Acceptance:**

- cargo test -p morphring-engines --test pdf_images passes
- T=$(mktemp -d); cargo run -q -p morphring -- convert crates/engines/tests/fixtures/sample.jpg --to pdf -o $T; pdfinfo $T/sample.pdf shows Pages: 1
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-28: Native PDF: page range and split with lopdf

- **Deps:** P2-03b, P2-04, P2-14
- **Commit:** `feat(engines): extract and split PDF pages with lopdf`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/pdf/pages.rs`, `crates/engines/tests/pdf_pages.rs`

Fill pdf/pages.rs (docs/research/v02-engines-and-crates.md section 2.2).
- Edge pdf->pdf (self edge) at cost 10.
- Load with lopdf and a decompression cap. Encrypted input fails with a clear message.
- split_pages: one file per selected page, suffix `_p{page}`. Else, a non-blank page_range keeps only the selected pages (delete_pages, prune objects, compress). Else re-save unchanged (Filee's same-format behaviour).
The pdf-split preset uses Subfolder `{name}`, which the allocator handles.
Golden: sample.pdf with page_range '2-3' gives 2 pages; split gives 3 files of 1 page. Encrypted PDF: build one in-test with lopdf if feasible, else `#[ignore]` with a reason comment.

**Acceptance:**

- cargo test -p morphring-engines --test pdf_pages passes
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-30: Native PDF: SVG to PDF with krilla-svg

- **Deps:** P2-03b, P2-04
- **Commit:** `feat(engines): convert SVG to PDF with krilla-svg`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/pdf/svg_pdf.rs`, `crates/engines/tests/pdf_svg.rs`

Fill pdf/svg_pdf.rs (docs/research/v02-engines-and-crates.md section 3.2).
- Edges svg->pdf and svgz->pdf at cost 10.
- Parse with usvg 0.47, `resources_dir` = the SVG's folder (local references only), fontdb system fonts loaded once in a OnceLock.
- Empty or infinite bounds fail with Filee's message. Page size = the SVG size in pt.
Golden: sample.svg to pdf gives 1 page; pdftotext of it contains the SVG's text (skip that part if pdftotext is missing).

**Acceptance:**

- cargo test -p morphring-engines --test pdf_svg passes
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-31: Native image engine (image crate) ahead of ImageMagick

- **Deps:** P2-03b, P2-04
- **Commit:** `feat(engines): add the native image engine`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/native_image/mod.rs`, `crates/engines/src/native_image/ops.rs`, `crates/engines/tests/native_image.rs`, `crates/engines/tests/routes_images.rs`

Fill native_image (docs/research/v02-engines-and-crates.md section 3.1).
- Edges among {png, jpg, gif, bmp, tiff, ico, qoi, ppm, tga} (ids from formats.rs; `ppm` covers pgm/pbm/pnm), self edges included. Still->gif edges cost 12 (Filee's still-to-gif cost, so routes.rs's mp4->gif pin stays true: 10+6+12 > 15+6); every other edge costs 8.
- `accepts(options)` is false when a preset uses an option this engine does not implement (keep_metadata false with keep_color_profile, dpi, tiff_compression other than Lzw/None, sharpening, webp_lossless); the planner then falls back to magick.
- Implemented: EXIF orientation; JPEG max side 65,500; flatten alpha onto the background for jpg/ppm/bmp; JPEG quality = options.quality or image.quality or 90; resize (percent, long edge, width; never upscaling; Lanczos3); grayscale; first frame for single-frame targets; ico sizes (largest <= 256).
- Always set image::Limits (max_alloc 512 MB, 100 MP).
Pure tests for the option mapping and accepts. Golden: png->jpg has no alpha and the right dimensions; a half-size preset; an EXIF-rotated jpg is oriented (build a tiny one in-test or `#[ignore]` with a reason). routes_images.rs (from P2-03b): png->jpg and png->png go to image at 8 + penalty(image); png->gif to image at 12; mp4->jpg becomes ffmpeg(mp4->png) then image(png->jpg); a preset with an option image declines routes png->jpg to magick.
Tests it does not own: the magick goldens in crates/cli/tests/{cli,convert}.rs and app/src-tauri/tests/commands.rs (`magick_converts_png_to_jpg`, `mp4_to_jpg_takes_two_hops`, `png_rings_and_converts_with_imagemagick`) assert only the output bytes, not the engine, so they stay valid once image wins the route. Run them before handing over. If any fails, stop and report to the orchestrator rather than editing those files.

**Acceptance:**

- cargo test -p morphring-engines --test native_image --test routes_images passes
- cargo test --workspace --locked passes, including crates/cli/tests/cli.rs and convert.rs unchanged
- cargo test --locked --manifest-path app/src-tauri/Cargo.toml passes unchanged
- T=$(mktemp -d); env PATH=/nonexistent $(pwd)/target/debug/morphring convert crates/engines/tests/fixtures/sample.png --to jpg -o $T exits 0 (magick cannot be found)
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-32: Native SVG engine (resvg) and svg<->svgz

- **Deps:** P2-03b, P2-04
- **Commit:** `feat(engines): render SVG natively with resvg`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/svg/mod.rs`, `crates/engines/tests/svg.rs`

Fill svg (Filee SvgRenderer, reverse-engineering/02 section 8.1).
- Edges svg/svgz -> png at cost 10 (other rasters chain through image or magick), plus svg->svgz and svgz->svg at cost 5.
- Gunzip input starting with 1F 8B. resources_dir = the SVG's folder.
- scale = max(dpi/96, 1024/long_edge), capped at 16,384 px per side and 100 MP. Empty or infinite bounds fail.
- svg->svgz gzips the text after checking for `<svg` in the first 4 KB; svgz->svg gunzips it.
Golden: sample.svg to png has a long edge >= 1024; svgz round trip; sample.svg to jpg routes svg->png->jpg.

**Acceptance:**

- cargo test -p morphring-engines --test svg passes
- T=$(mktemp -d); cargo run -q -p morphring -- convert crates/engines/tests/fixtures/sample.svg --to png -o $T produces $T/sample.png
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-33: Archive safety layer (ArchiveSafety port)

- **Deps:** P2-03a, XP-01
- **Commit:** `feat(engines): port Filee's archive safety rules`
- **CI OS check:** windows
- **Owns:** `crates/engines/src/archive/safety.rs`

Port src/Filee.Engines/Archives/ArchiveSafety.cs (docs/reverse-engineering/04-engines-archive-cad-font.md section 1.7; research doc section 4) as pure functions:
- `is_unsafe_path(raw)`: a leading / or \, a drive letter, or any .. segment.
- `resolve_entry_path(root, raw) -> Option<PathBuf>`: sanitise segments, rename Windows device names, the result stays under root.
- `MAX_ENTRIES = 1_000_000`.
- `check_disk(total_declared, free) -> Result` with a 64 MiB reserve. Free space via libc::statvfs (Unix) and GetDiskFreeSpaceExW (Windows, windows-sys); unknown free space skips the check, as Filee does. The two FFI calls are the only unsafe code, in one `#[allow(unsafe_code)]` fn each with a reason comment.
- `copy_limited(reader, writer, declared) -> io::Result<u64>` fails if more than declared arrives.
- `remove_links(root)`; `content_root(staging)` unwraps a single top folder.
Port Filee's ArchiveSafety tests, plus: foo/../bar rejected, C:\x rejected, CON renamed, a lying size caught.

**Acceptance:**

- cargo test -p morphring-engines archive::safety passes
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes
- CI windows-latest: the safety tests (including the free-space call) pass

### P2-34: Archives: extract zip/tar/tgz to a folder

- **Deps:** P2-33, P2-04, P2-03b
- **Commit:** `feat(engines): extract zip and tar archives safely`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/archive/extract.rs`, `crates/engines/tests/archive_extract.rs`

Fill archive/extract.rs (Filee ArchiveExtractor, reverse-engineering/04 section 1.5).
- Edges zip, tar, tgz -> folder at cost 10. Archive to archive (zip -> tgz) routes through P2-35's folder edges. Other archive formats stay unrouted (v0.3).
- Allocate the output with an empty ext (a folder). Extract into a staging folder next to it, then rename.
- Our own loop per format: refuse encrypted zip entries with Filee's message, skip symlinks and hardlinks, resolve_entry_path, copy_limited, the entry cap, the disk check.
- Progress by entries; cancel checked per entry.
Tests build malicious archives in-test with the zip and tar writers (../evil, /abs, C:\x, symlink then a file through the link, a lying size, too many entries via a small injected cap): each is refused or skipped and nothing lands outside the TempDir. Golden: sample.zip and sample.tar.gz extract to hello.txt and dir/inner.txt.

**Acceptance:**

- cargo test -p morphring-engines --test archive_extract passes
- T=$(mktemp -d); cargo run -q -p morphring -- convert crates/engines/tests/fixtures/sample.zip --to folder -o $T creates $T/sample/hello.txt
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-35: Archives: pack a file or folder into zip/tar/tgz

- **Deps:** P2-33, P2-34, P2-03b
- **Commit:** `feat(engines): pack files into zip and tar archives`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/archive/pack.rs`, `crates/engines/tests/archive_pack.rs`

Fill archive/pack.rs (Filee ArchivePacker, reverse-engineering/04 section 1.6).
- Edges: every non-archive readable format -> zip, tar, tgz at Filee's costs, plus folder -> zip/tar/tgz, so zip -> tgz routes zip -> folder -> tgz (P2-34 is on main; test that route through run_job).
- ZIP: UTF-8 names, DOS dates clamped to 1980..2107, deflate level from ArchiveOptions.level (Store, Fast, Normal, Maximum).
- TAR: PAX headers for long names, mode 0644 files / 0755 dirs. TGZ: GzEncoder at that level.
- Skip symlinks. Write atomically. CombineIntoOne (zip-all) is deferred to v0.3.
Tests: pack sample.txt into each format and read it back (names, contents, a non-ASCII name); level Store gives method Stored; the zip->tgz route.

**Acceptance:**

- cargo test -p morphring-engines --test archive_pack passes
- T=$(mktemp -d); cargo run -q -p morphring -- convert crates/engines/tests/fixtures/sample.txt --to zip -o $T; bsdtar -tf $T/sample.zip lists sample.txt
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-36: Pandoc engine

- **Deps:** P2-03b, P2-04, P2-21
- **Commit:** `feat(engines): add the Pandoc engine`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/pandoc/mod.rs`, `crates/engines/src/pandoc/args.rs`, `crates/engines/tests/pandoc.rs`, `crates/engines/tests/routes_pandoc.rs`

Port src/Filee.Engines/Office/PandocConverter.cs (docs/research/v02-engines-and-crates.md section 7; reverse-engineering/03 section 4).
- Reader and writer tables as given. Edges: {docx, odt, html, rtf} -> md and md -> {docx, odt, html, rtf, txt} at cost 10; html <-> {docx, odt} at 14; {rst, tex} -> the rest at 10; {md, html, docx, odt} -> {rst, tex} at 10. No pdf.
- `pandoc <in> -f R -t W -o <tmp out>`, plus --standalone for html/tex and `--extract-media=<outdir>/<stem>_files` for md/html/rst/tex. cwd = the input's folder. Timeout 2 min. Success = the output exists. Error 'Pandoc failed (exit N). <stderr summary>'.
- Fake progress, typical 2 s.
- A code comment on the network-image risk (offline principle), deferred to v0.4.
Arg-list tests. routes_pandoc.rs pins md->docx = pandoc and docx->pdf still soffice. Golden: sample.md to docx (a zip with word/document.xml), to html (contains <h1), to txt.

**Acceptance:**

- cargo test -p morphring-engines --test pandoc --test routes_pandoc passes
- T=$(mktemp -d); cargo run -q -p morphring -- convert crates/engines/tests/fixtures/sample.md --to docx -o $T produces $T/sample.docx
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-29: Native PDF: merger for the merge-pdf preset

- **Deps:** P2-03b, P2-04, P2-17, P2-41, P2-27
- **Commit:** `feat(engines): merge PDFs with lopdf`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/engines/src/pdf/merge.rs`, `crates/engines/tests/pdf_merge.rs`, `crates/cli/tests/convert_merge.rs`

Fill pdf/merge.rs: `LopdfMerger` implementing core::Merger; `merger()` returns `Some(&LopdfMerger)`.
- Port lopdf's examples/merge.rs: renumber objects per document, collect Page objects in order, rebuild one Pages tree and Catalog, drop old catalogs and outlines, keep annotations and links.
- Refuse encrypted inputs. Progress per input. Write to a temp file, then move into place.
Golden: merging sample.pdf with itself gives 6 pages. Through core run_job with options.pdf.merge_into_single on [sample.png, sample.pdf]: one 4-page output attached to file 1, file 2 Done with no outputs (P2-27 is on main, so png->pdf is native).
crates/cli/tests/convert_merge.rs: `morphring convert a.pdf b.pdf --preset "Merge PDF"` gives one a_merged.pdf (or the pattern from the built-in preset) with the summed page count.

**Acceptance:**

- cargo test -p morphring-engines --test pdf_merge passes
- cargo test -p morphring --test convert_merge passes
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes

### P2-40: CLI: morphring presets

- **Deps:** P2-05, P2-11
- **Commit:** `feat(cli): list presets with morphring presets`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/cli/src/commands/presets.rs`, `crates/cli/tests/presets.rs`

Fill commands/presets.rs (docs/reverse-engineering/07-cli-tests-docs.md section 5.5).
- Load Library::load(Store { dir: ctx.config_dir }).
- Print `{:<24} {:<28} TARGET` as the header, one line per preset in library order (id, display name, target), a blank line and 'Use one with: morphring convert <files> --preset <id or name>'. Exit 0.
Tests via run_in_with_config with a TempDir: defaults contain to-png and merge-pdf; a custom presets.json lists its own preset; a corrupt presets.json still lists the built-ins and leaves presets.json.broken.

**Acceptance:**

- cargo test -p morphring --test presets passes
- cargo run -q -p morphring -- presets | wc -l prints 71 (header + 68 presets + blank + hint)
- cargo clippy -p morphring --all-targets -- -D warnings passes

### P2-41: CLI: convert --preset, --keep-dates and the settings default output

- **Deps:** P2-05, P2-11, P2-12, P2-16
- **Commit:** `feat(cli): convert with --preset and --keep-dates`
- **CI OS check:** none (Linux only)
- **Owns:** `crates/cli/src/resolve.rs`, `crates/cli/src/commands/convert.rs`, `crates/cli/tests/convert_presets.rs`

Fill resolve.rs and wire convert.rs (docs/reverse-engineering/07-cli-tests-docs.md sections 6.1 to 6.3).
1. `resolve::job_from_args(to, preset, quality, overwrite, skip, keep_dates, output, cwd, &Library, &AppSettings) -> Result<JobSpec { target: Target, options: ConvertOptions, rule: OutputRule, label }, UsageError>`. --to: Preset::for_target (unknown or read-only formats keep the v0.1 messages). --preset: preset::find, else "morphring: no preset '<x>'. Run 'morphring presets' for the list." and exit 2.
2. Overrides in Filee's order: settings.with_default_output, -q to options.quality, --overwrite/--skip, --keep-dates, -o to CustomFolder (KeepFoldersFrom stays empty).
3. convert.rs builds Library and AppSettings from Store { dir: ctx.config_dir } and calls run_job with the spec. The JSON report's `target` shows the preset target as stored ('*' for half-size).
Tests: --preset jpg-70 gives sample_70.jpg; --preset half-size on a png gives sample_50.png at half width (FakeConverter self edge); --keep-dates copies the mtime; a settings.json with defaultOutput Subfolder 'converted' puts the output in ./converted/; an unknown preset exits 2.

**Acceptance:**

- cargo test -p morphring passes
- T=$(mktemp -d); cargo run -q -p morphring -- convert crates/engines/tests/fixtures/sample.png --preset grayscale -o $T produces $T/sample_gray.png (or the built-in pattern)
- cargo run -q -p morphring -- convert a.png --preset nope exits 2
- cargo clippy -p morphring --all-targets -- -D warnings passes

### P2-42: CLI: morphring watch

- **Deps:** P2-41, P2-15, XP-01
- **Commit:** `feat(cli): watch a folder and convert files as they arrive`
- **CI OS check:** windows, macos
- **Owns:** `crates/cli/src/commands/watch.rs`, `crates/cli/tests/watch.rs`

Fill commands/watch.rs (docs/reverse-engineering/07-cli-tests-docs.md section 5.3; research doc section 5).
- Resolve the job with resolve::job_from_args, then force Location CustomFolder = the rule's resolved output folder, with keep_folders_from = the folder.
- A missing folder exits 2 with 'morphring: Folder not found: <path>'.
- Unless --quiet, print 'Watching <folder> -> <preset>, saving to <out>. Press Ctrl+C to stop.'
- A notify RecommendedWatcher (recursive only with -r) feeds paths into a channel; Flag::Rescan triggers Settler::rescan of a walkdir listing.
- A 1 s ticker (recv_timeout loop) calls Settler::tick. Ready files run as one job through run_job (batches serial), printing report::file_line per file.
- --move-originals: scan existing files at start; move each Done source with watch::originals_target.
- ctx.cancel ends the loop, lets the current batch finish, exits 0. A max_user_watches error is reported clearly.
Tests: port Filee's Watch_converts_files_that_arrive_until_cancelled with --settle 0.2 (write a png after 500 ms, expect converted/arrived.jpg within 10 s, cancel, exit 0) and Watch_needs_an_existing_folder (exit 2, 'not found'). FakeConverter, no engines. Use generous timeouts: these also run on CI Windows and macOS (FSEvents latency).

**Acceptance:**

- cargo test -p morphring --test watch passes
- Manual here: mkdir /tmp/w; morphring watch /tmp/w --to jpg --settle 1 & then cp a png into /tmp/w gives /tmp/w/converted/<name>.jpg; kill -INT exits 0
- CI windows-latest and macos-latest: the watch tests pass
- cargo clippy -p morphring --all-targets -- -D warnings passes

### P2-52: App: settings store state and settings page

- **Deps:** P2-06, P2-11, P2-12
- **Commit:** `feat(app): add the settings page`
- **CI OS check:** none (Linux only)
- **Owns:** `app/src-tauri/src/settings.rs`, `app/ui/settings.html`, `app/ui/settings.js`, `app/ui/settings.css`

Fill settings.rs.
- Managed state `Prefs { store, settings: Mutex<AppSettings>, library: Mutex<Library> }`, loaded lazily with Store { dir: config_dir(app) } (use `app.try_state` and `app.manage` on first use, since lib.rs is not ours).
- `pub fn current(app) -> (AppSettings, Library)` for the other modules. settings_get returns the settings JSON; settings_set validates, saves atomically and replaces the state; presets_list returns `{ id, name, target }`.
Settings page (plain JS, window.__TAURI__, no inline scripts): default output (same folder, subfolder with a name field, or a custom folder from the dialog plugin); 'Keep file dates' and 'Keep history' toggles; a read-only preset list and the config folder path with an 'Open folder' button (opener plugin). Changes save on input, debounced.
Unit tests for validation (a blank subfolder name is rejected; a custom folder must be absolute) as plain functions.
Manual Hyprland check (tray > Settings, choose Subfolder 'out', settings.json shows it): run it if a Wayland session is reachable; otherwise If no such machine is reachable, the tester records this check as UNVERIFIED in the commit body (line `Unverified: <check>`) and the task still lands; R-03 lists it in the README.

**Acceptance:**

- cargo test --locked --manifest-path app/src-tauri/Cargo.toml passes
- cargo clippy --locked --manifest-path app/src-tauri/Cargo.toml --all-targets -- -D warnings passes
- A Rust test calls the settings_set logic with a TempDir config dir and finds defaultOutput.location 'Subfolder' in settings.json
- Manual Hyprland check run, or recorded as UNVERIFIED

### P2-53: App: record history and show the history view

- **Deps:** P2-06, P2-13
- **Commit:** `feat(app): record conversions and show history`
- **CI OS check:** none (Linux only)
- **Owns:** `app/src-tauri/src/history.rs`, `app/ui/history.html`, `app/ui/history.js`

Fill history.rs.
- `record(app, &JobRecord)` builds a HistoryEntry and calls History::add with Store { dir: settings::config_dir(app) } and AppSettings (read settings.json directly, so this does not depend on P2-52's state). The logic is a plain function taking the dir.
- history_list returns the entries; history_clear clears them.
History page: newest first, time, label, state, counts. Click an output to reveal it (opener reveal). A 'Clear' button. Refreshes on `job-finished`.
Tests: record writes one entry into a TempDir store.
Manual check (convert via the ring, then tray > History shows the job): run it if possible; otherwise If no such machine is reachable, the tester records this check as UNVERIFIED in the commit body (line `Unverified: <check>`) and the task still lands; R-03 lists it in the README.

**Acceptance:**

- cargo test --locked --manifest-path app/src-tauri/Cargo.toml passes
- cargo clippy --locked --manifest-path app/src-tauri/Cargo.toml --all-targets -- -D warnings passes
- Manual check run, or recorded as UNVERIFIED

### P2-50: App: ring from presets, convert with preset options and default output

- **Deps:** P2-52, P2-18
- **Commit:** `feat(app): build the ring from presets and honour their options`
- **CI OS check:** none (Linux only)
- **Owns:** `app/src-tauri/src/commands.rs`, `app/ui/ring.js`, `app/src-tauri/tests/commands.rs`

Fill in two app gaps: options are ignored, and the ring shows favourites.
- ring_model uses settings::current(app).1 (the Library) and slices::suggest with a ReachCtx. RingSlice gains `presetId` and keeps target and label.
- The convert command becomes `convert { files, presetId }`: find the preset, build Target and ConvertOptions from it, build the rule with AppSettings::with_default_output, run run_job, and use the preset display name for the {preset} token and the job label.
- ring.js invokes with presetId and documents the contract at the top.
- Keep Conversions testable without a window: inject the Library and AppSettings in Conversions::new for tests.
tests/commands.rs: a png gives the images profile slices in Filee order; presetId 'jpg-70' gives *_70.jpg; a Subfolder default output is honoured. Keep `png_rings_and_converts_with_imagemagick` (renamed if it no longer uses magick: it asserts output bytes only).
Manual Hyprland check (dropping a png shows PNG, JPG, WEBP, PDF, TIFF, BMP, ICO, Half size; Half size writes <name>_50.png): run it if possible; otherwise If no such machine is reachable, the tester records this check as UNVERIFIED in the commit body (line `Unverified: <check>`) and the task still lands; R-03 lists it in the README.

**Acceptance:**

- cargo test --locked --manifest-path app/src-tauri/Cargo.toml passes
- node --test app/ui/ passes
- cargo clippy --locked --manifest-path app/src-tauri/Cargo.toml --all-targets -- -D warnings passes
- Manual Hyprland check run, or recorded as UNVERIFIED

### P2-51: App: cancel a running job from the ring

- **Deps:** P2-50
- **Commit:** `feat(app): cancel a running conversion from the ring`
- **CI OS check:** none (Linux only)
- **Owns:** `app/src-tauri/src/commands.rs`, `app/ui/ring.js`, `app/ui/ring.css`, `app/src-tauri/tests/commands.rs`

Fill in the cancel gap.
- Conversions keeps the running job's CancelToken by job_id. cancel_job { jobId } (registered by P2-06) fires it; queued jobs are removed and reported Cancelled.
- In the ring's progress mode, Escape or a click on the centre cancels the showing job (ring.js); outside progress mode Escape still just hides the ring.
- ring-done carries Outcome::Cancelled.
tests/commands.rs: submit a slow job (FakeConverter with Sleep via a test-only constructor that takes engines), cancel it; the summary outcome is Cancelled and no outputs are left.
Manual check (Escape during a large video's progress stops it with no partial output): run it if possible; otherwise If no such machine is reachable, the tester records this check as UNVERIFIED in the commit body (line `Unverified: <check>`) and the task still lands; R-03 lists it in the README.

**Acceptance:**

- cargo test --locked --manifest-path app/src-tauri/Cargo.toml passes
- node --test app/ui/ passes
- Manual check run, or recorded as UNVERIFIED

### P2-54: App: toast for long jobs

- **Deps:** P2-06, P2-51
- **Commit:** `feat(app): show a toast for long-running jobs`
- **CI OS check:** none (Linux only)
- **Owns:** `app/src-tauri/src/toast.rs`, `app/ui/toast.html`, `app/ui/toast.js`, `app/ui/toast.css`

Port the toast (docs/reverse-engineering/05-app-ui-ring.md section 9.5), simplified.
- When a job still runs 1.5 s after the ring hid (or started from --convert with the ring hidden), show the `toast` window: 360 px wide, no focus, bottom-right of the primary monitor's work area with a 12 px margin. On Wayland, accept the compositor's placement.
- A card per job: '<file or N files> -> <label>', progress %, a status line, Cancel (cancel_job), Show (reveal the first output) and Dismiss.
- Finished cards go after 5 s on success, 15 s on errors. Hide the window when no cards remain.
- Driven only by P2-06's job-started, job-progress and job-finished events.
Timing rules are a pure function in toast.rs with tests (show_after, dismiss_after).
Manual Hyprland check (convert a video, close the ring, the toast shows progress, Cancel stops the job): run it if possible; otherwise If no such machine is reachable, the tester records this check as UNVERIFIED in the commit body (line `Unverified: <check>`) and the task still lands; R-03 lists it in the README.

**Acceptance:**

- cargo test --locked --manifest-path app/src-tauri/Cargo.toml passes
- cargo clippy --locked --manifest-path app/src-tauri/Cargo.toml --all-targets -- -D warnings passes
- Manual Hyprland check run, or recorded as UNVERIFIED

### XP-03: Windows: kill process trees with a Job Object

- **Deps:** P2-03a, XP-01
- **Commit:** `feat(engines): kill Windows process trees with a Job Object`
- **CI OS check:** windows
- **Owns:** `crates/engines/src/win_job.rs`

Fill win_job.rs (docs/research/windows-support.md section 2, option A). `#![allow(unsafe_code)]` on this module only, with a reason comment.
- `attach(child) -> Option<Job>`: CreateJobObjectW(null, null), SetInformationJobObject(JobObjectExtendedLimitInformation, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE), AssignProcessToJobObject(job, child.as_raw_handle()). Non-inheritable handle. None plus a debug log on failure.
- `Job::terminate()` calls TerminateJobObject(job, 1); Drop calls CloseHandle.
Assign after spawn (no CREATE_SUSPENDED); record the race note from the research doc.
Test under `#[cfg(windows)]`, without wmic (removed on windows-2025): spawn `cmd /c "start /b ping -n 30 127.0.0.1 & ping -n 30 127.0.0.1"` through process::run with a cancel token, cancel after 1 s, then poll `tasklist /fi "imagename eq ping.exe" /fo csv /nh` for up to 3 s and assert no ping.exe whose parent chain is ours remains. Simpler and preferred if it works: record the grandchild's PID by having the child print it (`powershell -NoProfile -c "$p = Start-Process ping -ArgumentList '-n','30','127.0.0.1' -PassThru; $p.Id; Start-Sleep 30"`), then check with OpenProcess + GetExitCodeProcess that it has exited.

**Acceptance:**

- cargo test -p morphring-engines passes on Linux (module not compiled)
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes
- CI windows-latest: the win_job tree-kill test passes

### XP-04: Engine discovery and install hints for Windows and macOS

- **Deps:** P2-03a, XP-01
- **Commit:** `feat(engines): find engines and give install hints on Windows and macOS`
- **CI OS check:** windows, macos
- **Owns:** `crates/engines/src/discovery.rs`

docs/research/windows-support.md sections 1 and 4; docs/research/macos-support.md section 7.
1. Windows hints in error.engine_missing and doctor: `winget install -e --id ImageMagick.ImageMagick | Gyan.FFmpeg | TheDocumentFoundation.LibreOffice | JohnMacFarlane.Pandoc`. Poppler and Ghostscript: `scoop install poppler` / `scoop install ghostscript`.
2. macOS hints: brew for every tool; LibreOffice is `brew install --cask libreoffice`.
3. Search folders. Windows: `%ProgramFiles%\LibreOffice\program` and `%ProgramFiles(x86)%\...` (prefer soffice.com over soffice.exe), `%ProgramFiles%\gs\gs*\bin` for gswin64c, `%ProgramFiles%\ImageMagick-*`. macOS: keep /opt/homebrew/bin and /usr/local/bin, add /Applications/LibreOffice.app/Contents/MacOS/soffice. All through the pure `locate_in(tool, os, PATH, extra)` with injected env values.
4. Regression test: Tool::Magick never resolves `convert`, so `C:\Windows\System32\convert.exe` on PATH is ignored.
5. dunce::canonicalize wherever discovery canonicalises, so no \\?\ paths reach an argv.
Pure unit tests per OS for hint text and search folders (the OS is a parameter, so they run on Linux). A `#[cfg(any(windows, target_os = "macos"))]` test asserts magick and ffmpeg are found when installed (CI installs them).

**Acceptance:**

- cargo test -p morphring-engines discovery passes
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes
- CI windows-latest finds magick and ffmpeg; macos-latest finds magick, ffmpeg, pdftoppm, gs, pandoc (the cfg test, or a `morphring doctor` line in the test log)

### XP-05: Windows: retry move_into_place on sharing violations

- **Deps:** P2-20, XP-01
- **Commit:** `fix(engines): retry renames blocked by Windows file locks`
- **CI OS check:** windows
- **Owns:** `crates/engines/src/process.rs`

docs/research/windows-support.md section 1 item 5. On Windows, `move_into_place` retries the rename 3 times with 100 ms backoff on ERROR_SHARING_VIOLATION (32) or ERROR_ACCESS_DENIED (5) (antivirus or the indexer may hold a new file), before falling back to copy and remove.
The retry policy is a pure `should_retry(raw_os_error, attempt) -> Option<Duration>` with tests on every OS; that is the real coverage.
A Windows lock test (hold the destination open without FILE_SHARE_DELETE, release after 150 ms on another thread, expect success) is timing-dependent on shared CI runners, so mark it `#[ignore = "timing-dependent; run by hand"]`.

**Acceptance:**

- cargo test -p morphring-engines process passes
- cargo clippy -p morphring-engines --all-targets -- -D warnings passes
- CI windows-latest: the engines tests pass (the ignored lock test is listed as ignored)

### XP-07: Windows ring window: re-assert topmost, never steal focus

- **Deps:** P2-06, XP-01
- **Commit:** `fix(app): keep the ring on top on Windows`
- **CI OS check:** windows
- **Owns:** `app/src-tauri/src/ring_window.rs`

docs/research/windows-support.md section 5. In ring_window::show:
- On Windows, set_always_on_top(false) then set_always_on_top(true) on every show (Filee RadialController.cs:301-305).
- If show() activates the window, keep the `focus: false` behaviour. Tauri APIs only; a raw SW_SHOWNOACTIVATE needs the `windows` crate, so leave a TODO linked to v0.4 if Tauri is not enough.
- On hide, the window is hidden, not click-through. macOS needs nothing here (transparency is XP-10).
Linux behaviour stays byte for byte the same (cfg branches).
Behaviour on a real Windows desktop: If no such machine is reachable, the tester records this check as UNVERIFIED in the commit body (line `Unverified: <check>`) and the task still lands; R-03 lists it in the README.

**Acceptance:**

- cargo test --locked --manifest-path app/src-tauri/Cargo.toml passes
- cargo clippy --locked --manifest-path app/src-tauri/Cargo.toml --all-targets -- -D warnings passes
- CI windows-latest app job compiles and tests this file
- Topmost behaviour on Windows recorded as UNVERIFIED in the commit body unless a Windows machine is available

### XP-10: macOS app: transparent ring, tray mode, files from Finder

- **Deps:** P2-06, XP-01
- **Commit:** `feat(app): support the ring, tray and Finder launches on macOS`
- **CI OS check:** macos
- **Owns:** `app/src-tauri/Cargo.toml`, `app/src-tauri/Cargo.lock`, `app/src-tauri/tauri.macos.conf.json`, `app/src-tauri/src/lifecycle.rs`, `app/src-tauri/src/tray.rs`, `app/src-tauri/icons/tray-template.png`, `app/src-tauri/icons/tray-template@2x.png`, `app/scripts/icons.sh`

docs/research/macos-support.md sections 3 to 5. lib.rs is not touched: P2-06 already manages `lifecycle::PendingOpen`, calls `lifecycle::setup(app)` from setup and routes every RunEvent to `lifecycle::on_run_event`.
1. Cargo.toml: `[target.'cfg(target_os = "macos")'.dependencies] tauri = { version = "2", features = ["tray-icon", "macos-private-api"] }`. tauri.macos.conf.json: `{ "app": { "macOSPrivateApi": true }, "bundle": { "fileAssociations": [...] } }` (image, audio, video, pdf and doc extensions, role Viewer, rank Alternate). Update Cargo.lock minimally.
2. lifecycle.rs: `setup` sets ActivationPolicy::Accessory when launched with --background (macOS). `on_run_event`: `RunEvent::Opened { urls }` (macOS) converts file:// URLs to paths and calls the same path as instance::on_second_instance (Debouncer, then open_ring); when the ring window does not exist yet (cold start), push them into `PendingOpen` and drain it at the end of `setup`. `RunEvent::Reopen`: tray::show_main. `ExitRequested`: prevent_exit while the tray exists.
3. tray.rs on macOS: a monochrome template icon (icon_as_template(true)) generated by icons.sh from icon.svg; show_menu_on_left_click(true); switch to Regular when the main window opens and back to Accessory when it closes.
Pure helpers (file URL to path, the pending-queue drain) get unit tests on every OS.
Not checkable by an agent: ring transparency in a bundled .app (tauri#13415) and the Opened event from Finder. If no such machine is reachable, the tester records this check as UNVERIFIED in the commit body (line `Unverified: <check>`) and the task still lands; R-03 lists it in the README.

**Acceptance:**

- cargo test --locked --manifest-path app/src-tauri/Cargo.toml passes on Linux
- cargo clippy --locked --manifest-path app/src-tauri/Cargo.toml --all-targets -- -D warnings passes
- CI macos-latest app job compiles and tests (tauri-build accepts the feature/config pair)
- Transparency and Opened recorded as UNVERIFIED in the commit body

### XP-09: Windows installer: NSIS (current user) in release

- **Deps:** XP-08, XP-06, P2-06
- **Commit:** `feat(app): build an NSIS installer for Windows`
- **CI OS check:** windows
- **Owns:** `app/src-tauri/tauri.windows.conf.json`, `app/src-tauri/windows/hooks.nsh`, `.github/workflows/release.yml`

docs/research/windows-support.md section 6. NSIS only: Tauri's MSI is per-machine and does not run hooks.nsh, so it would install without the Explorer verb (MSI deferred).
- tauri.windows.conf.json: `bundle.targets ["nsis"]`; `bundle.windows.nsis { installMode: "currentUser", installerHooks: "windows/hooks.nsh" }`; webviewInstallMode downloadBootstrapper; no signing.
- hooks.nsh: NSIS_HOOK_POSTINSTALL writes the HKCU verb keys from explorer::verb_entries (with the MorphRingManaged marker) and the Send To shortcut (CreateShortCut); NSIS_HOOK_PREUNINSTALL removes them. A release.yml step greps hooks.nsh for each key-path const in crates/platform/src/explorer.rs and fails if one is missing.
- release.yml: a `windows-app` job (windows-2025, same runner check as XP-08) runs the Check Tauri CLI step and builds with `npx --yes @tauri-apps/cli@$TAURI_CLI_VERSION build --bundles nsis --ci`, then copies `MorphRing_<ver>_x64-setup.exe` into dist. The publish job picks it up via `dist/*`. It is labelled preview through the CHANGELOG (R-03).
Whether the installed verb appears in Explorer: If no such machine is reachable, the tester records this check as UNVERIFIED in the commit body (line `Unverified: <check>`) and the task still lands; R-03 lists it in the README.

**Acceptance:**

- gh workflow run release.yml --ref <branch> produces the setup.exe in the artifact
- The hooks.nsh key-path check step passes in that run
- cargo test --locked --manifest-path app/src-tauri/Cargo.toml passes on Linux (the overlay does not affect Linux)

### XP-12: macOS .dmg (ad-hoc signed) in release

- **Deps:** XP-09, XP-10
- **Commit:** `feat(app): build an ad-hoc signed dmg for macOS`
- **CI OS check:** macos
- **Owns:** `app/src-tauri/tauri.macos.conf.json`, `.github/workflows/release.yml`

docs/research/macos-support.md section 7.
- tauri.macos.conf.json: `bundle.targets ["app", "dmg"]`, `bundle.macOS.signingIdentity "-"`, minimumSystemVersion 13.0.
- release.yml: a `macos-app` job (the same macOS runner label as XP-08's macos-cli, with the arm64 runner check) runs the Check Tauri CLI step and `npx --yes @tauri-apps/cli@$TAURI_CLI_VERSION build --bundles app,dmg --target aarch64-apple-darwin --ci`, copies `MorphRing_<ver>_aarch64.dmg` into dist, then smoke-tests `codesign -dv` on the .app (Signature=adhoc) and `hdiutil verify` on the dmg.
Opening the dmg on a Mac (Gatekeeper flow, ring transparency): If no such machine is reachable, the tester records this check as UNVERIFIED in the commit body (line `Unverified: <check>`) and the task still lands; R-03 lists it in the README.

**Acceptance:**

- gh workflow run release.yml --ref <branch> produces MorphRing_<ver>_aarch64.dmg; the job log shows codesign -dv with Signature=adhoc and hdiutil verify passing
- Mac hands-on check recorded as UNVERIFIED unless a Mac is available

### XP-13: Fix the remaining Windows and macOS test failures

- **Deps:** XP-02, XP-03, XP-04, XP-05, XP-06, XP-07, XP-10, XP-11, P2-19, P2-23, P2-24, P2-25, P2-26, P2-27, P2-28, P2-29, P2-30, P2-31, P2-32, P2-34, P2-35, P2-36, P2-37, P2-40, P2-41, P2-42, P2-50, P2-51, P2-52, P2-53, P2-54, P2-61
- **Commit:** `test: make the suite pass on Windows and macOS`
- **CI OS check:** windows, macos
- **Owns:** `.github/workflows/ci.yml`, `crates/*/tests/** (test files only)`, `#[cfg(test)] modules in crates/*/src/** and app/src-tauri/src/** (test code only)`, `app/src-tauri/tests/**`

Every code task is an ancestor, so nothing else that touches Rust or ci.yml runs at the same time (only release.yml/packaging tasks may: XP-09, XP-12, P2-60).
Run CI on a branch from current main (`--no-fail-fast` is already on), collect every failing test on windows-latest and macos-latest (root workspace and app), and fix each per docs/research/windows-support.md section 1 and macos-support.md section 8:
- paths as Path, never strings; dunce rather than canonicalize; CRLF normalised in assertions; /private/var on macOS;
- `.cmd` fakes replaced by cfg(unix) gates or Rust test binaries;
- tools missing on CI must SKIP, never fail;
- ci.yml: engine installs or env the tests need (e.g. a missing brew/choco package).
Ownership: ci.yml, any `*/tests/*.rs`, and any `#[cfg(test)]` module. Production code only for one-line cfg or path fixes. If a fix needs more, split it into its own task and report it to the orchestrator. The commit body lists every file touched and the failure it fixed.

**Acceptance:**

- CI on the branch: test (windows-latest), test (macos-latest), app (windows-latest) and app (macos-latest) are all green
- cargo test --workspace --locked and cargo test --locked --manifest-path app/src-tauri/Cargo.toml still pass on Linux
- git diff --stat main shows production-code changes only of one-line cfg/path fixes

### XP-14: Make Windows, macOS and MSRV CI jobs blocking

- **Deps:** XP-13
- **Commit:** `chore(ci): make Windows and macOS jobs blocking`
- **CI OS check:** windows, macos
- **Owns:** `.github/workflows/ci.yml`

Remove continue-on-error from test (windows/macos), app (windows/macos) and msrv. Keep deny-advisories and goldens-office non-blocking. Update the header comments (D11 superseded for v0.2: all three OSes block).

**Acceptance:**

- CI on the branch is fully green with no continue-on-error left except deny-advisories and goldens-office
- grep -n continue-on-error .github/workflows/ci.yml shows only those two

### R-02: Docs for 0.2: architecture, engineering rules, third-party notices

- **Deps:** XP-14, XP-12, P2-60, P2-00, P2-40
- **Commit:** `docs: update architecture and engineering guide for v0.2`
- **CI OS check:** none (Linux only)
- **Owns:** `docs/ARCHITECTURE.md`, `docs/ENGINEERING.md`, `THIRD-PARTY-NOTICES.md`, `docs/PLAN.md`

Bring the docs in line with the code on main.
- ARCHITECTURE.md: the new modules (options, preset, builtin, profiles, settings, history, watch, page_range, dates, job/merge; the engines; platform explorer/finder/gesture/integration; app settings/history/toast/ring_window/lifecycle), the registry order, Target and accepts/merger, the CI table and the release jobs. Remove the fixed gaps.
- ENGINEERING.md: MSRV 1.92; the CI table (all OSes block, IM7 in CI, the office goldens job); section 12 scope guard becomes v0.2; the Windows rules (Job Object, dunce, the retry); CC0-1.0 in the license list.
- THIRD-PARTY-NOTICES.md: the new linked crates and licences (image, resvg/usvg, krilla, krilla-svg, lopdf, zip, tar, flate2, notify, filetime, dunce, windows-sys, winreg, mslnk, tauri-plugin-dialog) and the external tools run as processes (poppler, Ghostscript, pandoc).
- PLAN.md section 9: mark v0.2 done, note the v0.4 items pulled in and the deferred ones (link PLAN-v0.2.md).

**Acceptance:**

- Every path named in ARCHITECTURE.md exists (grep backticked paths, test -e)
- No mention of rust-version 1.85 remains outside the CHANGELOG

### R-03: Release 0.2.0: README, CHANGELOG and version bump

- **Deps:** R-02
- **Commit:** `chore(release): prepare v0.2.0`
- **CI OS check:** windows, macos
- **Owns:** `README.md`, `CHANGELOG.md`, `Cargo.toml`, `Cargo.lock`, `crates/cli/Cargo.toml`, `crates/engines/Cargo.toml`, `app/src-tauri/Cargo.toml`, `app/src-tauri/Cargo.lock`, `app/src-tauri/tauri.conf.json`, `packaging/aur/morphring-bin/PKGBUILD`, `packaging/aur/morphring-bin/.SRCINFO`

1. Versions to 0.2.0: the workspace version; every internal path-dep requirement to `version = "0.2.0"` (a 0.x minor bump breaks ^0.1): in crates/cli/Cargo.toml the morphring-core, morphring-engines and morphring-platform deps and the core dev-dep; in crates/engines/Cargo.toml the core dep and dev-dep (`grep -rn 'version = "0.1' crates/*/Cargo.toml` must print nothing). The app Cargo.toml (its path deps have no version) and tauri.conf.json; both lockfiles with `cargo update -w`.
2. AUR: the 0.2.0 tarball does not exist until the orchestrator tags, so the PKGBUILD stays at pkgver=0.1.1 here. If P2-60 left `SKIP`, replace it with the real v0.1.1 checksum from the v0.1.1 release's SHA256SUMS.txt and regenerate .SRCINFO. R-04 bumps to 0.2.0 after the tag.
3. CHANGELOG `## [0.2.0]`. Added: presets, --preset, `presets`, `watch`, --keep-dates, settings, history, the PDF/EPS/SVG/archive/pandoc engines, native engines, app settings/history/toast/cancel, Windows (zip, NSIS installer) and macOS (tarball, dmg) previews. Changed: registry order, MSRV 1.92. Known issues: unsigned Windows (SmartScreen), ad-hoc macOS (Open Anyway or `xattr -dr com.apple.quarantine`), pandoc and network images, Win11 'Show more options'.
4. README: status; the engine table plus poppler, Ghostscript and pandoc with Arch/Debian/winget-scoop/brew columns; install sections for Linux (tarball, .deb, AppImage, AUR), Windows (zip, setup.exe) and macOS (tarball, dmg with the Gatekeeper steps); presets and watch usage; `morphring integrate` per OS. A 'Not yet verified on real hardware' list built from every `Unverified:` line in `git log v0.1.1..HEAD` (at least: macOS ring transparency, the Finder Quick Action, the macOS Opened event, Windows topmost/focus behaviour, the Explorer verb after install).
Run the release workflow by dispatch on the branch before handing over. The orchestrator tags v0.2.0.

**Acceptance:**

- cargo test --workspace --locked passes; ./target/debug/morphring --version prints 'morphring 0.2.0'
- grep -rn 'version = "0.1' crates/*/Cargo.toml prints nothing
- cargo test --locked --manifest-path app/src-tauri/Cargo.toml passes
- grep -c SKIP packaging/aur/morphring-bin/PKGBUILD is 0; makepkg --printsrcinfo | diff - .SRCINFO is empty
- The awk notes extraction for 0.2.0 is non-empty
- README has the 'Not yet verified' list matching git log's Unverified lines
- gh workflow run release.yml --ref <branch> green with every asset

### R-04: AUR package to 0.2.0 after the tag

- **Deps:** R-03
- **Commit:** `chore(packaging): bump morphring-bin to 0.2.0`
- **CI OS check:** none (Linux only)
- **Owns:** `packaging/aur/morphring-bin/PKGBUILD`, `packaging/aur/morphring-bin/.SRCINFO`

Runs only after the orchestrator has tagged v0.2.0 and the release workflow has attached the assets (draft or published). Set pkgver=0.2.0, pkgrel=1, take the tarball's sha256 from the v0.2.0 release's SHA256SUMS.txt (`gh release download v0.2.0 -p SHA256SUMS.txt`), never from a local build, and regenerate .SRCINFO. Nothing else changes.

**Acceptance:**

- makepkg --printsrcinfo | diff - .SRCINFO is empty
- In a temp copy, makepkg -f downloads the v0.2.0 tarball, the checksum verifies, and pacman -Qlp lists /usr/bin/morphring

## 8. Changes from the first draft (pragmatic review)

- P2-40 (`morphring presets`) is now a dep of XP-13 and R-02, so it ships and does not race XP-13 on `crates/cli/tests/presets.rs`.
- XP-01 is a dep of every task with a Windows/macOS CI check (XP-02, P2-16, P2-33, P2-42, and the XP tasks). XP-02 now runs after XP-01 and gets its failing-test list.
- `crates/core/src/formats.rs` is owned by P2-02a, which adds `qoi` and pins the access of every id the engine tasks use. `pnm` stays an extension of `ppm`.
- P2-18 uses a `ReachCtx { reach, can_merge }` struct it owns; `Reach` stays an enum.
- P2-23 depends on P2-20 and owns `ffmpeg/mod.rs` as well as `encoding.rs` (and `tests/ffmpeg.rs`).
- P2-29 depends on P2-27 and P2-35 on P2-34, so no test stays skipped forever.
- P2-31 sets still->gif to cost 12 and checks that the magick goldens in cli/app (which assert output bytes only) still pass, without editing them.
- P2-04 builds sample.pdf with LibreOffice from a flat ODT (embedded font, ToUnicode); the Hangul golden is dropped.
- P2-61 downloads the ImageMagick AppImage from GitHub release assets at a fixed tag.
- XP-09 ships NSIS current-user only; the MSI is deferred.
- XP-13 depends on P2-61 and owns ci.yml, every `*/tests/*.rs` and every `#[cfg(test)]` module; no code task runs beside it.
- P2-06 pre-registers `lifecycle::PendingOpen`, a `lifecycle::setup` hook and the run-event router, so XP-10 never touches lib.rs.
- R-01 pins the Tauri CLI once (`TAURI_CLI_VERSION`, 2.12.1 if accepted, else 2.11.5) and fails on a crate mismatch; XP-08/09/12 reuse it.
- XP-08 checks runner labels (arm64, sw_vers) with fallbacks.
- P2-01 checks `cargo +1.92` on the app workspace too and verifies each crate's version and date itself.
- R-03 bumps every internal path-dep requirement (including platform) and replaces any SKIP with the v0.1.1 checksum; the new R-04 bumps the AUR package to 0.2.0 after the tag with the real checksum.
- XP-02's tester saves an `integrate --dry-run` baseline from main first.
- P2-02 split into P2-02a (stubs, mod lines, formats; no deps) and P2-02b (options, contract, job hooks, callers); P2-03 split into P2-03a (stubs, discovery, process/progress hooks) and P2-03b (registry order and route pins).
- XP-03's test uses tasklist or a PID it printed (no wmic); XP-05's Windows lock test is `#[ignore]`; P2-19 is low priority.
- Every hardware-only check is recorded as `Unverified:` in the commit body, and R-03's README lists them.
