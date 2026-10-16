# Windows support (R&D)

Status: research, 2026-10-16. Target release: v0.4 "Windows and macOS" (PLAN.md section 9), with the cheap parts pulled into v0.2.
Scope: a Rust CLI (`crates/*`) plus a Tauri v2 app (`app/`) on Windows 10/11 x64. Topics: test path pitfalls, killing process trees, the Explorer menu, engine install hints, the ring window, installers and signing, and CI.

Every version below was checked on 2026-10-16 against crates.io, the winget-pkgs/Scoop/Chocolatey repos, the `actions/runner-images` readmes or the vendor docs linked at the end.

## 0. TL;DR

| Area | Recommendation | When |
|---|---|---|
| Windows CI | Fix the 8 failing tests (section 1), then make `test (windows-latest)` blocking. ImageMagick 7.1.2 is **already preinstalled** on the runner; add ffmpeg with `choco`. | v0.2 |
| Process kill | A Job Object per child with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, through `windows-sys` 0.61 directly (about 60 lines, no new crate family). Kill with `TerminateJobObject`. | v0.2 (it is small) |
| Explorer menu | **Stage 1:** a classic HKCU verb (`HKCU\Software\Classes\*\shell\MorphRing`) plus a Send To shortcut, written by `morphring integrate --install` with no admin rights. **Stage 2:** a Win11 top-level entry through an `IExplorerCommand` cdylib in a sparse package, only once we have a signing certificate. | Stage 1 in v0.4 (could be v0.2), stage 2 later in v0.4 or after |
| Engine hints | `winget install --id <id> -e` as the primary hint, with Scoop as the second option (table in section 4). | v0.2, in the `engine_missing` detail |
| Ring window | Keep the Tauri `ring` window (transparent, no decorations, `alwaysOnTop`, `shadow: false`, `focus: false`) and re-assert topmost on every show. Click-through with `set_ignore_cursor_events` is unreliable on Windows, so avoid needing it: hide the window when the ring is not in use. | v0.4 spike |
| Installer | Tauri **NSIS**, `installMode: currentUser`, WebView2 `downloadBootstrapper`. No MSI until someone asks for GPO deployment. | v0.4 |
| Signing | Ship unsigned "preview" builds first. Sign with **Azure Artifact Signing** (formerly Trusted Signing) through `signCommand` if the owner is eligible (individuals: US/Canada only). The free SignPath Foundation path needs a **public** OSI-licensed repo, and the repo is private today. | When a certificate exists |

## 1. Path handling and test pitfalls

The current Windows CI job already shows most of these. The `CI` run on `main` (2026-10-16) has `test (windows-latest)` **failing** with 8 of 66 `morphring` (cli) unit tests red, hidden by `continue-on-error` (D11):

| Failing test | Cause | Fix |
|---|---|---|
| `report::tests::*` (5 tests) | Fixtures use `/home/u/work`. On Windows a path with no drive letter is **not absolute**, so `strip_prefix(cwd)` fails and the line keeps the full path. `Path::join` also writes `\` (`"/home/u/work\\b.png"`). | Build fixtures from a real absolute root (`TempDir`, or `#[cfg(windows)] const CWD = r"C:\work"`) and compare `Path`s, not strings, or normalise with `.to_string_lossy().replace('\\', "/")` only in the assertion. |
| `doctor::tests::current_folders_end_in_morphring` | `directories::ProjectDirs` puts config at `%APPDATA%\morphring\config` and data at `...\data` on Windows. | Assert on the parent or on a `contains("morphring")` component per OS. Decide whether we want the `config` subfolder at all (Filee uses `%APPDATA%\Filee`). |
| `integrate::tests::*` (2 tests) | The tests write Linux file-manager entries (Dolphin, Nautilus, Nemo, Thunar), which make no sense on Windows. `expected.display()` also differs from the `Exec=` text. | `#[cfg(target_os = "linux")]` on these tests and on the Linux `integrate` backends. Windows gets its own backend (section 3). |

Other pitfalls to design for:

1. **8.3 short names.** The runner temp dir is `C:\Users\RUNNER~1\AppData\Local\Temp`. `std::fs::canonicalize` expands it to `runneradmin` **and** adds the `\\?\` verbatim prefix, so a canonical path never equals a `TempDir` path. Never compare a canonical path with a non-canonical one. Use `dunce::canonicalize` (dunce 1.0.5) where we need canonical paths and want them to stay readable, because many tools (ImageMagick, soffice, older ffmpeg builds) reject `\\?\` paths.
2. **Verbatim paths sent to engines.** Keep paths free of `\\?\` before they go into an argv. LibreOffice also needs `-env:UserInstallation=file:///C:/...`, a URL with forward slashes and percent-encoding, not a raw path.
3. **Case-insensitive file systems.** The OutputPathResolver already compares case-insensitively everywhere, and the `A.png`/`a.png` tests are already `#[cfg(target_os = "linux")]`. Keep it that way.
4. **Reserved names and characters.** `CON`, `NUL`, `COM1`, trailing dots or spaces, and `<>:"|?*`. The Windows-strict sanitiser already handles these on every OS. Add a Windows-only test that really creates the sanitised file.
5. **Open files cannot be deleted or renamed over.** `move_into_place` must handle `ERROR_SHARING_VIOLATION` (antivirus or the indexer briefly holding a new file). Retry the rename 3 times with 100 ms backoff before falling back to copy. `tempfile` cleanup can fail for the same reason, so tests should not assert that the temp dir is gone.
6. **`.exe` and PATHEXT.** `which` 8.0.6 resolves `PATHEXT`. Test helpers that write fake engines as `sh` scripts must stay `#[cfg(unix)]`. On Windows write a `.cmd`, but note that a `.cmd` cannot be spawned without `cmd.exe /c` since Rust 1.77.2 (the BatBadBut fix). A small Rust test binary (`[[bin]]` under `tests/`, or `CARGO_BIN_EXE_*`) is a better fake engine.
7. **`magick` vs `convert.exe`.** `C:\Windows\System32\convert.exe` is the FAT-to-NTFS tool. Only ever call `magick`, never `convert`. Discovery already does this, and a regression test should keep it that way.
8. **`soffice`.** The installer puts `soffice.exe` and `soffice.com` in `C:\Program Files\LibreOffice\program`, which is not on PATH. Discovery should probe that folder (and `%ProgramFiles(x86)%`) as it already does for the Linux `program` folder. Prefer `soffice.com` for headless runs, because it is the console wrapper and gives us stdout and the exit code.
9. **Line endings.** Golden text output (pandoc, `--json`) can come out CRLF. Normalise `\r\n` in the assertions, and keep `.gitattributes` with `* text=auto eol=lf` so fixtures do not change on checkout.
10. **Long paths.** Paths over 260 characters fail in engines even when Rust copes. Do not rely on `LongPathsEnabled`. Keep the work dir short (`%TEMP%\mr\<id>`).
11. **Console encoding.** Unicode file names in `println!` work in the Windows console, but an engine's stderr arrives in the OEM code page. `String::from_utf8_lossy` (which the runner already uses) is enough for messages, but never round-trip a path through stderr.

## 2. Killing the process tree: Job Objects

Today `kill_group` on Windows calls `child.kill()` (`crates/engines/src/process.rs`), which leaves `soffice.bin` and other grandchildren running. Filee uses .NET `Process.Kill(entireProcessTree: true)` (`src/Filee.Engines/Infrastructure/ProcessRunner.cs:78`), which walks a snapshot and is racy.

| Option | Crate / version | Pros | Cons |
|---|---|---|---|
| A. Raw Job Object via `windows-sys` | `windows-sys` 0.61.2 (features `Win32_System_JobObjects`, `Win32_Foundation`, `Win32_System_Threading`) | Same approach as cargo's own `src/cargo/util/job.rs`. Small, no async runtime, fits our "one place calls Command" rule. | About 60 lines of `unsafe` with `#[allow(unsafe_code, reason)]`. |
| B. `process-wrap` | 10.0.1 (2026-09-23) | Ready-made `JobObject` and `KillOnDrop` wrappers, and it covers the Unix process group too. | Replaces our `Command` flow. A new dependency for about 60 lines. `command-group` (5.0.1, 2023) is its unmaintained predecessor. |
| C. `win32job` | 2.0.3 (2025-05) | Safe API. | Uses the `windows` crate (a heavier build). Small maintainer base. |
| D. `taskkill /T /F /PID` | none | No unsafe code. | Spawns a process to kill a process. Racy, and it can miss children that already re-parented. |

**Recommendation: A.** The steps:

1. `CreateJobObjectW(null, null)`, then `SetInformationJobObject(JobObjectExtendedLimitInformation)` with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Do **not** let the handle be inherited (default `SECURITY_ATTRIBUTES` = null is fine). An inherited handle keeps the job alive (Raymond Chen, Old New Thing 2013-12-09).
2. Spawn with `CommandExt::creation_flags(CREATE_SUSPENDED)`, `AssignProcessToJobObject(job, child_handle)`, then resume the main thread. This closes the window in which the child could spawn `soffice.bin` before it is in the job. Getting the main thread handle needs a `Thread32First`/`Next` loop or `NtResumeProcess`. A simpler variant that is good enough for us: assign right after spawn without suspending. soffice takes over 100 ms before it forks, and `KILL_ON_JOB_CLOSE` sweeps anything it already started only if it was assigned first, so measure the race in CI before adding the suspend dance. (`PROC_THREAD_ATTRIBUTE_JOB_LIST` would be race-free, but `std::process::Command` cannot pass attribute lists until the unstable `windows_process_extensions_raw_attribute` is stabilised.)
3. On cancel or timeout: `TerminateJobObject(job, 1)`. On drop: `CloseHandle(job)`. Because of `KILL_ON_JOB_CLOSE` this also kills leftovers after a normal exit, the same as the Unix `killpg` sweep.
4. Nested jobs work on Windows 8 and later, so it does not matter that GitHub runners and some terminals already put us in a job.
5. Test: a `#[cfg(windows)]` test spawns `cmd /c start /b ping -n 30 127.0.0.1` (or a test binary that spawns a child and prints its PID), cancels it, and asserts that the grandchild PID is gone (`OpenProcess` + `WaitForSingleObject(0)`).

## 3. Explorer context menu

| Option | Where it shows | Admin? | Signing? | Effort | Notes |
|---|---|---|---|---|---|
| 1. Classic verb `HKCU\Software\Classes\*\shell\MorphRing\command` (`"...\morphring-app.exe" --convert "%1"`) | Win10: top level. Win11: under **Show more options** (Shift+F10) | No | No | Low | Multi-select starts **one process per file**. Use `MultiSelectModel=Player` and collect the arguments in the app with `tauri-plugin-single-instance` 2.5.4. Add `Directory\shell` for folders. Write it with `windows-registry` 0.100 or `winreg` 0.56. |
| 2. Send To shortcut `%APPDATA%\Microsoft\Windows\SendTo\MorphRing.lnk` | Win10 and Win11 under Send to (Win11: in Show more options) | No | No | Low | Passes **all** selected files in one command line (up to the 32K limit). A good complement to option 1. |
| 3. `IExplorerCommand` COM DLL + sparse package (`desktop4:FileExplorerContextMenus`, `com:SurrogateServer`, `uap10:AllowExternalContent`) | Win11 top-level menu | Unsigned: **yes**, an `Add-AppxPackage -AllowUnsigned` registration for all users. Signed: no | Signed for normal distribution. Unsigned works only with the special `OID.2.25.311729368913984317654407730594956997722=1` publisher | High | What Filee does: a C DLL built with Zig, an unsigned sparse MSIX, and one UAC prompt from the Inno Setup installer (`src/Filee.Platform.Windows/ExplorerMenuPackage.cs`, `installer/Filee.iss:191-239`). In Rust it would be a `cdylib` using `windows` 0.62 `#[implement(IExplorerCommand)]` plus `DllGetClassObject`. Microsoft notes that menu-building methods must be fast, because they run in Explorer. |
| 4. Classic `IContextMenu` shell extension DLL | Show more options only | Yes (HKLM) or HKCU | No | High | No benefit over option 1 for us. |

**Recommendation:** ship options 1 and 2 from `morphring integrate --install` (with `--uninstall` removing both), per user and with no admin rights. Also do it from the NSIS installer through `installerHooks` (`NSIS_HOOK_POSTINSTALL` / `PREUNINSTALL`, using `WriteRegStr HKCU ...`), so a fresh install has the menu without a CLI step. Treat option 3 as a separate task once a signing certificate exists. Doing it unsigned means copying Filee's admin-prompt dance and its all-users install, which we do not want to own early. Keep the DLL a no-op launcher: in `Invoke`, collect the paths and `CreateProcessW` the app, so a crash cannot take Explorer down for long.

## 4. Engine install hints (Windows)

Checked on 2026-10-16 in `microsoft/winget-pkgs`, `ScoopInstaller/Main` and `Extras`, and the Chocolatey community feed:

| Engine | winget id (latest) | Scoop | Chocolatey | On PATH after install? |
|---|---|---|---|---|
| ImageMagick | `ImageMagick.ImageMagick` (7.1.2.33; also `.Q8`/`.Q16` variants) | `scoop install imagemagick` (Main, 7.1.2-34) | `imagemagick` (7.1.2.2500) | Yes, after a new shell |
| FFmpeg | `Gyan.FFmpeg` (9.0.2; `Gyan.FFmpeg.Essentials` is smaller). `BtbN.FFmpeg.GPL`/`.LGPL` also exist | `scoop install ffmpeg` (Main, 9.0.2) | `ffmpeg` (9.0.2) | Yes (winget adds a portable link) |
| LibreOffice | `TheDocumentFoundation.LibreOffice` (26.8.1.1) | `scoop install extras/libreoffice` (26.8.1) | `libreoffice-fresh` (26.2.6, lags behind) | **No.** We probe `C:\Program Files\LibreOffice\program\soffice.com` |
| Pandoc | `JohnMacFarlane.Pandoc` (3.12.1) | `scoop install pandoc` (Main, 3.12.1) | `pandoc` (3.12.1) | Yes |
| Ghostscript | **Not in winget-pkgs** under `ArtifexSoftware` (only `ArtifexSoftware.mutool` 1.23.0). Do not print a winget id that does not exist. | `scoop install ghostscript` (Main, 10.08.0) | `Ghostscript` (10.8.0) | Binary is `gswin64c.exe`, not `gs`; discovery must look for it |

**Recommendation:** the `error.engine_missing` detail on Windows says `winget install -e --id <id>` (winget ships with Windows 10 1809+ and Windows 11). For Ghostscript, say `scoop install ghostscript` or link to the Artifex download page. Keep the hint table in one place per OS, next to the Linux `pacman -S` hints. The Filee-style engine downloader with pinned SHA-256 stays in v0.4.

## 5. Ring window on Windows

The current `ring` window config (`transparent`, `decorations: false`, `alwaysOnTop`, `skipTaskbar`, `focus: false`, `shadow: false`) is the right base for Windows too. What we know:

- **Transparency** with WebView2 works when the window is `transparent: true` and the page background is `transparent`. Without `shadow: false`, Windows 10/11 can draw a 1 px border and shadow around an undecorated window. We already set it.
- **Topmost** can be lost silently, for example after a fullscreen app or a UAC prompt. Call `set_always_on_top(false)` and then `set_always_on_top(true)` on every show. This is the Filee fix (`RadialController.cs:301-305`), already noted in `ring-overlay-and-gesture.md`.
- **No focus steal:** with `focus: false`, `show()` can still activate the window on Windows. If it does, use `ShowWindow(hwnd, SW_SHOWNOACTIVATE)` and add `WS_EX_NOACTIVATE` through `window.hwnd()` with `windows` 0.62.
- **Click-through:** `set_ignore_cursor_events(true)` on a transparent window does not reliably pass clicks through on Windows. Tauri issue #11461 was closed as "not planned" (2025-07-22) with no fix. The design does not need it: the ring is hidden except during a gesture, so make the window exactly ring-sized and hide it instead of making it click-through.
- **Drag and drop:** WebView2 handles external file drops through Tauri's `dragDropEnabled` with positions, as on Linux. The low-level mouse hook for the gesture is covered in `ring-overlay-and-gesture.md` (`SetWindowsHookExW`, a callback that only pushes to a channel).
- **DPI:** position the ring with `PhysicalPosition` from the hook's screen coordinates. Mixing logical and physical units puts the ring off-centre on 125%/150% monitors with different scales.

Spike before v0.4: show and hide 100 times on a Windows CI runner with a screenshot (the runner has a desktop session), and check focus with `GetForegroundWindow`.

## 6. Installer and code signing

Tauri versions: `tauri` 2.12.4, `tauri-cli` 2.12.3, `tauri-bundler` 2.10.3 (2.x line; 3.0 is in alpha).

| Option | Builds on | Admin to install | Size cost | Notes |
|---|---|---|---|---|
| **NSIS `-setup.exe`** | Windows (and cross from Linux with `cargo-xwin`, less tested) | No (`installMode: currentUser`, default) | Small | `installerHooks` for registry and Send To. The Tauri updater supports it. |
| MSI (WiX 3) | Windows only (WiX 3.14 is preinstalled on the runner) | Usually yes (per machine) | Small | Needed only for GPO/Intune deployment. |
| WebView2: `downloadBootstrapper` (default) | | | +0 MB | Needs internet at install time. Windows 10 (April 2018+) and 11 already have WebView2. |
| WebView2: `offlineInstaller` / `fixedRuntime` | | | +127 MB / +180 MB | Only for offline corporate installs. |

Signing options (`bundle.windows` in `tauri.conf.json`):

| Option | Cost | Eligibility | Tauri config | SmartScreen |
|---|---|---|---|---|
| Unsigned | 0 | anyone | none | "Unknown publisher" warning on every release |
| OV certificate on an HSM/cloud token (DigiCert, Sectigo, SSL.com eSigner) | about $200-400 a year | individuals OK | `signCommand` (for example SSL.com `CodeSignTool`) | Reputation builds per certificate over time. EV no longer gives instant reputation (since 2024). |
| **Azure Artifact Signing** (formerly Trusted Signing) | Basic about $9.99 a month (5,000 signatures) | Organisations in US/CA/EU/UK; **individuals in US/CA only** (as of mid-2026) | `signCommand: "artifact-signing-cli -e <endpoint> -a <account> -c <profile> -d MorphRing %1"`, with `AZURE_CLIENT_ID/SECRET/TENANT_ID` set as CI secrets (`cargo install artifact-signing-cli`) | Microsoft-issued, short-lived certificates. Reputation still builds over time. |
| SignPath Foundation | free | OSI licence (MIT is fine), **public repo**, an existing release, a published code-signing policy, builds in GitHub Actions with origin verification | SignPath GitHub action after `tauri build` | Certificate is in SignPath Foundation's name |

Tauri signs the app `.exe` and the installer with `signtool` (`certificateThumbprint`, `digestAlgorithm`, `timestampUrl`) or with a custom `signCommand` for each file. `TAURI_WINDOWS_SIGNTOOL_PATH` overrides the signtool lookup (`crates/tauri-bundler/src/bundle/windows/sign.rs`).

**Recommendation:** NSIS, current user, `downloadBootstrapper`, unsigned and labelled "preview" for the first Windows release. Pick signing by the owner's situation: SignPath if the repo goes public, otherwise Artifact Signing if the owner qualifies, otherwise an OV cloud certificate. Turn on `tauri-plugin-updater` and the Win11 sparse-package menu only once builds are signed.

## 7. GitHub Actions on Windows

- `windows-latest` now maps to **Windows Server 2025 with VS 2026** (image `windows-2025-vs2026`, 20260925.250.1, OS build 26100). `windows-2022` is still available. Pin `windows-2025` in the release workflow so a `-latest` migration cannot change a release build.
- **Preinstalled** on the 2025 images: **ImageMagick 7.1.2-25** (so `magick` golden tests can run on Windows before they do on Ubuntu, whose apt package is IM6), Chocolatey 2.7.4, WiX 3.14.1, Rust 1.98.1 on the VS2026 image and 1.99.0 on the plain 2025 image (our `rust-toolchain.toml` still decides). **Not** preinstalled: ffmpeg, LibreOffice, pandoc, Ghostscript.
- Installing them, fastest first:
  - ffmpeg: `choco install ffmpeg -y --no-progress` (about 30 s), or `FedericoCarboni/setup-ffmpeg` (gyan.dev builds, cached).
  - pandoc: `choco install pandoc -y` (about 20 s).
  - Ghostscript: `choco install ghostscript -y`.
  - LibreOffice: `choco install libreoffice-fresh -y` takes several minutes and about 350 MB. Keep the soffice golden tests local, as D7 does on Linux, or put them in a separate, non-blocking nightly job.
  - winget is present on the 2025 image but needs `--accept-source-agreements --disable-interactivity`, and it is slower and flakier than choco in CI. Prefer choco there.
- Refresh PATH after choco (`refreshenv` does not carry into later steps). Add the bin directory with `echo "C:\ProgramData\chocolatey\bin" >> $env:GITHUB_PATH`, which choco shims already use.
- Use `shell: bash` (Git Bash is preinstalled) for steps shared with Linux. Set `CARGO_TERM_COLOR` and use `Swatinem/rust-cache@v2` as on Linux. A Windows Rust build is about 2x slower, so keep `timeout-minutes` at 25 for that job.
- App job on Windows: `cargo check --manifest-path app/src-tauri/Cargo.toml` needs no extra packages (WebView2 and MSVC are on the image). Add `bundle.targets` `"nsis"` per platform through a `tauri.windows.conf.json` overlay, because the current `targets: ["deb", "appimage"]` would otherwise apply.

Proposed `ci.yml` change (v0.2):

```yaml
      - if: runner.os == 'Windows'
        shell: pwsh
        run: |
          choco install ffmpeg pandoc -y --no-progress
          echo "C:\ProgramData\chocolatey\bin" >> $env:GITHUB_PATH
          magick -version
```

## 8. Risks

| # | Risk | Mitigation |
|---|---|---|
| 1 | Windows CI is red today and hidden by `continue-on-error`, so more Windows regressions pile up unseen | Fix the 8 tests in v0.2 and remove `continue-on-error` for Windows right after. |
| 2 | The Job Object assignment race lets an early grandchild escape | `CREATE_SUSPENDED` + assign + resume, or measure first. A sweep on drop catches the rest. |
| 3 | Classic verb is hidden under "Show more options" on Win11, so users think it is missing | Also add Send To, document Shift+right-click, and plan the signed sparse package. |
| 4 | An unsigned installer triggers SmartScreen and antivirus false positives (a Rust binary that spawns processes and writes HKCU shell keys looks suspicious) | "Preview" label, SHA-256 sums next to the release, sign as soon as possible, and submit false positives to Microsoft. |
| 5 | WebView2 topmost and focus quirks make the ring steal focus from Explorer, which cancels the drag | `SW_SHOWNOACTIVATE` + `WS_EX_NOACTIVATE`, spike first (section 5). |
| 6 | A winget id or package goes away (Ghostscript is already missing) | Hint ids live in one table with a test, and are re-checked when engine versions are bumped. |
| 7 | An `IExplorerCommand` DLL bug crashes or hangs Explorer | Keep the DLL tiny (launch the app only) and use the surrogate server (`dllhost`) so it does not run inside `explorer.exe`. |
| 8 | Signing eligibility: individuals outside US/CA cannot use Artifact Signing, and SignPath needs a public repo | Decide when v0.4 is planned. An OV cloud certificate is the fallback for everyone. |

## 9. Sources

- Current Windows CI failures: `gh run view` of the latest `CI` run on `main`, job `test (windows-latest)` (2026-10-16).
- Tauri Windows installer: https://v2.tauri.app/distribute/windows-installer/
- Tauri Windows signing: https://v2.tauri.app/distribute/sign/windows/
- Tauri bundler signing code: https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-bundler/src/bundle/windows/sign.rs
- Tauri click-through issue: https://github.com/tauri-apps/tauri/issues/11461
- Win11 context menu with IExplorerCommand and a sparse package (updated 2026-07-16): https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/integrate-packaged-app-with-file-explorer
- Sparse packages: https://learn.microsoft.com/en-us/windows/msix/packaging-tool/sparse-packages
- Unsigned MSIX and the OID publisher: https://learn.microsoft.com/en-us/windows/msix/package/unsigned-package
- Windows 11 context menu blog post: https://blogs.windows.com/windowsdeveloper/2021/07/19/extending-the-context-menu-and-share-dialog-in-windows-11/
- 7-Zip sparse-package shell extension example: https://github.com/2xRon/7zipExplorerExtension
- Job Objects in cargo: https://doc.rust-lang.org/stable/nightly-rustc/src/cargo/util/job.rs.html
- Job handle inheritance pitfall: https://devblogs.microsoft.com/oldnewthing/20131209-00/?p=2433
- Race-free job assignment (`PROC_THREAD_ATTRIBUTE_JOB_LIST`) example: https://docs.rs/crate/conpty-oxide/0.1.1/source/src/core/job.rs
- Crates (versions as of 2026-10-16): https://crates.io/crates/windows-sys , https://crates.io/crates/process-wrap , https://crates.io/crates/win32job , https://crates.io/crates/dunce , https://crates.io/crates/windows-registry , https://crates.io/crates/tauri-plugin-single-instance
- winget manifests: https://github.com/microsoft/winget-pkgs/tree/master/manifests (paths `i/ImageMagick/ImageMagick`, `g/Gyan/FFmpeg`, `t/TheDocumentFoundation/LibreOffice`, `j/JohnMacFarlane/Pandoc`)
- Scoop buckets: https://github.com/ScoopInstaller/Main/tree/master/bucket , https://github.com/ScoopInstaller/Extras/tree/master/bucket
- Chocolatey: https://community.chocolatey.org/packages/ffmpeg , https://community.chocolatey.org/packages/imagemagick , https://community.chocolatey.org/packages/libreoffice-fresh , https://community.chocolatey.org/packages/pandoc , https://community.chocolatey.org/packages/Ghostscript
- Runner images: https://github.com/actions/runner-images (README label table), https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-VS2026-Readme.md , https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md
- winget in GitHub Actions: https://scivision.dev/github-actions-winget/
- setup-ffmpeg action: https://github.com/FedericoCarboni/setup-ffmpeg
- Azure Artifact Signing pricing: https://azure.microsoft.com/en-us/pricing/details/artifact-signing/ ; Windows code signing options: https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options
- SignPath Foundation: https://signpath.org/ , https://signpath.org/terms , https://docs.signpath.io/origin-verification
- Filee reference (read-only): `src/Filee.Platform.Windows/ExplorerMenuPackage.cs`, `installer/Filee.iss`, `src/Filee.Engines/Infrastructure/ProcessRunner.cs:78`, `src/Filee.App/Services/RadialController.cs:301-305`

Not verified in this pass: the exact Artifact Signing country list (from third-party write-ups; confirm in Microsoft's FAQ), SignPath's current per-project limits, and the timing of the Job Object assignment race against soffice on a real machine.
