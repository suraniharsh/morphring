# 05 — App UI: the ring (donut) menu, gestures, drop zone, settings, theming, i18n, lifecycle, updates

Reverse-engineering notes for `src/Filee.App/**` of Filee (C#/.NET 10 + Avalonia 11, MIT).
All paths below are relative to the Filee repo root. Line numbers are from the snapshot we read.

The goal: a Rust developer should be able to rebuild this without reading the C#.

---

## 0. The big picture in one paragraph

Filee is a tray app. A global input hook watches the mouse and keyboard. When the user starts a
**modifier + drag** (Ctrl + left-drag by default; Alt on macOS) over a file manager, Filee places an
almost-invisible, always-on-top, borderless, non-activating window **centered on the cursor**. When
the dragged files enter that window, a **donut menu** pops out. Each slice is a *preset* (for example
"to PDF" or "JPG 70%"). Dropping on a slice starts a conversion, and the donut turns into a **progress
ring** in place. If the job takes longer than 5 s, or fails, a **toast** window in the bottom-right
takes over. The same donut also opens in **click mode** (files known up front): from the file manager
context menu (`Filee --convert a b c`), from the drop zone on the Home page, or from a keyboard chord.
A settings window (left nav, 9 pages) edits presets, profiles (which presets a file type gets), the
donut look, gestures, theme and language.

```
             ┌───────── TriggerService (global hook thread) ─────────┐
 mouse/keys →│ GestureDetector (pure state machine, thread-safe)      │
             └──── DragStarted / DragEnded / SelectionGesture ───────┘
                               │ posted to UI thread
                               ▼
 CLI --convert ─┐        RadialController ──► RadialWindow (transparent, topmost)
 Drop zone ─────┼─► ShowForFiles(x,y,files)        └─ DonutMenu (hand-drawn control)
 Key chord ─────┘        ShowForDrag(x,y)                 ├─ slices / hover / open+close anim
                               │                          └─ progress ring mode
                               ▼
                       ConversionService.Start(files, preset) ─► JobQueue (Core)
                               │
                               └─► ToastWindow (bottom-right) when ring hands off
```

---

## 1. The donut control — `src/Filee.App/Controls/DonutMenu.cs`, `DonutMenu.Progress.cs`, `DonutGeometry.cs`

### 1.1 Why it is hand-drawn

`DonutMenu` is a single custom `Control` with no child elements (comment at `DonutMenu.cs:1-8`). Every
slice is drawn as a path, and hit testing is pure math on angles and radii. Reasons given: fast on
low-end PCs, and every slice can be hit-tested by its exact wedge shape. Animations are small spring
simulations advanced once per rendered frame.

The same control is used in three places:

| Where | Mode | File |
|---|---|---|
| Live toolbar at the cursor | normal (drag or click mode) | `Views/RadialWindow.axaml` |
| Settings → Donut toolbar editor | `IsEditMode=True` (drag to reorder, pencil button) | `Views/Pages/ToolbarPage.axaml:141` |
| Settings → Theme live preview | normal, `OuterRadius=120`, fixed items | `Views/Pages/ThemePage.axaml:69` |

### 1.2 Data model

```text
DonutItem {                        // DonutMenu.cs:22-34
  id: string
  label: string                    // bold first line, e.g. "To PDF"
  caption: string?                 // small second line, e.g. "PDF"; hidden if equal to label (case-insensitive)
  is_enabled: bool = true          // disabled = greyed, not clickable, no accent
  disabled_reason: string?         // tooltip + centre text when hovered
}

DonutHitKind = Outside | Center | Segment(index) | EmptyRing | EditButton(index)
             | ProgressRing | ProgressCancel                  // DonutGeometry.cs:10-28
```

Styled (bindable) properties with defaults (`DonutMenu.cs:39-71`):

| Property | Default | Notes |
|---|---|---|
| `Items` | null | list of `DonutItem` |
| `OuterRadius` | 150 | logical px |
| `HoleRatio` | 0.42 | inner radius = outer × clamp(ratio, 0.2, 0.75) (`:170`) |
| `SliceOpacity` | 0.96 | slice fill opacity |
| `CenterTitle`, `CenterSubtitle` | null | text in the hole |
| `IsEditMode` | false | editor behaviour |
| `AllowContextEdit` | false | live toolbar click mode: right-click a slice = edit preset |
| Brushes | Slice=White, SliceBorder=null, Center=White, Accent=MediumPurple, OnAccent=White, Foreground=Black, SecondaryForeground=Gray, Danger=IndianRed, Success=SeaGreen | overridden by theme style, see §6 |

Constants (`DonutMenu.cs:74-78`, `DonutMenu.Progress.cs:33-42`):

```
GapPixels        = 3      // constant pixel gap between slices
PopOut           = 7      // px a hovered slice moves outward
OpenDuration     = 0.42 s // per-slice open animation
OpenStagger      = 0.028 s
DragStartDistance= 5 px   // edit mode: press-move distance to start a slice drag
RingRadius       = 36     // progress ring (stroke centre)
RingStroke       = 6
CollapseSeconds  = 0.28, CollapseStagger = 0.02
MinRingSeconds   = 0.6    // ring always visible at least this long
HoldSuccessSeconds = 0.9, HoldFailureSeconds = 1.4, (Cancelled hold = 0.2)
FadeSeconds      = 0.25, HandOffSeconds = 0.22
```

Control size (`MeasureOverride`, `:334-338`): a square of side `2 × (OuterRadius + PopOut + 14)`
= 2 × (150 + 7 + 14) = **342 logical px** by default. The 14 px margin leaves room for the soft
shadow. `ClipToBounds = false`, cursor = hand.

### 1.3 Geometry (pure math) — `DonutGeometry.cs`

Angles are **clockwise from 12 o'clock, in radians**. Slice `i` is **centred** on `i × span`, so
slice 0 sits straight up and the rest follow clockwise (`DonutGeometry.cs:1-4`).

```text
span(n)          = n <= 0 ? 2π : 2π / n
mid_angle(i, n)  = i * span(n)
point_at(c, r, a)= (c.x + r*sin(a), c.y - r*cos(a))          // y grows downwards (screen coords)
angle_of(c, p)   = a = atan2(p.x - c.x, c.y - p.y); a < 0 ? a + 2π : a
index_at(a, n)   = n <= 0 ? -1 : floor((a + span/2) / span) mod n
hit_test(c, p, r_in, r_out, n, slack=0):
    d = |p - c|
    d < r_in            -> Center
    d > r_out + slack   -> Outside
    n == 0              -> EmptyRing
    else                -> Segment(index_at(angle_of(c,p), n))
```

The live control calls `hit_test` with `slack = PopOut (7)` so a popped-out slice is still hit
(`DonutMenu.cs:329`). Note there is no gap handling in hit testing: the 3 px visual gaps belong to
the nearest slice by angle. Edit mode adds one rule first: if a slice is hovered and the point is
within 13 px of its pencil button centre, the result is `EditButton(hoverIndex)` (`:326-328`).

**Wedge shape** (`CreateWedge`, `DonutMenu.cs:888-904`). To get a gap of constant *pixel* width (not
constant angle), the half-angle is reduced separately at each radius:

```text
outer_half = max(0.001, span/2 - (GapPixels/2) / r_out)
inner_half = max(0.001, span/2 - (GapPixels/2) / r_in)
path: M point_at(c, r_out, mid - outer_half)
      A r_out arc clockwise   to point_at(c, r_out, mid + outer_half)   (large-arc if 2*outer_half > π)
      L point_at(c, r_in,  mid + inner_half)
      A r_in arc counter-cw   to point_at(c, r_in,  mid - inner_half)   (large-arc if 2*inner_half > π)
      Z
```

If `span >= 2π - 0.001` (one slice, or the empty donut) it draws a full **ring** instead: two circles
(outer and inner), each made of two 180° arcs, with the **even-odd** fill rule (`:907-920`).

### 1.4 Rendering order (normal mode) — `Render`, `DonutMenu.cs:692-765`

1. **Soft shadow** (`DrawSoftShadow`, `:767-784`): no blur effect. It is one ellipse of radius
   `r_out + 14`, centred 4 px *below* centre, filled with a radial gradient:
   transparent at 0 → transparent at `(r_out-18)/R` → black alpha 46 at `(r_out-2)/R` → transparent at 1.
   Its opacity is the average of all slices' open progress. Cheap on any GPU.
2. **Empty donut** (n == 0): a full ring in the slice colour; if a drop target is set, an accent
   overlay at 0.35 opacity.
3. **Each slice** `i` (skipped if its open value ≤ 0.001 or span < 0.01):
   - `hover` = hover spring value (0 when a drop preview is showing).
   - `mid`/`span` come from layout springs in edit mode, otherwise from `mid_angle`/`span`.
   - Offset outward by `point_at((0,0), hover × PopOut, mid)`.
   - Transform about the centre: rotate by `(1 - min(open,1)) × -0.35` rad, scale by
     `(0.55 + 0.45 × open) × (1 + hover × 0.025)`, then translate by the offset.
   - Opacity `clamp(open,0,1)`, × 0.3 if this is the "dimmed" slice being dragged in the editor.
   - Fill = slice brush at `SliceOpacity × (enabled ? 1 : 0.55)`, 1 px border in `SliceBorderBrush`.
   - Accent overlay at `0.82 × emphasis` where `emphasis = 1` for the editor drop target else `hover`.
     Only for enabled slices. Comment: "no floor: a floor made it vanish with a jump" (`:747`).
   - Label (see below). "Hot" (accent text colour) when it is the active index or drop target and enabled.
4. **Drop preview** slice in edit mode (dashed accent outline `[4,3]`, 1.5 px, fill accent 0.22,
   label fades in once its span > 0.25 rad) (`:870-882`).
5. **Centre** (`DrawCenter`, `:786-803`): circle of radius `(r_in - 6) × (1 + centerPulse × 0.06)`,
   fill `CenterBrush`, border pen. When an external drag hovers the hole ("drop here = cancel") it
   adds a Danger overlay at 0.18 and draws the subtitle in Danger colour. Title: semibold 14 px,
   max 2 lines; subtitle: regular 11.5 px, max 2 lines; both max width `radius × 1.6`, centred
   vertically as a block with a 2 px gap.
6. **Pencil edit button** in edit mode on the hovered slice: accent circle r=12 with a 2 px ring in
   the centre colour, at `point_at(c, r_out - 14 + PopOut, mid)`; Material "edit" icon scaled to 13 px.

**Labels** (`DrawLabel`, `:805-822`): anchored at the slice's mid-radius `(r_in + r_out)/2` on the mid
angle. Text width = `clamp(span × midRadius - 10, 30, 120)`. Label = semibold 13 px, max 2 lines,
character ellipsis, centre aligned. Caption = regular 10.5 px, 1 line, shown only when not equal to the
label and `label.height + 14 < (r_out - r_in - 16)`. Colours: hot → OnAccent; enabled → Foreground;
disabled → SecondaryForeground.

Subtle bug fix worth copying (`DrawCentered`, `:840-847`): text is laid out centred inside a box of
`MaxTextWidth`, so the *box* must be centred on x, not the measured text width, or short labels
drift right.

`WithOpacity` multiplies a solid brush's alpha; other brush types pass through unchanged.

### 1.5 Interaction (normal mode) — `:360-468`

- **PointerMoved**: hit test → hover index = segment or edit-button index, else -1. Sets a tooltip to
  `DisabledReason` while a disabled slice is hovered. In edit mode, if a slice was pressed and the
  pointer moved > 5 px, fires `SegmentDragStarted(index, args)` instead.
- **PointerExited**: hover = -1 (unless a press is in progress).
- **PointerPressed**: right button on a slice, when `IsEditMode || AllowContextEdit` → fires
  `SegmentContextRequested(index)`. Left button → remember the press (slice index or -1).
- **PointerReleased** (only if a press was remembered): `EditButton` → `SegmentContextRequested`;
  `Segment` (not edit mode, enabled) → `SegmentInvoked(index)`; `Center` (not edit mode) →
  `CenterInvoked`. Release outside does nothing.
- `ActiveIndex = externalHighlight >= 0 ? externalHighlight : hoverIndex`. The external highlight is
  set by the window during OS drag-and-drop (pointer events do not arrive during a DnD).
  `ActiveIndexChanged(index)` fires on every change; the controller uses it to update the centre hint.

Events summary: `SegmentInvoked(int)`, `SegmentContextRequested(int)`, `CenterInvoked`,
`SegmentDragStarted(int, args)`, `ActiveIndexChanged(int)`, `ProgressCancelRequested`, `ProgressClicked`.

Public methods: `PlayOpenAnimation()`, `PlayCloseAnimation(done)`, `PulseCenter()`,
`SetExternalHighlight(index, centerHot)`, `LitAmount(index)` (for logs), `SetDimmedIndex`,
`SetDropTarget`, `SetDropPreview(slot, label, removed)`, `SlotAt(point, slots)`, `HitTest(point)`,
`BeginProgress()`, `SetProgress(0..1)`, `CompleteProgress(outcome, done)`, `HandOffProgress(done)`.

### 1.6 Animation engine — `:470-688`

**Spring** (`:476-505`): semi-implicit Euler, value + velocity.

```text
step(target, dt, stiffness=320, damping=20):
    steps = max(1, ceil(dt / (1/120)))       // sub-step: never integrate more than 1/120 s at once
    h = dt / steps
    repeat steps: force = k*(target - x) - c*v;  v += force*h;  x += v*h
    settled = |target - x| < 0.001 && |v| < 0.01  → snap x = target, v = 0
    return !settled                          // true = needs more frames
```

The sub-stepping is a real bug fix: with one big step on a late 50 ms frame, the springs overshot more
each frame and "slices stayed lit and flew apart on a busy machine" (`:481-485`).

Springs in use:

| What | Target | k | c | Kick |
|---|---|---|---|---|
| Hover (becoming active) | 1 | 320 | 20 (underdamped → "boing") | — |
| Hover (letting go) | 0 | 320 | 36 (critical, no bounce) | — |
| Centre pulse after a drop | 0 | 260 | 12 | velocity = 6 |
| Edit layout mid / span per slice + preview | target layout | 420 | 36 | — |
| Progress ring pop-in | 1 | 300 | 16 | velocity = 2 |
| Outcome badge ✓/! | 1 | 420 | 14 | velocity = 9 (overshoot "pop") |

Comment at `:639-640`: letting go must not bounce below 0, or the faded accent would flash again.

**Open animation** (time-based, not a spring, `:647-667`): per slice
`t = clamp((clock - i × 0.028) / 0.42, 0, 1)`, `open_i = easeOutBack(t)` with `c1 = 1.55`:
`1 + (c1+1)(t-1)^3 + c1 (t-1)^2`. **Close**: `t = clamp((clock - i × 0.01) / 0.16)`,
`open_i = 1 - t^3`. When all `t >= 1` the close callback fires (it hides the window).

**Frame loop** (`:514-613`): `StartAnimation()` invalidates and requests an animation frame if no
loop is running. `dt = clamp(frameTime - lastFrameTime, 0, 0.05)`, first frame `1/60`. A frame
requests the next only while `Advance(dt)` reports activity. A **watchdog** (fragile area,
`:507-577`): a frame request can be lost (window hidden in between), so a timer checks after
300 ms; if no frame came for 250 ms it re-requests, and after 2 stalls it **jumps every animation
to its end state** (`FinishAnimations`) so an opening donut always appears. Loop ids discard frames of
abandoned loops.

**Reduced motion** (`Motion.Enabled == false`): every value snaps to its target; open/close are instant;
the progress ring still keeps its *hold* timers so the user still sees ✓ for a moment (`:623-636`).

### 1.7 Progress ring mode — `DonutMenu.Progress.cs`

State flow (comment `:1-10`):

```
BeginProgress  → slices collapse into the centre (0.28 s + 0.02 s × (n-1) stagger)
               → ring springs in (scale 0→1)
SetProgress(x) → arc follows: shown += (target - shown) × min(1, dt × 9); snap when < 0.002
CompleteProgress(outcome, done)
               → Succeeded: target = 1; wait until ring visible ≥ 0.6 s AND arc ≥ 0.999
               → Failed/Cancelled: wait until ring visible ≥ 0.6 s
               → badge pops (✓ green / ! red; none for Cancelled)
               → hold 0.9 s (success) / 1.4 s (failed) / 0.2 s (cancelled)
               → fade 0.25 s (scale ×(1 - 0.15·fade), opacity 1 - fade) → done()
HandOffProgress(done) → shrink away in 0.22 s (scale × (1 - 0.7·easeInCubic)) → done()
```

Collapse rendering (`:259-301`): for slice `i`, `t = clamp((clock - i × 0.02) / 0.28)`,
`c = t^3`; transform about centre: rotate `c × 0.25` rad, scale `1 - 0.8c`; opacity `1 - c`.
The hole shrinks from `r_in - 6` to the plate radius `RingRadius + RingStroke + 2 = 44`.

Ring rendering (`:303-345`): plate shadow (radial gradient, 3 px down), plate circle (centre colour),
track circle (secondary text colour at 0.22, 6 px), arc from 12 o'clock clockwise, round caps.
Arc colour: accent while running, Success on success, Danger on failure (failure shows at least 2%
so the red is visible). Centre text: `"{round(p×100)}%"`, semibold 15 px, fading out as the badge
grows. Badge: filled circle r = 22 × scale; ✓ drawn as a polyline (−9,+0.5) → (−2.5,+7) → (+10,−6.5),
3.6 px round caps/joins in OnAccent; failure draws a "!" glyph 22 px.

Hover on the ring (pointer within `RingRadius + RingStroke + 14 = 56` px of the centre) shows a small
× cancel button: circle r=10 in Foreground at 45° (`point_at(c, 42, π/4)`), Material "close" icon in
the centre colour. Hit: within 11 px of that point → `ProgressCancel`; within 44 px of centre →
`ProgressRing`. Release on ProgressCancel → `ProgressCancelRequested`; on ProgressRing →
`ProgressClicked` (controller opens the output folder if the job has outputs).

`DrawArc` (`:407-425`): fraction ≤ 0.001 → nothing; ≥ 0.999 → full circle; else one SVG-style arc
with large-arc flag `sweep > π`.

---

## 2. The live toolbar window — `Views/RadialWindow.axaml(.cs)`

Window flags (`RadialWindow.axaml:6-20`):

```
WindowDecorations=None, Background=Transparent, TransparencyLevelHint=Transparent,
Topmost=True, ShowInTaskbar=False, ShowActivated=False, CanResize=False,
SizeToContent=WidthAndHeight
```

`ShowActivated=False` is critical: during a file drag the window must not steal focus from the file
manager, or the OS drag breaks. The window is **created once and reused** (show/hide) so it appears
instantly.

Content: a `Panel` (`Root`) with `Background="{Binding HitBackground}"` and `AllowDrop=True`, holding
the `DonutMenu` with `Opacity="{Binding DonutOpacity}"`.

**Clever/fragile trick** (`RadialViewModel.cs:23-33`, `:57-58`): while waiting for a drag to enter, the
donut is invisible (`DonutOpacity = 0`) but the panel background is `#01000000` (alpha 1/255) —
practically invisible but not fully transparent, so Windows still routes the drag to this window and
the files can arrive. After files enter, background goes back to `Transparent`.

Drag & drop handlers (`RadialWindow.axaml.cs:28-79`):

- **DragEnter**: collect local file paths (`TryGetLocalPath`, keep only `File.Exists` → folders and
  virtual items are ignored). If none → effect None, stop. Else reset the event counter, raise
  `FilesEntered(files)`, then run DragOver.
- **DragOver**: `hit = donut.HitTest(pos)`; set external highlight to the slice (or -1) and
  `centerHot = hit is Center`. Effect = **Copy** only if the data contains files and the hit is a
  segment; otherwise None. Comment: "Always COPY. Reporting MOVE would make Explorer delete the
  source files after the drop." (`:50`)
- **DragLeave**: log summary, clear highlight.
- **Drop**: hit test, clear highlight, effect Copy if on a segment, raise `Dropped(hit, files)`.
- **Esc** key → `EscapePressed`.
- `DragSummary(events, hit, lit)` logs one line per drag for support ("the drag never reached the
  donut" vs "it did but nothing lit up").

---

## 3. The controller — `Services/RadialController.cs`

State: `_window` (lazy), `_visible`, `_dropped`, `_editing`, `_progressJob`, `_lastRingJob`,
`_handOffTimer`. Constant `HandOffAfter = 5 s` (`:39`) — long enough for LibreOffice to finish a
typical document so the user sees ✓ where they dropped.

### 3.1 Drag mode

```
ShowForDrag(x, y)                                  // :56-64
  ReleaseProgressToToast()      // a running ring job moves to the toast immediately
  window = EnsureWindow(); _dropped = false
  vm.ResetForDrag()             // no items, title "Bring files here", DonutOpacity 0, HitBackground #01000000
  donut.AllowContextEdit = false
  PlaceAndShow(window, x, y, activate: false)

window.FilesEntered(files)                         // :124-130
  ignore if click mode or same files as already loaded (path comparer)
  vm.Load(files, clickMode: false); donut.PlayOpenAnimation()

window.Dropped(hit, files)                         // :131-138
  _dropped = true
  Segment → Convert(index, files or vm.Files);  anything else → Close()

OnDragGestureEnded()  (mouse button released, from the hook)   // :81-91
  if visible and drag mode: after 250 ms, if still not dropped → Close(animate)
  // the drop arrives around the same time as the button release, hence the delay
```

A gesture without files (dragging text, a rubber-band selection with Ctrl held) never shows anything
and closes on release.

### 3.2 Click mode

```
ShowForFiles(x, y, files)                          // :67-78
  if files empty → return
  ReleaseProgressToToast(); _dropped = false
  vm.Load(files, clickMode: true); donut.AllowContextEdit = true
  PlaceAndShow(window, x, y, activate: true); donut.PlayOpenAnimation()
```

Closing in click mode: Esc, click in the hole (`CenterInvoked`), or the window losing activation
(`Deactivated`, unless the preset editor popup is open, `_editing`) (`:145-149`). Right-click on a
slice opens the preset editor dialog owned by the radial window; on save the library is saved and the
donut reloaded (`EditPresetAsync`, `:257-276`).

### 3.3 Placement — `PlaceAndShow`, `:278-309`

```
screen  = screen containing (x,y) or primary
scaling = macOS ? 1 : screen.scaling          // input coordinates are physical pixels (except macOS points)
side    = 2 × (OuterRadius + 7 + 14);  sidePx = ceil(side × scaling)
pos     = (x - sidePx/2, y - sidePx/2) clamped into screen.WorkingArea
Motion.Refresh()
window.Position = pos; show if hidden; window.Position = pos again   // showing can change DPI scale
platform.RaiseTopmost(handle)   // Windows can push a "topmost" window below others (seen after Photos opened)
_visible = true; if activate: window.Activate()
```

Linux `RaiseTopmost` (`Filee.Platform.Linux/X11WindowSystem.cs:85-101`) sends client messages
`_NET_WM_STATE` (add `_NET_WM_STATE_ABOVE`) and `_NET_RESTACK_WINDOW` to the root window.

### 3.4 Convert and progress — `:166-255`

```
Convert(index, files):
  if index out of range, slice disabled, or no files → Close()
  job = conversions.Start(files, preset, showInToast: false)   // ring shows it first
  if null → Close() else ShowProgress(job)

ShowProgress(job):
  donut.BeginProgress(); donut.SetProgress(job.Progress / 100)
  subscribe job.PropertyChanged; start 5 s timer → if still running: HandOffToToast()

on job change: SetProgress; when IsRunning becomes false:
  outcome = Completed→Succeeded, Cancelled→Cancelled, else Failed
  Failed → conversions.ShowInToast(job)          // the toast explains the error
  detach; donut.CompleteProgress(outcome, HideWindow)

Close() while in progress mode → HandOffToToast() if a job is still attached (Esc / click elsewhere)
ProgressCancelRequested → job.Cancel()
ProgressClicked        → if last ring job has outputs: reveal first output in file manager
```

### 3.5 What the donut shows — `ViewModels/RadialViewModel.cs`

`Load(files, clickMode)` (`:61-95`):

1. Appearance from settings: `OuterRadius = clamp(settings.Donut.OuterRadius, 100, 240)`,
   `HoleRatio`, `SliceOpacity = settings.Donut.Opacity`.
2. **Profile**: `ProfileSelector.Select(profiles, files)` (`Filee.Core/Profiles/ToolbarProfile.cs:65-91`):
   - distinct lower-case extensions of all files;
   - first **normal** profile whose extension list contains *every* extension;
   - else first **mixed** (`IsFallback`) profile with a non-empty extension list containing every extension;
   - else the catch-all: first mixed profile with no extensions → first mixed profile → first profile.
3. **Formats** = distinct recognised formats of the files (`FormatRegistry.Detect`).
4. **Presets** = profile's `PresetIds` in order, resolved via the store (unknown ids dropped).
   Max 12 slices per profile (`ToolbarProfile.MaxSlices`).
5. **Items**: label = localized preset name; caption = target format name, or, for "same as source"
   presets, up to two source format names joined with "/"; `IsEnabled = availability.Check(...) is null`.
6. Centre title: one format → `"{count} × {FORMAT}"` (e.g. "3 × JPG"); otherwise `"{count} files"`.
   Subtitle: click mode → "Esc to close"; drag mode → none.

`ShowHint(index, centerHot)` (`:98-108`) updates the subtitle on hover: centre hot → "Drop here to
cancel"; enabled slice → "Drop → {label}" or "Click → {label}"; disabled → its reason; nothing → back
to "Esc to close" (click mode) or empty.

**Availability** (`Services/PresetAvailability.cs`): a slice is enabled if *at least one* of the
source formats can be routed to the target. Special cases: "combine into one archive" presets accept
any file; PDF-merge presets accept PDFs. If not possible, it asks the engine downloader whether an
optional package would make it possible → reason "Needs {package}"; else "no route"; if no format
was recognised → "unsupported source".

---

## 4. Gestures and triggers — `Services/Triggers/GestureDetector.cs`, `TriggerService.cs`

### 4.1 Settings model — `Filee.Core/Settings/TriggerGesture.cs`

```text
ModifierKeys (flags): None=0, Ctrl=1, Alt=2, Shift=4, Meta=8   // left/right treated the same
TriggerMouseButton: None, Left, Right, Middle, X1, X2
TriggerKind: Drag | Hold | KeyChord
TriggerScope: FileManager | Anywhere
TriggerGesture {
  id: 8-hex random; enabled = true; kind = Drag; modifiers = Ctrl; button = Left
  key: string?            // SharpHook key name without "Vc", e.g. "Space" (KeyChord only)
  drag_threshold: int = 0 // px; 0 = system default
  hold_ms: int = 450
  scope = FileManager
}
Defaults():  [ Drag, Ctrl (Alt on macOS), Left ],  [ KeyChord, Ctrl+Alt, key "Space" ]
```

Validation when saving from the UI (`TriggersPageViewModel.cs:95-103`): threshold clamped 0..200,
hold 150..3000 ms, button forced to None for KeyChord.

### 4.2 The state machine (pure, thread-safe, unit-tested)

```
MouseDown(button, x, y, mods):
  remember last (x,y); clear armed; clear late-rejected; if paused → return
  g = first enabled gesture with kind ∈ {Drag, Hold}, g.button == button, g.modifiers == mods (EXACT match)
  if g and ScopeFilter(g, x, y): armed = (g, button, x, y, token)
  if g.kind == Hold: request a timer of g.hold_ms with that token

MouseMove(x, y, mods, heldButton):
  if armed:  // DetectDragPastThreshold
     thr = g.drag_threshold > 0 ? g.drag_threshold : SystemDragThreshold
     if |dx| <= thr and |dy| <= thr → nothing
     else armed = null; if g.kind == Drag → fire DragStarted(g, x, y); dragActive = true
          (a Hold gesture that moves is just cancelled)
  else:      // DetectLateModifier
     if !dragActive && !lateRejected && !paused && heldButton != None && mods != None:
        g = first Drag gesture with button == heldButton and modifiers == mods
        if g and ScopeFilter → fire DragStarted (user started dragging, THEN pressed Ctrl — common in Explorer)
        if g and scope fails → lateRejected = true (don't re-check on every move)

MouseUp(...): clear armed; if dragActive → fire DragEnded(x,y); dragActive = false

HoldElapsed(token): if armed with same token and kind Hold → fire HoldTriggered(g, armed.x, armed.y)

KeyDown(key, mods): if not paused: first KeyChord gesture with key (case-insensitive) and exact mods,
   in scope at the last known cursor → KeyChordTriggered(g, lastX, lastY)
```

Events are raised **outside the lock**. Token = `TickCount64 ^ (x × 7919) ^ y`.

### 4.3 The hook service — `TriggerService.cs`

- Uses **SharpHook** (`EventLoopGlobalHook`, a wrapper over **libuiohook**). Hook callbacks must
  return fast: Windows silently removes a low-level hook whose callback takes > ~1 s, so all UI work is
  posted to the UI thread (`:1-3`, `:143`).
- Mapping: Button1..5 → Left, Right, Middle, X1, X2; modifier mask → Ctrl/Alt/Shift/Meta; held button
  read from the event mask; key code name minus `"Vc"` prefix.
- `MouseDragged` and `MouseMoved` both go to `MouseMove`, which also updates `LastCursor`.
- **Scope filter** (`InScope`, `:133-141`): look up the process owning the window under the point.
  Reject if it is Filee itself or in the excluded-apps list (names trimmed, `.exe` removed,
  case-insensitive). `Anywhere` → accept; `FileManager` → accept only if the platform says the window
  under the point is the file manager or desktop.
- `Apply(settings)`: configure detector with enabled gestures (only Drag kinds when the platform does
  not support selection shortcuts), set paused, excluded set.
- Hold timer: `Task.Delay(ms)` then `HoldElapsed(token)` on a thread-pool thread.
- Hook failure (e.g. macOS without Accessibility): log, store `Error`, notify status; the app keeps
  working via drop zone and context menu. macOS polls every 2 s for the permission and starts the
  hook by itself.
- Debug: env `FILEE_TRACE_GESTURES=1` logs each press and scope decision.

App wiring (`App.axaml.cs:177-206`): `DragStarted → radial.ShowForDrag(x,y)`, `DragEnded →
radial.OnDragGestureEnded()`, `SelectionGesture` (hold or key chord) → read the file manager's
current selection (`platform.GetFileManagerSelection()`), and if non-empty → `ShowForFiles`. A
re-entrancy flag drops a second selection gesture while one is pending. On failure it opens Settings →
Triggers and shows the error.

### 4.4 Linux specifics (very relevant: our user runs Arch on Wayland)

From `Filee.Platform.Linux/LinuxPlatformServices.cs` and `X11WindowSystem.cs`:

- `IsWaylandSession` = `XDG_SESSION_TYPE == "wayland"` or `WAYLAND_DISPLAY` set.
- **Global pointer gestures require X11**: `SupportsGlobalPointerGestures = X11 available` = Linux
  && not Wayland && `DISPLAY` set. **On Wayland Filee disables the drag gesture entirely.** The UI
  message (`en.json`, `triggers.linux_wayland`): "Cursor-following gestures are unavailable in this
  Wayland session. Use an X11 session for drag gestures, or convert through file manager actions and
  the drop zone."
- `SupportsSelectionShortcut = false` on Linux (no shared "selected files" API). Instead the first
  enabled keyboard shortcut is written as a Thunar accelerator for the Filee custom action.
- `SystemDragThreshold = 8` px on Linux (4 elsewhere / fallback).
- Window under point: `XGrabServer`, walk from the root with `XTranslateCoordinates` down to depth 32,
  reading `_NET_WM_PID` (→ `/proc` process name) or else `WM_CLASS` at each level; ungrab.
- File manager names: `thunar, nautilus, org.gnome.nautilus, nemo, dolphin, caja, pcmanfm, pcmanfm-qt, xfdesktop`.
- Reduced motion: `gsettings get org.gnome.desktop.interface enable-animations` == `false`.
- Reveal in file manager: `gdbus call --session --dest org.freedesktop.FileManager1
  --object-path /org/freedesktop/FileManager1 --method org.freedesktop.FileManager1.ShowItems
  "['file:///…']" ""`, fallback `xdg-open <dir>`.
- Context-menu integration writes (`LinuxDesktopIntegration.cs:93-140`): autostart
  `~/.config/autostart/filee.desktop` with `Exec=<exe> --background`; Thunar sendto `.desktop`,
  Nautilus script `~/.local/share/nautilus/scripts/Filee` (absolutises relative args, `exec <exe>
  --convert "$@"`), Nemo action, KDE service menu `~/.local/share/kio/servicemenus/filee.desktop`,
  and a Thunar `uca.xml` custom action — all running `<exe> --convert %F`.

So on Linux, the *real* entry point for most users is: file manager action → `--convert files` →
click-mode donut at the last known cursor.

---

## 5. Drop zone — `Views/Pages/HomePage.axaml(.cs)`

A big rounded `Border` (min height 220, 2 px accent-soft border, `SoftRadiusLarge`, upload icon,
"Drop files here", hint "Or hold {Ctrl} while dragging files in your file manager…").
DragEnter/Over → effect Copy if the data has files, add class `over` (accent-soft background,
accent border, `scale(1.015)` with a BackEaseOut 0.3 s transition). Drop → local existing files only
(folders ignored), convert drop point to **screen** coordinates, then
`RadialController.ShowForFiles(screenX, screenY, files)` — i.e. the donut pops up in click mode
exactly where the files were dropped (`HomePage.axaml.cs:123-137`).

Below it: "Recent conversions" history list (out of scope here).

---

## 6. Theming — `Services/ThemeService.cs`, `Styles/Soft.axaml`, `Services/Motion.cs`

Settings (`Filee.Core/Settings/AppSettings.cs:21-38`):

```text
ThemeSettings { mode: System|Light|Dark = System; accent: "#7F77DD"; corner_radius: 16; reduce_motion: bool? = null (follow OS) }
DonutSettings { outer_radius: 150; hole_ratio: 0.42; opacity: 0.96 }
```

Palette tokens (`Soft.axaml:13-36`):

| Token | Light | Dark |
|---|---|---|
| PageBrush | `#F6F5F1` | `#1B1B19` |
| SurfaceBrush | `#FFFFFF` | `#262624` |
| SurfaceAltBrush | `#EFEDE6` | `#31312E` |
| SubtleBorderBrush | `#16000000` | `#1CFFFFFF` |
| TextPrimaryBrush | `#1F1E1D` | `#F1EFE8` |
| TextSecondaryBrush | `#6B6A66` | `#AEACA4` |
| DangerBrush | `#C73A39` | `#F09595` |
| SuccessBrush | `#0F7A5A` | `#5DCAA5` |
| WarningBrush | `#9A5B0C` | `#EF9F27` |
| CardShadow | `0 1 3 #12000000, 0 6 18 #0D000000` | `0 1 3 #40000000, 0 8 24 #33000000` |

Donut colour mapping (`Soft.axaml:209-219`): Slice = Surface, SliceBorder = SubtleBorder,
Center = Surface, Accent = Accent, OnAccent = OnAccent, Foreground = TextPrimary,
SecondaryForeground = TextSecondary, Danger, Success.

Runtime-derived tokens (`ThemeService.Apply`, `:24-63`):

```text
Accent        = parse(theme.accent) or #7F77DD
AccentSoft    = accent @ 0.18 alpha;  AccentFaint = accent @ 0.09
AccentStrong  = shade(accent, -0.25)
OnAccent      = luminance(accent) > 0.6 ? #1F1E1D : white
   luminance  = (0.2126 R + 0.7152 G + 0.0722 B) / 255
   shade(c,a) = a >= 0 ? v + (255 - v)·a : v·(1 + a)     (per channel, rounded)
SystemAccentLight1..3 = shade +0.15/+0.30/+0.45;  Dark1..3 = shade -0.15/-0.30/-0.45
r = clamp(corner_radius, 0, 28): SoftRadius r, SoftRadiusSmall round(0.55 r), SoftRadiusLarge round(1.35 r)
Font family: "Noto Sans, <CJK order>, Segoe UI, Malgun Gothic, Microsoft YaHei, PingFang SC, Apple SD Gothic Neo, sans-serif"
   CJK order = Noto Sans SC first when UI language starts with "zh", else Noto Sans KR first
```

Accent swatches on the Theme page: `#7F77DD #1D9E75 #D85A30 #378ADD #D4537E #BA7517 #639922 #5F5E5A`,
plus a free `#RRGGBB` text box (ignored until it parses).

Style details: base font 14 px; h1 24 semibold; h2 16 semibold; caption 12; buttons get a springy
`scale(0.95)` on press (0.22 s BackEaseOut); nav items slide `translateX(3px)` on hover; page content
fades in over 0.28 s while sliding up 10 px.

**Reduced motion** (`Motion.cs`): one global `Enabled` flag. `Configure(userChoice?, systemReader)`.
When following the system, the value is re-read (`Refresh()`) every time a window is shown, because
"a moment when Windows reported animations off (a remote session, waking from sleep) must not switch
them off for good". Each window gets class `reduced-motion`, which strips transitions/animations in
the styles; the donut reads `Motion.Enabled` directly.

---

## 7. i18n — `Services/LocalizationService.cs`, `Assets/i18n/*.json`

- Flat JSON dictionaries, **dotted keys**, one file per language: `en.json`, `ko.json`, `zh-CN.json`
  (564 keys each, grouped by prefix: `app, nav, tray, home, donut, toast, error, engine, profile, preset,
  format, general, update, triggers, toolbar, presets, theme, engines, about, common, feedback, setup, watch`).
- Placeholders are .NET composite format: `"{0} × {1}"`. `Format` falls back to the raw string on a
  format error.
- Lookup: current language → English → **the key itself**.
- Language setting: `"auto" | "en" | "ko" | "zh-CN"`. `auto` resolves from the OS UI culture:
  starts with `ko` → ko, `zh` → zh-CN, else en (macOS reads its preferred-language list because Finder
  launches get no `LANG`).
- Live switching: every key (union of English + current) is published into an app resource
  dictionary so XAML `{DynamicResource key}` updates instantly; code-built labels are refreshed by
  rebuilding the current settings page on `LanguageChanged`.
- Donut-related English strings (`en.json:35-44`):

```json
"donut.center_drag": "Bring files here",
"donut.files_same": "{0} × {1}",
"donut.files_mixed": "{0} files",
"donut.drop_to": "Drop → {0}",
"donut.click_to": "Click → {0}",
"donut.cancel": "Drop here to cancel",
"donut.close_hint": "Esc to close",
"donut.edit": "Edit preset",
"donut.empty": "Drag presets here",
"donut.same_format": "Same format"
```

Tray strings: `tray.open "Open Filee"`, `tray.pause "Pause shortcuts"`, `tray.resume "Resume
shortcuts"`, `tray.quit "Quit"`, `tray.update "Update to {0}"`, `tray.tooltip "Filee: hold {0} and drag files"`.

---

## 8. Settings window and pages — `Views/MainWindow.axaml`, `ViewModels/MainWindowViewModel.cs`

Main window 1080×760 (min 820×560), centred; left nav 230 px with logo, tagline, nav list; bottom:
update button (only when an update exists), GitHub + Sponsor links, version text. Right side: the
page in a ScrollViewer (max width 980). **Closing the window hides it**; Filee keeps running in the
tray (`MainWindow.axaml.cs: OnClosing` cancels and hides).

Pages are **recreated on every visit** so they always show the latest settings, and old pages are
disposed (they may hold event subscriptions). Nav entries (key → page):

| Key | Page | What it edits |
|---|---|---|
| `home` | HomePage | drop zone, recent conversions, macOS accessibility reminder |
| `general` | GeneralPage | language, start with system, file manager integration, default output folder, keep file dates, history on/off/clear, preset library import/export/reset, open data folder |
| `triggers` | TriggersPage | list of gestures (kind, Ctrl/Alt/Shift/Meta toggles, button, recorded key, drag distance, hold time, scope), excluded apps (tags with running-app suggestions), hook status + retry, platform notice |
| `toolbar` | ToolbarPage | profiles list (searchable by name or extension), profile name, normal (extension chips) vs mixed (check list), live donut editor + palette of unused presets, appearance sliders |
| `presets` | PresetsPage | preset library (separate RE doc) |
| `watch` | WatchFoldersPage | watch folders (separate RE doc) |
| `theme` | ThemePage | mode, accent swatches + hex, softness (radius 0..28), reduce motion, live donut preview |
| `engines` | EnginesPage | engine downloads/own copies (separate RE doc) |
| `about` | AboutPage | version, update check, links |

Every setting change calls `store.SaveSettings()` immediately (no Save button); `App` listens to
`SettingsChanged` and re-applies theme, language, triggers, engine priority and system integration
(`App.axaml.cs:290-318`).

### Donut editor details (`Views/Pages/ToolbarPage.axaml.cs`, `ViewModels/Pages/ToolbarPageViewModel.cs`)

- Sources: a palette chip (add) or a donut slice (reorder / remove). Targets: a slice slot (insert at),
  the hole or empty ring (append), the palette box (remove).
- Drag starts after 5 px. A floating "ghost" chip follows the pointer (+12, +12).
- While dragging, `FindTarget` computes the slot with one **extra** slice when adding
  (`SlotAt(point, count + 1)`) so the gap does not move away from under the pointer; the donut opens
  a gap with springs (`SetDropPreview`) and shows a translucent dashed slice of the dragged preset.
- Double-click a chip = append. Right-click chip or slice / pencil = open preset editor.
- Max 12 slices (`toolbar.max_reached` message). Centre shows profile name and `"{used} / 12"`.
- Appearance sliders: outer radius 110..220, hole 25..65 %, opacity 50..100 %; saved rounded
  (`HoleRatio = round(x)/100`).
- Normal profiles are always kept before mixed profiles in the list; the last mixed profile cannot be
  deleted or turned normal (Filee always needs a catch-all).
- New profile default: name "New profile", presets `["to-pdf", "to-png"]`.

Inconsistencies to be aware of: the settings comment says hole ratio 0.2–0.7, the slider is
0.25–0.65, the control clamps 0.2–0.75; outer radius slider is 110–220 but the view model clamps
100–240. Pick one range in our port (suggest 0.25–0.65 and 110–220 in UI, clamp the same in core).

---

## 9. App lifecycle, tray, single instance

### 9.1 Startup order — `Program.cs`, `App.axaml.cs`

```
Main(args):
  options = CommandLine.Parse(args)
  instance = SingleInstance.Acquire()
  if --quit or --uninstall-cleanup: forward "--quit" to the first instance, wait ≤ 20 s for it to exit, cleanup, exit
  if not first instance: ForwardToFirst(args); exit 0
  run Avalonia with ShutdownMode = OnExplicitShutdown     // lives until tray Quit

OnFrameworkInitializationCompleted:
  build DI container (AppHost); store.Load(); localization attach; ApplySettings
  wire TriggerService → RadialController; triggers.Start()
  SettingsChanged → ApplySettings
  create tray; create toast window (hidden); start watch folders; warm up LibreOffice after 10 s
  instance.ArgumentsReceived → HandleCommandLine (on UI thread); instance.StartListening()
  on exit: dispose hook, tray, watch folders (≤ 5 s)
  HandleCommandLine(startup options, firstLaunch: true)
  WatchForUpdates (first check after 20 s)
```

`HandleCommandLine` (`App.axaml.cs:320-362`):

- `--quit` → shutdown.
- `--convert` files present → **debounce 350 ms** collecting files (Explorer may start one process
  per selected file), then `OpenPendingFiles`: distinct paths, position = `TriggerService.LastCursor`;
  if that is (0,0) (hook not running, e.g. Wayland!) → main window position + (200, 200). Then
  `ShowForFiles`.
- otherwise show the main window unless started with `--background` on first launch (a second launch
  without args always shows it); first run opens Home and possibly the engine setup window.

### 9.2 Command line — `Services/CommandLine.cs`

```
(no args)                 open main window
--background              tray only (autostart uses this)
--settings                open main window
--convert a b ...         donut for these files (only args that exist as file or directory, made absolute;
                          unknown "--x" options ignored)
--convert-list <file>     one path per line (UTF-8, BOM ok, blanks skipped); file deleted afterwards only if it
                          is %TEMP%/Filee-convert-*.txt (so a crafted command line can't delete other files)
--install-engines=a,b     installer: download these engine packages (empty = user chose none)
--start-with-windows=on|off (alias --start-with-system), --context-menu=on|off   (on/1/true/yes, off/0/false/no)
--quit, --uninstall-cleanup
Options are case-insensitive; "--name=value" form supported.
```

### 9.3 Single instance — `Services/SingleInstance.cs`

- Name = `"Filee-" + first 12 hex chars of SHA256(UTF-8 user name)` (a stable hash; .NET string hash
  is randomized per process).
- Named mutex (Windows: `Local\` = per session; Unix: current user only, across sessions).
- First instance runs a loop: named-pipe server (current user only), accepts one connection, reads a
  **JSON array of strings** (the args), raises `ArgumentsReceived`. Errors from half-written clients
  are ignored.
- Later instance: connect with a 3 s timeout, write the JSON args, exit. Failures are silently ignored.

### 9.4 Tray — `Services/TrayService.cs`

Menu: `[Update to x.y.z]` (only when an update exists, shows "Downloading… 42%" while busy),
`Open Filee`, `Pause shortcuts` / `Resume shortcuts` (toggles `settings.Paused`, saved), separator,
`Quit` (explicit shutdown). Left-click on the icon opens the main window. Tooltip:
`"Filee: hold Ctrl and drag files"` (modifier text of the first enabled Drag gesture) or "paused" text.
Refreshes on language change, settings change and update-state change.

### 9.5 Toast — `Views/ToastWindow.axaml(.cs)`, `Services/ConversionService.cs`, `ViewModels/JobViewModel.cs`

360 px wide, height to content, borderless, transparent, topmost, no taskbar, **not activated**,
placed at the primary screen's working-area bottom-right with a 12 px margin, re-placed on size
change. Visible exactly while `ToastJobs` is non-empty (visibility updated via a posted call, not
inside `CollectionChanged`, to avoid a duplicate-card bug, issue #60). Cards slide in from +40 px.

Job card: title `"{file or N files} → {preset name}"`, progress %, status text (Done / "{n} of {m}
failed" / Conversion failed / Cancelled / Skipped (file exists)), buttons Show (reveal first output),
Cancel, Dismiss, Retry failed. Finished jobs are removed after **5 s** (success) or **15 s** (errors).
After 3 fully successful conversions a one-time "Is Filee helping you?" card appears.

Progress updates from worker threads are **coalesced**: an atomic "refresh pending" flag ensures at
most one queued UI refresh per job.

---

## 10. Update check — `Services/UpdateService.cs`, `Services/ReleaseNotes.cs`

- First check 20 s after start, then every **6 h**, only while `settings.CheckForUpdates` (default true).
- `GET https://api.github.com/repos/<owner>/<repo>/releases/latest` with headers
  `User-Agent: Filee/<version>` and `Accept: application/vnd.github+json`, 20 s timeout. Non-2xx
  (e.g. 404 before the first release) → logged, ignored. Offline errors → logged, ignored.
- Version compare: strip leading `v`, cut at `-` or `+`, pad `"1"` → `"1.0"`, compare major.minor.build.
- If newer: fetch `.../releases?per_page=30`, skip drafts and prereleases, collect the notes of every
  version in `(current, latest]`, newest first.
- Release notes carry a hidden block GitHub does not render:

  ```
  <!-- update
  level: recommended          # required | recommended | optional (default recommended)
  en: Excel page breaks carry over to PDF
  ko: ...
  zh: ...
  -->
  ```

  Overall level = max level of the versions in range. Behaviour: **Required** → card every start;
  **Recommended** → card once per version (`settings.NotifiedUpdateVersion`); **Optional** → only the
  sidebar button.
- In-app install is Windows-only (Inno Setup): find assets ending `-win-Setup.exe` and
  `-SHA256SUMS.txt`, download with a 30 s *stall* timeout (reset on every chunk), verify SHA-256
  against the `sha256sum`-style line, run `Setup.exe /SILENT /NORESTART`. Everything else (portable,
  Linux, macOS) just opens the release page / website.

---

## 11. Other small but useful things

- **Logging**: file logger into `<dataDir>/logs` (`Services/FileLogger.cs`), level Information.
- **Composition root** `Services/AppHost.cs`: one singleton per service; views call `AppHost.Get<T>()`.
- **Fonts**: embedded Noto Sans / Noto Sans KR / Noto Sans SC.
- **Startup failure dialog** on macOS VMs without a display link (`StartupFailure.cs`).

---

## 12. Clever bits and fragile bits (summary)

Clever:
1. Constant-pixel gaps between slices by computing separate half-angles at each radius.
2. Fake soft shadow with a radial gradient ring (no blur).
3. Sub-stepped springs so late frames don't explode.
4. A frame-loop watchdog that force-finishes animations if frames stop coming.
5. The alpha-1 background so the invisible window still receives the OS drag.
6. Late-modifier detection: Ctrl pressed *after* the drag began still opens the donut.
7. Ring-to-toast hand-off: quick jobs finish in place (✓), slow ones move to the toast after 5 s.
8. Always report COPY on drop so the file manager never deletes sources.
9. 350 ms debounce of `--convert` launches to merge one-process-per-file context menus.
10. Editor computes slots with one extra slice so the gap stays under the pointer.

Fragile:
1. Topmost windows getting pushed down (Windows); re-raise every show.
2. Showing a window can change its DPI scale; set the position twice.
3. Global hooks must return quickly (Windows removes slow hooks).
4. The drop and the mouse-up race; 250 ms grace before closing.
5. Wayland: no global pointer, no window positioning, no "window under cursor" → no drag gesture.
6. `LastCursor` is (0,0) when the hook is not running; fallback position near the main window.

---

## 13. Porting to Rust (MorphRing: Rust core + CLI, Tauri v2 UI, Linux-first)

### Architecture mapping

| Filee | MorphRing |
|---|---|
| `DonutGeometry` (pure math) | `morphring-core::ring::geometry` — pure Rust, unit-tested; also export the same math to the frontend (port to TS, or compile to WASM later) |
| `DonutMenu` rendering + animation | Tauri webview: one `<svg>` (or `<canvas>`) drawn from JS/TS. Wedge path = SVG `A` arcs exactly as in §1.3. Springs in a `requestAnimationFrame` loop |
| `RadialWindow` | A second Tauri window `ring`: `decorations: false`, `transparent: true`, `alwaysOnTop: true`, `skipTaskbar: true`, `focus: false`, `resizable: false`, `visible: false`, created at startup and reused |
| `RadialController` | Rust side state machine in the Tauri app (`ring.rs`), exposes commands/events to the ring window |
| `RadialViewModel.Load` | Core function `ring_items(files, profiles, presets, catalog) -> RingModel { items, center_title, subtitle }` — pure and testable |
| `GestureDetector` | Pure Rust struct, same state machine, `Mutex`-free if driven from one hook thread; emits enum events over a channel |
| `TriggerService` (SharpHook) | `rdev` (`listen`) on X11 / Windows / macOS; **not available on Wayland**. Alternative later: `evdev` (needs `input` group) for raw events, still no cursor position or window-under-cursor on Wayland |
| `SingleInstance` | `tauri-plugin-single-instance` (D-Bus on Linux) — its callback receives argv + cwd; reuse the same `--convert` parser |
| `CommandLine` | `clap` in the CLI crate; the GUI binary parses the same flags (`--background`, `--settings`, `--convert`, `--convert-list`) |
| `TrayService` | Tauri v2 `tray-icon` feature (`TrayIconBuilder`, `Menu`) — on Linux needs `libayatana-appindicator` |
| autostart | `tauri-plugin-autostart` or write `~/.config/autostart/morphring.desktop` with `Exec=<exe> --background` like Filee |
| `LocalizationService` | Flat dotted-key JSON per language in `assets/i18n/`; a tiny `Localizer { current, english }` with fallback current → en → key; `{0}` placeholders via simple replace. Ship strings to the frontend as one JSON blob per language |
| `ThemeService` | CSS custom properties on `:root` (+ `[data-theme=dark]`), accent-derived tokens computed in Rust or JS with the exact formulas in §6; `prefers-color-scheme` for System mode |
| `Motion` | CSS `prefers-reduced-motion` + a `reduce_motion: Option<bool>` setting; on Linux also `gsettings get org.gnome.desktop.interface enable-animations` |
| `UpdateService` | `ureq`/`reqwest` + `semver` against GitHub releases API; later `tauri-plugin-updater` |
| Toast | either a third Tauri window bottom-right, or (simpler for v0.1.0) desktop notifications via `tauri-plugin-notification` / `notify-rust` |
| Settings store | `serde` JSON at `$XDG_CONFIG_HOME/morphring/settings.json` (`directories` crate), with the same defaults |

### Pitfalls

1. **Wayland is the user's daily driver.** On Wayland a client cannot: read the global cursor
   position, place its own window at absolute coordinates, or query the window under the cursor.
   Tauri's `set_position` is a no-op there. Options, in order of effort:
   - (a) v0.1.0: **don't follow the cursor on Wayland.** Open the ring window centred on screen (or
     let the compositor place it), in click mode. Entry points: file manager action
     (`morphring --convert %F`), the drop zone in the main window, and the CLI.
   - (b) Run the GUI under XWayland (`GDK_BACKEND=x11`) — positioning works for our window, and
     cursor position is known while it is over X11 windows; but most Wayland-native file managers won't
     report under-cursor info and the global hook still sees nothing from Wayland clients.
   - (c) Later: `gtk-layer-shell` (wlroots/KDE, not GNOME) for an overlay surface; or the
     `org.freedesktop.portal.GlobalShortcuts` portal for a keyboard chord.
2. **Tauri needs `webkit2gtk-4.1` on Linux, which is not installed yet** (`sudo pacman -S webkit2gtk-4.1`,
   plus `libayatana-appindicator` for the tray). Without it the Tauri crate won't build. Keep the CLI
   building without Tauri (separate crate, separate feature) so the core + CLI can ship regardless.
3. **Transparent windows on Linux** need a compositor and `transparent: true` + a transparent
   `html, body { background: transparent }`. WebKitGTK may draw a white background for a frame; keep
   the window hidden until the first render.
4. **Do not activate the ring window in drag mode** (`focus: false`), or the source drag can break;
   activate in click mode so Esc and focus-loss closing work.
5. **Drag-and-drop into the webview**: use Tauri v2's window drag-drop events
   (`DragDropEvent::Enter { paths, position } / Over { position } / Drop { paths, position } / Leave`)
   — they give physical-pixel positions, perfect for running `hit_test` in Rust. The webview's own
   HTML5 DnD gives no real paths. Always treat a drop as a copy.
6. **Coordinates**: positions from hooks are physical pixels; Tauri window sizes are logical. Convert
   with the monitor's scale factor exactly like `PlaceAndShow`, and clamp into the monitor work area.
7. **Spring sub-stepping**: cap a single integration step at 1/120 s, and clamp frame dt to 50 ms.
8. **Hook threads**: `rdev::listen` blocks its thread; run it on a dedicated thread, send events over
   a channel, never do UI work in the callback.
9. **Debounce `--convert`** (350 ms) inside the single-instance callback — some file managers start one
   process per file.
10. Use `File.Exists`-equivalent filtering (`Path::is_file`) for dropped paths; folders are ignored by
    Filee's donut (they're only used by archive presets elsewhere).

### Suggested Rust types

```rust
pub struct RingItem { pub id: String, pub label: String, pub caption: Option<String>,
                      pub enabled: bool, pub disabled_reason: Option<String> }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit { Outside, Center, Segment(usize), EmptyRing, EditButton(usize), ProgressRing, ProgressCancel }

pub fn span(n: usize) -> f64;                      // 2π / n, 2π for n == 0
pub fn mid_angle(i: usize, n: usize) -> f64;       // i * span(n)
pub fn point_at(c: (f64, f64), r: f64, a: f64) -> (f64, f64);
pub fn angle_of(c: (f64, f64), p: (f64, f64)) -> f64;
pub fn index_at(a: f64, n: usize) -> Option<usize>;
pub fn hit_test(c: (f64, f64), p: (f64, f64), r_in: f64, r_out: f64, n: usize, slack: f64) -> Hit;
pub fn wedge_svg_path(c: (f64, f64), r_in: f64, r_out: f64, mid: f64, span: f64, gap_px: f64) -> String;

pub struct Spring { pub value: f64, pub velocity: f64 }
impl Spring { pub fn step(&mut self, target: f64, dt: f64, k: f64, c: f64) -> bool /* still moving */; }

pub enum GestureEvent { DragStarted { x: i32, y: i32 }, DragEnded { x: i32, y: i32 },
                        HoldTriggered { x: i32, y: i32 }, KeyChord { x: i32, y: i32 } }
```

---

## 14. v0.1.0 candidates (realistic for "ship today")

Must-have (small, high value):

1. **`ring::geometry` in core** (span, mid_angle, point_at, angle_of, index_at, hit_test,
   wedge path) with unit tests copied from the formulas above. ~150 lines, zero dependencies.
2. **`ring_model(files)` in core**: profile selection (normal → mixed-with-extensions → catch-all),
   max 12 slices, caption rules, centre title `"{n} × {FMT}"` / `"{n} files"`, enabled/disabled by
   "does a route exist for at least one source format". Testable without any UI.
3. **Click-mode ring window in Tauri**: transparent, undecorated, always on top, SVG donut with the
   exact wedge shape, 3 px gaps, hover pop-out (7 px + accent fill), labels + captions, centre text,
   Esc / click hole / focus loss closes, click slice → convert. Open animation (easeOutBack, 0.42 s,
   0.028 s stagger) is cheap and gives the "feel"; honour `prefers-reduced-motion`.
4. **Entry points that work on Wayland**: `morphring --convert <files>` (GUI) forwarded through
   `tauri-plugin-single-instance`, and a **drop zone** in the main window that opens the ring. Ring
   is centred on screen on Wayland, at the cursor on X11/Windows when a position is known.
5. **Simple progress feedback**: after a click, swap the ring to a percentage/✓/! state (even without
   the collapse animation), auto-hide after ~0.9 s on success, keep errors visible with the message.
6. **Settings JSON with Filee's defaults**: donut radius 150, hole 0.42, opacity 0.96; theme
   System/#7F77DD/radius 16; language "auto".
7. **Tray** with Open / Quit (Pause can wait until gestures exist), main window close = hide.
8. **i18n plumbing, English only**: dotted keys, fallback to key; reuse Filee's donut/tray wording.
9. **Light/dark tokens** from §6 as CSS variables, accent-derived `OnAccent` via the luminance rule.

Defer to later releases:

- Global modifier+drag gesture (X11/Windows/macOS via `rdev`), late-modifier detection, hold and key
  chords, excluded apps, scope filter (needs window-under-cursor per platform).
- Edit-mode donut editor with spring layout and drop preview; profile editor; preset editor.
- Ring-to-toast hand-off, toast window, retry, history.
- File manager integrations (Nautilus script, Nemo action, Dolphin service menu, Thunar uca.xml) —
  although a single Nautilus/Dolphin entry is a cheap stretch goal since it is just a file write.
- Update check with release-notes levels (a minimal "newer tag exists" check is a ~40-line stretch goal).
- Frame-loop watchdog (only needed if we see lost frames in WebKitGTK).
