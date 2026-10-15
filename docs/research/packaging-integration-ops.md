# Packaging, Integration and Ops: Research

Status: R&D recommendation for **v0.1.0 (ship today)** and the releases after it.
Date checked: 2026-10-15. Crate and tool versions come from crates.io and the GitHub release pages on that date.

Filee paths below are relative to the Filee repository root. We read Filee as a reference. We do not copy it.

---

## 1. TL;DR: what v0.1.0 ships

| Area | v0.1.0 (today) | Later |
|---|---|---|
| Linux packages | Tarball of the `morphring` CLI, plus Tauri `.deb`, `.rpm` and `.AppImage` built by `tauri-action` on `ubuntu-22.04` | AUR `morphring-bin` (repackages the `.deb`), then Flathub |
| Windows | NSIS `-setup.exe` from `tauri-action` (unsigned, with a SmartScreen warning) | MSI, code signing, Win11 `IExplorerCommand` menu |
| macOS | Unsigned `.dmg` (aarch64 and x86_64), labelled "experimental" | Notarization, Finder Sync / Services |
| Release automation | One GitHub Actions workflow on `v*` tags using `tauri-apps/tauri-action@v1`, plus a `SHA256SUMS.txt` | Updater JSON and signed updates (`tauri-plugin-updater`) |
| Engines (ffmpeg, magick, soffice, pandoc, gs) | **Detect only.** Look them up on `PATH` with the `which` crate and show an "install with your package manager" hint | On-demand download with SHA-256 pins (Windows and macOS only) |
| File manager menu | **Dolphin service menu + Nautilus script + Thunar action**, written by `morphring integrate --install` | Nemo action, Windows registry verb, then IExplorerCommand |
| Watch folders | Not in v0.1.0 | `notify` 8.2 + `notify-debouncer-full` 0.7, with a size and mtime settle check |
| Update check | Not in v0.1.0 (or a passive "check for updates" link at most) | Opt-in GitHub `releases/latest` check, later the Tauri updater |

The rule: **v0.1.0 = convert files through the CLI and the ring UI on Linux, installable in one click from a GitHub Release.** Anything that needs signing certificates, a store review or a background daemon waits.

---

## 2. Packaging formats compared

| Format | Tool | Effort today | Pros | Cons / risks | v0.1.0? |
|---|---|---|---|---|---|
| `.deb` | `tauri build -b deb` | Very low | Native deps (`libwebkit2gtk-4.1-0`), small (~5 MB) | Debian/Ubuntu only | **Yes** |
| `.rpm` | `tauri build -b rpm` | Very low | Fedora/openSUSE | Not tested by us | **Yes** (CI build, untested) |
| AppImage | `tauri build -b appimage` (linuxdeploy) | Low | Runs on any distro; good for Arch users | Must be built on an **old** glibc (Ubuntu 22.04). Bundles WebKit, so 70 MB or more. Needs FUSE2 (present on the dev machine: `fuse2 2.9.9`). WebKitGTK on Wayland/NVIDIA has known rendering bugs (`WEBKIT_DISABLE_DMABUF_RENDERER=1` is the usual workaround) | **Yes** |
| AUR `-bin` | Hand-written PKGBUILD that unpacks the release `.deb` (official Tauri guide) | Low, but needs an AUR account and SSH key | The natural channel for the user (Arch) | Has to be updated on every release. Should be a follow-up commit, not a blocker | Right after the release |
| AUR `-git` | PKGBUILD that runs `tauri build -b deb` | Medium | Built from source | Needs Node, pnpm and Rust in makedepends; slow | Later |
| Flatpak | `flatpak-builder` manifest + `flatpak-cargo-generator` | High | Flathub reach, sandbox | **The sandbox breaks our model.** We shell out to host `ffmpeg`, `magick` and `soffice`, which needs `flatpak-spawn --host` (Flathub reviewers reject that) or bundling the engines (huge). A context menu that writes into `~/.local/share` also needs extra permissions | No (v0.3+) |
| Snap | snapcraft | Medium | Ubuntu | Same sandbox problem; not wanted on Arch | No |
| Windows NSIS `.exe` | `tauri build` (default on Windows) | Low (CI only) | Per-user install, no admin needed, WebView2 bootstrapper | Unsigned binaries trigger SmartScreen warnings | **Yes** (unsigned, "preview") |
| Windows MSI | WiX via `tauri build -b msi` | Low | Needed for corporate GPO deployment | Builds only on Windows; WiX version quirks | Later |
| macOS `.app` / `.dmg` | `tauri build -b dmg` | Low (CI) | Standard | **Unsigned and un-notarized apps are blocked by Gatekeeper**: users have to right-click > Open or run `xattr -dr com.apple.quarantine`. An Apple Developer ID costs $99/year | Experimental, or skip |
| CLI tarball | `cargo build --release` + `tar` (or cargo-dist) | Very low | Headless use; easy for scripts and for the context menu | No GUI | **Yes** |

Notes:
- Tauri v2 (latest crate `tauri` 2.12.4) supports `.deb`, `.rpm`, AppImage, `.app`, `.dmg`, MSI and NSIS from its own bundler. It documents AUR, Flatpak and Snap as distribution guides, not as bundler targets.
- **Correction to the project context:** `webkit2gtk-4.1 2.52.6`, `gtk3` and `libsoup3` *are* installed on the dev machine (`pacman -Q`). So local `tauri dev` and `tauri build -b deb` should work today.
- The dev desktop is **Hyprland** (Wayland). The only file manager installed is **Dolphin**, so Dolphin is the menu we must test by hand.

### Filee for comparison
- Windows: an Inno Setup installer (`installer/Filee.iss`, built by `build/build-installer.ps1`), plus a portable zip and a `SHA256SUMS.txt` (`.github/workflows/release.yml`, header comment, lines 1-10).
- Linux and macOS: **only portable archives with `install.sh` / `uninstall.sh`** (`installer/linux/`, `installer/macos/`), built in `.github/workflows/portable.yml`. Filee ships no `.deb`, no AppImage and no AUR package.
- Release notes come from the matching section of `CHANGELOG.md` plus GitHub's generated notes (`.github/workflows/release.yml`, the "Publish the GitHub release" step). This is worth copying later. For today a plain `CHANGELOG.md` section is enough.

---

## 3. Release automation

| Option | Version | What it does | Fit |
|---|---|---|---|
| `tauri-apps/tauri-action` | v1.0.0 (released 29 June) | Builds the Tauri app on each runner in a matrix and uploads the bundles to a draft or published GitHub Release. Can upload `latest.json` for the updater (`uploadUpdaterJson`; this was renamed from `includeUpdaterJson` in v1) | **Use for the GUI** |
| `cargo-dist` (dist) | 0.32.0 (2026-05-22) | Generates a CI workflow for plain Rust binaries: tarballs/zips, shell and PowerShell installers, Homebrew, checksums | Good for the **CLI**, but it does not understand Tauri bundles. Running two release generators adds friction on day one |
| Hand-written workflow | n/a | `cargo build` + `gh release upload` | Fine for the CLI tarball inside the same workflow |

**Recommendation:** use a single `.github/workflows/release.yml` triggered by `v*` tags:
1. Matrix `ubuntu-22.04`, `windows-latest`, `macos-latest` (aarch64, and `--target x86_64-apple-darwin` if time allows) running `tauri-action@v1` with `releaseDraft: true`.
2. A job on `ubuntu-22.04` that builds `morphring` (the CLI) with `cargo build --release --locked`, tars it as `morphring-cli-<ver>-x86_64-linux.tar.gz` and uploads it.
3. A final job that downloads every asset, writes `SHA256SUMS.txt` with `sha256sum` and uploads it. We publish the draft by hand after a smoke test.

Building on ubuntu-22.04 matters for AppImage glibc compatibility (Tauri AppImage docs). Revisit cargo-dist for the CLI once we add Homebrew and winget.

---

## 4. Engines: detection and on-demand download

### What Filee does
- Engines are pinned by URL and SHA-256 in `src/Filee.Engines/Infrastructure/EngineInstaller.cs`. The download is hashed incrementally while streaming (around lines 123-149) and **discarded on a mismatch** with a clear error. A marker file holding the installed hash (line 222) lets the installer tell "installed and current" from "stale" (lines 61 and 70).
- Engine versions live in `src/Filee.Engines/Infrastructure/EngineVersions.cs`. The Windows installer does not bundle the large engines; the app downloads them on first run (`.github/workflows/release.yml`, header comment).

### Recommendation
- **v0.1.0: detect only.** On Linux the package manager is the right source for ffmpeg, ImageMagick, LibreOffice, pandoc and ghostscript: it handles security updates and codecs, and downloading binaries ourselves would duplicate that. Use `which` 8.0.6 to find the binaries. Add `morphring doctor`, which prints what was found, the versions (`ffmpeg -version`, `magick -version`, `soffice --version`) and the install command for the distro (`pacman -S pandoc ghostscript`, `apt install ...`).
- Declare the engines as `optdepends` in the PKGBUILD and as `Recommends:` in the `.deb` (`bundle.linux.deb.depends` only covers hard dependencies, so this is a later tweak).
- GUI caveat: apps started from a desktop launcher do not inherit the shell `PATH`. Tauri recommends `fix-path-env-rs` for that. Also check the usual fixed locations (`/usr/bin`, `/opt/homebrew/bin`, `C:\Program Files\...`).
- **Later (Windows and macOS): on-demand download** that follows Filee's pattern:
  - A manifest compiled into the binary: `{id, version, url, sha256, size, archive_kind}`. Never trust a hash fetched from the same server as the file.
  - Stream with `reqwest` 0.13.5 (or `ureq` 3.4.2 for the sync CLI) into a temp file in the destination folder, and hash on the fly with `sha2` 0.11.1.
  - Compare in constant time, then `rename` atomically into `<data_dir>/engines/<id>-<version>/` and write a marker with the hash.
  - HTTPS only, with a size cap and a timeout. Show the license and source URL before downloading (GPL ffmpeg builds come with obligations).

---

## 5. File manager context menus

| Target | Mechanism | Path | Effort | Notes |
|---|---|---|---|---|
| **Dolphin (KDE)** | Service menu `.desktop`, `Type=Service`, `Actions=` | `~/.local/share/kio/servicemenus/morphring.desktop` | Very low | `Exec=morphring-app --convert %F` (`%U`/`%F` passes several files). Make the file **executable** (KDE requires that for local service menus). Use `MimeType=all/allfiles;`. This is the menu we can test on the dev box |
| **Nautilus (GNOME)** | Script | `~/.local/share/nautilus/scripts/MorphRing` (executable) | Very low | Appears under the "Scripts" submenu, not at the top level. Files arrive as arguments (and in `NAUTILUS_SCRIPT_SELECTED_FILE_PATHS`) |
| Nautilus top-level item | nautilus-python extension (`Nautilus.MenuProvider`) | `~/.local/share/nautilus-python/extensions/` | Medium | Requires the `python-nautilus` package; the API changed with GTK4/Nautilus 43+. Later |
| **Thunar (XFCE)** | Custom action in `~/.config/Thunar/uca.xml` | XML merge | Low to medium | Must merge into existing XML without clobbering it (Filee does this, `src/Filee.Platform.Linux/LinuxDesktopIntegration.cs`, lines 131-170). Simpler alternative: a "Send To" `.desktop` in `~/.local/share/Thunar/sendto/` |
| Nemo (Cinnamon) | `.nemo_action` | `~/.local/share/nemo/actions/` | Very low | Same as Filee (line 127) |
| Windows, classic menu | Registry verb `HKCU\Software\Classes\*\shell\MorphRing\command` | Registry | Low | Only under "Show more options" on Windows 11. Multi-select starts one process per file, so we need a single-instance lock (`tauri-plugin-single-instance`) to collect them |
| Windows 11, top-level menu | `IExplorerCommand` COM DLL + sparse MSIX package with identity | DLL + manifest | **High** | Filee builds a C DLL with Zig plus an unsigned sparse MSIX, which needs one UAC prompt to register (`src/Filee.Platform.Windows/ExplorerMenuRegistration.cs`, lines 1-3; `ExplorerMenuPackage.cs`). In Rust this means `windows` crate COM implementations plus signing. Later |
| macOS | Services menu (`NSServices` in Info.plist) or a Finder Sync / Action extension | Bundle | Medium to high | Filee uses Services (`installer/macos/*/ServicesMenu.strings`, `src/Filee.Platform.MacOS/MacOSIntegrationFiles.cs`). A Finder extension needs Xcode, an appex and signing. Later |

**Filee's Linux approach** (`src/Filee.Platform.Linux/LinuxDesktopIntegration.cs`, lines 102-129) writes all of the Linux menus at once from a settings toggle. It writes atomically (temp file + rename, lines 205-225) and quotes paths carefully (`DesktopArgument`/`ShellArgument`, lines 172-196). We should keep those two habits: atomic writes, and escaping of `Exec=` values per the Desktop Entry spec (`%` becomes `%%`, and quote the path).

**v0.1.0 recommendation:** add a `morphring integrate --install | --uninstall` CLI subcommand that writes the Dolphin, Nautilus-script and Thunar-sendto files (three small templates, no XML merging), plus a matching toggle in the GUI if time allows. Each entry calls `morphring-app --convert %F`, which opens the ring menu at the cursor for those files. Wayland caveat: an app cannot read the global cursor position under Hyprland/GNOME Wayland, so the ring has to open centered or at a remembered spot. That needs its own ticket.

---

## 6. Watch folders (post-v0.1.0)

| Crate | Version | Role |
|---|---|---|
| `notify` | 8.2.0 stable (9.0.0-rc.5 exists; avoid the RC) | inotify / ReadDirectoryChangesW / FSEvents backends |
| `notify-debouncer-full` | 0.7.0 stable (0.8.0-rc.2) | Merges event bursts and tracks renames |

**Detecting copy completion.** There is no portable "file closed" event.
- Linux: inotify has `IN_CLOSE_WRITE`, which notify exposes as `EventKind::Access(AccessKind::Close(AccessMode::Write))`. That is a strong signal, but it fires on every close: a downloader may open and close the file repeatedly, and network shares may never send it.
- Browsers write `*.part` / `*.crdownload` and then rename, so ignore temp extensions and react to the rename's target.
- Windows has no close event. Filee tries to open the file with `FileShare.None`, which fails while a writer holds it.
- macOS Finder gives a file a 1984-01-24 creation date while it is still copying it (Filee `src/Filee.Core/Watching/FolderWatcher.cs`, lines 29-30).

Filee's algorithm (`FolderWatcher.cs`, lines 1-5, 27, 145, 295-311): poll every second, and treat a file as complete when **size and mtime stay unchanged for a settle time** (2 s on Windows, 5 s elsewhere), the exclusive open succeeds (Windows), and Finder does not report a copy in progress. Hidden files, `~$` files and the output and originals folders are ignored so converted files cannot loop back in. A key of `path|len|mtime` stops the same file from being converted twice.

**Our plan:** debounced notify events feed a candidate map, the same settle check runs, plus `IN_CLOSE_WRITE` as a fast path on Linux. Never watch the output folder recursively. Fall back to `PollWatcher` for NFS/SMB.

---

## 7. Update checks

| Option | Pros | Cons |
|---|---|---|
| GET `https://api.github.com/repos/suraniharsh/morphring/releases/latest`, compare `tag_name` with `semver` 1.0.28, show a notice | Tiny, no signing needed. This is what Filee does (`src/Filee.App/Services/UpdateService.cs`, lines 151-187: a 404 before the first release and any offline error are only logged) | Anonymous rate limit of 60 requests/hour per IP (fine at one request per day). The user still installs by hand |
| `tauri-plugin-updater` 2.14.0 + `latest.json` from tauri-action | Real in-app update with a minisign signature | Needs a signing key pair kept in CI secrets. On Linux it only updates **AppImage** installs; `.deb`/AUR installs must be left to the package manager |

**Recommendation:** nothing in v0.1.0. For v0.2, add an opt-in, once-a-day `releases/latest` check that only shows a "vX available" link, and turn it off entirely when the app was installed from AUR or `.deb` (detect via a build-time flag, as Filee does with its `FileeRelease` assembly metadata, `UpdateService.cs`, lines 130-133). Adopt the Tauri updater for AppImage, Windows and macOS when we start signing.

---

## 8. Risks

1. **AppImage + WebKitGTK on Wayland/NVIDIA:** blank windows and DMABUF errors are common. Document `WEBKIT_DISABLE_DMABUF_RENDERER=1` and recommend the `.deb`/AUR builds, which use the system WebKit.
2. **Unsigned Windows and macOS builds** scare users (SmartScreen, Gatekeeper). Label them "preview" in the release notes and keep Linux as the supported platform for v0.1.0.
3. **CI time on release day:** a first `tauri-action` matrix often fails on missing system packages. On ubuntu-22.04 the job needs `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`. Budget one iteration, and fall back to a Linux-only release if Windows or macOS fail.
4. **Context menu path quoting:** paths with spaces or `%` break `Exec=` lines. Unit-test the escaping, as Filee does.
5. **Global cursor position on Wayland** is unavailable, so the ring-at-cursor UX needs a fallback.
6. **Engine licensing**, if we ever download ffmpeg builds (GPL vs LGPL). Detect-only avoids the problem for now.
7. **AUR maintenance:** someone must bump `pkgver` and `sha256sums` on every release. Automate later with a workflow that pushes to the AUR over SSH.

---

## 9. Sources

- Tauri v2 distribute overview: https://v2.tauri.app/distribute/
- Tauri AppImage guide (glibc, size, linuxdeploy): https://v2.tauri.app/distribute/appimage/
- Tauri AUR guide (PKGBUILD examples, deps): https://v2.tauri.app/distribute/aur/
- Tauri Debian / RPM / Windows installer / DMG: https://v2.tauri.app/distribute/debian/ , https://v2.tauri.app/distribute/rpm/ , https://v2.tauri.app/distribute/windows-installer/ , https://v2.tauri.app/distribute/dmg/
- Tauri Flatpak guide: https://v2.tauri.app/distribute/flatpak/
- Tauri updater plugin: https://v2.tauri.app/plugin/updater/ ; crate https://crates.io/crates/tauri-plugin-updater (2.14.0)
- tauri-action releases (v1.0.0, `uploadUpdaterJson`): https://github.com/tauri-apps/tauri-action/releases
- cargo-dist: https://crates.io/crates/cargo-dist (0.32.0), docs https://opensource.axo.dev/cargo-dist/
- notify: https://crates.io/crates/notify (8.2.0) ; notify-debouncer-full: https://crates.io/crates/notify-debouncer-full (0.7.0)
- sha2 0.11.1, reqwest 0.13.5, ureq 3.4.2, semver 1.0.28, which 8.0.6: https://crates.io
- KDE Dolphin service menus: https://develop.kde.org/docs/apps/dolphin/service-menus/
- Nautilus scripts: https://help.gnome.org/users/nautilus/stable/ (and the `nautilus-python` project at https://gitlab.gnome.org/GNOME/nautilus-python)
- Thunar custom actions: https://docs.xfce.org/xfce/thunar/custom-actions
- Windows 11 context menu (IExplorerCommand + sparse package): https://learn.microsoft.com/windows/apps/desktop/modernize/integrate-packaged-app-with-file-explorer
- GitHub REST API, latest release: https://docs.github.com/rest/releases/releases#get-the-latest-release
- Desktop Entry spec (Exec escaping): https://specifications.freedesktop.org/desktop-entry-spec/latest/
- Filee (reference): `installer/`, `build/`, `.github/workflows/release.yml`, `src/Filee.Platform.Linux/LinuxDesktopIntegration.cs`, `src/Filee.Platform.Windows/ExplorerMenuRegistration.cs`, `src/Filee.Core/Watching/FolderWatcher.cs`, `src/Filee.Engines/Infrastructure/EngineInstaller.cs`, `src/Filee.App/Services/UpdateService.cs`
