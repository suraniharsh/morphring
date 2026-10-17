# 08: What the shipped Filee binary says (static analysis with REA)

Scope: the **released build** of Filee 1.11.0 (the latest release on 2026-10-17), checked with REA's static
.NET tools, then compared against `docs/PLAN-v0.2-parity.md` (PX-01..PX-189) and the source-based notes
01-07. The earlier notes were written against the **v1.10.3** source (see the headers of 06 and 07). Most of
what follows is either (a) a check that the shipped build does what the source says, or (b) something that
changed between 1.10.3 and 1.11.0, or that the audit and plan got wrong.

Clean-room rule: this file describes behaviour, values and wording that the binary shows. It does not
reproduce decompiled code. Filee is MIT licensed ("Copyright (c) 2026 Myeonggu Kim (KnifeLemon) and Filee
contributors", `LICENSE` in the repo), so taking values and behaviour from it is fine. Credit stays in
THIRD-PARTY notices as before.

Nothing was run. REA reads the files without loading or executing them. No Filee binary, installer or
engine was started.

---

## 1. Inputs

| Item | Value |
|---|---|
| Release | `v1.11.0`, "Filee 1.11.0", published 2026-10-17T05:14Z |
| Assets downloaded | `Filee-1.11.0-win-Portable.zip`, `Filee-1.11.0-linux-x64.tar.gz`, every `*SHA256SUMS.txt` |
| Not downloaded | `win-Setup.exe` (Inno Setup wrapper of the same files), linux-arm64, osx-x64, osx-arm64 tarballs |
| SHA-256 win portable zip | `c1efb31a39f1bd6e25e12749e85bb63dd5a2f14306df9fa5eec3e6b987416a7b` (matches `Filee-1.11.0-SHA256SUMS.txt`) |
| SHA-256 linux-x64 tar.gz | `a4f4836bcf503a03999ab0783fb06a23fadddb5869784b9b433b33fdaab30570` (matches its SUMS file) |
| Source reference | `github.com/KnifeLemon/Filee` at `ff56a0c` (= tag `v1.11.0`), cloned to `_tools/filee-src` and only read |
| Build commit stamped in the binary | the `AssemblyInformationalVersion` of every Filee assembly is `1.11.0+ff56a0cd1e2516516df3737d8d1cfb7ac69738dc`, which is the tag commit |
| Runtime | `net10.0`, bundled Microsoft.NETCore.App 10.0.13 (self-contained) |
| REA | `rea-agents` 6.5.0 from npm, installed locally in `_tools/rea-cli` (CLI only; `rea setup` was not run and no agent or MCP config was changed) |

Managed assemblies analysed (SHA-256 prefix, MVID prefix):

| File | Version | Code | SHA-256 | MVID |
|---|---|---|---|---|
| linux/Filee.dll (app, 30 MB, mostly `!AvaloniaResources`) | 1.11.0.0 | CIL | `bb4ac245e81d32b1` | `f8523607` |
| linux/Filee.Core.dll | 1.11.0.0 | CIL | `73fd5e73afde0661` | `cde12350` |
| linux/Filee.Engines.dll | 1.11.0.0 | CIL | `90b65733886db142` | `03d4a3da` |
| linux/filee-cli.dll | 1.11.0.0 | CIL | `c9d42d093d39b81b` | `e7479ad0` |
| linux/Filee.Platform.{Linux,MacOS,Windows}.dll | 1.11.0.0 | CIL | `edf6e9eb` / `e2c4078c` / `db6c4a43` | `c74b7feb` / `33e7a242` / `8b2cb62b` |
| win/Filee.dll, Filee.Core.dll, Filee.Engines.dll, filee-cli.dll, Filee.Platform.*.dll | 1.11.0.0 | CIL + ReadyToRun | `9873630c` / `52589d17` / `8eb99d28` / `8466d7cc` / ... | different build |
| win/FileeExplorerMenu.dll | (native C, 10 KB) | not managed | `88ab3f4c0f3946ca` | n/a |
| win/FileeExplorerMenu.msix | sparse package, 24 KB | zip | `957182e9cd77f2b6` | n/a |

"Plugins": Filee has no plugin assemblies. The extra pieces are the native Explorer command DLL plus MSIX
(Windows), and the bundled engine programs `engines/7zip` (7-Zip 26.03: `7z.exe`/`7z.dll` on Windows,
`7zz`/`7zzs` on Linux) and `engines/rhwp` (rhwp 0.8.7). Everything else is downloaded on demand from the
embedded manifest (section 3.3).

## 2. Commands used

All paths absolute in the real runs; shortened here. `R=_tools/rea-cli/node_modules/.bin/rea`.

```sh
gh release download v1.11.0 -R KnifeLemon/Filee -p 'Filee-1.11.0-win-Portable.zip' \
   -p 'Filee-1.11.0-linux-x64.tar.gz' -p '*SHA256SUMS.txt'      # into _tools/filee-bin, then sha256sum
unzip -q ...win-Portable.zip -d win ; tar xzf ...linux-x64.tar.gz -C linux
$R inspect-managed-artifact   <dll> --json        # identity, R2R vs CIL, references, resources, attributes
$R inspect-managed-members    <dll> --json        # types, methods, call edges, user strings (ldstr literals)
$R inspect-managed-members    Filee.Engines.dll --type <T> ... --json   # in 21 batches, see 2.1
$R inspect-managed-native-boundaries <dll> --json # P/Invoke declarations
$R compare-managed-members    linux/<dll> win/<dll> --format json        # Linux build vs Windows build
```

Our own small Python scripts (kept in the session scratchpad, run with `python3 -I`) then:
mapped every `ldstr` and integer/float constant to the method that loads it; compared every literal of
4+ characters against the v1.11.0 source tree; byte-matched the embedded resources against the source files;
and matched the single-quoted message texts in the plan against the binary's literals and `en.json`.
`strings`/`readelf` were used on the native files REA's managed tools do not cover.

### 2.1 REA blockers and workarounds

- **Filee.Engines.dll is too big for one pass.** Whole-assembly member inspection stops with
  `resource-limit: Managed metadata exceeds the 67108864 byte representation budget`. REA's documented way
  out is selecting types. Type names came from the v1.11.0 source and the shipped `Filee.Engines.xml`, plus
  compiler-generated nested names (`<X>d__N`, `<>c`, `<>c__DisplayClassN_M`) taken from the `#Strings` heap.
  21 batches of 25 names covered **823 of 1,143 types and 6,298 of 7,747 methods**, with 5,219 distinct
  literals. Every batch reported `coverage: complete` for what it selected. The rest are small nested
  helper types, so absence claims about Engines are weaker than for the other assemblies.
- **Native parts.** `FileeExplorerMenu.dll`, the `libuiohook*.so` files and the engine executables have no
  CLI metadata, so REA reports `managed member inspection is unavailable` for them. `strings` and `readelf -d`
  were used instead. REA's native tools (Ghidra/Rizin providers) were not needed for these small questions.
- **Nothing else was missing.** `ps` (procps) is on PATH. No `dotnet`, ILSpy or monodis was needed or used.

## 3. Shipped binary vs GitHub source

### 3.1 No hidden behaviour

- **Literal drift: none.** Every string literal in all seven assemblies (Engines as far as covered) appears
  in the v1.11.0 source. The only "misses" were compiler-made: record `ToString` fragments (`", Name = "`),
  System.Text.Json source-generated camelCase property names, XAML `avares://` URIs and Avalonia namespace
  tables, and string constants the compiler joins from several `+` pieces.
- **Embedded resources are byte-identical to the repo**: `Filee.Engines.engines.json` (9,646 B, SHA-256
  `4d04eada...`), `Filee.Engines.Hwpx.blank.hwpx` (8,005 B), `Filee.Cli.en.json` (40,449 B, the app's
  `en.json`), and inside `!AvaloniaResources` the three `i18n/*.json`, `Sponsors/sponsors.json` and the icons.
- **Only in the binary**: the Noto Sans / Noto Sans KR / Noto Sans SC fonts under `/Assets/Fonts/` (downloaded
  at build time by `build/fetch-fonts.ps1`, not committed; Latin TTF from notofonts at a pinned commit, CJK
  OTF subsets from noto-cjk Sans2.004) and the `AssemblyMetadata("FileeRelease","true")` attribute. That flag is
  only set by `-p:FileeRelease=true` in CI, and it is what lets a published build register Explorer and
  startup entries. A source build never does.
- **No telemetry.** The only HTTP clients are the update service and the engine downloader. The only fixed hosts
  are `api.github.com/repos/KnifeLemon/Filee/releases...`, `github.com/KnifeLemon/Filee` (repo, `/issues/new/choose`,
  `/releases/latest`), `https://filee.sh/` (download page and online CLI help), the engine URLs in
  `engines.json`, and the five sponsor links. No analytics, crash upload or usage ping exists.

### 3.2 Linux build vs Windows build

`compare-managed-members` Linux vs Windows: Core 3,163 methods unchanged, 0 changed; App 7,164 unchanged;
Platform.Linux/Windows unchanged. Only `filee-cli` differs, in 6 methods, all of which hold multi-line help
text: the Windows build was compiled from a CRLF checkout, so its help and summary text uses `\r\n`. No
behaviour depends on the OS at compile time. All OS branches are runtime checks. The Windows build adds
ReadyToRun native code next to the same CIL.

### 3.3 Bundled and pinned versions (from `deps.json` and `engines.json`)

Libraries: Avalonia 12.1.3, Magick.NET-Q8 14.17.2 (ImageMagick in process, not the `magick` CLI), PDFtoImage
5.2.1 on PDFium 147.0.7690, PDFsharp 6.2.4, PdfPig 0.1.16, ACadSharp 3.8.0, SharpGLTF.Core 1.0.7, Svg.Skia
5.1.1, SkiaSharp 3.119.4, HarfBuzzSharp 8.3.1.3, Markdig 1.4.0, MimeKitLite 4.18.1, MsgReader 6.2.0,
OpenMcdf 3.3.0, ExcelDataReader 3.9.0, RtfPipe 2.0, SharpCompress 1.0.0, Tomlyn 2.10.1, YamlDotNet 18.1.0,
UTF.Unknown 2.7.0, Unhwp 0.13.0 (native `libunhwp.so`/`unhwp.dll` through a NuGet wrapper), SharpHook 8.0.0,
CommunityToolkit.Mvvm 8.4.2, Tmds.DBus.Protocol 0.94.1.

Engine downloads (same as v1.10.3, so 02 section 3.2 is still right): rhwp 0.8.7, 7-Zip 26.03, Pandoc 3.12,
FFmpeg 9.0.2 (Gyan shared build on Windows, BtbN `linux64-gpl-9.0` on Linux), calibre 9.15.0, and on Windows
only LibreOffice 26.2.6 (+ H2Orestart 0.7.14, Temurin JRE 21.0.12.1) and Ghostscript 10.08.0 (+ VC runtime
14.44, both from conda-forge).

## 4. Findings with evidence

Notation: `Type.Method` names what REA showed the value in. "Source" means it was double-checked in the
v1.11.0 source.

### 4.1 Format catalog (Core `FormatRegistry` static constructor)

- The table has **245 entries**: 244 file formats plus the `folder` pseudo-format (category Archive, empty
  extension, used by extract/zip-all). The release note's "244 formats" leaves out `folder`. PX-01's format
  list matches the binary id for id (image ids `apng mng qoi dds pcx cur xpm xbm wbmp pam pfm sgi ras miff dcm
  pcd exr hdr xcf raw`, RAW with 25 camera extensions, and so on).
- ImageMagick (`ImageEncoder` constructor): 35 readable ids, 28 writable. EMF is read on Windows only, WMF everywhere.
  Max output side: jpg/jpeg 65,500, webp 16,383, **avif 16,384**, others unlimited (`ImageEncoder.MaxSide`).
  Reader density for vector inputs is 36-1200 dpi, at least 150 for emf/wmf (`ImageReader.Settings`).

### 4.2 Presets, profiles, schema (Core `BuiltInData`, `SettingsMigrations`, `UserDataStore`)

- **101 built-in presets**, not 94: base 25, `ImageToolPresets` 7, HWP 1, spreadsheet 2, catalog 42
  (array length 42), data 6, codec/subtitle 10, 3D 6, trace 2. The 7 new ones are `rotate-left`
  (`{name}_left`), `rotate-right` (`{name}_right`), `rotate-180` (`{name}_180`), `flip-horizontal`
  (`{name}_mirrored`), `flip-vertical` (`{name}_flipped`), each with target `*` (same format) and quality 95,
  plus two **group presets** `rotate` (members rotate-left, rotate-right, rotate-180) and `flip`
  (flip-horizontal, flip-vertical) whose target is the literal `group`. They sit after `remove-background`
  and are on **no** donut by default.
- **18 built-in profiles** (images, pdf, office, hwp, mixed, spreadsheets, presentations, text, ebooks, video,
  audio, vector, archives, cad, fonts, data, subtitles, models), which agrees with PX-08. The images donut is `to-png,
  to-jpg, to-webp, to-pdf, trace-svg, trace-line-art, to-tiff, to-bmp, to-ico, half-size`. Video: `to-mp4,
  mp4-720, mp4-hevc, to-webm, to-mov, to-gif, to-mp3, extract-subtitles, to-mkv, to-avi`.
- **Settings schema is 17** (`AppSettings` constructor stores 17; migrations end at 17). v17 inserts the rotate/flip
  tools and groups after `remove-background`. It also **repairs** a preset with one of those ids that an
  older Filee re-saved: it turns it back into a group, or puts the rotation or flip back. This is the only
  migration that edits an existing preset.
- New preset JSON fields: `kind` (Conversion/Group, read leniently by whole name), `subPresetIds`, `isGroup`
  (derived). `image.rotation` (None/Right90/Left90/Rotate180) and `image.flip` (None/Horizontal/Vertical, done
  after rotation), with a derived `turnsOrMirrors`.
- Defaults in constructors: ImageOptions quality 90, resizePercent 50, longEdge 1920, width 1200,
  **backgroundTolerance 25**, background `#FFFFFF`, icoSizes 256/128/64/48/32/16. PdfOptions renderDpi 150.
  TracingOptions colors 16, detail 50. OutputRule subfolder `converted`, pattern `{name}`. History keeps
  200 entries. Library files are `settings.json`, `presets.json`, `profiles.json`, `history.json`. An unreadable
  file is copied to `.broken` and defaults are used. Writes go to `.tmp` and are then moved into place.
- Settings defaults: language `auto`, theme accent `#7F77DD`, corner radius 16, donut outer radius 150, hole
  0.42, opacity 0.96. Default engine priority (26 ids, in order): magick, pdfium, pdfsharp, rhwp, unhwp, markdown,
  spreadsheet, ooxml, hwpx-writer, docx-writer, pdf-text, email, data, vector, icns, font, cad, model3d,
  archive, ebook, subtitle, ffmpeg, pandoc, ghostscript, calibre, libreoffice.
- Triggers: `TriggerGesture.Defaults` builds **two** gestures: a drag (Ctrl, or Alt on macOS, left button) and
  a **key chord Ctrl+Alt+Space**. Default hold time is 450 ms. Drag threshold 0 means the system value (8 px
  on Linux, `GetSystemMetrics` on Windows).
- UI ranges (Toolbar page sliders): size 110-220, hole 25-65 %, opacity 50-100 %. Theme corner radius 0-28.
  The ring renderer itself clamps the hole ratio to 0.2-0.75 (`DonutMenu.InnerRadius`).
- Watch folders: settle default 2 s. Names ignored: those starting `~$`, those ending
  `.tmp .temp .part .partial .crdownload .download .opdownload .!ut .filepart`, and `desktop.ini` / `thumbs.db`.

### 4.3 Donut groups and rotate/flip (new in 1.11.0, not in the plan)

From the app assembly and the 85 i18n keys added since 1.10.3 (564 -> 649 keys in each of en, ko, zh-CN):

- A **group** is one slice that stands for up to **6** presets (Core `Preset.MaxSubPresets`). Resting on it or
  moving outward fans the members out on a sub-ring around it ("Move outward to choose"). The caption is
  "Group of {0}". Leaving the donut or closing it folds the group. A click or drop on a neighbour's outer half
  converts with that neighbour, and a ring opened with Enter waits for the mouse to come from the hole. An
  empty group says "No presets left in this group. Edit it in Settings → Donut toolbar".
- **Group editor window** (480x560, min 400x420): name, member list "Presets ({0} / {1})", Add (preset
  picker), Move up/down, Remove, "A group holds up to {0} presets.", Delete with "Delete the group "{0}"? Its
  presets stay." The Toolbar page has a "New group" button.
- `UserDataStore.EnsureConsistency` keeps groups consistent: members missing from the library are dropped, and
  a group's target stays `group`.
- **CLI**: `--preset <group>` fails with `'<x>' is a group on the donut, not a conversion. Use one of: a, b`
  or, when the group is empty, `... and has no presets left. Run 'filee presets' for the list.`
- **Rotate/flip** is an image step done once, by the engine that turns the image. A JPEG turned this way keeps
  its quality. A long image turns its pages, not the whole strip. PDF merge and TIFF join turn every page. A
  non-image source fails with error key `error.cannot_turn` = "Rotate and flip work on images only". In the preset
  editor, rotation also shows for PDF and is dropped where it can't apply (commit history; strings
  `presets.rotation.*`, `presets.flip.*`). On the ring the `.short` labels are used (Left / Right / 180° /
  Horizontal / Vertical).

### 4.4 CLI (`filee-cli`, verified, small corrections)

Verbs: `convert`, `watch`, `formats`, `presets`, `help`, `version` (help also via `-h`, `--help`, `/?`;
version via `-v`, `--version`). Flags: `-t/--to`, `-p/--preset`, `-o/--output`, `-r/--recursive`,
`-q/--quality` (1-100), `--overwrite`, `--skip`, `--keep-dates`, `--json`, `--quiet`, and for watch `--move-originals`
and `--settle <s>` (default 2). Exit codes 0 / 1 / 2 / 3. JSON keys: `preset, target, converted, skipped, failed,
files[source, state, outputs, error]`. This all agrees with 07. **There is no `presets export|import|reset` in
the Filee CLI.** Export/import/reset exist only on the GUI General page (`general.export`, `general.import`,
`general.imported` = "Imported {0} items", `general.reset_library`). The default export file name is
`filee-presets.json`.

App switches (`Filee.App.Services.CommandLine`): `--background --settings --convert --convert-list --quit
--uninstall-cleanup --install-engines= --start-with-system= (alias --start-with-windows=) --context-menu=`.
Values: on/off/yes/no/true/false/1/0. Timeouts (Program static constructor): **--quit waits up to 20 s**, and
forwarding to the running instance waits 30 s. Polling is every 100 ms, and 1.11.0 stops waiting once the
other process has already ended.

### 4.5 External tool command lines (Engines `MediaEncoding`, `FfmpegRunner`, others)

Checked against PX-22..PX-32. Most values agree. These are exact values the plan leaves vague or gets wrong:

- Every run starts with `-hide_banner -nostdin -nostats -v error -y` and reports progress with
  `-progress pipe:1` (`out_time_us`). The GIF palette pass uses `-stats_enc_pre pipe:1 -stats_enc_pre_fmt
  frame_time={t}`.
- x264: `-preset medium -crf 18/23/28 -pix_fmt yuv420p`. x265: `-preset medium -crf 22/26/30 -x265-params
  log-level=error`, plus `-tag:v hvc1` in mp4/m4v/mov. SVT-AV1: `-preset 8 -crf 26/32/40`. libaom fallback:
  `-b:v 0 -crf 26/32/40 -cpu-used 6 -row-mt 1 -tiles 2x2`. VP9: `-b:v 0 -crf 24/32/38 -row-mt 1 -deadline good
  -cpu-used 4`. Xvid: `mpeg4 -tag:v XVID -g 250 -bf 2`. wmv2: `-g 250`. Theora: `-q:v 8/6/4`.
- Missing encoder text: "This FFmpeg has no HEVC encoder (libx265). Filee's FFmpeg download has one." and
  "This FFmpeg has no AV1 encoder (libsvtav1 or libaom-av1). Filee's FFmpeg download has one."
- Broadcast frame rates: **8 values**, 23.976, 24, 25, 29.97, **30**, 50, 59.94, **60**. VOB: `-b:v
  8000/6000/4000k -maxrate 9000k -minrate 0 -bufsize 1835k -g 18 (NTSC) / 15 (PAL) -bf 2`, rate 30000/1001 or
  25, muxer `-muxrate 10080000 -packetsize 2048`. mpg/mxf: `-g 15 -bf 2`. DV: NTSC yuv411p, PAL yuv420p.
  Containers: `+faststart` for mp4/m4v/mov/3gp/3g2/m4a/m4b, `-mpegts_m2ts_mode 1` for m2ts, `-id3v2_version 3`
  for mp3, `-rf64 auto` for wav. The source-only tags `major_brand`, `minor_version` and `compatible_brands` are not copied.
- Subtitles: picture subtitles fail with "The video's subtitles are pictures (Blu-ray PGS, DVD or DVB
  subtitles), not text. Picture subtitles can't be turned into text without OCR.". No track gives "The video has
  no subtitle track.".
- GIF: frames at fps 15/12/10 with long-side caps 640/480/320 by quality, scaled with lanczos. `palettegen
  max_colors` is followed by `paletteuse` with dither `sierra2_4a`, `bayer:bayer_scale=4` or `=5`, and
  `diff_mode=rectangle`, then `-loop 0`. APNG: `-c:v apng -pix_fmt rgb24 -pred mixed -plays 0`. The square-pixel
  step rounds the width to even (`trunc(iw*sar/2)*2`, then `setsar=1`).
- Still picture to video: input `-flags2 +icc_profiles -loop 1 -framerate 30`, `premultiply` then scale to
  BT.709 TV range (`out_color_matrix=bt709:out_range=tv,format=yuv420p`), `-tune stillimage -colorspace
  bt709`. Size cap **35,651,584 pixels (8192x4352)** and 16,384 per side, lanczos.
- PDF long image: at most 200,000,000 pixels per stacked part (`PdfiumConverter.Stack/Split`).
  Multi-frame pdf->gif/apng/mng uses a 100 cs (1 s) delay per page.
- LibreOffice PDF/A: `pdf:<family>_pdf_Export:{"SelectPdfVersion":{"type":"long","value":"2"}}`. This agrees with PX-48.
  Per-worker profile directories `libreoffice-profiles/worker-N`. H2Orestart is found under
  `share/uno_packages` or `share/extensions` (Resources/... on macOS).
- Ghostscript runs with `GS_OPTIONS`, `GS_LIB` and `GS_FONTPATH` cleared. Edges eps/ps to and from pdf, and to png, each cost 10.
- Engine lookup (`EngineEnvironment`): bundled `engines/` next to the app (or the repo when `Filee.slnx` is
  found), then the per-user download root `<LocalAppData>/Filee/engines`, then PATH plus `/opt/homebrew/bin`,
  `/usr/local/bin`, `/usr/bin`, `/bin`, `/opt/local/bin`, `/usr/lib{,64}/libreoffice/program`, `/opt/libreoffice*`,
  `/opt/calibre`, and the `.app` bundles on macOS.
- Friendly read errors (exact): XCF "This GIMP file can't be read. Filee reads 8-bit XCF files saved without
  "better but slower compression"; in GIMP, export the picture as PNG or PSD instead. (...)". DICOM "This DICOM
  image is compressed in a way Filee can't read (more than 8 bits per sample, or an unusual compression).
  Export it uncompressed from the viewing software and try again. (...)". AI without PDF part: "This
  Illustrator file was saved without PDF compatibility, so it is PostScript. Install Ghostscript in Settings →
  Engines to convert it, or save it in Illustrator with "Create PDF compatible file"." DRM e-book: "This book is
  protected with DRM (copy protection), so it cannot be converted. Filee does not remove DRM; ...".

### 4.6 Update check (App `UpdateService`, `ReleaseNotes`)

- Calls `GET https://api.github.com/repos/KnifeLemon/Filee/releases/latest` with User-Agent `Filee`,
  `Accept: application/vnd.github+json` and a 20 s timeout. Release notes come from `releases?per_page=30`,
  skipping draft and prerelease, so the notice can sum up every skipped version. The check interval is **6 h**
  (source; v1.10.3 also had 6 h).
- The level comes from a hidden HTML comment at the top of each release body: an `<!-- update` block with a
  `level:` line (required / recommended / optional) and one-line summaries per language (`ko:`, `en:`, `zh:`).
  No block counts as **recommended**. Optional shows only the sidebar button, recommended shows a card once
  per version, required shows a highlighted card at every start.
- "Download" opens **`https://filee.sh/`**, not the GitHub release page. The in-app installer path (Windows:
  pick `-win-Setup.exe` plus `-SHA256SUMS.txt` from the release assets, check the hash, run it, detect
  installed vs portable by `unins000.exe`) is the blocked part, as the plan already says.
- Default `CheckForUpdates` is true in Filee. MorphRing deliberately defaults it to false (offline rule).

### 4.7 Shell integration (Platform assemblies)

- **Windows 11 menu**: the MSIX is **built at build time and shipped** (`FileeExplorerMenu.msix` next to
  `FileeExplorerMenu.dll`). Registration runs elevated (`runas`) with `powershell.exe -NoLogo -NoProfile
  -NonInteractive -Command`, wrapped with `$ErrorActionPreference='Stop'` and an error file in %TEMP%:
  `Add-AppxPackage -Path <msix> -ExternalLocation <install dir> -AllowUnsigned -ForceUpdateFromAnyVersion`.
  It is removed with `Get-AppxPackage -Name Filee.ExplorerMenu | Remove-AppxPackage`. Exit code 1223 means UAC
  was cancelled, and the wait has a time limit in minutes. Identity: `Filee.ExplorerMenu`, publisher `CN=Filee,
  OID.2.25.311729368913984317654407730594956997722=1` (the unsigned-namespace OID that `-AllowUnsigned`
  requires), family `Filee.ExplorerMenu_c73vxh346rtay`, version fixed at 1.0.0.0, MinVersion 10.0.22000,
  MaxVersionTested 10.0.26100, `desktop5:ItemType Type="*"` (files only), CLSID
  `6DB0E670-807F-4D22-BE3F-20492D0F4AB1`, COM surrogate, STA. State is **On** only when the registered external
  location is this install folder and the package version is at least the manifest version. Otherwise it is
  **Outdated**.
- The native command DLL reads its **title from `%APPDATA%\Filee\explorer-menu.txt`** (written by the app in
  the UI language), falling back to "Convert with Filee". Icon `Filee.exe,0`. It starts `Filee.exe --convert
  "<paths>"`, or writes `%TEMP%\Filee-convert-<pid>-<tick>-<n>.txt` and passes `--convert-list` for long
  selections.
- Classic verb `HKCU\Software\Classes\*\shell\Filee` (MUIVerb, Icon, `MultiSelectModel=Player`, command
  `"<exe>" --convert "%1"`). Send To shortcut `Filee.lnk`. Run key `HKCU\...\Run\Filee` = `"<exe>" --background`.
- Linux: Thunar `uca.xml` action `filee-convert` (patterns `*`, all five file classes, icon document-save-as,
  `--convert %F`) plus an optional accelerator in `accels.scm`. Also Nautilus script `Filee`, Nemo
  `filee.nemo_action`, KDE `kio/servicemenus/filee.desktop`, Thunar `sendto/filee.desktop`, and autostart
  `filee.desktop` with `--background`. Files are written mode 0644 (scripts 0755) through `.tmp` and rename.
  Reveal-in-folder uses `org.freedesktop.FileManager1.ShowItems` through `gdbus`, then `xdg-open`.
- macOS: LaunchAgent `com.filee.app.plist` (`--background`, `LimitLoadToSessionType=Aqua`). Context menu
  through an **in-process NSServices provider** (`convertFiles:userData:error:`) switched on with `defaults
  write pbs NSServicesStatus` and `pbs -update`. The old `~/Library/Services/Filee.workflow` is deleted. The
  Finder selection is read with JXA through `osascript` (30 s timeout).
- **Linux global hook, a nuance for 06 section 1.2.** The tarball ships SharpHook's newer libuiohook as a small
  dispatcher, `libuiohook.so`, plus three backends: `-xrecord`, `-x11` and `-wayland`. The Wayland one links
  `libinput.so.10`, `libudev.so.1` and `libwayland-client.so.0` and reports "No permission to open ... A udev
  rule which grants access to it is most likely missing". The dispatcher picks a backend from
  `XDG_SESSION_TYPE` / `WAYLAND_DISPLAY`. Filee never reaches the Wayland backend: its gate
  (`X11WindowSystem.IsAvailable` = Linux, not a Wayland session, and `DISPLAY` set) turns gestures off first.
  So the shipped behaviour matches 06, but "there is no other backend" is no longer true of the library.
  MorphRing's evdev path on Wayland goes beyond Filee, and Filee's packaging does not have the input-group
  problem only because it never tries.

## 5. Plan corrections

"Contradicts" means the plan states something the shipped binary does differently. "Missing" means the
binary has a feature with no PX task.

| PX | Kind | Plan says | Shipped binary (1.11.0) | Proposed change |
|---|---|---|---|---|
| PX-01 | exact value | (no count) | 245 registry entries = 244 formats + `folder` | Pin the count in the formats test |
| PX-02 | contradicts | background_tolerance default 20 | default **25** | Use 25 |
| PX-02 | missing | option fields for bg removal, codecs, tracing | also `image.rotation` (None/Right90/Left90/Rotate180) and `image.flip` (None/Horizontal/Vertical, after rotation); `kind`, `subPresetIds` | Add the fields here (skeleton) or in PX-190/191 |
| PX-04 | contradicts | default triggers: one Ctrl+left drag (Alt on macOS) | two: that drag **and** key chord Ctrl+Alt+Space; hold 450 ms; drag threshold 0 = system | Add the chord default (inert on Linux, where Filee has no selection API) |
| PX-04 | exact value | ranges only | theme accent `#7F77DD`, radius 16; donut 150 / 0.42 / 0.96; renderer clamps hole to 0.2-0.75 | Pin defaults in the settings test |
| PX-07 | contradicts | 94 built-ins | **101** (adds rotate-left/right/180, flip-horizontal/vertical, groups `rotate`, `flip`) | Count test to 101, or 94 now and 101 after PX-191 |
| PX-09 | contradicts | schema 1 to 16; never changes an existing preset | schema **17**; v17 also repairs rotate/flip presets saved by an older version | Cover v17. Allow the one repair (restore kind/sub-ids or rotation/flip when the saved preset lost them) |
| PX-23 | contradicts | broadcast rate nearest of 23.976/24/25/29.97/50/59.94 | 8 rates, adding **30 and 60** | Use the 8-rate table |
| PX-26 | wording | "This FFmpeg was built without HEVC (libx265)..." | "This FFmpeg has no HEVC encoder (libx265). Filee's FFmpeg download has one." (+ AV1 twin) | Use these texts (product name swapped) |
| PX-26 | exact value | "CRF per Filee" | SVT-AV1 preset 8, CRF 26/32/40; libaom `-b:v 0 -cpu-used 6 -row-mt 1 -tiles 2x2`; x265 `log-level=error` | Spell out |
| PX-30 | wording | 'This video has no subtitles.' | "The video has no subtitle track." plus the picture-subtitle text in 4.5 | Use Filee's texts |
| PX-31 | exact value | `scale=iw*sar:ih,setsar=1` | width rounded to even: `trunc(iw*sar/2)*2`; GIF fps 15/12/10, caps 640/480/320, dithers sierra2_4a / bayer 4 / bayer 5, `diff_mode=rectangle` | Spell out |
| PX-32 | exact value | "zscale/colormatrix", cap 8192x4352 | `-framerate 30`, `premultiply`, `scale out_color_matrix=bt709:out_range=tv`, `-flags2 +icc_profiles`; cap is a **pixel count** 35,651,584, 16,384 per side | Match |
| PX-34 | wording | "This XCF file uses features ImageMagick cannot read", "This file is not an image ImageMagick can read", "...DICOM file uses compression..." | XCF and DICOM texts in 4.5 | Use Filee's texts |
| PX-45 | exact value | JPEG 65,500, WebP 16,383, PNG 200 MP | also AVIF 16,384; others unlimited; stacking cap 200,000,000 pixels per part | Add AVIF |
| PX-143 | contradicts | marker 'Update: required\|recommended\|optional'; once a day; opens release page | hidden `<!-- update ... level: ... -->` block with per-language summaries, missing = recommended; optional = button only, recommended = card once, required = card every start; **every 6 h**; notes from last 30 releases; download page `https://filee.sh/` (MorphRing: its own page) | Rewrite the spec |
| PX-166 | contradicts | --quit polls up to 10 s | **20 s** (forwarding 30 s, poll 100 ms); since 1.11.0 does not wait for an already-ended process | Use 20 s |
| PX-169 | exact value | title 'Convert with MorphRing', >32 paths -> list file | title read from `<config>/explorer-menu.txt` (localized by the app), fallback text; list file `<tmp>/<App>-convert-<pid>-<tick>-<n>.txt`; files only | Read the title file. Write it on language change |
| PX-170 | contradicts | build the sparse package at runtime; `Add-AppxPackage -Register`; Outdated = package version vs app | MSIX built at **build time** and shipped; `Add-AppxPackage -Path <msix> -ExternalLocation <dir> -AllowUnsigned -ForceUpdateFromAnyVersion`; publisher needs the unsigned-namespace OID; Outdated = other folder or package version < manifest version | Build the MSIX in the release job (PX-171) and register it with -Path |
| PX-181 | wording / scope | CLI `presets export/import/reset`; 'Imported N items (A added, R replaced).' | Filee has these on the **GUI** General page only; text "Imported {0} items" | Keep as a MorphRing extra (mark it "beyond Filee") or move to PX-129 only |
| PX-189 | wording | 'Download X in Settings > Engines' | "Needs {0}. Install it in Settings → Engines" | Use Filee's text |
| 06 notes | doc fix | "Linux: XRecord ... no other backend" | libuiohook in SharpHook 8 has x11/xrecord/wayland(libinput) backends; Filee gates gestures off on Wayland before any backend loads | Amend 06 section 1.2 (no task) |

### New tasks (PX-190+)

Each is one commit, the same style as the other PX tasks.

| PX | Title | Depends on | Content |
|---|---|---|---|
| PX-190 | Core: group presets | PX-07 | `PresetKind::{Conversion, Group}`, `sub_preset_ids` (max 6), target `group`, JSON `kind`/`subPresetIds`/`isGroup` (lenient enum by whole name); library consistency drops missing members; a group never routes. Tests: round trip of a Filee 1.11 `presets.json` with groups; a member deletion leaves the group consistent |
| PX-191 | Core: rotate/flip options, five tool presets and the Rotate/Flip groups | PX-190, PX-09 | `ImageOptions.rotation` / `flip` (flip after rotation), the 5 tool presets (target same-as-source, quality 95, the name suffixes in 4.2) plus groups `rotate` and `flip` after remove-background on no donut; library migration step equal to Filee v17 including the repair case; built-in count 101 |
| PX-192 | Engines: turn and mirror images once | PX-191 | Apply rotation/flip in the image pipeline (native image engine and magick) only on the step that writes the image. JPEG keeps the source quality; a long image turns its pages; PDF merge and multi-page TIFF turn every page; a non-image source fails with key `cannot_turn` "Rotate and flip work on images only"; PDF render rotates pages |
| PX-193 | App: groups on the ring | PX-190, DR-01 | A group slice fans its members out on a sub-ring on rest or outward motion ("Move outward to choose", caption "Group of {0}", empty-group text); fold on leave or close; click or drop on a neighbour's outer half converts with it; a ring opened by keyboard waits for the pointer to leave the hole |
| PX-194 | App: group editor and "New group" | PX-190, PX-13 | Group editor window (name required, members "n / 6", add via preset picker, move up/down, remove, delete with "its presets stay") and a "New group" button on the Toolbar page; group keys `group.*` in en/ko/zh-CN |
| PX-195 | CLI: refuse group presets like Filee | PX-190 | `--preset <group>` exits 2 with "'<x>' is a group on the donut, not a conversion. Use one of: ..." or the empty-group text; `presets` lists groups with target `group` |
| PX-196 | i18n: the 85 keys Filee added in 1.11.0 | PX-148 | Port `donut.group_*`, `group.*`, `preset.rotate_*`/`flip_*` (+ `.short`), `presets.rotation.*`, `presets.flip.*`, `presets.trace_*`, `presets.video_codec.*`, `presets.audio_codec.*`, `presets.background_tolerance*`, `presets.keep_enclosed_background*`, `profile.{data,models,subtitles}`, `format.category.{Data,Model3D,Subtitle}`, `error.cannot_turn`; key-parity test at 649 Filee keys mapped |

## 6. What this pass did not cover

- `Filee-1.11.0-win-Setup.exe` (Inno Setup script behaviour is in the source `installer/Filee.iss`, covered by 06/07)
  and the macOS/arm64 archives were not downloaded. The managed assemblies are built the same way, but their
  hashes were not checked.
- About 19 % of Filee.Engines methods (small nested helpers) were outside the type batches. The literal drift check
  covers them only through the source side.
- No runtime behaviour was observed (no binary was run), so timing, UI layout and engine output were not checked
  here. They remain as the notes in 02-05 describe them.
