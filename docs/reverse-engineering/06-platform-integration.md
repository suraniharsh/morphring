# 06 - Platform integration (hooks, context menus, installers, CI)

Reverse-engineering notes for the OS-specific parts of Filee (v1.10.3, `Directory.Build.props:18`).
All paths are relative to the Filee repository root. Line numbers refer to the snapshot we read.

Scope of this document:

- `src/Filee.Platform.Linux/**`, `src/Filee.Platform.MacOS/**`, `src/Filee.Platform.Windows/**`
- `src/Filee.ExplorerMenu/**` (native Windows 11 shell extension, plain C)
- `installer/**`, `build/**`, `.github/workflows/**`, `Directory.*.props`, `global.json`
- The parts of `src/Filee.App` that consume these: `Services/Triggers/*`, `Services/SingleInstance.cs`,
  `Services/CommandLine.cs`, `Services/SystemIntegrationService.cs`, `Services/UninstallCleanup.cs`, `Program.cs`.
  They are needed to understand how hooks and context menus actually reach the donut.

---

## 1. Big picture

Filee is a tray app. It gets files from four entry points:

| Entry point | How files arrive | Needs global hook? |
|---|---|---|
| **Drag gesture** (hold Ctrl/Alt and drag files) | Global mouse hook sees "modifier + button down + moved past threshold" over a file manager. Filee pops the donut window under the cursor. The user drops the files on the donut, which is a normal OS drop target. **The hook never sees file paths.** | Yes |
| **Hold / key chord** (Ctrl+Alt+Space) | Hook sees the chord. Filee asks the file manager for its current selection (Windows: Shell COM; macOS: Finder via JXA). **Not supported on Linux.** | Yes |
| **Context menu / Send To / Services** | File manager launches `Filee --convert <files...>`. A second process forwards args to the running instance over a named pipe. | No |
| **Drop window / watch folders** | Normal in-app UI. | No |

The OS layer is one interface, `IPlatformServices` (`src/Filee.Core/Platform/IPlatformServices.cs:4-63`), with
three implementations plus `NullPlatformServices` (no-op fallback, lines 87-115). The app picks one at start
(`src/Filee.App/Services/AppHost.cs:38-40`, `Services/UninstallCleanup.cs:14-16`).

### 1.1 The `IPlatformServices` contract

| Member | Meaning | Linux | Windows | macOS |
|---|---|---|---|---|
| `SupportsGlobalPointerGestures` | Can we run the hook at all? | `X11WindowSystem.IsAvailable` (X11 and not Wayland) | `true` (default) | `AXIsProcessTrusted()` |
| `SupportsSelectionShortcut` | Can we read the file manager selection? | `false` | `true` | `true` |
| `IsFileManagerAt(x,y)` | Is the window under the point a file manager / desktop? | process name in a set | window class or `explorer` | process is `Finder` |
| `ProcessNameAt(x,y)` | Process name owning the window under the point | X11 tree walk + `_NET_WM_PID` | `WindowFromPoint` + `GetAncestor(GA_ROOT)` | `CGWindowListCopyWindowInfo` |
| `RunningAppNames()` | For the "excluded apps" picker | `_NET_CLIENT_LIST` | processes with a main window | on-screen windows at layer 0 |
| `GetFileManagerSelection()` | Selected files | throws `NotSupportedException` | `Shell.Application` COM | `osascript -l JavaScript` |
| `SystemDragThreshold` | Pixels before a drag counts | **8** (hard-coded) | `max(2, GetSystemMetrics(SM_CXDRAG))` | **4** |
| `PrefersReducedMotion` | Disable animations | `gsettings get org.gnome.desktop.interface enable-animations` == `false` | `SPI_GETCLIENTAREAANIMATION` | `NSWorkspace.accessibilityDisplayShouldReduceMotion` |
| `SetStartWithSystem(on, exe)` | Autostart | `~/.config/autostart/filee.desktop` | `HKCU\...\Run` value | `~/Library/LaunchAgents/com.filee.app.plist` |
| `SetContextMenu(on, exe, label)` | File manager entry | Thunar, Nautilus, Nemo, Dolphin files | HKCU verb + Send To `.lnk` (+ Win11 title file) | `pbs` NSServicesStatus |
| `Get/SetModernContextMenu*` | Windows 11 top-level menu | Unsupported | sparse MSIX + COM DLL | Unsupported |
| `RevealInFileManager(path)` | "Show in folder" | DBus `FileManager1.ShowItems`, fallback `xdg-open <dir>` | `explorer.exe /select,"path"` | `open -R path` |
| `RaiseTopmost(hwnd)` | Re-raise always-on-top donut without focusing | EWMH `_NET_WM_STATE_ABOVE` + `_NET_RESTACK_WINDOW` | `SetWindowPos(HWND_TOPMOST, NOACTIVATE...)` | `setLevel:3`, collection behavior, `orderFrontRegardless` |

Linux-only extra: `SetThunarShortcut(accelerator)` and `ToThunarAccelerator(gesture)` (section 3.4).

Integration is only written by **release builds**: `SystemIntegrationService.RegistrationEnabled =
UpdateService.IsReleaseBuild` (`Services/SystemIntegrationService.cs:17`). That flag is an assembly attribute that
is only present when built with `-p:FileeRelease=true` (`src/Filee.App/Filee.App.csproj:61-64`). Running from
source never touches the user's desktop. **We should copy this idea** (for example a cargo feature or an env var).

---

## 2. Global input hook and gesture detection

### 2.1 Library

Filee uses **SharpHook 8.0.0** (`Directory.Packages.props:25`), a .NET wrapper around **libuiohook**.
`TriggerService` (`src/Filee.App/Services/Triggers/TriggerService.cs`) owns an `EventLoopGlobalHook` and
subscribes to `MousePressed`, `MouseReleased`, `MouseDragged`, `MouseMoved`, `KeyPressed` (lines 92-104).
`RunAsync()` runs the hook on its own thread. Failure (for example no Accessibility permission on macOS) is caught,
stored in `Error`, and the app keeps working without gestures (lines 105-124).

What libuiohook does per OS:

- **Windows:** `SetWindowsHookEx(WH_MOUSE_LL / WH_KEYBOARD_LL)`. The callback must return fast: Windows silently
  removes a low-level hook whose callback takes about 1 second (comment at `TriggerService.cs:1-3`). So the hook
  thread only updates a state machine and posts events to the UI thread.
- **macOS:** `CGEventTap`. Needs the Accessibility permission (and Input Monitoring for keys).
- **Linux:** the **X11 RECORD extension** (XRecord) on an X display. There is no other backend.

### 2.2 Why Linux needs X11

Everything on Linux goes through Xlib, and Wayland intentionally offers no equivalent:

1. **Seeing global input.** libuiohook uses XRecord. Under Wayland the compositor never sends input for other
   apps' windows to a client. Through XWayland, XRecord only sees events going to other XWayland windows, never to
   native Wayland apps (such as Nautilus or Dolphin on a modern desktop).
2. **"Which window is under the cursor?"** `X11WindowSystem.ProcessNameAt` (`src/Filee.Platform.Linux/X11WindowSystem.cs:16-44`)
   walks the window tree with `XTranslateCoordinates`. Wayland has no global coordinates and no API to query
   another client's windows.
3. **Keeping the donut on top.** `RaiseTopmost` sends EWMH client messages (lines 85-103). Wayland windows cannot
   place themselves; only layer-shell (wlroots, KDE) or the compositor can do this.

So Filee decides at start:

```
IsWaylandSession = XDG_SESSION_TYPE == "wayland" (case-insensitive) || WAYLAND_DISPLAY is non-empty
X11 available    = OS is Linux && !IsWaylandSession && DISPLAY is non-empty
```

(`LinuxPlatformServices.cs:20-22`, `X11WindowSystem.cs:12-14`). Note: **on a Wayland session gestures are
switched off even if XWayland is running**. The user message is in `LinuxPlatformServices.cs:28-30`:
"The cursor-following gesture requires an X11 desktop session. File actions and the drop zone remain available on
Wayland." CI forces X11 by running tests under `xvfb-run -a env -u WAYLAND_DISPLAY XDG_SESSION_TYPE=x11`
(`.github/workflows/portable.yml:73,132`).

Also, on Linux the hold and key-chord gestures are dropped entirely, because there is no selection API:
`TriggerService.Apply` keeps only `Drag` gestures when `SupportsSelectionShortcut` is false (`TriggerService.cs:63`).
The key chord is instead turned into a **Thunar keyboard shortcut** for the context action (section 3.4).

### 2.3 X11 details (`X11WindowSystem.cs`)

Library: `libX11.so.6` via P/Invoke. If it is missing (`DllNotFoundException` / `EntryPointNotFoundException`),
`OpenDisplay` returns 0 and everything degrades to "no window" (lines 105-118). Every call opens and closes its own
`Display*`; there is no persistent connection.

`ProcessNameAt(x, y)` (lines 16-44):

1. `XOpenDisplay(NULL)`; `XGrabServer` so windows cannot disappear between hit-test and property reads.
2. `window = root`. Loop up to 32 levels:
   - `name = ReadProcessName(window) ?? name` (keep the deepest non-null name);
   - `XTranslateCoordinates(display, root, window, x, y, ..., &child)`; stop when it fails or `child == 0`;
   - `window = child`.
3. `finally`: `XUngrabServer`, `XCloseDisplay`.

`ReadProcessName(window)` (lines 120-164):

1. Read `_NET_WM_PID` (atom looked up with `only_if_exists=1`), format 32, one item. The value is a C `long`.
   Turn the PID into a process name via `/proc` (.NET `Process.ProcessName` reads `/proc/<pid>/comm`, which is
   **truncated to 15 characters**).
2. Fallback: `WM_CLASS` (format 8, up to 256 longs, clamp to 1024 bytes), split on NUL, return the **first** string
   (the instance name, for example `nautilus` or `org.gnome.Nautilus`).

`ClientProcessNames()` (lines 47-83): read `_NET_CLIENT_LIST` from the root window (up to 4096 items, format 32;
each item is a C long, so read `sizeof(long)` bytes per window), then `ReadProcessName` for each.

`RaiseTopmost(window)` (lines 85-103): sends two `ClientMessage` events (type 33, format 32) to the root window
with mask `SubstructureRedirectMask | SubstructureNotifyMask` (`(1<<20)|(1<<19)`):

- `_NET_WM_STATE` with data `[1 (_NET_WM_STATE_ADD), _NET_WM_STATE_ABOVE, 0, 1 (source = application)]`
- `_NET_RESTACK_WINDOW` with data `[1, 0, 0, 0]`

then `XFlush`. The `XClientMessageEvent` struct is laid out by hand with `Size = 192` (24 longs on 64-bit).

File manager set (`LinuxPlatformServices.cs:15-18`), compared case-insensitively:
`thunar, nautilus, org.gnome.nautilus, nemo, dolphin, caja, pcmanfm, pcmanfm-qt, xfdesktop`.
(Caja and PCManFM are detected but get no context menu entry.)

### 2.4 macOS hit testing (`src/Filee.Platform.MacOS/NativeMethods.cs`)

`ProcessNameAt` calls `CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly=1, 0)` (front to back), and for
each window dictionary:

- skip unless the point is inside `kCGWindowBounds` (top-left origin, in points; matches Avalonia coordinates);
- skip if `kCGWindowLayer > 0` (Dock, menu bar; the Dock has a transparent full-screen window);
- skip if `kCGWindowAlpha <= 0`;
- PID from `kCGWindowOwnerPID`, process name from it; skip `WindowServer`; return the first match.

`IsFileManagerAt` = name equals `Finder`. `WindowOwnerNames` lists layer-0 windows.
`IsFinderFrontmost` checks `NSWorkspace.sharedWorkspace.frontmostApplication.bundleIdentifier == "com.apple.finder"`.
`RaiseTopmost` (lines 118-133) sets collection behavior (`canJoinAllSpaces | fullScreenAuxiliary`, and on macOS 13+
`canJoinAllApplications` (1<<18)), `setLevel: 3` (floating), then `orderFrontRegardless`, so the donut shows even
over a full-screen Finder Space.

Accessibility: `AXIsProcessTrusted()` gates the hook. `RequestAccessibility()` calls
`AXIsProcessTrustedWithOptions({AXTrustedCheckOptionPrompt: true})`, which shows macOS's own alert. `TriggerService`
polls every **2 seconds** until permission appears, then starts the hook without a restart (`TriggerService.cs:184-201`).
The prompt is shown 1.5 s after start, only if not launched with `--background` and a drag gesture is enabled
(`App.axaml.cs:84-92`).

### 2.5 Windows hit testing (`src/Filee.Platform.Windows/WindowsPlatformServices.cs`)

`RootWindowAt` = `GetAncestor(WindowFromPoint(pt), GA_ROOT)`. File manager if the class is one of `CabinetWClass`,
`ExploreWClass`, `Progman`, `WorkerW` (desktop) **or** the owning process is `explorer` (lines 20-42).

Selection (lines 54-106): `GetForegroundWindow()`, then `Shell.Application.Windows()`; for each window whose `HWND`
equals the foreground window, read `Document.SelectedItems()` and keep paths that are existing files. With Explorer
tabs several entries share one HWND, so the first one with a non-empty selection wins. Must run on an STA thread.

### 2.6 Gesture data model (`src/Filee.Core/Settings/TriggerGesture.cs`)

```text
ModifierKeys   (flags) None=0, Ctrl=1, Alt=2, Shift=4, Meta=8   // left/right are the same
TriggerMouseButton     None, Left, Right, Middle, X1, X2
TriggerKind            Drag | Hold | KeyChord
TriggerScope           FileManager | Anywhere

TriggerGesture {
  id: 8 hex chars (random)
  enabled: true
  kind: Drag
  modifiers: Ctrl
  button: Left
  key: Option<String>        // libuiohook key name without "Vc", e.g. "Space"; required for KeyChord
  drag_threshold: 0          // 0 = use platform SystemDragThreshold
  hold_milliseconds: 450
  scope: FileManager
}
```

Defaults (lines 75-83): `Drag` with `Ctrl` (macOS: `Alt`, because Ctrl+click is right-click there and Option is copy
in Finder) + `Left`; and `KeyChord` `Ctrl+Alt` + `Space`. App settings also have `Paused` and `ExcludedProcesses`
(names, `.exe` stripped, case-insensitive).

### 2.7 Gesture state machine (`src/Filee.App/Services/Triggers/GestureDetector.cs`)

Pure, thread-safe (one lock), no UI or hook dependencies. **Port this 1:1; it is the most valuable piece here.**

State: `gestures`, `armed: Option<{gesture, button, x, y, token}>`, `drag_active`, `late_rejected`, `last_x/last_y`,
`paused`, `system_drag_threshold` (default 4), `scope_filter(gesture, x, y) -> bool`.

- **MouseDown(button, x, y, mods)** (lines 54-82): store last pos; clear `armed`, `late_rejected`. If paused, stop.
  Find the first enabled `Drag` or `Hold` gesture with `button == button && modifiers == mods` (**exact** modifier
  match). If found and `scope_filter` passes, arm it with `token = tick_ms ^ (x*7919) ^ y`. For `Hold`, ask the
  owner to call `HoldElapsed(token)` after `hold_milliseconds` (`HoldTimerRequested`).
- **MouseMove(x, y, mods, held_button)** (lines 85-99): store last pos. If armed: when `|dx| > threshold || |dy| >
  threshold`, disarm; fire `DragStarted` only if it was a `Drag` (a `Hold` is cancelled by moving). If not armed:
  **late modifier** detection (lines 120-133): if no drag is active, not rejected before, a button is held and some
  modifier is down, find a `Drag` gesture matching `held_button` and `mods`; if the scope passes, fire `DragStarted`;
  otherwise set `late_rejected` so we do not hit-test on every move. This covers "start dragging, then press Ctrl".
- **MouseUp** (lines 135-149): clear armed, reset `late_rejected`; if a drag was active, fire `DragEnded`.
- **HoldElapsed(token)** (lines 152-167): if still armed with the same token and kind `Hold`, fire `HoldTriggered`
  at the armed position.
- **KeyDown(key, mods)** (lines 170-188): if not paused, find a `KeyChord` with matching key (case-insensitive) and
  exact modifiers; check the scope at the **last known cursor position**; fire `KeyChordTriggered`.

Events are raised **outside** the lock. The service posts them to the UI thread (`TriggerService.cs:35-40`).

Scope filter (`TriggerService.cs:133-141`):

```
process = platform.process_name_at(x, y)
if process == own process name or process in excluded: return false
return gesture.scope == Anywhere || platform.is_file_manager_at(x, y)
```

Modifier mask mapping uses libuiohook's `EventMask` (`HasCtrl/HasAlt/HasShift/HasMeta`); held button is read from
the mask bits `Button1..Button5` (lines 155-171). Button mapping: 1=Left, 2=Right, 3=Middle, 4=X1, 5=X2.

Debug switch: `FILEE_TRACE_GESTURES=1` logs every press and scope decision (`TriggerService.cs:25-26`). Worth copying
as `MORPHRING_TRACE_GESTURES`.

---

## 3. File manager context menus

### 3.1 Command line contract (`src/Filee.App/Services/CommandLine.cs`)

All menus end up calling the GUI binary with these options (parsed case-insensitively; `--name=value` form):

| Option | Effect |
|---|---|
| (none) / `--settings` | open main window |
| `--background` | tray only (used by autostart) |
| `--convert a b ...` | every following argument that does **not** start with `--` and **exists** (file or dir) is added as an absolute path |
| `--convert-list <file>` | read UTF-8 file (BOM ok), one path per line, trim, skip blanks, keep existing ones; afterwards delete the file **only if** it is `%TEMP%/Filee-convert-*.txt` (safety against crafted command lines, lines 92-122) |
| `--install-engines=a,b` | installer: download these engines (empty = user chose none) |
| `--start-with-windows=on\|off` / `--start-with-system=` | installer sets option; values `on/1/true/yes`, `off/0/false/no` |
| `--context-menu=on\|off` | installer sets option |
| `--quit` | ask running instance to exit and wait (no window) |
| `--uninstall-cleanup` | quit, then remove per-user integration and downloaded engines |

Unknown options are ignored so older builds tolerate newer flags.

### 3.2 Single instance and argument forwarding (`src/Filee.App/Services/SingleInstance.cs`, `Program.cs`)

- Name: `"Filee-" + HEX(SHA256(UTF8(username)))[..12]` (stable across processes; `string.GetHashCode` is not).
- Lock: named mutex (`Local\<name>` on Windows; on Unix .NET emulates it per user, `CurrentUserOnly=true`).
- Channel: named pipe with the same name, current-user only (on Unix .NET makes this a Unix domain socket under
  `$TMPDIR`). One connection = one JSON array of strings (`["--convert","/a","/b"]`). The server loops forever;
  a half-written message is ignored.
- Second launch: `ForwardToFirst(args)` with a **3 s** connect timeout, then exit 0 (`Program.cs:22-27`).
- `--quit` / `--uninstall-cleanup`: forward `--quit`, wait up to **20 s** for the mutex, then wait for any other
  process of the same executable path to exit; exit code 1 if still running (`Program.cs:52-73`).

This matters for context menus: Windows' classic verb uses `MultiSelectModel=Player`, which can start one process
per selected file; the running instance collects them.

### 3.3 Linux (`src/Filee.Platform.Linux/LinuxDesktopIntegration.cs`)

Base dirs: `XDG_CONFIG_HOME` (fallback `~/.config`) and `XDG_DATA_HOME` (fallback `~/.local/share`); an env value
is only used if it is an absolute path (`LinuxPlatformServices.cs:80-86`). Label comes from translations
(`general.context_menu_label`, for example "Convert with Filee").

`SetContextMenu(enabled, exe, label)` writes or deletes **five** things (lines 102-130):

| File manager | File | Content |
|---|---|---|
| Thunar custom action | `$XDG_CONFIG_HOME/Thunar/uca.xml` | XML `<action>` with `unique-id = filee-convert` (see below) |
| Thunar Send To | `$XDG_DATA_HOME/Thunar/sendto/filee.desktop` | `[Desktop Entry]` `Type=Application`, `Name=<label>`, `Exec=<exe> --convert %F`, `Icon=document-save-as`, `Terminal=false` |
| Nautilus (GNOME Files) script | `$XDG_DATA_HOME/nautilus/scripts/Filee` (mode 755) | POSIX sh, see below |
| Nemo (Cinnamon) action | `$XDG_DATA_HOME/nemo/actions/filee.nemo_action` | `[Nemo Action]` `Name=<label with % doubled>`, `Exec=<exe> --convert %F`, `Selection=notnone`, `Extensions=nodirs;`, `UriScheme=file`, `Icon-Name=document-save-as` |
| Dolphin (KDE) service menu | `$XDG_DATA_HOME/kio/servicemenus/filee.desktop` (mode 755; Plasma 6 refuses non-executable service menus) | `[Desktop Entry]` `Type=Service`, `MimeType=application/octet-stream;`, `Actions=convert;`, `X-KDE-Protocols=file`, then `[Desktop Action convert]` `Name=`, `Icon=document-save-as`, `Exec=<exe> --convert %F` |

The Nautilus script (exact text, line 127):

```sh
#!/bin/sh
[ "$#" -gt 0 ] || exit 0
for file do
  case "$file" in
    /*) set -- "$@" "$file" ;;
    *) set -- "$@" "$PWD/$file" ;;
  esac
  shift
done
exec '<exe>' --convert "$@"
```

(Nautilus passes names relative to the current folder; the loop makes them absolute in place.)

Thunar `uca.xml` handling (lines 132-168):

- Load the user file with whitespace preserved; if missing, copy the system default from the first absolute dir in
  `XDG_CONFIG_DIRS` (default `/etc/xdg`) that has `Thunar/uca.xml`; otherwise start with `<actions/>`.
  (Copying the defaults is important: creating the user file hides the system actions.)
- Root must be `<actions>`, else error.
- Remove every `<action>` whose `<unique-id>` is `filee-convert`. If enabled, append:

```xml
<action>
  <icon>document-save-as</icon>
  <name>LABEL</name>
  <unique-id>filee-convert</unique-id>
  <command>'/path/to/Filee' --convert %F</command>   <!-- shell-quoted, % doubled -->
  <description>LABEL</description>
  <patterns>*</patterns>
  <audio-files/><image-files/><other-files/><text-files/><video-files/>
</action>
```

Disable = remove our action (file rewritten only if it exists) and delete the four other files.

Quoting helpers (lines 170-203), all reject empty/whitespace, NUL, CR, LF:

- `DesktopArgument(path)`: **reject `=`** (desktop-entry Exec limitation); wrap in `"`; backslash-escape
  `"`, `` ` ``, `$`, `\`; then apply `DesktopValue` (escape `\`→`\\`, CR/LF/TAB as `\r \n \t`); then `%`→`%%`.
- `ShellArgument(path)`: `'` + path with `'` replaced by `'\''` + `'`.
- `DesktopValue(text)`: as above, for `Name=` values.

All writes are atomic (lines 205-225): write `path.<guid>.tmp` as UTF-8 without BOM with `\n` endings, `chmod 644`
(or `755` when executable), `rename` over the target, always delete the temp file in `finally`.

Autostart (lines 91-100): `$XDG_CONFIG_HOME/autostart/filee.desktop`:

```ini
[Desktop Entry]
Type=Application
Name=Filee
Exec="<exe>" --background
Terminal=false
X-GNOME-Autostart-enabled=true
```

Reveal (`LinuxPlatformServices.cs:58-76`): if the path is a file, run

```
gdbus call --session --dest org.freedesktop.FileManager1 --object-path /org/freedesktop/FileManager1 \
  --method org.freedesktop.FileManager1.ShowItems "['file:///abs/uri']" ""
```

(single quotes in the URI backslash-escaped). On failure, or for a directory, `xdg-open <dir>`. Helper commands run
with a **3 s** timeout, stdout/stderr captured; optional ones only log on failure (lines 88-112).

### 3.4 Thunar keyboard shortcut (the Linux stand-in for the key chord)

Because Linux has no selection API, the first enabled `KeyChord` gesture becomes a GTK accelerator for the Thunar
action, only while the context menu is on and the app is not paused (`SystemIntegrationService.cs:35-48`).

- Accel path: `<Actions>/ThunarActions/uca-action-filee-convert`.
- File: `$XDG_CONFIG_HOME/Thunar/accels.scm`.
- `ToThunarAccelerator` (lines 13-47): modifiers in the order `<Control><Alt><Shift><Super>`, then the key mapped from
  libuiohook names: `Space→space`, `Enter→Return`, `Backspace→BackSpace`, `PageUp→Page_Up`, `PageDown→Page_Down`,
  `Semicolon→semicolon`, `Equals→equal`, `Comma→comma`, `Minus→minus`, `Period→period`, `Slash→slash`,
  `BackQuote→grave`, `OpenBracket→bracketleft`, `Backslash→backslash`, `CloseBracket→bracketright`,
  `Quote→apostrophe`, `PrintScreen→Print`, `Pause`, navigation keys unchanged, single letters/digits lowercased,
  `NumPadN→KP_N`, `F1..F24` unchanged; anything else is an error.
- `UpdateThunarShortcut(contents, accel)` (lines 60-83): validate `^(<(Control|Primary|Alt|Shift|Super|Meta)>)*[A-Za-z0-9_]+$`;
  scan lines `(gtk_accel_path "PATH" "KEY")` (optionally commented with `;`); if another **active** path already
  uses the same accelerator (normalized: `<Primary>`→`<Control>`, lowercase, modifiers sorted) throw
  "Thunar already uses X for Y"; delete our own line (commented or not); append
  `(gtk_accel_path "<Actions>/ThunarActions/uca-action-filee-convert" "<Control><Alt>space")`.
- Fragile: **Thunar rewrites `accels.scm` when it exits**, so the user must close Thunar first
  (`LinuxPlatformServices.cs:31-32`).

### 3.5 Windows

Classic verb (`WindowsPlatformServices.cs:194-207`), per user, no admin:

```
HKCU\Software\Classes\*\shell\Filee
  MUIVerb          = <label>
  Icon             = "<exe>",0
  MultiSelectModel = Player
  command\(Default)= "<exe>" --convert "%1"
```

Send To shortcut: `%APPDATA%\Microsoft\Windows\SendTo\Filee.lnk` → target exe, args `--convert`, icon `exe,0`,
created through `WScript.Shell` COM. Autostart: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `Filee`
= `"<exe>" --background`.

Windows 11 top-level menu (only Windows 11 build >= 22000 on x64): a **sparse MSIX** ("package with external
location") that declares an `IExplorerCommand` COM server living in the install folder.

- Package identity (`ExplorerMenuPackage.cs`): name `Filee.ExplorerMenu`, publisher
  `CN=Filee, OID.2.25.311729368913984317654407730594956997722=1` (the special OID lets Windows 11 accept an
  **unsigned** package), family name `Filee.ExplorerMenu_c73vxh346rtay`, manifest version `1.0.0.0` (decoupled from
  the app version so re-registration, which needs admin, is rare), CLSID `6DB0E670-807F-4D22-BE3F-20492D0F4AB1`,
  files `FileeExplorerMenu.dll` + `FileeExplorerMenu.msix` next to `Filee.exe`.
- Manifest: `windows.fileExplorerContextMenus` with `ItemType Type="*"` (files only), verb `ConvertWithFilee`;
  `windows.comServer` `SurrogateServer` class with `Path=FileeExplorerMenu.dll`, `ThreadingModel=STA`; capabilities
  `runFullTrust`, `unvirtualizedResources`; `AppListEntry="none"`; `AllowExternalContent=true`.
- Register (admin, one UAC prompt): `powershell.exe -NoLogo -NoProfile -NonInteractive -Command "<script>"` with
  `Add-AppxPackage -Path '<msix>' -ExternalLocation '<installdir>' -AllowUnsigned -ForceUpdateFromAnyVersion`.
  Remove: `Get-AppxPackage -Name 'Filee.ExplorerMenu' | Remove-AppxPackage` (tries without admin first).
  The script is wrapped in `try { ...; exit 0 } catch { [IO.File]::WriteAllText('<errfile>', $_.Exception.Message); exit 1 }`;
  plain `-Command` instead of `-EncodedCommand` because antivirus distrusts the latter. PowerShell single-quote
  escaping also doubles the typographic quotes `‘ ’ ‚ ‛`. Timeout 3 minutes; `ERROR_CANCELLED` (1223) = user said no.
- State: `GetPackagesByPackageFamily` (kernel32) and `GetPackagePathByFullName2(..., EffectiveExternal=5)` give
  Off / On / Outdated (other folder or older manifest version).
- On/off **without admin**: the DLL reads `%APPDATA%\Filee\explorer-menu.txt` on every right-click. The file's first
  line is the localized title; no file = entry hidden. When the packaged entry is On, the classic verb is deleted to
  avoid a duplicate under "Show more options".

The DLL (`src/Filee.ExplorerMenu/FileeExplorerMenu.c`, 515 lines of C, no CRT, `-nostdlib`, entry `DllMain`):

- `GetState`: hidden unless the title file exists **and** every selected item has `SFGAO_FILESYSTEM | SFGAO_STREAM`
  (rules out folders, drives, items inside ZIP folders).
- `GetTitle`: title file, else `Convert with Filee`. `GetIcon`: `<dir>\Filee.exe,0`.
- `Invoke`: build `"<dir>\Filee.exe" --convert "p1" "p2" ...`. If the command line is **>= 32000 chars**, write the
  paths as UTF-8 lines to `%TEMP%\Filee-convert-<pid>-<tick>-<n>.txt` (`CREATE_NEW`, 5 attempts) and run
  `--convert-list <file>`. `CreateProcessW`, then `AllowSetForegroundWindow(pid)` so Filee can bring the donut to
  the front, return immediately.

### 3.6 macOS

Finder entry is an **NSServices** service inside `Filee.app` itself (`installer/macos/Info.plist:17-27`): menu item
`Convert with Filee`, message `convertFiles`, port `Filee`, `NSSendFileTypes = [public.item]`, empty
`NSRequiredContext` (on by default). Localized via `*.lproj/ServicesMenu.strings`.

`MacOSServicesProvider.Register` (`src/Filee.Platform.MacOS/MacOSServicesProvider.cs:33-51`) creates an Objective-C
class `FileeServicesProvider : NSObject` at runtime with method `convertFiles:userData:error:` (type `v@:@@^@`), sets
it as `NSApp.servicesProvider`, calls `NSUpdateDynamicServices()`. The handler reads `NSURL` objects from the
pasteboard (`readObjectsForClasses:[NSURL] options:nil`), keeps file URLs, and posts `--convert <paths>` to the UI
thread. Registered first thing at start so a service clicked while Filee was not running is delivered.
Exceptions must not unwind into Objective-C (caught and dropped).

Toggling (`MacOSPlatformServices.cs:200-206`):

```
/usr/bin/defaults write pbs NSServicesStatus -dict-add \
  '"com.filee.app - Convert with Filee - convertFiles"' \
  '{ "enabled_context_menu" = 1; "enabled_services_menu" = 1; "presentation_modes" = { "ContextMenu" = 1; "ServicesMenu" = 1; }; }'
/System/Library/CoreServices/pbs -update
```

(0 instead of 1 to disable; also deletes a legacy `~/Library/Services/Filee.workflow`.)

Selection (lines 158-175): only if Finder is frontmost, run
`/usr/bin/osascript -l JavaScript -e "<script>"` with a **30 s** timeout:

```js
ObjC.import('Foundation');
const finder = Application('com.apple.finder');
JSON.stringify(finder.selection().map(item => ObjC.unwrap($.NSURL.URLWithString(item.url()).path)));
```

Needs the Automation permission (`NSAppleEventsUsageDescription`). Autostart: `~/Library/LaunchAgents/com.filee.app.plist`
with `Label=com.filee.app`, `ProgramArguments=[exe, --background]`, `LimitLoadToSessionType=Aqua`, `RunAtLoad=true`,
`KeepAlive=false`. Reveal: `open -R <file>` or `open <dir>`. Accessibility settings deep link:
`x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility`.

### 3.7 Applying settings (`src/Filee.App/Services/SystemIntegrationService.cs`)

Called on start and whenever settings change. It caches the last applied `(enabled, exe)` and `(enabled, exe, label)`
tuples and only rewrites when they differ. Each step is wrapped separately; errors are collected and shown once as
"integration failed". The label follows the UI language, so changing language rewrites the menus.

`UninstallCleanup.Run()` (`Services/UninstallCleanup.cs`): `SetContextMenu(false)`, `SetStartWithSystem(false)`,
Linux `SetThunarShortcut(null)`, Windows remove packaged menu (40 s cap); only if all succeeded, delete the downloaded
engines folder. Settings are always kept. Exit code reflects success, and the uninstall scripts **keep the app if
cleanup failed** so the user can retry.

---

## 4. Installers and packaging

### 4.1 Linux: portable tarball + user-local install script

Artifact: `Filee-<ver>-linux-{x64,arm64}.tar.gz` plus `...-SHA256SUMS.txt`. Contents: self-contained .NET publish of
`Filee` (GUI) and `filee-cli`, `engines/rhwp`, `engines/7zip`, `filee.png`, `install.sh`, `uninstall.sh`,
`INSTALL.txt`, `LICENSE`, `THIRD-PARTY-NOTICES.md`. No .deb, .rpm, AppImage or Flatpak.

`installer/linux/install.sh` (no root):

1. `data_dir=${XDG_DATA_HOME:-$HOME/.local/share}`, `install_dir=$data_dir/filee/application`, `bin_dir=$HOME/.local/bin`,
   `desktop_file=$data_dir/applications/com.filee.app.desktop`.
2. Refuse if any of those already exist (also checks symlinks). Never overwrites.
3. Require `python3` and executable `Filee` and `filee-cli` in the extracted folder.
4. `cp -R <extracted> $install_dir`; write marker `$install_dir/.filee-package` = `com.filee.app`.
5. `ln -s $install_dir/filee-cli ~/.local/bin/filee`.
6. Python writes the launcher: `Name=Filee`, `Comment=Convert files with Filee`,
   `Exec="<install_dir>/Filee" --convert %F`, `Icon=<install_dir>/filee.png`, `Terminal=false`, `Categories=Utility;`,
   `X-Filee-Managed=true` (marker used by the uninstaller).
7. `update-desktop-database` if present.

`installer/linux/uninstall.sh`: Python verifies the target is not a symlink, resolves to exactly
`$data_dir/filee/application`, and has the marker; then runs `Filee --uninstall-cleanup` (with `set -e`, a failure
stops here); removes the `~/.local/bin/filee` symlink only if it points to our CLI; removes the `.desktop` only if it
has `X-Filee-Managed=true`; `rm -rf` the install dir.

Runtime deps listed for Ubuntu 24.04 (`installer/linux/INSTALL.txt:13`): `libx11-6 libice6 libsm6 libxrandr2
libxkbcommon0 libxi6 libxtst6 libfontconfig1 libfreetype6 ...`. `libxtst6` is what libuiohook needs (XRecord/XTest).

### 4.2 macOS: tarball containing an ad-hoc signed `.app`

`build/build-portable.ps1` (macOS branch, lines 67-108):

1. Publish GUI and CLI into `Filee.app/Contents/MacOS`.
2. `Info.plist` from template with `__VERSION__` / `__BUNDLE_VERSION__` (prerelease suffix stripped); `plutil -lint`.
3. Icon: `sips -z` to 16/32/128/256/512 at 1x and 2x into an `.iconset`, then `iconutil -c icns`.
4. Copy `*.lproj` into `Resources`.
5. **Sign every file**, deepest path first, main executable last, then the bundle:
   `codesign --force --sign <identity> --entitlements installer/macos/entitlements.plist <file>`; with a real
   identity add `--timestamp --options runtime`. Default identity is `-` (**ad-hoc**). Verify with
   `codesign --verify --deep --strict`.
6. Entitlements: `com.apple.security.cs.allow-jit` (.NET) and `com.apple.security.automation.apple-events` (Finder).

Not notarized. `install.sh` copies with `ditto` to `~/Applications/Filee.app` and symlinks
`~/.local/bin/filee` → `Contents/MacOS/filee-cli`. `uninstall.sh` checks the path resolves inside `~/Applications`
and that `CFBundleIdentifier` (via `PlistBuddy`) is `com.filee.app`. Minimum macOS 14.

### 4.3 Windows: Inno Setup installer + portable zip

`installer/Filee.iss` (Inno Setup 7.1.0, downloaded and SHA-256 verified by `build/build-installer.ps1`, installed in
Inno's own portable mode so the build machine is untouched):

- Fixed `AppId` GUID `C8686E06-8327-40A2-B0F7-3DEDD61D9248` (never change). Per-machine install into Program Files
  (`PrivilegesRequired=admin`), x64 only, `MinVersion=10.0.17763`. Languages en/ko/zh-CN.
- Tasks (fresh install only): classic context menu, Windows 11 top menu, start with Windows, add `filee` to PATH
  (`{app}\cli\filee.cmd` → `..\filee-cli.exe`; HKLM `Environment\Path`, `REG_EXPAND_SZ`), desktop icon.
- Engine page generated by `build/tools/make-installer-engines.cs` from `engines.json`.
- `PrepareToInstall`: run `Filee.exe --quit` and wait; if it fails, `taskkill /F /IM Filee.exe /FI "USERNAME eq <user>"`.
- `ssInstall`: **rename the loaded `FileeExplorerMenu.dll` aside** (`.<timestamp>.old`) and schedule deletion at reboot,
  because Explorer keeps it loaded and a loaded DLL can be renamed but not overwritten. Also migrates a Velopack
  install of Filee <= 1.1.
- `ssPostInstall`: register the Explorer package if chosen (or refresh it on update), add PATH, then
  `ExecAsOriginalUser(Filee.exe, "--install-engines=... --start-with-windows=on|off --context-menu=on|off")`
  (updates use `--background`). Starting **as the original user** matters: an elevated Filee could not receive drag
  and drop from a non-elevated Explorer.
- `[UninstallRun]`: `Filee.exe --uninstall-cleanup`. Restart Manager filter is `*.exe` only, otherwise it would offer
  to close Explorer.

The DLL is compiled with a pinned **Zig 0.16.0** (`zig cc -target x86_64-windows-gnu -shared -O2 -s -nostdlib
-Wall -Wextra -Werror ... -lkernel32 -luser32 -lole32 -lshell32 -lshlwapi -Wl,--entry=DllMain`), downloaded from
mirrors with SHA-256 check and curl stall detection (`build/build-explorer-menu.ps1`). The MSIX is packed with the
Windows packaging COM API (`IAppxFactory` / `IAppxPackageWriter`) by `build/tools/make-explorer-package.cs`, so no
Windows SDK is needed.

### 4.4 Supply chain habits worth copying

- Every download (fonts, engines, Zig, Inno Setup) is pinned to a version **and** a SHA-256, cached under
  `build/.cache`, and re-verified from cache (`build/fetch-fonts.ps1`, `build/fetch-portable-engines.ps1`).
- Archive extraction rejects absolute paths, `..`, `:` and any entry that is not a regular file or directory
  (`fetch-portable-engines.ps1:39-53`).
- Archives are written as `.partial` then renamed; refuse to overwrite an existing release file.
- `SHA256SUMS.txt` in `sha256sum` format (`<hex>  <name>`) for every artifact.

---

## 5. CI and release pipeline (`.github/workflows/`)

Toolchain: .NET SDK `10.0.100` with `rollForward: latestFeature` (`global.json`); central package versions in
`Directory.Packages.props`; shared props (`net10.0`, nullable, analyzers, version `1.10.3`, repo URL used by the
updater) in `Directory.Build.props`.

**`ci.yml`** (push to main, PRs; skips `**/*.md` and `docs/**`; concurrency group cancels older runs):

- job `portable` = reuse `portable.yml`.
- job `build` on `windows-latest`: restore, `dotnet format --verify-no-changes`, fetch fonts, build Release,
  build the Explorer DLL (Zig cache via `actions/cache/restore` + explicit `save` so it is saved even when later steps
  fail), fetch small engines (rhwp, pandoc, 7zip), UI tests alone with `--blame-crash --blame-hang-timeout 5m`, other
  tests, `build-installer.ps1 -CheckOnly` (compiles the .iss against a stub, ~1 min), upload results/screenshots.

**`portable.yml`** (reusable, also manual): matrix `ubuntu-24.04/linux-x64`, `ubuntu-24.04-arm/linux-arm64`,
`macos-15/osx-arm64`, `macos-15-intel/osx-x64`, `fail-fast: false`, 40 min timeout.

1. apt install X11 libs + `xvfb` + CJK fonts (Linux).
2. Cache engine downloads keyed on `hashFiles(engines.json)`.
3. Build; run each test project; on Linux under `xvfb-run -a env -u WAYLAND_DISPLAY XDG_SESSION_TYPE=x11`.
4. `build-portable.ps1`.
5. **Smoke test the artifact**: one archive exists, checksum matches, extract, `codesign --verify` (macOS),
   `filee-cli version`, `rhwp --version`, `7zz i`, and a real conversion `filee-cli convert sample.txt --to hwpx --json`.
6. **Launch the GUI** under Xvfb: `Filee --settings --install-engines= --context-menu=off --start-with-system=off`,
   wait 8 s, check it is alive (macOS: screenshot, second launch must hand over), then `Filee --quit` from a new
   session (`os.setsid`) and expect exit 0 within 15 s. Collect logs from `~/.config/Filee/logs` or
   `~/Library/Application Support/Filee/logs`.
7. Upload `Releases/*-<rid>.tar.gz` and sums.

**`release.yml`** (on tag `v*`, `contents: write`): `portable` with `version = tag`, then `windows` job: build DLL
with version, build + test, `build-installer.ps1 -Version X -Portable` (Setup.exe, Portable.zip, SHA256SUMS), download
all `portable-*` artifacts, compose notes = the `## X.Y.Z` section of `CHANGELOG.md` (continuation lines joined) +
PR list from `gh api repos/.../releases/generate-notes` + "New Contributors" tail, then
`gh release create vX --title "Filee X" --notes-file notes.md --verify-tag <files>`.
`.github/release.yml` maps PR labels to note categories.

**Signing summary:** Windows binaries and the MSIX are **unsigned** (the OID publisher trick makes the MSIX
acceptable); macOS is **ad-hoc signed, not notarized** (users click "Open Anyway"); Linux is unsigned with SHA-256
sums. No secrets are needed by the pipeline except the default `GITHUB_TOKEN`.

---

## 6. Clever bits and fragile bits

Clever:

- The drag gesture does not need to know which files are dragged: the donut is just a drop target placed under the
  cursor. This is what makes it work in any file manager.
- Late-modifier detection (press Ctrl after the drag began).
- Windows 11 menu toggled by a plain text file, so admin rights are needed only once.
- Long selections go through a list file whose deletion is restricted to the app's own temp-file pattern.
- Release-only registration; uninstall scripts keep the app when cleanup fails.
- CI launches the real GUI and drives `--quit`, catching startup crashes that unit tests miss.

Fragile:

- Linux gestures are X11 only and disabled on any Wayland session (including XWayland).
- `/proc/<pid>/comm` truncation to 15 chars; WM_CLASS fallback uses the instance part only.
- `XGrabServer` freezes the whole X server while hit testing; a crash between grab and ungrab is bad.
- Thunar must be closed before editing `accels.scm`; writing `uca.xml` loses comments outside elements.
- Dolphin service menu uses `MimeType=application/octet-stream;` (works because most types inherit it; `all/allfiles`
  is the documented catch-all).
- `DesktopArgument` refuses exe paths containing `=`.
- Hit testing runs on the hook thread for every arming press; must stay fast (Windows drops slow hooks).
- macOS: changing the app location breaks Accessibility/Automation grants (TCC is tied to the signature/path).

---

## 7. Porting to Rust

### 7.1 Suggested module layout

```
crates/platform/            (new crate, no Tauri dependency, unit-testable)
  src/lib.rs                trait Platform (mirror of IPlatformServices) + NullPlatform
  src/gesture.rs            GestureDetector state machine (pure, port 1:1, heavy unit tests)
  src/cli_args.rs           --convert / --convert-list / --background / --quit parsing
  src/linux/desktop.rs      context menu files, autostart, Thunar accels (pure string builders + atomic write)
  src/linux/x11.rs          hit test, client list, raise (x11rb), behind cfg + feature "x11"
  src/windows/*.rs          registry verb, Run key, SendTo (later)
  src/macos/*.rs            LaunchAgent, pbs, CGWindowList (later)
```

### 7.2 Crates

| Need | Crate | Notes |
|---|---|---|
| Global mouse/keyboard (X11, Windows, macOS) | `rdev` (listen only; its `unstable_grab` feature is not needed), or bindings to libuiohook | `rdev` listens via XRecord on Linux, like libuiohook. Run it on a dedicated thread; send events over a `crossbeam-channel` / `std::sync::mpsc`. `rdev::listen` blocks forever and cannot be stopped cleanly on all OSes; keep one listener for the app lifetime and gate with a `paused` flag. |
| Wayland input (later) | `evdev` (needs user in `input` group, privacy-heavy), or `ashpd` for the `GlobalShortcuts` portal (keys only, no mouse) | No portable Wayland way to get global mouse drags. Hyprland/Sway have IPC (`hyprctl cursorpos`, `hyprctl activewindow -j`, `swaymsg -t get_tree`) that can replace hit testing. |
| X11 hit test, EWMH | `x11rb` (pure Rust, no libX11 needed) | `translate_coordinates`, `get_property(_NET_WM_PID / WM_CLASS / _NET_CLIENT_LIST)`, `send_event` ClientMessage. Prefer `x11rb` over `x11`/Xlib FFI: no unsafe struct layouts. |
| Process name from PID | read `/proc/<pid>/comm` directly (or `/proc/<pid>/exe` basename to avoid 15-char truncation) | `sysinfo` is heavier than needed. |
| Global key chord in Tauri | `tauri-plugin-global-shortcut` | Uses `global-hotkey`, which is **X11 only on Linux**; it does not work on native Wayland. |
| Single instance + arg forwarding | `tauri-plugin-single-instance` (DBus on Linux, named mutex/pipe elsewhere) for the GUI; or `interprocess` local sockets for our own protocol | Forward the argv as JSON `Vec<String>`; keep a 3 s connect timeout. Socket in `$XDG_RUNTIME_DIR`. |
| Autostart | `tauri-plugin-autostart` or our own `.desktop` writer (trivial, testable) | Pass `--background`. |
| XDG dirs | `dirs` / `directories` | Match Filee: env var only if absolute. |
| Thunar `uca.xml` | `quick-xml` (or `roxmltree` + string append) | Preserve existing actions; copy `/etc/xdg/Thunar/uca.xml` when the user file is missing. |
| Atomic writes | `tempfile::NamedTempFile::persist` in the same dir + `std::fs::set_permissions` | Same semantics as Filee's `Write`. |
| Reveal in file manager | `zbus` call to `org.freedesktop.FileManager1.ShowItems`, fallback `open` crate / `xdg-open`; or `tauri-plugin-opener` `reveal_item_in_dir` | Avoid shelling out to `gdbus` if zbus is already a dependency (Tauri pulls it on Linux). |
| Windows registry / COM / SendTo | `windows` crate (`Win32_System_Registry`, `Win32_UI_Shell`, `IShellLinkW`), `winreg` for simple keys | Later release. |
| Windows 11 `IExplorerCommand` DLL | `windows` crate with `#[implement(IExplorerCommand)]`, `cdylib` | Can replace the C file; keep the "no blocking, read one file" rule. |
| macOS | `objc2`, `objc2-app-kit`, `core-graphics`, `core-foundation` | Services provider can be done with `objc2` `declare_class!`. |
| Packaging | Tauri bundler (`.deb`, `.rpm`, `.AppImage`, `.msi`/NSIS, `.dmg`), `cargo-dist` for the CLI tarballs | Tauri on Linux needs `webkit2gtk-4.1` (not installed on the dev machine yet). |
| CI | `actions/checkout`, `dtolnay/rust-toolchain`, `Swatinem/rust-cache`, `tauri-apps/tauri-action` | Add `xvfb-run` for any GUI smoke test. |

### 7.3 Pitfalls to remember

- Never do slow work (hit testing that spawns processes, DBus calls) inside the hook callback. Do the X11 hit test
  with a persistent `x11rb` connection on the hook thread, or move scope checking to a worker with a tiny timeout.
- Exact modifier matching: Ctrl+Shift must not trigger a Ctrl gesture. Left and right modifiers are the same.
- The scope check at press time needs the window **under the cursor**, not the focused window.
- Tauri windows on Linux run on GTK; on Wayland GTK will pick the Wayland backend and the donut cannot position
  itself at the cursor. For gesture mode, force X11 with `GDK_BACKEND=x11` (still XWayland-limited) or document
  "drop zone + context menu only on Wayland", exactly as Filee does.
- `.desktop` `Exec` quoting rules differ from shell quoting; port `DesktopArgument` and `ShellArgument` with tests,
  including `%` doubling and the `=` rejection.
- Nautilus script must be executable; Dolphin service menu must be executable on Plasma 6.
- Write integration only from release builds (cargo feature `release-integration` or check an env var) so `cargo run`
  never edits the developer's file manager config.
- Keep integration idempotent: remove our entry by id, then add it.
- Use a marker (`X-MorphRing-Managed=true`, `.morphring-package`) so uninstall only deletes what we created.

---

## 8. v0.1.0 candidates (ship today, Linux first)

Realistic for today:

1. **CLI-first context menu integration** (no GUI needed): a `morphring integrate install|uninstall` command that
   writes the Nautilus script, Nemo action, Dolphin service menu and Thunar `uca.xml` entry, all calling
   `morphring convert --pick %F` (or the GUI binary when it exists). Pure string builders + atomic writes, easy to
   unit test against Filee's exact outputs in section 3.3.
2. **Argument contract**: accept `--convert <paths...>` (existing files only, made absolute) and `--background` in the
   GUI binary from day one, so menu files written today stay valid later.
3. **`install.sh` / `uninstall.sh`** for a user-local install (`~/.local/share/morphring`, `~/.local/bin/morphring`,
   `.desktop` launcher with a managed marker), copied from section 4.1 but without the Python dependency
   (`printf` is enough when we validate the path first).
4. **Release workflow**: on tag `v*`, build `x86_64-unknown-linux-gnu` CLI (and Tauri `.deb`/`.AppImage` if
   webkit2gtk builds in CI), produce `morphring-<ver>-linux-x64.tar.gz` + `SHA256SUMS.txt`, smoke test
   `morphring --version` and one real ffmpeg/ImageMagick conversion, then `gh release create --verify-tag`.
   CI on push/PR: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`.
5. **Reveal in file manager** via `org.freedesktop.FileManager1.ShowItems` with `xdg-open` fallback (tiny).
6. **Port `GestureDetector` as a pure module with tests**, even if the live hook is not wired yet. It has no OS
   dependencies and de-risks v0.2.

Defer to later releases:

- Live global hook (X11 via `rdev`/`x11rb`) and the cursor donut; on the user's Wayland session it would not work
  anyway without compositor-specific code.
- Thunar accelerator editing, autostart, single-instance forwarding (needed once the GUI is a tray app).
- Windows (registry verb, SendTo, Windows 11 sparse package + COM DLL, installer) and macOS (Services, LaunchAgent,
  codesign/notarization).
