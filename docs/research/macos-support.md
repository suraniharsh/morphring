# macOS Support: Research

Status: R&D recommendation for v0.2 (CI and CLI) and v0.4 (app and Finder integration). See PLAN.md section 9.
Date checked: 2026-10-16. Nothing here was run on a Mac. Every claim comes from current docs, the Tauri crate sources in `~/.cargo/registry`, or the GitHub runner-image manifests. Each "verify" item in section 9 must be confirmed on a CI macOS runner or a real Mac before we rely on it.

Versions checked:

| Item | Version | Where |
|---|---|---|
| `tauri` | 2.11.6 in our lock; 2.12.4 on docs.rs | `~/.cargo/registry`, docs.rs |
| `tauri-runtime-wry` | 2.11.4 | registry source |
| `tauri-plugin-single-instance` | 2.4.5 | registry source |
| `tauri-utils` / `tauri-build` | 2.9.3 / 2.6.3 | registry source |
| GitHub `macos-latest` | **macOS 26, arm64**, image 20260907.0351.1, Xcode 26.6, Rust 1.98.1, Homebrew 6.0.22, Node 24.20 | actions/runner-images |
| Homebrew `ffmpeg` | 9.0.2 (bottles arm64_tahoe, arm64_sequoia, arm64_golden_gate) | formulae.brew.sh |
| Homebrew `imagemagick` | 7.1.2-34, ships `magick` | formulae.brew.sh |

---

## 1. TL;DR recommendation

1. **CLI first (v0.2).** Make `macos-latest` CI a real, blocking job. Install ffmpeg and imagemagick with `brew` so the golden tests run on macOS. Ship an arm64 CLI tarball. The CLI needs no signing to be useful: a binary fetched with `curl` or built by `cargo install` gets no quarantine flag.
2. **App (v0.4): ad-hoc signed `.dmg`, not notarized.** Set `bundle.macOS.signingIdentity = "-"`. Document the Sequoia/Tahoe "Open Anyway" flow and the `xattr -dr com.apple.quarantine` command. Move to Developer ID signing and notarization ($99/year) once the app has users. Tauri's CI flow already supports it, so the switch only adds secrets.
3. **Ring window:** set `app.macOSPrivateApi: true` and add the `macos-private-api` feature to `tauri` in `app/src-tauri/Cargo.toml`. Without them the ring is opaque on macOS, and release builds give no warning. Test transparency in a **bundled** `.app`, because of open bug tauri#13415.
4. **Tray app:** call `ActivationPolicy::Accessory` (no Dock icon) only when the user starts the app in tray mode. Handle `RunEvent::Reopen`. Use a template tray icon.
5. **Files from Finder:** handle **`RunEvent::Opened { urls }`** (Apple Events), *and* keep the existing `single-instance` argv path. Add `bundle.fileAssociations` (role `Viewer`, rank `Alternate`) so "Open With > MorphRing" works.
6. **Finder context menu:** a **Quick Action (`.workflow`)** written by `morphring integrate --install` into `~/Library/Services`. The Quick Action runs `open -a MorphRing "$@"`. Skip Finder Sync extensions until we sign. They need an appex target, a sandbox and a real signature, and Tauri cannot build them.
7. **Homebrew:** publish the **CLI as a formula in our own tap** (`suraniharsh/homebrew-tap`). A cask for the app is only worth doing after notarization. Homebrew 5.0 deprecated `--no-quarantine` and disables homebrew-cask casks that fail Gatekeeper from September 2026.

---

## 2. Options table

| Concern | Option | Works unsigned? | Effort | Verdict |
|---|---|---|---|---|
| **Transparent ring** | `macOSPrivateApi: true` + `tauri` feature `macos-private-api` | Yes (bars the Mac App Store only) | Very low | **Do it** |
| | Opaque rounded window, no transparency | Yes | None | Fallback if #13415 bites |
| | `window-vibrancy` crate (NSVisualEffectView blur) | Yes | Low | Later polish |
| **Tray / menu bar** | `TrayIconBuilder` (already enabled by our `tray-icon` feature) + `icon_as_template(true)` | Yes | Low | **Do it** |
| | `set_activation_policy(Accessory)` / `set_dock_visibility(false)` at runtime | Yes | Low | **Do it** in tray mode |
| | `LSUIElement` in a merged `Info.plist` | Yes | Low | No: hides the Dock icon even when the main window is open |
| **Single instance** | `tauri-plugin-single-instance` (Unix socket on macOS) | Yes | Already in use | **Keep**, for CLI-style launches of the inner binary |
| | LaunchServices (one instance per bundle) + `RunEvent::Opened` | Yes | Low | **Add**: this is how Finder, `open` and Dock drops deliver files |
| **Finder integration** | Quick Action `.workflow` in `~/Library/Services` | Yes | Low-medium | **Recommended** |
| | `NSServices` in `Info.plist` + an ObjC service provider | Yes | Medium (objc2 glue) | No: a Quick Action gets the same menu with no native code |
| | Finder Sync extension (`.appex`) | **No** (needs a signature, a sandbox, App Groups) | High (Swift target outside Tauri) | No, revisit after Developer ID |
| | `fileAssociations` / "Open With" | Yes | Very low | **Do it** |
| | Drag onto the Dock icon | Yes (comes with `Opened`) | None | Free |
| **Bundle** | `.dmg` via `tauri build --bundles app,dmg` | Yes | Low | **Do it** (arm64; x86_64 optional on `macos-15-intel`) |
| | `universal-apple-darwin` | Yes | Low (+2 targets, doubles size) | Later, only if Intel users ask |
| **Signing** | None | Blocked on Apple Silicon ("damaged") | — | No |
| | Ad-hoc `signingIdentity: "-"` | Runs after "Open Anyway" or `xattr` | Very low | **v0.4 default** |
| | Developer ID + notarization (App Store Connect API key) | Opens with no warning | $99/year + secrets | When we have users |
| **Homebrew** | Formula in our own tap for the CLI | Yes (binaries built by brew, or bottles from our tap) | Low | **v0.2** |
| | Cask in our own tap for the app | Gatekeeper still prompts; `--no-quarantine` is deprecated | Low | After notarization |
| | homebrew-cask (official) | **No** (unsigned casks disabled since 2026-09) | Medium | After notarization + notability |

---

## 3. Transparent windows

- Config: `app.macOSPrivateApi: true` in `tauri.conf.json` (config key `macos_private_api`, `tauri-utils` config.rs:3072).
- Cargo: `tauri = { version = "2", features = ["tray-icon", "macos-private-api"] }`. `tauri-build` checks that the Cargo features match the config, and the `tauri` CLI rewrites them. We build the app with plain `cargo` in CI, so add the feature by hand.
- The trap: `tauri-runtime-wry` 2.11.4 (lib.rs:880-894) only calls `.transparent()` on macOS when the feature is on. The "not enabled" warning prints **only in debug builds**, so a release build silently gives an opaque window. This may explain part of tauri#13415 ("transparent in dev, white after `tauri build --bundles dmg`", open since 2025-05, Tauri 2.5.1, macOS 15.4.1, still unresolved after a 2025-11 report).
- The CSS must keep `html, body { background: transparent }`. `ring.html` already does this for Linux.
- Using private APIs bars the **Mac App Store**. That does not matter for dmg distribution.
- The ring is `alwaysOnTop` and `skipTaskbar`. `skipTaskbar` has no effect on macOS. To keep the ring off the Dock and out of Cmd-Tab, use the Accessory activation policy.
- macOS, unlike Hyprland, **can** read the global cursor position. A cursor-placed ring is feasible here (`AppHandle::cursor_position`). Tie this to the "cursor-placed ring" v0.4 item.

## 4. Tray and menu bar

- The existing `TrayIconBuilder` code works. On macOS, use a monochrome PNG and `.icon_as_template(true)` so it follows the light/dark menu bar. Set `.show_menu_on_left_click(true)`: macOS users expect a left click to open the menu.
- `App::set_activation_policy(ActivationPolicy::Accessory)` (before run) or `AppHandle::set_activation_policy` / `set_dock_visibility(false)` (runtime; tauri app.rs:634-663). Switch back to `Regular` when the main window opens. Otherwise that window cannot become key, and it gets no app menu.
- Handle `RunEvent::Reopen { has_visible_windows }` (macOS only): show the main window when the user clicks the Dock icon.
- On macOS, closing the last window does not quit the app. Match that: call `ExitRequested::prevent_exit` while the tray exists. Quit is Cmd-Q or the tray menu.
- Accessory apps have no menu bar, so Cmd-C/V in the webview breaks unless there is an app menu. Keep the default menu on `Regular`.

## 5. Single instance and receiving files

There are two ways a file reaches the app on macOS. We need both.

1. **Apple Events** (Finder "Open With", Dock drop, `open -a MorphRing file`, Quick Action using `open`). LaunchServices keeps one instance per bundle and sends `kAEOpenDocuments` to it. Tauri turns this into `RunEvent::Opened { urls: Vec<Url> }` (app.rs:259-266, cfg macOS/iOS/Android). **argv stays empty**, so today's argv code alone misses every Finder launch. Handle it in `App::run(|app, event| ...)`: convert the `file://` URLs to paths and call the same "show ring with these files" function the single-instance callback uses. Events can arrive **before** `setup` finishes on a cold start, so queue them until the ring window exists.
2. **Direct exec** (`/Applications/MorphRing.app/Contents/MacOS/morphring-app --ring a.png`, or a future `morphring ring` CLI shim). This bypasses LaunchServices, so a second process starts. `tauri-plugin-single-instance` 2.4.5 handles it on macOS through a Unix socket (`platform_impl/macos.rs`): the second process forwards its args and cwd, then exits 0. The plugin must be the first one registered. It already is.

`bundle.fileAssociations` writes `CFBundleDocumentTypes` (tauri-utils `file_associations_plist`). List the image, audio, video and doc extensions with `role: "Viewer"`, `rank: "Alternate"` so we never take over default handlers.

## 6. Finder integration

**Recommended: a Quick Action.** A Quick Action is an Automator "service" workflow bundle (`MorphRing.workflow/Contents/document.wflow` + `Info.plist` with `NSServices`) in `~/Library/Services`. It shows up on a Finder right-click under **Quick Actions** and in the Services menu. It needs no signing and no app extension, and `morphring integrate --install` can write it, mirroring our Dolphin/Nautilus/Thunar installers.

- Workflow: input "files or folders" in "Finder", one "Run Shell Script" action with "pass input: as arguments", body `open -a MorphRing "$@"`. Using `open` keeps a single instance and delivers the files through `RunEvent::Opened`. Do not rely on `PATH` inside the script: Automator gives it a minimal PATH, so use absolute paths only.
- Template it from a real `.wflow` saved once by Automator on a Mac, and commit it as a resource. Do not generate the XML from scratch.
- Registration risk: reports (Jamf community, Sonoma era) say a copied workflow sometimes appears only after `/System/Library/CoreServices/pbs -update` or a re-login. The user may also have to enable it in **System Settings > General > Login Items & Extensions > Finder** (the "Extensions" pane). `integrate --install` should run `pbs -update` and print that hint.
- A file written locally by our binary carries no quarantine flag, so Gatekeeper does not touch it.

**Rejected for now:**
- **Finder Sync extension:** gives a real top-level context-menu item and badges. But it needs an `.appex` built with Xcode/Swift, App Sandbox, a Developer ID (or development) signature, and the user must enable it in Extensions. Tauri has no appex support, so we would need a separate Xcode project embedded in `Contents/PlugIns`. Revisit after notarization.
- **Hand-written `NSServices` provider:** the same menu placement as a Quick Action, but it needs Objective-C delegate glue (`objc2`) and an `unsafe` exception. Not worth it.

## 7. Bundling, signing and notarization

- Build: `cargo tauri build --bundles app,dmg --target aarch64-apple-darwin` on `macos-latest`, or `tauri-apps/tauri-action@v1`, which already drives our release. The DMG layout options (`bundle.macOS.dmg.windowSize`, `appPosition`, `applicationFolderPosition`, `background`) **are not applied on CI** (tauri#1731), so expect a plain DMG window.
- **GUI apps do not inherit the shell `PATH`.** A Finder-launched app sees `/usr/bin:/bin:/usr/sbin:/sbin`, so `which ffmpeg` fails. Our engine `locate()` already probes `/opt/homebrew/bin` and `/usr/local/bin`. Add `/Applications/LibreOffice.app/Contents/MacOS/soffice`. Alternatively, use `fix-path-env-rs`, which Tauri recommends but which runs the login shell at startup. Probing fixed folders is simpler and deterministic.
- **Apple Silicon requires a signature on every native binary.** An unsigned arm64 app downloaded in a browser shows "is damaged and can't be opened". Tauri's `signingIdentity: "-"` gives an ad-hoc signature, which fixes this.
- **Sequoia (15.0+) and Tahoe (26):** the Control-click > Open bypass is gone. For a quarantined, un-notarized app, the user must launch once, then go to **System Settings > Privacy & Security > Open Anyway** and enter an admin password. The button disappears after about an hour. The alternative is `xattr -dr com.apple.quarantine /Applications/MorphRing.app`. The README must document both.
- **Notarization:** needs an Apple Developer Program membership ($99/year) and a "Developer ID Application" certificate. Tauri reads `APPLE_CERTIFICATE` (base64 .p12), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` and `KEYCHAIN_PASSWORD`, plus either `APPLE_API_ISSUER`/`APPLE_API_KEY`/`APPLE_API_KEY_PATH` (preferred, an App Store Connect API key) or `APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID`. Tauri runs notarytool and stapling itself. Ad-hoc signatures cannot be notarized.
- The CLI: binaries fetched with `curl`, `cargo install` or `brew` get no quarantine flag and run. A `.tar.gz` downloaded in Safari does get quarantined, so the README should tell users to run `xattr -d com.apple.quarantine morphring` or to use brew. `codesign -s - morphring` in the release job is cheap insurance. The Rust linker already ad-hoc signs arm64 binaries.

## 8. GitHub Actions on macOS

- `macos-latest` = **macOS 26 arm64** (image 20260907.0351.1). `macos-15` arm64 is still available, `macos-14` is deprecated, and Intel needs `macos-15-intel` / `macos-26-intel`. Pin `macos-26` in release jobs so a label flip cannot change the toolchain mid-cycle. `macos-latest` is fine for CI.
- Preinstalled: Xcode 26.6, Rust 1.98.1, Node 24, Homebrew 6.0.22. **Not** preinstalled: ffmpeg, imagemagick, ghostscript, pandoc, LibreOffice.
- Suggested step:

```yaml
- name: Install engines (macOS)
  if: runner.os == 'macOS'
  env:
    HOMEBREW_NO_AUTO_UPDATE: "1"
    HOMEBREW_NO_INSTALL_CLEANUP: "1"
    HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK: "1"
  run: |
    brew install ffmpeg imagemagick
    ffmpeg -hide_banner -version | head -1
    magick -version | head -1
```

- Both formulas ship `arm64_tahoe` bottles (ffmpeg 9.0.2 with 11 runtime deps, imagemagick 7.1.2-34 with `magick`). Expect **2 to 4 minutes**. If that hurts, cache `$(brew --cellar)` keyed on `brew --version` plus the formula versions, but brew caches break often, so start uncached. Do not `brew install --cask libreoffice` in CI: it is close to 1 GB. soffice golden tests keep skipping, as on Linux (D7).
- macOS has a case-insensitive APFS, BSD `stat`/`sed`, and `/var` → `/private/var` symlinks in `tempfile` paths. Canonicalize before comparing paths in tests.
- App job: `cargo check --manifest-path app/src-tauri/Cargo.toml --locked` needs no system packages on macOS (WKWebView is in the OS).

## 9. Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| Ring opaque in bundled builds (tauri#13415, open) | Medium | Feature + config set together; test the `.dmg` build in CI with a screenshot step; opaque rounded fallback |
| `RunEvent::Opened` before the ring window exists (cold start from Finder) | High | Queue paths in app state; drain after `setup` |
| Quick Action not showing until `pbs -update` / re-login / Extensions toggle | Medium | `integrate --install` runs `pbs -update` and prints the toggle path; verify on macOS 26 |
| Unsigned UX ("Open Anyway" plus admin password) scares users | High | Label macOS as "preview"; recommend the brew CLI; budget for Developer ID |
| Homebrew policy tightens (tap trust in brew 6.0, cask disable) | Medium | Ship the CLI as a source/bottle formula, not a cask, until notarized |
| GUI PATH lacks `/opt/homebrew/bin` | Certain | Fixed-folder probing in `locate()` (already planned) + LibreOffice.app path |
| `macos-latest` moves to a new major (did so mid-2026) | Certain, yearly | Pin `macos-26` in release.yml |
| Accessory policy breaks keyboard shortcuts in the main window | Medium | Switch to `Regular` while the main window is shown |
| Private API use | Low | Bars only the Mac App Store, which is not a goal |

Verify on a Mac or a CI runner before v0.4: (a) the ring is transparent in a bundled `.app`; (b) `open -a MorphRing a.png` with the app running and with it closed fires `Opened` exactly once; (c) the Quick Action appears after install on macOS 26; (d) the ad-hoc signed DMG opens through "Open Anyway".

## 10. Sources

- Tauri v2 window customization (transparency, macOSPrivateApi): https://v2.tauri.app/learn/window-customization/
- Config reference `macOSPrivateApi`: https://v2.tauri.app/reference/config/#macosprivateapi
- Transparent-in-dev, white-in-dmg bug: https://github.com/tauri-apps/tauri/issues/13415
- window-vibrancy: https://github.com/tauri-apps/window-vibrancy
- `RunEvent` (docs.rs renders the Linux view, so `Opened`/`Reopen` are hidden; read the source): https://docs.rs/tauri/latest/tauri/enum.RunEvent.html, local `tauri-2.11.6/src/app.rs` lines 259-282 and 634-663
- Single-instance plugin: https://v2.tauri.app/plugin/single-instance/
- macOS code signing: https://v2.tauri.app/distribute/sign/macos/
- DMG distribution: https://v2.tauri.app/distribute/dmg/ (CI layout caveat: https://github.com/tauri-apps/tauri/issues/1731)
- Gatekeeper in Sequoia: https://eclecticlight.co/2024/08/10/gatekeeper-and-notarization-in-sequoia/
- Sequoia Control-click removal: https://www.osnews.com/story/141055/bug-or-intentional-macos-15-1-completely-removes-ability-to-launch-unsigned-applications/
- "Open Anyway" walkthrough: https://www.macworld.com/article/2457844/what-to-do-when-you-cant-open-an-app-you-just-installed-in-macos-sequoia.html
- Homebrew 5.0.0 (`--no-quarantine` deprecated, unsigned casks disabled September 2026): https://brew.sh/2025/11/12/homebrew-5.0.0/
- Example of a cask hit by the policy: https://goodfirstissue.org/hluk/CopyQ/issues/3498
- Quick Actions as Services in `~/Library/Services`: https://scriptingosx.com/2018/11/mojave-quick-action-to-package-apps/, https://support.apple.com/guide/automator/aut4bb6b2b4f/mac, https://eclecticlight.co/2019/02/15/quick-actions-5-third-party-solution/
- Programmatic Quick Action install caveats: https://community.jamf.com/general-discussions-2/enabling-automator-quick-action-workflow-via-jamf-pro-32669
- Runner images and labels: https://github.com/actions/runner-images, manifest https://github.com/actions/runner-images/blob/main/images/macos/macos-26-arm64-Readme.md
- Homebrew formulas: https://formulae.brew.sh/formula/ffmpeg, https://formulae.brew.sh/formula/imagemagick
