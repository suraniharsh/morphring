# Research: ring overlay window and global modifier+drag gesture

Status: R&D recommendation, written 2026-10-15 for v0.1.0 planning.
Scope: how to show the ring (donut) menu at the cursor, and how to notice a "modifier + drag" anywhere on the desktop, on Linux (X11 and Wayland), Windows and macOS, from Rust.

---

## 1. What Filee actually does (and the trick that matters)

Read this first. It changes which problem we have to solve.

**The ring is a drop target, not a click-through overlay.** Filee never reads the drag data from the file manager and never makes the window click-through. The flow is:

1. A global input hook (SharpHook 8.0.0, which wraps libuiohook, `Directory.Packages.props:25`) sends raw mouse and keyboard events to a pure state machine, `src/Filee.App/Services/Triggers/GestureDetector.cs`.
2. The state machine arms on "button down with these exact modifiers" (`GestureDetector.cs:54-82`). It fires `DragStarted` once the pointer moves past a threshold (`:105-114`). It also handles the modifier being pressed *after* the drag began, which is common: start dragging, then press Ctrl (`:120-133`). Hook callbacks must return fast, because Windows silently drops a slow low-level hook (`TriggerService.cs:1-3`).
3. On `DragStarted`, `RadialController.ShowForDrag` places an **invisible, non-activated, topmost, transparent** window centred on the cursor (`src/Filee.App/Services/RadialController.cs:56-64`, `:278-309`). The window is declared in `src/Filee.App/Views/RadialWindow.axaml:13-21`: `Topmost`, `ShowActivated="False"`, `ShowInTaskbar="False"`, `DragDrop.AllowDrop="True"`. It must not take focus, or the file manager's drag breaks (comment at `RadialWindow.axaml:4`).
4. The file manager's own drag (OLE, XDND or Cocoa) enters that window. `DragEnter` hands over the file list and the donut animates in (`RadialWindow.axaml.cs:43-54`). `DragOver` hit-tests slices and always answers COPY, never MOVE, so Explorer does not delete the source files (`:56-67`). `Drop` runs the preset.
5. If the drag carried no files (text, a rubber-band selection), nothing ever shows. The window closes 250 ms after button release (`RadialController.cs:81-91`).
6. Windows can quietly push a topmost window below others, so Filee re-asserts topmost on every show (`RadialController.cs:301-305`, `WindowsPlatformServices.cs:28-31`).

Consequences for us:

- **We do not need "XDND drag source detection".** Every OS already delivers the dragged file list to any window under the pointer that accepts drops. We only need (a) a trigger that says "show the target now", and (b) a window that appears under the cursor without stealing focus.
- We do not need click-through. A click-through window could not receive the drop.
- **Filee does not support the gesture on Wayland at all.** `src/Filee.Platform.Linux/LinuxPlatformServices.cs:24-30` and `X11WindowSystem.cs:12-14` turn it off whenever `WAYLAND_DISPLAY` is set. Users get the drop zone (`src/Filee.App/Views/Pages/HomePage.axaml.cs:1`) and file-manager actions instead (see also `README.md:79`). Our user runs Hyprland on Wayland, so copying Filee gives them **no gesture**. This is the part where R&D has to beat Filee.
- On macOS, Filee gates gestures on `AXIsProcessTrusted()` and polls every 2 s until the user grants permission (`MacOSPlatformServices.cs:18-23`, `TriggerService.cs:184-201`).
- On X11, the "is this a file manager?" scope check finds the window under the cursor by walking `XTranslateCoordinates` children and reading `_NET_WM_PID` (`X11WindowSystem.cs:16-44`). Wayland has no equivalent, so scope filtering ("only over the file manager") cannot carry over there.

## 2. User machine facts (checked 2026-10-15)

| Fact | Value | Why it matters |
|---|---|---|
| Session | `XDG_CURRENT_DESKTOP=Hyprland`, `XDG_SESSION_TYPE=wayland` (uwsm) | Primary target is wlroots-like layer-shell, not GNOME |
| Hyprland | 0.56.2, xdg-desktop-portal-hyprland 1.4.1 | Has layer-shell, `hyprctl cursorpos`, `bindn` non-consuming binds, GlobalShortcuts portal |
| Also installed | KWin / Plasma 6.7.5, xdg-desktop-portal-kde, Dolphin 26.08.1 | KDE works the same way (layer-shell + portal). Dolphin service menus are an easy fallback trigger |
| Groups | user is in `input` | Can read `/dev/input/event*` **without root**, so evdev/libinput listening works today |
| gtk-layer-shell | 0.10.1 (system lib) | Needed by the `gtk-layer-shell` crate |
| webkit2gtk-4.1 | **2.52.6 installed** | The project context says it is missing. It is present, so Tauri can build now |
| GPU | Intel Iris Xe (TigerLake) | The NVIDIA + DMABUF WebKit transparency bugs are unlikely here |
| Xwayland | 24.1.13 | X11 code paths only see X11 clients, which is useless for global input on this session |

## 3. Crate versions (crates.io, 2026-10-15)

| Crate | Version | Updated | Note |
|---|---|---|---|
| tauri | 2.12.4 | 2026-10-14 | Uses `gtk ^0.18` (GTK3) on Linux |
| gtk-layer-shell | 0.8.2 | 2024-12-09 | GTK3 bindings, `gtk ^0.18`, **matches Tauri**. Low churn; gtk3-rs itself is in maintenance mode |
| gtk4-layer-shell | 0.8.1 | 2026-08-10 | GTK4 only, so it cannot attach to a Tauri window |
| tauri-plugin-widgets | 0.6.2 | 2026-10-04 | Working example of putting a Tauri window on layer-shell (398 downloads, so read it for the pattern rather than depending on it) |
| smithay-client-toolkit | 0.21.1 | 2026-07-23 | Pure Rust Wayland client: layer-shell plus `wl_data_device` DnD |
| evdev | 0.13.2 | 2025-09-15 | Raw `/dev/input` reader |
| input (libinput bindings) | 0.10.0 | 2026-04-05 | Gives normalised pointer motion, including touchpads |
| rdev | 0.5.3 | **2023-06-26** | X11 only on Linux ("will not work in Wayland"). Stale |
| device_query | 4.0.1 | 2025-07-21 | Polling, X11 only on Linux |
| ashpd | 0.13.13 | 2026-07-17 | xdg-desktop-portal client (GlobalShortcuts, InputCapture) |
| global-hotkey / tauri-plugin-global-shortcut | 0.8.0 / 2.4.1 | 2026 | Key chords only. X11/Win/mac; no Wayland |
| x11rb | 0.14.0 | 2026-07-16 | XInput2 raw events, XDND atoms |
| windows | 0.62.2 | 2025-10-06 | `SetWindowsHookExW` and friends |
| objc2-core-graphics | 0.3.2 | 2025-10-04 | `CGEventTapCreate` |
| winit | 0.30.13 | 2026-09-04 | No layer-shell. No file-drop on Wayland |
| egui / eframe | 0.37.0 | 2026-10-15 | On top of winit |
| iced | 0.14.0 | 2025-12-07 | On top of winit. Third-party `iced_layershell` exists |
| slint | 1.18.1 | 2026-09-21 | winit backend by default |
| tiny-skia / softbuffer | 0.12.0 / 0.4.8 | 2026 | DIY 2D rendering |
| hyprland (IPC) | 0.3.13 | 2025-09-24 | Hyprland socket client. Raw socket is 10 lines anyway |

## 4. Overlay window: toolkit options

Requirements, in priority order: (1) appears at the cursor, (2) does not take focus, (3) receives an external file drop **with a position**, (4) transparent, round, animated, (5) above other windows, (6) works on Wayland/Hyprland, (7) cross-platform, (8) low effort, since we already chose Tauri.

| Option | Wayland placement at cursor | Accepts external file drop with position | Transparent / no-focus / on top | Cross-platform | Effort for us | Verdict |
|---|---|---|---|---|---|---|
| **Tauri v2 window + gtk-layer-shell (Linux)** | Yes, with the reparent trick (below): layer surface on `Overlay` layer, anchored top-left, margins = cursor. A plain Tauri toplevel **cannot** position itself on Wayland, and `always_on_top` is ignored there | Yes. Tauri `DragDropEvent::{Enter, Over, Drop}` carries paths and a position, from WebKitGTK (Linux), OLE (Win) and Cocoa (mac) | Yes: `transparent`, `decorations(false)`, `focused(false)`, `skip_taskbar`, `always_on_top`. On mac transparency needs `macOSPrivateApi` | Yes. Layer-shell code is Linux-only, ~60 lines | **Lowest.** One UI stack, ring drawn in SVG/Canvas | **Pick for v0.1.0** |
| GTK4 + gtk4-layer-shell, separate process | Yes, native | Yes (`GtkDropTarget`). There is an unresolved Oct 2025 report of GTK4 drops not arriving under Hyprland | Yes | Linux only | Medium. Second UI stack | Fallback if WebKit misbehaves |
| smithay-client-toolkit + tiny-skia, separate process | Yes, full control | Yes. Raw `wl_data_device` offers with surface coordinates; we must do MIME/URI parsing | Yes | Wayland only; X11/Win/mac need separate code | High | Long-term candidate for a tiny, instant overlay |
| egui / eframe 0.37 | **No layer-shell** (winit) | **No file-drop on Wayland** in winit 0.30; hover events carry no position | Transparent yes; on-top only on X11/Win/mac | Yes | Medium | Reject for the overlay |
| iced 0.14 (+ iced_layershell) | Only with the third-party crate | Unclear on Wayland (winit path has no DnD) | Yes | Partial | Medium-high | Reject for now |
| Slint 1.18 | No built-in layer-shell | No external file drop from the OS today | Yes | Yes | Medium | Reject for the overlay |
| winit + tiny-skia | No | No on Wayland | Yes | Yes | High | Reject |

How the Tauri layer-shell trick works, as done in `tauri-plugin-widgets` (`src/linux/mod.rs`):

1. `win.hide()`. A GTK window must not be mapped when `init_layer_shell()` runs.
2. Take `win.gtk_window()` and `win.default_vbox()`, then remove the vbox (which holds the WebKit view) from the old window.
3. Create `gtk::ApplicationWindow::new(&app)`, call `set_app_paintable(true)`, add the vbox, then `init_layer_shell()`.
4. Our settings: `set_layer(Layer::Overlay)`, `set_keyboard_interactivity(false)` (no focus steal), `set_exclusive_zone(-1)`, anchor `Top` and `Left`, `set_margin(Top/Left, cursor - radius)`, and `set_monitor(..)` for the output under the cursor.
5. Moving the ring later only needs new margins. Hide and show map and unmap the surface. Create it once at startup, as Filee does (`RadialController.cs:116-164`).

Click-through, if we ever want it for the progress ring: Tauri has `set_ignore_cursor_events(true)`. On layer-shell we can instead shrink the GDK input shape to the ring's circle, so drops outside the ring fall through to the window below.

## 5. Global gesture detection: platform options

| Platform | Mechanism | Sees modifier+drag in other apps? | Permission / cost | Notes |
|---|---|---|---|---|
| **X11** | XInput2 raw events (`XI_RawButtonPress`, `XI_RawMotion`, `XI_RawKeyPress`) via `x11rb` 0.14 on the root window, plus `XQueryPointer` for position | Yes | None | Better than `rdev` (stale, uses XRecord). Bonus: `XGetSelectionOwner(XdndSelection) != None` tells you an XDND drag is really in progress, a cheap "is this a file drag?" check Filee does not use |
| **Wayland, any compositor** | Read `/dev/input/event*` with `evdev` 0.13 or `input` 0.10 (libinput). Track modifier keys, `BTN_LEFT` and motion distance | Yes | User must be in the `input` group (ours is), or use a udev `uaccess` rule. Note: `input` group membership is keylogger-level access | No absolute cursor position. Deltas are pre-acceleration device units, so the threshold is approximate. Touchpads send ABS, not REL, which is why libinput (`input` crate) is the better choice. Handle hotplug. Never `EVIOCGRAB` |
| Wayland: xdg-desktop-portal **GlobalShortcuts** (`ashpd`) | Compositor binds a shortcut and notifies us | **No.** Key chords only, no mouse-button+modifier, no drag | Portal dialog. KDE: yes. Hyprland: yes via XDPH, but it ignores the app's suggested key and shows no UI; the user binds it in config / `hyprctl globalshortcuts`. GNOME: since GNOME 48 | Good for a **"ring for the current selection" hotkey**, not for drag |
| Wayland: **InputCapture** portal (`ashpd`) | Captures input when the pointer hits a barrier | No. Built for Synergy-style apps; takes the input away rather than watching it | Portal | Not suitable |
| **Hyprland specific** | `bindn = CTRL ALT, mouse:272, exec, morphring arm` (`n` = non-consuming, so the file manager still gets the click), plus a release bind to disarm. `hyprctl cursorpos` / socket `j/cursorpos` gives the global cursor position | Press yes; drag distance no (pair with the DnD-enter trick below) | None, user config line | No special permission. Needs a spike to confirm mouse binds with `n`/`r` flags behave during a drag |
| KDE specific | KWin script reading `workspace.cursorPos`, over D-Bus | Position only | None | Clunky. Use evdev for the trigger |
| GNOME (Mutter) | Nothing: no layer-shell, no cursor position API | Only via evdev | `input` group | Overlay placement is the blocker on GNOME. Fall back to drop zone / Nautilus script |
| **Windows** | `SetWindowsHookExW(WH_MOUSE_LL / WH_KEYBOARD_LL)` from `windows` 0.62 on a dedicated thread with its own `GetMessageW` loop | Yes | None | The callback must return in well under `LowLevelHooksTimeout` (~1 s on Win10/11) or Windows silently removes the hook, so only push to a channel. UIPI: no events while an elevated window has focus. Re-assert `HWND_TOPMOST` on each show (Filee bug fix, `RadialController.cs:301-305`) |
| **macOS** | `CGEventTapCreate(kCGSessionEventTap, kCGHeadInsertEventTap, kCGEventTapOptionListenOnly, mask)` via `objc2-core-graphics`, on a run-loop thread | Yes | A listen-only tap needs **Input Monitoring** (`CGPreflightListenEventAccess` / `CGRequestListenEventAccess`). An active tap needs Accessibility; Filee/libuiohook checks `AXIsProcessTrusted`. Re-enable the tap on `kCGEventTapDisabledByTimeout` | Overlay as a non-activating `NSPanel` at a high window level. Filee uses scale 1 on mac because coordinates are already points (`RadialController.cs:282`) |

### The Wayland trick: no cursor position needed

Wayland gives an unprivileged app neither the global cursor position nor the window under it. But a **drop target learns the pointer position for free**: `wl_data_device.enter` carries surface-local x/y.

So, on a layer-shell compositor:

1. evdev/libinput (or a Hyprland `bindn`) sees modifier + button down, then motion past the threshold.
2. We map a **transparent, full-output layer surface** on the `Overlay` layer with `keyboard_interactivity = none`. The file manager keeps its implicit pointer grab and its drag continues.
3. The drag enters our surface. WebKitGTK/Tauri gives us `DragDropEvent::Enter { paths, position }`. We draw the ring centred at `position` and **shrink the input shape to the ring's circle**, so releasing elsewhere drops onto the real window below.
4. If there are no files in the drag, unmap on button release (evdev) after 250 ms, matching Filee.

Which output gets the surface: on Hyprland, ask `j/cursorpos` or `j/monitors` (focused monitor). Elsewhere, use the compositor's default (the focused output).

Shortcut when we have a cursor position (Hyprland IPC or X11): skip the full-screen surface and map a small surface at the cursor with margins, exactly like Filee.

## 6. Recommendation

### v0.1.0 (today, Hyprland user)

Ship what certainly works, and put the gesture behind an "experimental" flag. Do not let it block the release.

1. **Must ship, all platforms:** the Tauri main window with a **drop zone**. A drop opens the ring in click mode at the drop point, as in Filee `HomePage.axaml.cs:1`. Add a `morphring ring <files…>` CLI entry that opens the ring for given files, and a **Dolphin service menu** (`.desktop` in `~/.local/share/kio/servicemenus/`) plus a generic `.desktop` "Open with". This is exactly Filee's Wayland feature set and needs zero special input access.
2. **Ring overlay = a pre-created hidden Tauri window**, ring drawn in SVG/Canvas, with the gtk-layer-shell reparent trick on Wayland. Settings: `Layer::Overlay`, no keyboard interactivity. On X11/Win/mac, a plain transparent `always_on_top` window with `focused(false)`.
3. **Experimental gesture on Linux/Wayland:** a `morphring-input` thread using the `input` crate (libinput), enabled only if `/dev/input` is readable. Default gesture Ctrl+Alt+left-drag. Use the DnD-enter trick for placement and Hyprland IPC `cursorpos` when available. Port `GestureDetector.cs` almost verbatim as a pure, unit-tested Rust state machine (`crates/core` or a `gesture` crate), including the late-modifier case.
4. Leave out of 0.1.0: Windows hooks, macOS taps, X11 XInput2, the GlobalShortcuts "selection" hotkey, and scope filtering ("only over the file manager").

### Long term

- Make one `InputBackend` trait that feeds `GestureDetector`, with backends `libinput` (Wayland), `x11rb-xi2` (X11), `win-ll-hook` and `mac-eventtap`. The `rdev` crate is a tempting single answer but is stale and X11-only. A libuiohook-style split of roughly 300 lines per OS is worth owning.
- Add an `ashpd` GlobalShortcuts hotkey for "ring for current selection" (KDE, Hyprland, GNOME 48+). The selection itself comes from the file manager (Dolphin D-Bus, Nautilus scripts), since Linux has no shared selection API (Filee `LinuxPlatformServices.cs:25-27`).
- If WebKit show-latency or transparency bites, move the overlay to a tiny separate process: `smithay-client-toolkit` + `tiny-skia` on Wayland, with native windows elsewhere. The Tauri app talks to it over a local socket. The protocol is small: show at (x,y), slices, highlight, progress, hide.
- GNOME: no layer-shell, so treat it as drop zone + hotkey only, unless a GNOME Shell extension is written later (out of scope).

## 7. Risks and spikes (in order, timebox ~30 min each)

| # | Risk | Spike / mitigation |
|---|---|---|
| 1 | WebKitGTK inside a reparented layer-shell window may not receive DnD, or Tauri's drag-drop handler may break after the vbox move | Spike first: a Tauri window, reparent, drag from Dolphin, log `DragDropEvent`. Fallback: GTK4 + gtk4-layer-shell overlay process |
| 2 | Hyprland may not route DnD to layer surfaces as expected (there is an open, unanswered GTK4-drop-on-Hyprland report, Oct 2025) | Same spike on Hyprland 0.56.2 and on KWin 6.7.5 |
| 3 | Show latency: mapping a WebKit surface mid-drag may take more than 100 ms | Pre-create, keep the page loaded, toggle only map/unmap. Measure |
| 4 | Security and UX of the `input` group: it can read every keystroke | Off by default. Explain clearly in settings. Only parse modifier keys and button/motion events; never log key codes |
| 5 | A full-output transparent surface swallows a drop if the input shape is not shrunk in time | Shrink the input region on the first `Enter`. Unmap on button-up |
| 6 | gtk3-rs bindings (`gtk 0.18`, `gtk-layer-shell 0.8`) are in maintenance mode. Tauri depends on them until it moves to GTK4 | Isolate in one `linux/layer.rs` module |
| 7 | Windows LL hook removed for slow callbacks; macOS tap disabled by timeout | Callback only pushes to a channel. Watchdog re-installs the hook or re-enables the tap |
| 8 | Ctrl/Alt+drag conflicts with file-manager copy/link semantics, and Hyprland's SUPER+LMB moves windows | Default to Ctrl+Alt. Make it configurable. Always answer COPY (Filee `RadialWindow.axaml.cs:64`) |

## 8. Sources

- Filee source (MIT): `src/Filee.App/Services/Triggers/{GestureDetector,TriggerService}.cs`, `src/Filee.App/Services/RadialController.cs`, `src/Filee.App/Views/RadialWindow.axaml(.cs)`, `src/Filee.Platform.Linux/{LinuxPlatformServices,X11WindowSystem}.cs`, `src/Filee.Platform.MacOS/MacOSPlatformServices.cs`, `README.md:79` — https://github.com/KnifeLemon/Filee
- tauri-plugin-widgets Linux layer-shell code: https://docs.rs/crate/tauri-plugin-widgets/0.5.0/source/src/linux/mod.rs
- gtk-layer-shell crate: https://docs.rs/gtk-layer-shell/ ; gtk4-layer-shell: https://docs.rs/gtk4-layer-shell/
- rdev ("listen uses X11 APIs, will not work in Wayland"): https://docs.rs/crate/rdev/latest , https://github.com/Narsil/rdev
- hotkey-listener (evdev-based example): https://docs.rs/hotkey-listener
- Tauri Linux graphics issues (WebKitGTK, `WEBKIT_DISABLE_DMABUF_RENDERER`): https://v2.tauri.app/develop/debug/linux-graphics/
- Hyprland global shortcuts portal: https://wiki.hypr.land/configuring/core/binds/globals/
- hyprctl `cursorpos`: https://wiki.hypr.land/0.55.0/Configuring/Advanced-and-Cool/Using-hyprctl/
- Critique of Wayland GlobalShortcuts behaviour across compositors: https://dec05eba.com/2024/03/29/wayland-global-hotkeys-shortcut-is-mostly-useless/
- GNOME 48 global shortcuts for apps: https://release.gnome.org/48/developers/ ; earlier request: https://gitlab.gnome.org/GNOME/xdg-desktop-portal-gnome/-/issues/85
- KDE cursor position via KWin script: https://discuss.kde.org/t/is-there-a-better-way-to-obtain-the-mouse-cursor-location/20559
- GTK4 drop not arriving on Hyprland (unresolved): https://discourse.gnome.org/t/drag-and-drop-not-getting-event-files-when-using-hyprland-works-on-wayland-x11-any-clue/32192
- niri GlobalShortcuts discussion: https://github.com/YaLTeR/niri/pull/1165
- Crate versions: crates.io API, queried 2026-10-15.

Not verified from a primary source in this pass, and to be confirmed in the spikes: Hyprland `bindn` mouse-button behaviour during a drag; DnD delivery to layer surfaces on Hyprland 0.56; the exact Windows `LowLevelHooksTimeout` default; and the macOS Input Monitoring vs Accessibility split for listen-only taps. The last is based on Apple's documented `CGPreflightListenEventAccess` API (macOS 10.15+).
