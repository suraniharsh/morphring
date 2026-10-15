# 02 — Engines: contract, tool discovery, image / media / PDF / vector / text

Reverse-engineering notes on how Filee's conversion engines work, written so the engines can be rebuilt in Rust
without reading the C#.

Scope: `src/Filee.Engines/{Infrastructure,Magick,Media,Heic,Icns,Vector,Text,Pdf}/**` and `src/Filee.Engines/EngineRegistry.cs`,
plus the parts of `src/Filee.Core/Conversion/*` that define the engine contract and call it.

All paths are relative to the Filee repository root. Line numbers are from the snapshot we reviewed.

---

## 1. The big picture

Filee has no single "convert" function. Instead it keeps a **graph of formats**:

- Every engine (an `IConverter`) lists the **edges** it can do directly, for example `png → jpg (cost 10)`.
- `RoutePlanner` runs Dijkstra over every edge of every engine that is available right now and picks the cheapest
  route, up to **3 steps** (`RoutePlanner.MaxSteps = 3`, `src/Filee.Core/Conversion/RoutePlanner.cs:19`).
- `JobQueue` runs the steps one after another. The output of step N becomes the input of step N+1. Middle results go to a
  private work folder, and only the last step writes to the user's chosen place.

So "video → JPG" is really `video →(ffmpeg) png →(magick) jpg`, and "EPS → WebP" is `eps →(ghostscript) png →(magick) webp`.
Edge **costs** are tuned by hand so the planner picks the sensible route (see §4.3 and §5.2).

Engines are listed in one place, `src/Filee.Engines/EngineRegistry.cs:30-55`. The list order only breaks ties.
Built-in engines come first and the optional downloads (FFmpeg, Pandoc, Ghostscript, calibre, LibreOffice) come last.

---

## 2. The engine contract (`src/Filee.Core/Conversion/IConverter.cs`)

### 2.1 Data types

| Type | Fields | Notes |
|---|---|---|
| `ConversionEdge` (record struct) | `From: string`, `To: string`, `Cost: int = 10` | Format ids are lower-case (`"png"`, `"mp4"`). `From == To` is allowed and means re-encode, split, or compress (line 17). |
| `EngineStatus` | `IsAvailable`, `ReasonKey?` (localization key such as `engine.reason.not_installed`), `Detail?` (for example the exe path, shown as a tooltip), `Version?` | Built with `Available(detail, version)` / `Unavailable(reasonKey, detail)` (lines 24-28). |
| `IOutputAllocator` | `string? Allocate(string extension, string? suffix = null)` | Returns the path to write the next output file to. `extension` is given without the dot. An empty extension asks for a **folder**. `suffix` is used for many outputs from one input, such as `"_p2"`. Returns **null when this output must be skipped** (the file exists and the policy is "skip"). Engines must handle null by not writing (lines 31-43). |
| `ConversionStep` | `InputPath`, `From`, `To`, `Preset`, `Output: IOutputAllocator`, `WorkDirectory` | `WorkDirectory` is a private scratch folder for the job. It is deleted afterwards (lines 52-58). |

### 2.2 `IConverter`

```text
Id: string                 stable, lower-case ("magick", "ffmpeg", "pdfium", ...) — used in settings
DisplayName: string        "ImageMagick", "FFmpeg"
Edges: list<ConversionEdge>
MaxParallelism: int        0 = one per CPU core
GetStatus() -> EngineStatus     must be cheap; may cache
ConvertAsync(step, IProgress<double>?, CancellationToken) -> list<string>   paths actually written
```

The doc comment's rules (lines 63-66): an engine must be **thread-safe up to MaxParallelism** concurrent calls, must
**honour cancellation**, and must **never modify the input file**.

Extra capability interfaces. The queue finds them by type, using the first engine that implements each one
(`EngineRegistry.cs:58-67`):

- `IPdfMerger.MergeAsync(inputs, output, ct)`: implemented by `PdfSharpConverter`.
- `ITiffMerger.MergeAsync(inputs, output, ImageOptions, ct)`: implemented by `MagickImageConverter`.
- `IFileCombiner`: implemented by the archive engine (not covered in this doc).

### 2.3 How the queue calls engines (`src/Filee.Core/Conversion/JobQueue.cs`)

- **Parallelism gate**: one `SemaphoreSlim` per engine id, sized `MaxParallelism <= 0 ? ProcessorCount : MaxParallelism`
  (`JobQueue.cs:401-406`). Every `ConvertAsync` call waits on that gate. Current values are: magick, pdfsharp, icns,
  vector, ghostscript, markdown and pdf-text use `0` (one per core); **pdfium uses `1`** (PDFium is not thread-safe);
  **ffmpeg uses `2`** (its encoders already use every core).
- **Progress**: each engine reports values from 0 to 1 for its own step. The queue maps a report to
  `(stepIndex + p) / stepCount` and **ignores any value that is not higher than the last one**, because reports come in
  out of order from the thread pool.
- **Allocator**: the last step gets the "final" allocator (the user's output rules, conflict policy). Middle steps get a
  `TempAllocator(workDir, baseName)`.
- **Fan-out**: if one step writes several files (PDF pages), the next step runs once per file.
- **Result**: an empty output list on the final step marks the file `Skipped`. An exception marks it `Failed` and the
  exception message is shown to the user, which is why engines throw friendly English messages.
  `OperationCanceledException` marks the file `Cancelled`.

---

## 3. Infrastructure (`src/Filee.Engines/Infrastructure/`)

### 3.1 Finding external tools: `EngineEnvironment.cs`

The optional engines are FFmpeg, Ghostscript, Pandoc, calibre, LibreOffice, rhwp and 7-Zip. Each is a
**package id** that maps to the programs that must **all** exist in one folder (`Programs()`, lines 72-81):

| package | programs (non-Windows) | programs (Windows) |
|---|---|---|
| `ffmpeg` | `ffmpeg`, `ffprobe` | `ffmpeg.exe`, `ffprobe.exe` |
| `ghostscript` | `gs` | `gswin64c.exe` |
| `pandoc` | `pandoc` | `pandoc.exe` |
| `libreoffice` | `soffice` | `soffice.exe` |
| `calibre` | `ebook-convert` | `ebook-convert.exe` |
| `rhwp` | `rhwp` | `rhwp.exe` |
| `7zip` | `7zz` | `7z.exe` |

`ProgramName()` (line 83) turns a Windows name into the Unix one: it removes `.exe` and maps `gswin64c.exe → gs`.

**Lookup order** (header comment lines 1-5; see `FfmpegConverter.Locate`, `Media/FfmpegConverter.cs:147-154`):

1. **User's own copy** (`OwnCopies[id]`, set in Settings → Engines). The user may pick the program or a folder.
2. **Filee's copy** (`FindBundled(name)`, lines 31-49): `<exe dir>/engines/<name>`, then `DownloadRoot/<name>`, then
   (dev only) walk up to 8 parent folders looking for `Filee.slnx` and use `<repo>/engines/<name>`.
3. **System copy** (`SystemCopyFolder(id)`, cached in a `ConcurrentDictionary` until `ForgetSystemCopies()`):
   - every `PATH` entry, with quotes trimmed and env vars expanded. On Windows it also reads the saved User and Machine
     PATH, so a tool installed after Filee started is still found.
   - on Unix it also tries `/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin`, `/bin`, `/opt/local/bin`.
   - LibreOffice extras: `/Applications/LibreOffice.app`, `~/Applications/LibreOffice.app`, `/usr/lib/libreoffice/program`,
     `/usr/lib64/libreoffice/program`, `/opt/libreoffice*/program` (newest first).
   - calibre extras: `/Applications/calibre.app`, `~/Applications/calibre.app`, `/opt/calibre`.
   - Windows extras: `Program Files{,(x86)}\LibreOffice\program`, `...\Calibre2`, `...\gs\gs*\bin` (newest first).
   - folders inside `DownloadRoot` or the app folder are **excluded**, because those count as "Filee's copy".
   - turned off entirely by `SearchSystem = false` (used in tests).

`FolderWithPrograms(id, path)` (lines 102-120) accepts either a file or a folder. It then tries `start`, `start/bin`,
`start/program`, `start/Contents/MacOS` and, for LibreOffice and calibre, `start/<App>.app/Contents/MacOS`. The first
folder holding **all** required programs wins. This rule lets users point at the root of an FFmpeg zip or a `.app`
bundle.

`DownloadRoot` defaults to `LocalApplicationData/Filee/engines`. On Linux that resolves to `~/.local/share/Filee/engines`.

Global switches that change which engine claims which edge: `LibreOfficeForSlides` (default true), `LibreOfficeForHwp`,
`LibreOfficeForDocuments`, `LibreOfficeForSheets` (lines 57-67).

### 3.2 Pinned downloads: `engines.json` + `EngineDownloads.cs`

`engines.json` is embedded in the assembly. It has a `components` map for **win-x64** and a
`platforms.<rid>.components` map for the other RIDs. Each component has `version`, `url`, `sha256`, `size` and `kind`
(`zip | tar.gz | tar.xz | dmg | msi | oxt | conda`).

Linux x64 components in this snapshot:

| id | version | kind | source |
|---|---|---|---|
| rhwp | 0.8.7 | tar.gz | github.com/edwardkim/rhwp releases |
| 7zip | 26.03 | tar.xz | github.com/ip7z/7zip releases |
| pandoc | 3.12 | tar.gz | github.com/jgm/pandoc releases |
| ffmpeg | 9.0.2 | tar.xz (~151 MB) | BtbN/FFmpeg-Builds `linux64-gpl-9.0` |
| calibre | 9.15.0 | tar.xz | github.com/kovidgoyal/calibre releases |

**No Ghostscript and no LibreOffice download exist for Linux.** `UnavailableReason()` tells the user to use the
distribution's package manager instead (`EngineDownloads.cs:60-66`).

Packages, i.e. what the user installs together (`EngineDownloads.cs:34-42`):
`libreoffice` (on Windows it also brings `h2orestart` and `jre`), `pandoc`, `ghostscript` (on Windows it also brings
`vcruntime`), `ffmpeg`, `calibre`. A package is **suggested** (ticked by default) if its download is under 60 MB and it
is not ghostscript (`IsSuggested`, line 79).

`EngineVersions.cs` builds version labels: `Library("Markdig", type)` gives `"Markdig 1.4.0"`, `BuiltIn` gives
`"Filee x.y.z"`, and `Component(id)` gives the pinned version.

`CopyVersions.cs` checks the version of a user or system copy by running `ffmpeg -hide_banner -version` or
`<tool> --version` (10 s timeout). It compares major.minor against the pinned version and only says "update
recommended". Regexes used: `ffmpeg version n?(\d+)\.(\d+)`, `^pandoc(?:\.exe)? (\d+)\.(\d+)`, `calibre (\d+)\.(\d+)`,
`(?:MsiProductVersion=|LibreOffice\s+)(\d+)\.(\d+)`, and a fallback `^\s*(\d+)\.(\d+)`. Results are cached per
`path|mtime`.

### 3.3 Installer: `EngineInstaller.cs`

This is a safe download-and-unpack pipeline:

1. Work out which components are pending: those whose marker file `<folder>/.filee-component` does not hold the pinned
   sha256 (lines 69-72). Installed means a marker exists. Up to date means the marker equals the pinned sha256.
2. **Download** to `<root>/.downloads/<id>-<version><ext>.part`, using 64 KiB reads and an incremental SHA-256. Progress
   is reported at most every 150 ms. **Redirects are followed by hand** (up to 10 hops), so a mirror's HTTPS → HTTP
   redirect is allowed. This is safe because the hash is pinned. If the hash does not match, the `.part` file is
   deleted and the install throws. On success the file is renamed (lines 114-173).
3. **Unpack** into a staging folder `<root>/~xxxxxx`, then `Replace()` swaps it into `<root>/<id>` only when complete
   (delete the old folder, then move). The staging folder is always deleted in `finally`.
   - zip / tar.gz / tar.xz: extract, then `SingleRoot()` (if the archive has one top-level folder, use that folder), then
     `EnsureExecutablePrograms` (on Unix, `chmod +x` the required programs), then a check that the required programs
     exist, else throw.
   - dmg (macOS): `hdiutil attach -readonly -nobrowse -mountpoint <m> <file>`, then `ditto <App>.app <staging>`, then
     `hdiutil detach`.
   - msi (Windows): `msiexec /a "<msi>" /qn /l*v "<log>" TARGETDIR="<dir>"`, with a 15 min timeout. Exit code 1618 means
     "another install running". LibreOffice is then trimmed of help, readmes, gallery and dictionaries other than en and ko.
   - conda (Windows Ghostscript): a zip holding `pkg-*.tar.zst`. Only `Library/bin/*` is extracted, with a check against
     path traversal.
   - oxt: unzipped into `libreoffice/share/extensions/<Name>`.
4. Write the marker with the sha256.

`TransferRate.cs` gives download speed as a sliding 5-second average, plus time remaining.

### 3.4 Running processes: `ProcessRunner.cs`

`RunAsync(exe, args, timeout, ct, workingDirectory?, env?, onOutput?) -> ProcessResult(ExitCode, StdOut, StdErr)`

- Runs without a shell. Arguments are passed as an argument list, so there is no quoting bug. No window opens. Output
  is read as UTF-8.
- The default working directory is **the exe's folder**. This matters on Windows, where DLLs load from there.
- stdout and stderr are both collected, and every line also goes to `onOutput`.
- A timeout and the caller's token are linked. When either fires, the **whole process tree is killed**. Then it throws
  `OperationCanceledException` if the user cancelled, or `TimeoutException("<exe> did not finish within N s.")` if the
  timer ran out.
- After exit it calls `WaitForExit()` again so the async readers finish draining.

### 3.5 Fake progress: `ProgressEstimate.cs`

This is for tools that report nothing while they run. A timer fires every 150 ms and reports
`from + (to - from) * (1 - exp(-elapsed / typical))`, with defaults `from = 0.1` and `to = 0.9`. After `typical`
seconds the value is about 63 % of the way. Dispose it to stop. With no progress sink it does nothing. The engine
reports `1` itself when done.

Typical durations used: Ghostscript 2 s, HEIC encode 5 s per picture, FFmpeg with an unknown duration 20 s.

---

## 4. ImageMagick engine (`src/Filee.Engines/Magick/`)

Filee links **Magick.NET-Q8** (8-bit) in-process. MorphRing will most likely shell out to `magick` (or use a Rust
crate). Each Magick.NET call below is followed by the matching **`magick` CLI** option.

### 4.1 Formats (`ImageEncoder.cs:17-34`)

- **Readable**: `png jpg webp tiff bmp gif ico avif heic jxl jp2 psd psb tga ppm xcf raw`. `emf` and `wmf` are added
  **on Windows only**, because ImageMagick renders them through GDI+.
- **Writable**: `png jpg webp tiff bmp gif ico avif jxl jp2 psd psb tga ppm`. **HEIC is not writable here**: the bundled
  Magick.NET has no HEVC encoder, so HEIC goes through FFmpeg (§6).
- Multi-frame targets: `tiff`, `gif`. Animated formats: `gif`, `webp`. Formats with no alpha: `jpg`, `ppm`, `heic`.

`MagickImageConverter` (`MagickImageConverter.cs`): `Id = "magick"`, `MaxParallelism = 0`. It has an edge for every
(readable × writable) pair, **including X → X** (re-encode).

### 4.2 Edge cost rule (`ImageEncoder.Edge`, line 50)

`cost = 12` when the target is `gif` and the source is neither multi-frame nor animated. Otherwise `cost = 10`. This
penalises still → GIF, so a still photo reaches video through PNG (see §5.2).

### 4.3 Conversion flow (`MagickImageConverter.ConvertAsync`, lines 66-85)

```text
images = ImageReader.Read(input, from, preset)     ; progress 0.3
frames = SelectFrames(images, from, to)
for each frame: ApplyOptions(frame, preset.Image, to) ; progress 0.7
written = Write(frames, to, allocator, options, inputPath, KeepsAnimation(from,to)) ; progress 1
```

### 4.4 Reading (`ImageReader.cs`)

| source | special handling | `magick` CLI equivalent |
|---|---|---|
| `raw` (camera RAW) | Forces the DNG coder (LibRaw) for every RAW extension, including `.srw` and `.raw`, which IM would otherwise read as headerless pixels. Defines: camera white balance on, auto WB off, output sRGB, AHD interpolation, auto-brightness on (lines 152-165). | `magick dng:IN -define dng:use-camera-wb=true -define dng:use-auto-wb=false -define dng:output-color=1 -define dng:interpolation-quality=3 ...` (check the define names against the local IM; Arch's IM uses a darktable delegate for DNG) |
| `psd`, `psb` | Reads **only frame 0**, the composite (lines 166-172). | `IN.psd[0]` |
| `emf`, `wmf` | Density `max(150, clamp(preset.Pdf.RenderDpi, 36, 1200))`, transparent background. Density is written back onto the image afterwards so a later PDF step sizes it right (lines 131-135, 173-178). | Windows only; skip on Linux |
| `xcf` | Layers are flattened by hand. Canvas size comes from the XCF header: magic `"gimp xcf "` at bytes 0-8, big-endian u32 width at byte 14 and height at byte 18, each accepted in 1..524288 (lines 207-217). Layers are composited bottom-up at their page offsets using each layer's compose op. `CompositeOperator.No` marks a **hidden** layer and is skipped. `Undefined` becomes `Over`. If layer 0 is grayscale, the result is GrayscaleAlpha. A read failure gives a friendly message: "8-bit XCF without better compression only". | `magick IN.xcf -background none -layers flatten OUT` is close enough for v0.1 |

### 4.5 Choosing frames (`SelectFrames`, lines 111-126)

1. If the source is animated (gif or webp) with more than one frame, **coalesce** first (`-coalesce`), so every frame is
   a full image.
2. A single frame is returned as is.
3. psd/psb: frame 0 only.
4. ico: the **largest** frame by width × height.
5. An animated source going to a target that is not multi-frame and not animated: first frame only.
6. Otherwise all frames (TIFF pages, GIF→GIF, GIF↔WebP animations).

### 4.6 `ApplyOptions(image, options, target)` (lines 164-238), in order

| step | Magick.NET | `magick` CLI |
|---|---|---|
| 1 | `AutoOrient()` (always) | `-auto-orient` |
| 2 | Resize `Percent` when `ResizePercent` is >0 and ≠100 | `-resize 50%` |
| | Resize `LongEdge` when >0: fit in an N×N box, **shrink only** | `-resize 1920x1920>` |
| | Resize `Width` when >0 and image width > W: W × round(h·W/w) | `-resize 1200x>` |
| 3 | `FitFormat`: shrink so the long side ≤ `MaxSide(target)`: **jpg 65 500, webp 16 383, avif 16 384**, others unlimited. This fixes issue #50, where a tall JPEG failed. | `-resize 65500x65500>` |
| 4 | `Grayscale` when set | `-colorspace Gray` |
| 5 | `Dpi` when set | `-units PixelsPerInch -density N` |
| 6 | `KeepMetadata == false`: strip everything, but re-attach the ICC profile when `KeepColorProfile` is set | `+profile "!icc,*"` (keep ICC) or `-strip` |
| 7 | Target in NoAlpha (jpg/ppm/heic) and image has alpha: flatten onto `Background` (default `#FFFFFF`, invalid values fall back to white) | `-background "#FFFFFF" -alpha remove -alpha off` |
| 8 | Per format: `jpg avif jxl jp2 webp` → `Quality = clamp(q,1,100)`. For jxl/jp2, 100 means lossless. webp + `WebpLossless` → `webp:lossless=true`. tiff → compression `Lzw`/`Zip`/`None`. ico → `PrepareIcon`. psd/psb → force `ClassType.Direct`, because IM writes a broken layer section for palette images with alpha. | `-quality Q`, `-define webp:lossless=true`, `-compress LZW|Zip|None`, `-type TrueColorAlpha` |

`PrepareIcon` (lines 277-294): pad to a square with transparency, centered (`-background none -gravity center
-extent SxS`). If the side is over 256, resize to 256×256. Sizes are `IcoSizes` filtered to 1..256, de-duplicated and
sorted descending (default `[256,128,64,48,32,16]`, fallback `[256,48,32,16]`), passed as `-define icon:auto-resize=256,128,...`.

### 4.7 Writing (`Write`, lines 247-275; `OutputFormat`, lines 92-104)

- **Netpbm**: the target `ppm` writes `.pbm` or `.pgm` when the **source file's extension** was `.pbm` or `.pgm`. A
  `Grayscale` preset writes `.pgm`. Otherwise `.ppm`.
- If there is more than one frame and the target is multi-frame (tiff/gif) or the job is an animation (gif↔webp), write
  **one** file via `Allocate(ext)`.
- Otherwise write one file per frame. With more than one frame the suffix is `_p{n}` (1-based).
- A null path from the allocator means skip that output and keep going.

### 4.8 Multi-page TIFF merge (`MagickImageConverter.MergeAsync`, lines 37-64)

Every frame of every input is cloned, flattened onto the background (default white), set to 8-bit depth, given the
preset's TIFF compression, and gets the attribute `tiff:subfiletype=2`. All are written as one TIFF.
**Fragile**: ImageMagick's TIFF coder reads `tiff:subfiletype` with the values `PAGE`/`REDUCEDIMAGE`/`MASK`, so `"2"`
may do nothing. In CLI form, use `-define tiff:subfiletype=PAGE`.

CLI sketch: `magick in1.tif in2.png -background white -alpha remove -alpha off -depth 8 -compress LZW -define tiff:subfiletype=PAGE out.tif`

### 4.9 Status

Always available. Version string `"Magick.NET x · ImageMagick 7.1.2-5"`.

---

## 5. FFmpeg engine (`src/Filee.Engines/Media/`)

`FfmpegConverter`: `Id = "ffmpeg"`, `MaxParallelism = 2`. It needs **both** `ffmpeg` and `ffprobe` from the **same
folder** (`Locate`, `FfmpegConverter.cs:147-154`).

### 5.1 Status (`GetStatus`, lines 62-76; `VersionOf`, 157-179)

It runs `ffmpeg -hide_banner -version` with a 10 s limit and parses the first line with
`version\s+n?(\d+(?:\.\d+)+|\S+)`. The result is cached per `path|mtime`. A non-zero exit or a failure to start gives
`Unavailable("engine.reason.error", message)`. This covers "present but broken", for example a missing DLL.

### 5.2 Edges (`BuildEdges`, lines 45-60)

| from | to | cost |
|---|---|---|
| every **video** format | every video target + every audio target + `png` (poster frame) | 10 |
| every video format | `gif` | **15** (slow, 256 colours; so video→jpg goes through png) |
| every **audio** format | every audio target (including itself, which re-encodes) | 10 |
| `gif` | `mp4 webm mov` | 10 |
| `png` | `mp4 webm mov` | **11** |
| every image ImageMagick reads | `heic` | 10 |

Why these numbers work: photo→mp4 can go `jpg→png (10) + png→mp4 (11) = 21` or `jpg→gif (12) + gif→mp4 (10) = 22`, so
PNG wins and keeps full colour. Animated GIFs and multi-page TIFF or PDF pages go `→gif (10) + gif→mp4 (10) = 20`, which
keeps their frames.

Video source formats (from `src/Filee.Core/Formats/FormatRegistry.cs:131-152`): mp4 m4v mov mkv webm avi wmv flv mpg ts
m2ts 3gp 3g2 ogv vob dv mxf, plus these read-only ones: mod rm swf wtv cavs.
Audio source formats: mp3 m4a m4b aac wav flac ogg opus wma aiff ac3 amr au caf weba mka mp2 voc, plus read-only dss.

### 5.3 Conversion flow (`ConvertAsync`, lines 78-124)

```text
if to == "heic": return HeicWriter.Convert(...)                 (§6)
info = ffprobe(input)                                            progress 0.02
output = Allocate(primaryExtension(to)); null -> return []
still = (from == "png")
cover = (!still && info.cover && to in {ogg, opus, mka}) ? CoverArt.Prepare(...) : null
passes = still ? PlanStill(...) : Plan(...)     ; throws BEFORE writing if a needed stream is missing
duration = still ? 5 s : info.duration
run each pass; progress = 0.02 + 0.97 * (done + weight * clamp(pos/duration))
if output missing or 0 bytes -> throw "FFmpeg did not write any output."
on ANY exception: delete the partial output, rethrow
progress 1
```

### 5.4 Probing (`MediaInfo.cs`)

Command (1 min timeout through ProcessRunner):

```text
ffprobe -hide_banner -v error -print_format json -show_format -show_streams <input>
```

A non-zero exit gives `"FFmpeg cannot read this file: <summary>"`. Parsing rules:

- A stream whose `disposition.attached_pic == 1` is a **cover**. The first one whose codec is `mjpeg` or `png` is kept
  as `CoverStreamIndex`. It is never treated as video.
- The first real video stream gives width and height. `sample_aspect_ratio` (`"a:b"`, 0 or missing means 1) gives the
  SAR. **Rotation** comes from `side_data_list[].rotation`, falling back to `tags.rotate`. Frame rate is `avg_frame_rate`
  if it is in (0, 1000), else `r_frame_rate`, else 0. `codec_name` and `pix_fmt` are kept too.
- The first audio stream gives channels, sample_rate and codec_name.
- Duration is `format.duration`, falling back to the longest stream `duration`. 0 means null (unknown).
- ffprobe writes numbers as strings (`"2.000000"`), so both strings and JSON numbers must be accepted.
- Derived values: `IsRotated = |rotation % 180| == 90`. `DisplayHeight` is the width when rotated.
  `DisplayAspect = w·SAR/h`, inverted when rotated, and 16/9 when unknown.

### 5.5 The runner (`FfmpegRunner.cs`)

- There is **no overall timeout**, because a film can take an hour. Instead a **stall watchdog** checks every 1 s. If
  there has been no stdout or stderr line for **10 min**, it kills the process tree and throws `TimeoutException`.
- Cancellation kills the whole tree.
- Only the **last 40 stderr lines** are kept.
- Progress parsing (`ParsePosition`, lines 307-323): `out_time_us=<µs>` gives seconds (`N/A` is ignored), and
  `frame_time=<seconds>` is used by the GIF palette pass.
- `Summarize(stderr)`: split into lines, trim, strip leading `[tag @ 0x...] ` prefixes (repeatedly), drop empty lines,
  de-duplicate, keep the **last 3**, join with spaces. Error messages look like
  `"FFmpeg failed (exit code N): <summary>"`.

### 5.6 Common arguments

Every run starts with: `-hide_banner -nostdin -nostats -v error -y` (`MediaEncoding.Common`, line 112).
Every encode ends with `-f <muxer> -progress pipe:1 <output>`. The muxer is **always explicit**, because some extensions
are ambiguous.

### 5.7 Target table (`MediaEncoding.cs:28-81`)

Video targets (muxer, video codec, audio codec):

| target | muxer | video | audio | notes |
|---|---|---|---|---|
| mp4 | mp4 | libx264 | aac | |
| m4v | **mp4** | libx264 | aac | FFmpeg's `m4v` muxer would write raw MPEG-4 part 2 |
| mov | mov | libx264 | aac | |
| mkv | matroska | libx264 | aac | |
| flv | flv | libx264 | aac | |
| ts | mpegts | libx264 | aac | |
| m2ts | mpegts | libx264 | aac | + `-mpegts_m2ts_mode 1` |
| 3gp / 3g2 | 3gp / 3g2 | libx264 | aac | |
| webm | webm | libvpx-vp9 | libopus | WebM spec only allows VP8/VP9/AV1 with Vorbis/Opus |
| avi | avi | mpeg4 (tag XVID) | libmp3lame (CBR) | |
| wmv | asf | wmv2 | wmav2 | |
| mpg | **vob** | mpeg2video | mp2 | `mpeg` would write MPEG-1 system |
| vob | **dvd** | mpeg2video | ac3 | DVD-compliant |
| ogv | ogg | libtheora | libvorbis | |
| dv | dv | dvvideo | pcm_s16le | |
| mxf | mxf | mpeg2video | pcm_s16le | |

Audio targets:

| target | muxer | codec |
|---|---|---|
| mp3 | mp3 | libmp3lame |
| m4a, m4b | **ipod** | aac |
| aac | adts | aac |
| wav | wav | pcm_s16le (or per `WavFormat`) |
| flac | flac | flac |
| ogg | ogg | libvorbis |
| opus | opus | libopus |
| wma | asf | wmav2 |
| aiff | aiff | pcm_s16be |
| ac3 | ac3 | ac3 |
| amr | amr | libopencore_amrnb |
| au | au | pcm_s16be |
| caf | caf | pcm_s16be |
| weba | webm | libopus |
| mka | matroska | libopus |
| mp2 | mp2 | mp2 |
| voc | voc | pcm_s16le |

The local Arch `ffmpeg n9.0.2` has libx264, libx265, libvpx-vp9, libtheora, libopus, libmp3lame and libopencore_amrnb.

### 5.8 Video command (`Video`, lines 137-173)

```text
<Common> -i IN -map 0:V:0 [-map 0:a:0]                 # 0:V = real video, not cover pictures
  if CanCopyVideo:  -c:v copy  [+ -tag:v hvc1 if hevc and target in mp4/m4v/mov]
  else:             -vf <filter> <video codec args>
  if no audio (or RemoveAudio): -an
  elif CanCopyAudio: -c:a copy
  else:              <audio codec args, inVideo=true>
  -map_metadata 0 -metadata major_brand= -metadata minor_version= -metadata compatible_brands=
  <container args>
  -max_muxing_queue_size 4096 -f <muxer> -progress pipe:1 OUT
```

A missing video stream throws `"The file has no video stream."`. A missing audio stream is fine.

**Remux (stream copy) rules** (lines 181-213). This is the cleverest part, because iPhone MOV→MP4 takes seconds and loses
nothing.

| target | copyable video codecs | copyable audio codecs |
|---|---|---|
| mp4, m4v | h264, hevc | aac, alac, ac3, eac3, mp3 |
| mov | h264, hevc, prores | aac, alac, ac3, eac3, mp3, pcm_s16le, pcm_s24le |
| mkv | h264, hevc, vp9, av1 | aac, alac, ac3, eac3, mp3, opus, vorbis, flac |
| webm | vp8, vp9, av1 | opus, vorbis |
| ts | h264, hevc | aac, ac3, eac3, mp3 |
| m2ts | h264, hevc | aac, ac3, eac3 |

- `CanCopyVideo` requires all of: quality ≠ Small; the codec is in the list; the pix_fmt is one of
  `yuv420p | yuvj420p | yuv420p10le` (prores is exempt from this check); and no downscale is needed
  (`!(MaxHeight > 0 && DisplayHeight > max(16, MaxHeight))`).
- `CanCopyAudio` requires all of: quality ≠ Small; `AudioBitrateKbps <= 0`; the channel count is unchanged
  (`AudioChannels` not 1 or 2, or equal to the source); and the codec is in the list.

**Scale filter** (`ScaleFilter`, line 219):

- With `MaxHeight > 0`: `scale=w=-2:h='trunc(min(ih,H)/2)*2'`, where H = max(16, MaxHeight).
- Otherwise: `scale=w='trunc(iw/2)*2':h='trunc(ih/2)*2'`. This always makes both sides even, which 4:2:0 encoders need.

**DVD/DV filter** (`DvdOrDvFilter`, lines 227-237): PAL is 576 lines and NTSC 480. `wide = DisplayAspect >= 1.55`. The
picture is fitted into 720×H keeping its aspect, padded with black, then `setdar=16/9` or `4/3`.
`IsPal(fps)`: |fps−25| < 0.5, or |fps−50| < 1, or |fps−12.5| < 0.3.

**Video codec args** (`VideoCodecArguments`, lines 242-284). Quality is High/Balanced/Small. Quantizer = 2/4/7.

| codec | args |
|---|---|
| libx264 | `-c:v libx264 -preset medium -crf {18/23/28} -pix_fmt yuv420p` |
| libvpx-vp9 | `-c:v libvpx-vp9 -b:v 0 -crf {24/32/38} -row-mt 1 -deadline good -cpu-used 4 -pix_fmt yuv420p` |
| mpeg4 | `-c:v mpeg4 -tag:v XVID -q:v {2/4/7} -g 250 -bf 2 -pix_fmt yuv420p [-r NearestRate]` |
| wmv2 | `-c:v wmv2 -q:v {q} -g 250 -pix_fmt yuv420p [-r NearestRate]` |
| libtheora | `-c:v libtheora -q:v {8/6/4} -pix_fmt yuv420p [-r NearestRate]` |
| mpeg2video (vob) | `-c:v mpeg2video -b:v {8000/6000/4000}k -maxrate 9000k -minrate 0 -bufsize 1835k -g {15 PAL/18 NTSC} -bf 2 -pix_fmt yuv420p -r {25 / 30000/1001}` |
| mpeg2video (mpg, mxf) | `-c:v mpeg2video -q:v {q} -g 15 -bf 2 -pix_fmt yuv420p -r BroadcastRate` |
| dvvideo | `-c:v dvvideo -pix_fmt {yuv420p PAL / yuv411p NTSC} -r {25 / 30000/1001}` |

- `BroadcastRates`: 24000/1001, 24, 25, 30000/1001, 30, 50, 60000/1001, 60.
- `NearestRate(fps)`: if within 0.01 of a broadcast rate, use it. Otherwise `clamp(round(fps), 1, 120)`. Null when
  fps ≤ 0, in which case `-r` is left out.
- `BroadcastRate(fps)`: the nearest broadcast rate, or `25` when unknown.

**Container args** (`ContainerArguments`, lines 314-327):

- mp4/m4v/mov/3gp/3g2/m4a/m4b: `-movflags +faststart`
- m2ts: `-mpegts_m2ts_mode 1`
- vob: `-muxrate 10080000 -packetsize 2048`
- mp3: `-id3v2_version 3` (what Windows Explorer and car stereos read)
- wav: `-rf64 auto`

### 5.9 Audio command (`Audio`, lines 343-363)

```text
<Common> -i IN [-i metadata.txt]  -map 0:a:0
  [if cover && target in {mp3,m4a,m4b,flac}: -map 0:<coverIdx> -c:v copy -disposition:v:0 attached_pic]
  [if mka cover: -attach cover.jpg -metadata:s:t mimetype=image/jpeg -metadata:s:t filename=cover.jpg]
  <audio codec args, inVideo=false>
  [-ac 1|2]  [-ar N for wav]
  -map_metadata {0 | 1 if metadata file} -metadata major_brand= ... 
  <container args> -f <muxer> -progress pipe:1 OUT
```

A missing audio stream throws `"The file has no audio track."`.

**Audio codec args** (`AudioCodecArguments`, lines 398-468). `channels = probed or 2`. Bitrates are given as `-b:a Nk`,
clamped to [min, max]. `Scaled(stereo, ch) = stereo * clamp(ch,1,8) / 2`.

| codec | rule |
|---|---|
| aac | requested or Scaled({256/192/128}, ch), clamp [16, 256·ch] |
| libopus | requested or Scaled({160/128/64}, ch), clamp [6, 256·ch] |
| libmp3lame, in video or bitrate set | CBR: Snap(requested or {256/192/128}, MP3 table), clamp [32, 320] |
| libmp3lame, audio-only, no bitrate | VBR `-q:a {0/2/5}` |
| libvorbis, bitrate set | clamp [32, 500] |
| libvorbis | `-q:a {6/4/2}` |
| wmav2 | requested or {192/128/96}, clamp [32, 320]; `-ac 2` if ch > 2 |
| mp2 | Snap(requested or {320/224/160}, MP2 table), clamp [32, 384]; `-ar 32000` if source rate < 32 k |
| ac3 for vob | Snap(requested or (ch > 2 ? 448 : 192), AC3 table), clamp [64, 448]; `-ar 48000` |
| ac3 | only if requested: Snap, clamp [32, 640] |
| libopencore_amrnb | `-ar 8000 -ac 1 -b:a 12.2k` |
| pcm_s16le for dv | `-ar 48000 -ac 2` |
| pcm_s16le for mxf | `-ar 48000` |
| pcm_s16le for voc with ch > 2 | `-ac 2` |

`Snap(k, table)` returns the largest table value ≤ k, or the smallest value in the table if none qualifies. The tables
are:

- MP3: 32 40 48 56 64 80 96 112 128 160 192 224 256 320
- MP2: 32 48 56 64 80 96 112 128 160 192 224 256 320 384
- AC3: 32 40 48 56 64 80 96 112 128 160 192 224 256 320 384 448 512 576 640

WAV codecs (`WavCodec`): Pcm16 → `pcm_s16le` (default), Pcm8 → `pcm_u8`, Pcm24 → `pcm_s24le`, Pcm32 → `pcm_s32le`,
Float32 → `pcm_f32le`.
`ChannelAndRateArguments`: `-ac N` when AudioChannels is 1 or 2 (not for AMR). `-ar N` for wav when the rate is
between 8000 and 192000.

### 5.10 Cover art for Ogg / Opus / MKA (`CoverArt.cs`)

mp3, m4a, m4b and flac take a picture stream directly (see above). Ogg-family and Matroska targets need extra steps.
Any failure here returns null, and the file is then converted **without** a cover instead of failing.

1. Extract: `<Common> -i IN -map 0:<idx> -c copy -frames:v 1 -f image2 <work>/cover-xxxx/cover.bin`. Sniff the first
   bytes: FF D8 means JPEG, 89 'PNG' means PNG, anything else gives up. Rename to cover.jpg or cover.png.
2. mka: `-attach` the file (see §5.9).
3. ogg/opus: dump the tags with `<Common> -i IN -f ffmetadata metadata.txt`. Build a FLAC `METADATA_BLOCK_PICTURE` from
   big-endian fields: type=3 (front cover), mime length, mime, desc length=0, width, height, depth=24, colors=0, data
   length, data. Base64 it, escape `= ; # \ \n` with a backslash, and insert `METADATA_BLOCK_PICTURE=<b64>` as **line 2**
   (just after `;FFMETADATA1`). Write it back as UTF-8 without a BOM and pass it as input 1, with `-map_metadata 1`.

### 5.11 Animated GIF (two passes, `AnimatedGif`, lines 490-516)

- Frame chain: `fps={15/12/10},[sar fix,]scale=w='min(iw,{640/480/320})':h={ih | 'min(ih,H)'}:force_original_aspect_ratio=decrease:flags=lanczos`.
- SAR fix when |SAR−1| > 0.01: `scale=w='trunc(iw*sar/2)*2':h=ih,setsar=1,`.
- Colours are {256/256/128}. Dither is {`sierra2_4a` / `bayer:bayer_scale=4` / `bayer:bayer_scale=5`}.
- Pass 1 (weight 0.4) builds the palette. A second copy of the frames goes to a null muxer just to get per-frame
  progress, because palettegen reports nothing until it ends:

```text
<Common> -i IN -filter_complex "[0:V:0]<chain>,split[a][b];[a]palettegen=max_colors=N[palette]"
  -map [palette] -frames:v 1 -update 1 -f image2 <work>/palette-<guid>.png
  -map [b] -stats_enc_pre pipe:1 -stats_enc_pre_fmt "frame_time={t}" -f null -
```

- Pass 2 (weight 0.6):

```text
<Common> -i IN -i palette.png -filter_complex "[0:V:0]<chain>[frames];[frames][1:v]paletteuse=dither=D:diff_mode=rectangle[gif]"
  -map [gif] -loop 0 -f gif -progress pipe:1 OUT
```

### 5.12 Poster frame: video → PNG (`PosterFrame`, lines 536-546)

It takes one full-size frame from 10 % into the video, because the first frames are often black:

```text
<Common> -ss <duration/10, "0.###"> -i IN -map 0:V:0 -vf "<sarfix>null" -frames:v 1 -update 1 -c:v png -f image2 -progress pipe:1 OUT
```

### 5.13 Still PNG → video (`PlanStill`, lines 561-595)

The targets are mp4, webm and mov only. The clip is 5 s long. The quality is bumped one step up (Small→Balanced, else
High), because identical frames cost almost nothing to encode.

```text
<Common> -flags2 +icc_profiles -loop 1 -framerate 30 -i IN.png -map 0:V:0 -t 5
  -vf "premultiply=inplace=1,scale=W:H:flags=lanczos:out_color_matrix=bt709:out_range=tv,format=yuv420p"
  <video codec args> [-tune stillimage for x264] -colorspace bt709 -an <container args> -f <muxer> -progress pipe:1 OUT
```

`StillScale`: `factor = min(1, sqrt(8192·4352 / (w·h)), 16384 / max(w,h))`, then also limited by MaxHeight/h. Each side
is rounded **down** to an even number (`floor(v/2 + 1e-6)*2`, min 2). This keeps a 48 MP phone photo within H.264/VP9
level 6.2.

---

## 6. HEIC writer (`src/Filee.Engines/Heic/`)

The flow is: ImageMagick reads the image, FFmpeg's libx265 encodes the pixels, and **Filee writes the HEIF container
itself** (FFmpeg has no HEIF muxer).

1. `Prepare` (HeicWriter.cs:64-89): read the image, take `SelectFrames(..., "heic")`, and run `ApplyOptions(...,"heic")`,
   which flattens alpha because HEIC is in NoAlpha. Keep the ICC bytes. Keep the Exif with **Orientation set to 1 and
   the thumbnail removed**, because the pixels are already upright. Strip, set 8-bit, and write PNG24 to the work folder.
2. `Layout` (lines 95-98): if the picture fits in 512×512, use one tile of even size. Otherwise use a grid of 512×512
   tiles, `ceil(w/512) × ceil(h/512)`.
3. FFmpeg args (lines 108-133):

```text
<Common> -i pic.png -vf "[pad=PW:PH:0:0,fillborders=right=dx:bottom=dy:mode=smear,]scale=out_color_matrix=bt601:out_range=pc,format=yuv420p[,untile=CxR]"
  -c:v libx265 -preset medium -crf <50 - 0.4*quality, clamp 0..51> -force_key_frames expr:1 -forced-idr 1
  -x265-params bframes=0:open-gop=0:info=0:range=full:colormatrix=smpte170m:log-level=error -f hevc out.hevc
```

   Padding uses edge **smear**, not black, so the padding does not bleed into the picture. Every tile is a forced IDR
   frame rather than `keyint=1`, because `keyint=1` makes x265 signal "Main Intra", which the `heic` brand forbids and
   Windows decodes wrongly. CRF examples: quality 90 → 14, 80 → 18.
4. `HevcStream.Parse`: split the Annex B stream on `00 00 01`, trimming trailing zero bytes. Collect VPS (32), SPS (33)
   and PPS (34); they must stay the same across pictures. Only IDR NALs (19 or 20) are allowed. A slice with
   `first_slice_segment_in_pic_flag` (`unit[2] & 0x80`) starts a new picture. Build an `hvcC` record by parsing the SPS
   (with emulation prevention removed) for profile-tier-level, chroma format and bit depths. Temporal sub-layers are
   rejected.
5. `HeifFile.Write`: `ftyp(heic, 0, mif1, heic)` + `meta{hdlr pict, dinf/dref/url, pitm=1, iloc(4-byte offsets),
   iinf(infe v2: grid|hvc1|Exif, tiles hidden), iref(dimg grid→tiles, cdsc exif→1), iprp{ipco[hvcC, ispe(tile),
   pixi(3×depth), colr(prof ICC | nclx 1/13/6 full-range), ispe(full) for grid], ipma}}` + `mdat`. `meta` is built twice:
   once to learn its size, then again with the real absolute offsets. Limits: grid at most 256×256, file under 4 GiB.
   The Exif item is a 4-byte offset to the `II*\0`/`MM\0*` header, followed by the bytes.

Progress: 0.15 after prepare. Each picture then gets an equal share, with a `ProgressEstimate` of 5 s during the encode.
More than one picture (TIFF pages) gives the suffix `_p{n}`.

**For MorphRing on Linux**: the local `magick` lists `HEIC rw+` via libheif 1.23.5, so `magick in.jpg -quality 80 out.heic`
works today. Port the hand-made HEIF writer only if we need Windows parity with no libheif.

---

## 7. ICNS engine (`src/Filee.Engines/Icns/`)

`IcnsConverter`: `Id = "icns"`, built in. Edges: every readable image → `icns`, and `icns` → every writable image
(using `ImageEncoder.Edge`).

**Write** (IcnsConverter.cs:35-68): read the image, take the first selected frame, run `ApplyOptions(..., "icns")`, then
`MakeSquare` (set alpha, transparent extent to a centered square). For each type in `WrittenTypes`, Lanczos-resize to
an exact size (aspect ignored), strip, and encode **PNG32**. Each size is encoded once and shared:

| type | px | | type | px |
|---|---|---|---|---|
| icp4 | 16 | | ic13 | 256 (128@2x) |
| ic11 | 32 (16@2x) | | ic08 | 256 |
| icp5 | 32 | | ic14 | 512 (256@2x) |
| ic12 | 64 (32@2x) | | ic09 | 512 |
| ic07 | 128 | | ic10 | 1024 (512@2x) |

Container (`IcnsFile.Write`): `"icns"` + u32be total. Then a `"TOC "` entry, size `8 + 8n`, listing (type, size) pairs.
Then each entry: type + u32be(8 + len) + data. Every length counts its own 8-byte header.

**Read**: parse the entries, stopping quietly at a truncated one. Candidates are image types holding PNG, JP2
(`00 00 00 0C 'jP  '`) or J2K (`FF 4F FF 51`) data, `ARGB` entries (ic04/ic05: four planes, each in Apple RLE), and
legacy `is32/il32/ih32/it32` RGB planes in RLE with an optional `s8mk/l8mk/h8mk/t8mk` alpha mask. it32 has a 4-byte zero
prefix. Data that is exactly 4·size² bytes is raw ARGB. Candidates are sorted by size descending, with encoded entries
before legacy ones of the same size. The engine decodes the first one that works and falls back to the next on error.
Apple RLE: a control byte < 0x80 means copy n+1 bytes; ≥ 0x80 means repeat the next byte n−125 times.

---

## 8. Vector engines (`src/Filee.Engines/Vector/`)

### 8.1 `VectorConverter` (built in; Svg.Skia + SkiaSharp)

Edges: `svg|svgz → pdf`, `svg ↔ svgz`, `svg|svgz → every writable raster` (cost from `ImageEncoder.Edge`), and
`ai → pdf`.

- **Security** (SvgRenderer.cs:26-33, 56-57): external images and elements may only be **local**, XML entities are
  **never** resolved, and JavaScript is off. Converting a file must never fetch from the network.
- **Load**: gunzip if the file starts `1F 8B`. If there is no picture, or the bounds are ≤ 0 or infinite, throw "no
  drawable content or no size".
- **SVG → raster** (`RasterScale`, lines 137-153): `scale = max(dpi/96, 1024/longEdge)`, with dpi =
  clamp(RenderDpi, 36, 1200), default 150. **Small icons are rendered at least 1024 px on the long edge.** A LongEdge
  resize sets `scale = LongEdge/longEdge` exactly. Percent multiplies the scale. Upper limits: long edge 16384 and 100 MP
  total. Render onto a transparent RGBA bitmap, convert premultiplied alpha to straight alpha, set density to
  96·scale, then run `ApplyOptions` **with Resize forced to None**, because it is already the right size.
- **SVG → PDF**: one page sized `bounds × 72/96` pt. Metadata Creator "Filee", RasterDpi 300 (for filters such as blur),
  PDF/A when `Document.PdfA` is set. **Text is drawn as glyph outlines** (`OutlinedTextPlayback.cs`, HarfBuzz shaping),
  because Skia's PDF backend embeds whole fonts: one line of Korean would otherwise add 13 MB.
- **svg → svgz**: check for `<svg` in the first 4 KB. If the input is already gzip, copy it. Otherwise gzip at the
  smallest size. **svgz → svg**: gunzip, then the same check.
- **ai → pdf** (lines 119-165): if `%PDF-` appears in the first 1 KB, **copy the file** (Illustrator 9+ embeds a full
  PDF). If it starts `%!PS` or `C5 D0 D3 C6` (DOS EPS), it is PostScript and goes to Ghostscript pdfwrite with EPSCrop,
  or fails with "install Ghostscript". Anything else is not an AI file.

### 8.2 `GhostscriptConverter` (optional, AGPL, separate process)

Edges: `eps→pdf`, `ps→pdf`, `pdf→eps`, `pdf→ps`, `eps→png`, `ps→png`. `MaxParallelism = 0`. Timeout is 5 min. Locate
order: own copy, then bundled, then system (`gs`).

Environment: `GS_OPTIONS=""`, `GS_LIB=""`, `GS_FONTPATH=""`, so the user's settings cannot change the results.

Base command (`RunAsync`, lines 367-388):

```text
gs -dSAFER -dBATCH -dNOPAUSE -dQUIET <device args> -sOutputFile=<scratch path> -f <abs input>
```

Output always goes to an **ASCII scratch folder** `<work>/gs-xxxxxxxx/`, because in `-sOutputFile` a `%` is a page
placeholder and user paths could contain one. Files are moved to the final place afterwards. A run counts as failed if
the exit code is non-zero **or** nothing was written. The error includes the last 6 lines of stderr + stdout.

| conversion | device args | output |
|---|---|---|
| eps/ps → pdf | `-sDEVICE=pdfwrite -dAutoRotatePages=/None` + `-dEPSCrop` for EPS | `out.pdf` |
| pdf → ps | `-sDEVICE=ps2write` [+ `-sPageList=1,3,5`] | `out.ps` |
| pdf → eps | `-sDEVICE=eps2write` [+ PageList] | `page-%d.eps`, one per page, sorted numerically; suffix `_p{realPageNumber}` |
| eps/ps → png | `-sDEVICE=pngalpha -r{dpi} -dTextAlphaBits=4 -dGraphicsAlphaBits=4` + `-dEPSCrop` for EPS | `page-%d.png`; each then goes through `ApplyOptions(...,"png")`; suffix `_p{n}` |

The page list comes from `PageRange.Parse(preset.Pdf.PageRange, pageCount)`, with the page count read by PDFsharp. If
PDFsharp cannot open the file, **all pages** are converted rather than failing.

`pngalpha` keeps a transparent background. Going through PDF would paint white paper behind the drawing.

**Note**: Ghostscript is **not installed** on the target machine. On Arch it is `pacman -S ghostscript`.

---

## 9. PDF engines (`src/Filee.Engines/Pdf/`)

### 9.1 `PdfSharpConverter` (built in, implements `IPdfMerger`)

Edges: every readable image → `pdf`, and `pdf → pdf`.

**Image → PDF** (lines 177-226): every selected frame becomes one page.

- Source DPI: from the density (pixels/cm × 2.54 when needed), else **96**.
- `ApplyOptions(frame, options, "pdf")`.
- Encoding (`EncodeForPdf`): a jpg source with no alpha stays JPEG at quality `max(preset, 85)`. Everything else becomes
  PNG32 if it has alpha, PNG (gray) if grayscale or bilevel, or PNG24 otherwise. **Palette PNGs are never used**, because
  PDFsharp rejects them when they have transparency.
- Page size: `FitImage` (default) makes the page exactly the image size in points (w/dpi·72). A4 is 595.28×841.89 and
  Letter 612×792, switched to landscape when the image is wider than tall. Margin is `MarginMm` (ignored for FitImage).
  The image is fitted and centered.
- Progress `(i+1)/frames`.

**PDF → PDF** (lines 229-260): `PageRange.Parse(...)`. With `SplitPages` set, each page goes to its own file with suffix
`_p{page+1}`. Otherwise the selected pages are extracted into one file.

**Merge**: append every page of every input in order, then save.

### 9.2 `PdfiumConverter` (PDF → raster; PDFtoImage / PDFium; `MaxParallelism = 1`)

Edges: `pdf → every writable image`.

- Render options: dpi = clamp(RenderDpi, 36, 1200), with annotations, with form fill, **white background**.
- Each page bitmap is encoded to PNG, loaded as a Magick image, and given its density.
- Target `tiff`/`gif`: all pages go into one multi-page file via `ImageEncoder.Write`.
- Otherwise each page is its own file with suffix `_p{pageNumber}` (only when more than one page is selected).
- **CombinePages** (if set, the target is not tiff/gif, and there are 2+ pages): stack the pages vertically on a white
  strip as wide as the widest page. If the stack exceeds **200 MP** it is scaled by `sqrt(200M / (w·h))`.
  `ApplyOptions` runs on the tall image, which also applies the format's max side.
  - `TrimPageMargins`: on each page, find the bounding box of the content with an 8 % fuzz, and keep a margin of
    max(4, dpi/6) px above and below (about 4 mm). A blank page shrinks to 2× the margin.
  - `SplitTallImage`: break into runs between pages so each run fits `MaxSide(target) / scale`. Suffix `_{n}`.
- Progress per page. Stacking reports up to 0.8.

### 9.3 `PdfTextConverter` (PDF → TXT; PdfPig)

The edge is `pdf → txt`. It uses `PdfDocumentReader.ReadText` + `PlainText`. This is a **heuristic layout analyser**
(about 1 100 lines in `PdfDocumentReader.cs` and `PdfDocumentReader.Layout.cs`). It:

- groups words into lines;
- detects column gutters and tab stops;
- builds paragraphs (bullets, numbers, indents, early line ends);
- removes running heads and footers that repeat on most pages, ignoring digits so "Page 3" matches "Page 4";
- decides reading order across columns;
- expands ligatures;
- leaves no spaces between CJK characters.

It includes the invisible OCR layer of scanned PDFs. It throws "The PDF contains no text (it may be scanned...)" when
nothing is found. Output is **UTF-8 with BOM**, one paragraph per line, with blank lines between paragraphs.

The same reader also feeds PDF → HWPX/DOCX. That path is out of scope here.

### 9.4 `PageRange.Parse` (`src/Filee.Core/Conversion/PageRange.cs`)

1-based input such as `"1-3, 5, 8-"`. Supported forms are `"3"`, `"2-5"`, `"7-"` (to the end) and `"-4"` (from the
start), separated by commas. Reversed ranges are swapped, out-of-range pages are clamped, invalid parts are ignored.
The result is sorted, distinct and 0-based. **If nothing valid remains, all pages are used.** A page count ≤ 0 gives an
empty list.

---

## 10. Text engine (`src/Filee.Engines/Text/MarkdownConverter.cs`)

Edges: `md → html (cost 4)`, `md → txt (cost 5)`. They are cheaper than Pandoc's, so the built-in path wins.

- Input is decoded with `HwpxConverter.DecodeText`, which sniffs the encoding (BOM, UTF-8, legacy code pages).
- HTML: parse with Markdig using a shared pipeline (`MarkdownReader.Pipeline`). **If the output folder differs from the
  input folder**, relative image URLs are rewritten to absolute `file://` URIs so images still load (this matters when
  the HTML is a middle step for LibreOffice). The title is the first heading's text, or the file name. The body is
  wrapped in a self-contained page with CJK-friendly fonts and inline CSS (max-width 860px, line-height 1.7, table
  borders, code background `#f4f3f8`). Written as UTF-8 **without** BOM.
- TXT: `Markdown.ToPlainText`. Written as UTF-8 **with** BOM.

---

## 11. Clever bits and fragile bits

Clever:

- **Edge costs as routing policy** (12 for still→gif, 15 for video→gif, 11 for png→video, 4/5 for built-in markdown).
  Routing behaviour is set with a few integers and needs no special cases.
- **Remux first**: stream copy whenever the container takes the codec and the pixel format is safe to play. Huge speed
  and quality win.
- **Hidden progress pass for GIF palettegen** (`-stats_enc_pre` to a null muxer).
- **Stall watchdog instead of a timeout** for FFmpeg.
- **Explicit `-f` muxer** every time, and `m4v→mp4`, `mpg→vob`, `m4a→ipod` muxer choices that avoid surprising defaults.
- **Even dimensions** everywhere (`trunc(x/2)*2`), so odd-sized inputs never fail 4:2:0 encoders.
- **Never leave half files**: FFmpeg output is deleted on failure. Ghostscript writes to scratch first, then moves.
  Downloads write `.part` then rename.
- **The allocator returns null** for "skip existing", and every engine handles it.
- **Format size caps**: JPEG 65 500, WebP 16 383, AVIF 16 384, video 8192×4352 / 16384 side, raster SVG 16384 / 100 MP,
  PDF stack 200 MP.
- **No network access** while converting SVGs.

Fragile or worth double-checking:

- `tiff:subfiletype` is set to `"2"` (§4.8). ImageMagick expects `PAGE`.
- `FfmpegConverter._tools` is cached from the last `GetStatus()`. If the user changes their own copy, a stale path
  survives until the next status check.
- RAW reading depends on the LibRaw/DNG delegate. Arch's ImageMagick routes DNG through `darktable-cli`, which may not be
  installed.
- ImageMagick needs Ghostscript to read PDF/EPS. Filee avoids this by using PDFium and Ghostscript directly, and MorphRing
  must avoid it too (use poppler `pdftoppm`, which is installed).
- `PdfSharpConverter` falls back to 96 dpi when an image has no density, so a 3000 px photo becomes a 31-inch page.
  This is expected behaviour, but surprising.
- The AI detector checks only the first 1 KB.
- There is no overall FFmpeg timeout, so a 10-minute stall is the only safety net.

---

## 12. Porting to Rust

### 12.1 Core trait (maps 1:1)

```rust
pub struct Edge { pub from: FormatId, pub to: FormatId, pub cost: u32 } // default 10
pub struct EngineStatus { pub available: bool, pub reason: Option<&'static str>, pub detail: Option<String>, pub version: Option<String> }
pub trait OutputAllocator: Send + Sync { fn allocate(&self, ext: &str, suffix: Option<&str>) -> Option<PathBuf>; }
pub struct Step<'a> { pub input: &'a Path, pub from: FormatId, pub to: FormatId, pub preset: &'a Preset,
                      pub output: &'a dyn OutputAllocator, pub work_dir: &'a Path }
pub trait Converter: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn edges(&self) -> &[Edge];
    fn max_parallelism(&self) -> usize { 0 }             // 0 = num_cpus
    fn status(&self) -> EngineStatus;
    fn convert(&self, step: &Step, progress: &dyn Fn(f64), cancel: &CancelToken) -> Result<Vec<PathBuf>>;
}
```

- **Sync vs async**: everything here is a child process or CPU work. A **blocking** `convert()` run on a thread pool
  (rayon, or `std::thread` + a per-engine `Semaphore`) is simpler than tokio and fits a CLI + Tauri app well. If we use
  tokio, use `tokio::process::Command` with `kill_on_drop(true)` and `tokio::sync::Semaphore` per engine id.
- **Cancellation**: `tokio_util::sync::CancellationToken`, or an `Arc<AtomicBool>` polled every 100-250 ms in the wait
  loop.
- **Progress**: a `Fn(f64)` callback, or an `mpsc`/`watch` channel to the UI. Keep the **monotonic clamp** in the queue.
- **Routing**: Dijkstra with a max of 3 hops over `(engine, edge)`. A `BinaryHeap<Reverse<(cost, node)>>` is enough. The
  `petgraph` crate is optional and not needed.

### 12.2 Process runner (the most important building block)

- Use `std::process::Command` with `.args()` (no shell, so no quoting issues), `Stdio::piped()` for both streams, and
  one reader thread per stream that sends lines to a callback.
- **Kill the whole tree**: on Unix, start the child in its own process group
  (`std::os::unix::process::CommandExt::process_group(0)`, stable) and `libc::killpg(pgid, SIGKILL)`. On Windows use a
  Job Object. The `command-group` crate wraps both platforms.
- Keep two modes: `run(timeout)` for gs, ffprobe and soffice, and `run_with_stall_watchdog(10 min)` for ffmpeg.
- Keep only the last 40 stderr lines in a `VecDeque`. Port `summarize()` (strip `[...] ` prefixes, de-duplicate, last 3).
- Set the working dir to the exe's folder only on Windows. On Linux it doesn't matter, so use the work dir.
- Pass env overrides for gs (`GS_OPTIONS`, `GS_LIB`, `GS_FONTPATH` = "").

### 12.3 Tool discovery

- The `which` crate covers PATH lookup. Add the fixed Unix folders, then require **all** programs of a package in one
  folder (ffmpeg + ffprobe). Cache the result in a `OnceLock`/`DashMap` with a `forget()` method.
- Version: run `ffmpeg -hide_banner -version` once, cache per (path, mtime), use the `regex` crate. Arch currently ships
  `ffmpeg n9.0.2`, which matches Filee's pinned 9.0.2.
- Config override: `engines.<id>.path` in our settings file maps to the "own copy".

### 12.4 Per engine

| Filee engine | Rust approach for MorphRing | crates / tools |
|---|---|---|
| ImageMagick (Magick.NET) | Shell out to `magick` and build args from `ApplyOptions` (§4.6 table). Fallback for basic formats: the `image` crate. | `magick` CLI (installed, Q16-HDRI 7.1.2-32 with HEIC/AVIF/JXL/WebP/JP2) |
| FFmpeg | Port `MediaEncoding` as **pure functions returning `Vec<String>`**, then unit-test the args the way Filee does. Port `MediaInfo::parse` with serde_json, accepting numbers as strings or numbers. | `serde_json`, `ffmpeg`/`ffprobe` CLI. **Do not** use `ffmpeg-next` bindings in v0.1 (build pain, licensing). |
| HEIC writer | v0.1: `magick … out.heic` (libheif). Later: port HeifFile/HevcStream (~600 LOC, pure byte work). | — |
| ICNS | Port IcnsFile (~240 LOC, simple), or use the `icns` crate (PNG-only write). Resize with `magick` or `image` + `fast_image_resize`. | `icns`, `image` |
| SVG | **`resvg` + `usvg` + `tiny-skia`** (pure Rust, no network by default) for raster. `svg2pdf` (typst) for SVG→PDF, which writes vector output and subsets fonts, avoiding Skia's whole-font problem. | `resvg`, `svg2pdf`, `flate2` (svgz) |
| Ghostscript | Same args as §8.2 through the process runner. Only "available" when `gs` is found. | `gs` CLI (not installed yet) |
| PDF → image | `pdftoppm -r DPI -png [-f N -l M] in.pdf prefix` (poppler, installed), or the `pdfium-render` crate (needs libpdfium). Keep MaxParallelism = 1 for pdfium. | poppler-utils |
| Image → PDF / split / merge | `lopdf` (pure Rust) or `printpdf`. `qpdf --pages` / `--split-pages` is an easy CLI route. Simplest path for v0.1: `magick img1 img2 out.pdf` (no Ghostscript needed for writing). | `lopdf`, `qpdf` |
| PDF → TXT | v0.1: `pdftotext -layout` or plain `pdftotext` (installed). The `pdf-extract` crate is an alternative. Filee's heuristic reader is a long-term port. | poppler-utils |
| Markdown | `pulldown-cmark` (or `comrak` for GFM tables) + the same HTML wrapper and image-URL rewrite. `encoding_rs` + `chardetng` for encoding sniffing. | `pulldown-cmark`/`comrak` |
| Installer | `reqwest` (blocking, redirects **off**, follow by hand ≤ 10 hops), `sha2` incremental hashing, `.part` then rename, staging dir then rename. Unpack with `tar` + `xz2`/`flate2`, or `zip`. On Unix, `std::fs::set_permissions` with mode `0o755`. | `reqwest`, `sha2`, `tar`, `xz2`, `flate2`, `zip`, `tempfile` |

### 12.5 Pitfalls to avoid

- **Even dimensions** in every FFmpeg scale filter. Copy the exact filter strings, including the quotes inside `-vf`.
  Because we pass args as a list, the inner single quotes are for FFmpeg's expression parser, not the shell. Keep them.
- **Locale**: always format floats with `.` (Rust's `format!` already does). Use `{:.3}` trimmed for `-ss`, matching
  `"0.###"`.
- **Atomic output**: write into the work dir or a temp name, then rename. Delete partial outputs on error or cancel.
- **Allocator `None` = skip**, not an error.
- **ffprobe numbers are strings.**
- `-progress pipe:1` puts progress on **stdout**, and errors go to stderr. Read both pipes at once or the child can
  deadlock on a full pipe.
- Ghostscript's `-sOutputFile` treats `%` specially, so always use a scratch folder.
- ImageMagick on Arch: reading PDF/EPS needs `gs`, and DNG needs `darktable-cli`. Do not route PDF through magick.
- Wayland or Tauri has no effect here. Engines are headless.

---

## 13. v0.1.0 candidates (ship today)

Ranked by value per hour, Linux-first, using tools that are **already installed** (ffmpeg, magick, soffice, poppler,
resvg):

1. **Converter trait + registry + Dijkstra route planner** (max 3 hops, cost-based) with Filee's cost rules. Small, and
   it unlocks multi-step routes.
2. **Process runner** with timeout, stall watchdog, process-group kill, line callback and stderr tail + `summarize()`.
   Every other engine depends on it.
3. **Tool discovery**: PATH + fixed folders + "all programs in one folder" + a config override. No downloader in v0.1.
   Show a "not installed: `sudo pacman -S ghostscript`" style reason instead.
4. **FFmpeg engine (subset)**: ffprobe parse, **remux rules**, video targets `mp4 webm mkv mov`, audio targets
   `mp3 m4a wav flac ogg opus`, the 2-pass **GIF**, the **poster frame** (video→png), `-progress` parsing with weighted
   passes. Port `MediaEncoding` as pure, unit-tested functions. Skip vob/dv/mxf/amr/cover-art-for-ogg for now.
5. **ImageMagick engine via CLI**: formats `png jpg webp tiff bmp gif ico avif heic jxl`. Port the `ApplyOptions` subset
   (auto-orient, the three resize modes, format max side, grayscale, strip keeping ICC, alpha flatten for jpg, quality,
   webp lossless, ico auto-resize), the frame-selection rules and the `_p{n}` naming. **HEIC write comes free** through
   libheif.
6. **ProgressEstimate** (exponential fake progress) for tools that report nothing (magick, gs, soffice).
7. **SVG → PNG/JPG/…** with `resvg` (min 1024 px long edge, DPI 150, caps 16384 / 100 MP) and **svg ↔ svgz**.
8. **PDF → PNG/JPG** with `pdftoppm` (DPI clamp 36..1200, page range parser, `_p{n}`), and **image → PDF / PDF merge**
   with `magick` or `lopdf`.
9. **PageRange parser** (tiny, pure, easy to test).
10. **PDF → TXT** with `pdftotext` (UTF-8 BOM output to match Filee).

Defer to v0.2+: the engine downloader/installer, the custom HEIF writer, the ICNS writer/reader (unless someone has a
spare hour; it is self-contained), Ghostscript EPS/PS (needs `gs` installed), XCF flattening, RAW defines, PDF combine/
trim/split-tall, the PdfDocumentReader layout engine, vob/dv/mxf targets, and Ogg/MKA cover art.
