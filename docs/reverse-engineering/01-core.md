# 01 — Filee.Core: reverse-engineering spec

Scope: `src/Filee.Core/**` of Filee (C#/.NET 10, MIT). This covers the format registry, route planning, the job
queue, presets and output naming, settings storage and migrations, history, toolbar profiles and watch folders.
All paths below are relative to the Filee repository root. Line numbers refer to the snapshot we reviewed.

Filee.Core is small (about 3,700 lines in 25 files), pure logic, and has **no UI or OS dependencies** (see
`src/Filee.Core/Filee.Core.csproj:1-5`). It also **never starts an external program**. Every external tool call
(ImageMagick, FFmpeg, LibreOffice, Ghostscript, Calibre, Pandoc...) lives in `src/Filee.Engines/**` behind the
`IConverter` interface. Those calls are covered by the engines spec, not here. What this document gives you is the
contract those engines plug into.

Size of each part, as a rough map:

| File | Lines | What it is |
|---|---|---|
| `Conversion/JobQueue.cs` | 487 | Background queue, parallelism, failure isolation, output allocation |
| `Settings/UserDataStore.cs` | 434 | JSON persistence and schema migrations |
| `Watching/FolderWatcher.cs` | 313 | Watch folder: settle detection and batch conversion |
| `Presets/BuiltInData.cs` | 281 | Factory presets and toolbar profiles |
| `Formats/FormatRegistry.cs` | 266 | All formats and extension lookup |
| `Presets/Preset.cs` | 263 | Preset data model and option groups |
| `Settings/AppSettings.cs` | 177 | Settings root object |
| `Presets/OutputRule.cs` / `OutputPathResolver.cs` | 146 / 144 | Output location, naming, rename steps, collisions |
| `Conversion/IConverter.cs` | 120 | Engine contract |
| `Watching/FileNameFilter.cs` | 118 | Glob/regex filter for watch folders |
| `Conversion/RoutePlanner.cs` | 100 | Dijkstra over formats |
| everything else | < 100 each | Job model, catalog, page range, history, profiles, gestures, localization, platform |

---

## 1. Format registry

Files: `src/Filee.Core/Formats/FileFormat.cs`, `src/Filee.Core/Formats/FormatRegistry.cs`.

### 1.1 Data model

```text
enum FormatCategory { Image, Pdf, Document, Spreadsheet, Presentation, Hwp, Text,
                      Video, Audio, Ebook, Archive, Vector, Cad, Font }

record FileFormat(
    Id: string,              // stable lower-case id, used by presets and converter edges ("jpg")
    DisplayName: string,     // UI label ("JPG", "JPEG XL")
    Category: FormatCategory,
    Extensions: string[],    // no dot; FIRST one is used when writing; compound allowed ("tar.gz")
    Writable: bool = true)   // false = read-only, never offered as a target
  PrimaryExtension = Extensions[0]
```

### 1.2 The list

`FormatRegistry.Known` (`FormatRegistry.cs:13-216`) is a static, ordered list. The order is the order shown in
format pickers. About 170 formats in total. Summary by category (read-only formats in brackets):

- **Image**: png, jpg (jpg/jpeg/jpe/jfif), webp, tiff (tiff/tif), bmp (bmp/dib), gif, ico, avif, heic (heic/heif),
  jxl, jp2 (jp2/j2k/jpf/jpx), psd, psb, tga (tga/icb/vda/vst), ppm (ppm/pgm/pbm/pnm), icns, [xcf],
  [raw: dng cr2 cr3 crw nef nrw arw srf sr2 orf rw2 raf pef dcr kdc erf mos mrw x3f 3fr fff iiq mef rwl srw raw].
  Note that all camera RAW extensions map to a single format id `raw`.
- **Vector**: svg, svgz, eps (eps/epsf/epsi), ps, odg, [ai, emf, wmf, cdr, cgm, vsd (vsd/vsdx)].
- **Pdf**: pdf.
- **Document**: docx, docm, dotx, dotm, doc, odt, rtf, [dot, wps, wpd, lwp, abw (abw/zabw), pages, sdw, pub, eml].
- **Hwp** (Korean Hancom): hwpx, hwp.
- **Spreadsheet**: xlsx, xlsm, xltx, xls (xls/xlt), ods, csv, tsv (tsv/tab), [et, numbers, sdc].
- **Presentation**: pptx, pptm, potx, ppsx, ppt (ppt/pps/pot), odp, [key, dps, sda (sda/sdd)].
- **Text**: txt, md (md/markdown), html (html/htm/xhtml), rst, tex (tex/latex).
- **Ebook**: epub, mobi, azw3, fb2, cbz, htmlz, txtz, lit, lrf, pdb, pml, rb, snb, tcr,
  [azw, azw4, prc, cbr, cb7, cbt, cbc, chm, oeb (oeb/opf)].
- **Video**: mp4, m4v, mov (mov/qt), mkv, webm, avi (avi/divx), wmv (wmv/asf), flv (flv/f4v),
  mpg (mpg/mpeg/mpe/m1v/m2v), ts, m2ts (m2ts/mts/m2t), 3gp (3gp/3gpp), 3g2, ogv, vob, dv (dv/dif), mxf,
  [mod (mod/tod), rm (rm/rmvb), swf, wtv (wtv/dvr-ms/dvr), cavs].
- **Audio**: mp3, m4a, m4b, aac, wav (wav/wave), flac, ogg (ogg/oga), opus, wma, aiff (aiff/aif/aifc), ac3, amr,
  au (au/snd), caf, weba, mka, mp2 (mp2/mpa), voc, [dss].
- **Archive**: zip, 7z, tar, tgz (tar.gz/tgz/taz), tbz2 (tar.bz2/tbz2/tar.bz/tbz), txz (tar.xz/txz), gz, bz2 (bz2/bz),
  xz, [rar, tar7z (tar.7z), tlz (tar.lz/tlz), tz (tar.z/tz), lz, lzma, z, jar, cab, cpio, deb, rpm, dmg, iso, img,
  lha (lha/lzh), arj, alz, egg], plus the pseudo format **`folder`** (`FormatRegistry.Folder`, extension `""`).
- **Cad**: dwg, dxf.
- **Font**: ttf, otf, woff, woff2, eot.

Copy the full table from `FormatRegistry.cs:13-216` verbatim into a Rust `static` when porting. It is plain data.

### 1.3 Lookup rules

- `ById`: case-insensitive dictionary from id to format (`:218-219`).
- `ByExtension`: case-insensitive dictionary from every non-empty extension to its format (`:221-223`). Extensions
  are unique across formats; .NET's `ToDictionary` would throw at startup if two formats claimed the same one.
  A Rust port should assert this in a unit test.
- `CompoundExtensions`: every extension containing a `.`, sorted **longest first** (`:226-227`).
- `ExtensionOf(path)` (`:247-257`):
  1. Take the file name only.
  2. For each compound extension (longest first): if `name.len() > compound.len() + 1` and the name ends with
     `"." + compound` (case-insensitive), return the last `compound.len()` characters of the name. **The original
     casing is kept**, so `Backup.TAR.GZ` returns `TAR.GZ`.
  3. Otherwise return `Path.GetExtension(name)` without the dot (the last `.xxx`), or `""` if none.
  The length check stops a file named exactly `.tar.gz` from being read as an extension with an empty stem.
- `NameWithoutExtension(path)`: the file name minus `"." + ExtensionOf(...)`, so `backup.tar.gz` gives `backup`.
- `FindByExtension(ext)`: trims leading dots, returns null for empty input.
- `Detect(path) = FindByExtension(ExtensionOf(path))`. **Detection is by extension only.** There is no magic-byte
  sniffing anywhere in Core.

Gotcha: `Path.GetExtension(".bashrc")` is `.bashrc` in .NET, so dotfiles are seen as having the extension
`bashrc`. Rust's `Path::extension()` returns `None` for `.bashrc`. This only matters for edge cases. Watch folders
ignore dotfiles anyway.

---

## 2. Converter contract and catalog

Files: `src/Filee.Core/Conversion/IConverter.cs`, `src/Filee.Core/Conversion/ConverterCatalog.cs`.

### 2.1 Types

```text
record struct ConversionEdge(From: string, To: string, Cost: int = 10)
  // One direct conversion. From == To is allowed (re-encode, split, compress, resize).
  // Cost 10 = normal; higher for lossy or slow. Multi-step routes add up the costs.

record EngineStatus(IsAvailable: bool, ReasonKey: string?, Detail: string?, Version: string?)
  // ReasonKey is a localization key such as "engine.reason.not_installed".

interface IOutputAllocator {
  Allocate(extension: string, suffix: string? = null) -> string?
  // extension without dot; "" asks for a FOLDER path (archive extraction, the converter creates it)
  // suffix such as "_p2" when one input gives several outputs (PDF pages)
  // returns null = "skip this output" (file exists + Skip policy)
}

record ConversionStep(InputPath, From, To, Preset, Output: IOutputAllocator, WorkDirectory)

interface IConverter {
  Id: string                 // lower-case, stable ("magick", "ffmpeg", "libreoffice")
  DisplayName: string
  Edges: ConversionEdge[]
  MaxParallelism: int        // 0 = one per CPU core
  GetStatus() -> EngineStatus   // should be cheap; result is cached by the catalog
  ConvertAsync(step, progress: IProgress<double>?, ct) -> Task<string[]>   // returns the files written
}
```

Rules stated in the contract (`IConverter.cs:63-66`): a converter must be thread-safe up to `MaxParallelism`
concurrent calls, must honour cancellation, and must **never modify the input file**.

The important design point: **converters do not choose their own output paths.** They ask the allocator, once per
output file. This puts naming, collision handling and the "skip" policy in one place, and lets a converter write
several outputs (one per page) without knowing the naming rules.

Three optional "batch" helpers are separate interfaces, used only by the queue's special modes:

- `IPdfMerger.MergeAsync(inputs, outputPath, ct)`: implemented by the PDF engine.
- `ITiffMerger.MergeAsync(inputs, outputPath, ImageOptions, ct)`: implemented by the ImageMagick engine.
- `IFileCombiner.CanCombineInto(format)` and `CombineAsync(inputs, outputPath, format, preset, workDir, progress, ct)`:
  implemented by the archive engine. Files are stored under their own names, renamed `name (2)` on clashes, and the
  archive is written only once complete.

For reference, the engine ids and their `MaxParallelism` in `src/Filee.Engines`: magick 0, pdfium 1, pdfsharp 0,
rhwp 2, unhwp 0, markdown 0, spreadsheet 0, ooxml 0, hwpx-writer 0, docx-writer 0, pdf-text 0, email 0, vector 0,
icns 0, font 0, cad clamp(cores/2, 1, 4), archive 2, ebook 0, ffmpeg 2, pandoc 0, ghostscript 0, calibre 2,
libreoffice "Workers" (configurable).

### 2.2 ConverterCatalog

- Holds every registered converter in **registration order** (`ConverterCatalog.cs:12`).
- `Priority`: a list of engine ids in preferred order. Setting it raises `Changed`.
- `PriorityIndex(c)` = the position of `c.Id` in `Priority` (case-insensitive), or `Priority.len()` if not listed.
- `StatusOf(c)`: computed lazily **once for all converters** on first call, then cached. A throwing `GetStatus`
  becomes `Unavailable("engine.reason.error", ex.Message)` (`:71-81`).
- `Refresh()`: drops the cache (after the user changes an engine path) and raises `Changed`.
- `CreatePlanner()`: a `RoutePlanner` over the **available** converters only, with per-step penalty
  `min(PriorityIndex(c), 9)` (`:53-54`).
- `CreatePlanner(assumeInstalled)`: same, but also counts the listed engine ids as available. The UI uses this to
  show "installing X would unlock these conversions".

Default `EnginePriority` (`AppSettings.cs:89-94`):
`magick, pdfium, pdfsharp, rhwp, unhwp, markdown, spreadsheet, ooxml, hwpx-writer, docx-writer, pdf-text, email,
vector, icns, font, cad, archive, ebook, ffmpeg, pandoc, ghostscript, calibre, libreoffice`.
Built-in engines come first and LibreOffice is the last-resort fallback. Because the penalty is capped at 9, only
the first nine engines are actually told apart. Every engine from position 9 onward (`docx-writer` onward) gets the
same penalty of 9, and ties between them are broken by edge enumeration order (see below).

---

## 3. Route planning (multi-step routes)

File: `src/Filee.Core/Conversion/RoutePlanner.cs`.

```text
record RouteStep(Converter, From, To)
record Route(Steps: RouteStep[], Cost: int)
const MaxSteps = 3
```

`Plan(from, to)` (`:36-92`):

1. Lower-case both ids.
2. **Same format** (`from == to`): look only for an explicit **self edge** (`From == To == from`) among all
   converters and take the cheapest (`edge.Cost + penalty`). No multi-hop round trips (png to bmp to png) are ever
   considered. Returns a one-step route or null.
3. **Different formats**: Dijkstra over format nodes.
   - Edges = every `(converter, edge)` with `From != To`, cost = `edge.Cost + penalty(converter)`, with ids
     lower-cased.
   - `dist[from] = 0`, `hops[from] = 0`, min-priority queue keyed by cost.
   - Pop `(node, cost)`. Skip stale entries (`cost > dist[node]`). Stop when `node == to`. Do not expand a node with
     `hops[node] >= MaxSteps` (3).
   - Relax each outgoing edge: if `dist[next]` exists and `<= newCost`, skip. Otherwise set dist, hops and prev
     (the predecessor node and the RouteStep), and enqueue.
   - **Ties keep the first edge found.** Because of the `<=`, the first converter (in the order converters were
     listed, then edge order) that reaches a node at a given cost wins.
4. If `to` has no predecessor, return null. Otherwise walk prev back from `to` and reverse.

`CanConvert(from, to) = Plan(from, to) != null`.

Design notes:

- The **priority penalty (0-9) is smaller than the base edge cost (10)**, so it breaks ties between engines but
  should not make a route longer. Even so, two 10-cost hops (20) can never lose to one hop that costs over 20 after
  penalties. That is the intent.
- A worked example from the comments: HWPX to PNG = HWPX to PDF (rhwp) followed by PDF to PNG (PDFium). Nothing
  declares this route anywhere. It appears simply because those two engines are installed.

**Fragile spots** (worth fixing in the port):

- The hop limit is applied inside Dijkstra, which makes the search inexact. A node first reached by a cheap
  3-hop path stores `hops = 3` and is never expanded, even if a slightly more expensive 2-hop path to it would have
  led on to the target. With the real edge costs this rarely matters. A correct version keys the state on
  `(format, hops)`. The graph is tiny, so that is cheap.
- `MaxSteps` caps the route at 3 converter calls.
- The planner scans the whole edge list on every pop (`edges.Where(e => e.Edge.From == node)`), which is
  O(V·E). That is fine for about 170 formats and a few hundred edges.
- A planner is created **once per job** (`JobQueue.cs:67`), so an engine that becomes available mid-job is not
  used until the next job.

---

## 4. Job model and job queue

Files: `src/Filee.Core/Conversion/ConversionJob.cs`, `src/Filee.Core/Conversion/JobQueue.cs`.

### 4.1 Data model

```text
enum JobState  { Queued, Running, Completed, CompletedWithErrors, Failed, Cancelled }
enum FileState { Pending, Running, Done, Skipped, Failed, Cancelled }

class FileResult {
  SourcePath: string
  State: FileState = Pending
  Progress: f64 (0..1)
  Outputs: string[]
  ErrorKey: string?      // localization key: error.unsupported_source | error.no_route | error.conversion_failed
  ErrorDetail: string?   // raw detail: ".xyz", "docx → png", or the exception message
}

class ConversionJob {      // one "drop": N files, one preset
  Id = first 10 hex chars of a new GUID
  CreatedAt = local now
  Sources: string[]
  Preset = preset.Clone()       // snapshot: later edits do not affect a running job
  PresetDisplayName: string     // already localized; used for the {preset} token
  Files: FileResult[] (one per source, same order)
  State: JobState = Queued
  Progress = mean(Files.Progress), or 1 when there are no files
  Outputs = all files' Outputs
  Cancel() -> cancellation token
}
```

### 4.2 Queue topology and parallelism

- **One unbounded channel and one worker task** (`JobQueue.cs:21, 39, 423-436`). **Jobs run one after another**,
  in FIFO order. A job cancelled while still queued is marked `Cancelled` without running.
- **Inside a job, files run in parallel**: `Parallel.ForEachAsync` with `MaxDegreeOfParallelism =
  Environment.ProcessorCount` (`:88-91`). The loop's own token is `CancellationToken.None` so that every file gets
  visited and marked, even after a cancel.
- **Per-engine concurrency gates**: one semaphore per converter id, sized `MaxParallelism` (0 means CPU count),
  shared across all jobs (`:401-406`). The gate is taken **around each single step call**, not around a whole
  file, so a two-step route releases engine A before it waits for engine B. That avoids deadlocks and lets a slow
  engine (FFmpeg = 2) throttle itself while cheap engines keep going.
- Effective parallelism = min(CPU cores, engine gate).

### 4.3 `RunAsync(job)` control flow (`:58-113`)

1. `State = Running`, notify.
2. Work directory: `$TMP/Filee/<jobId>/`, created now and **deleted recursively at the end** (errors ignored: a
   viewer may still hold a temp file).
3. `planner = catalog.CreatePlanner()`.
4. Pick a **mode** (first match wins):
   - **merge**: `Preset.Pdf.MergeIntoSingle && TargetFormat == "pdf" && Files.len() > 1 && pdfMerger != null`
   - **multi-page TIFF**: `Preset.Image.MultiPageTiff && TargetFormat == "tiff" && tiffMerger != null`
     (one file is enough: a 10-page PDF becomes one 10-page TIFF)
   - **combine**: `Preset.Archive.CombineIntoOne && combiner != null` and the target is an Archive-category format
     other than `folder`
   - otherwise **per-file** (the normal path)
5. Final job state (`:93-96`):
   - the token was cancelled: `Cancelled`
   - every file is `Done` or `Skipped`: `Completed`
   - at least one file is `Done`: `CompletedWithErrors`
   - otherwise: `Failed`
6. Any exception that escapes the mode functions: log it and set `Failed`. (Per-file errors never get this far.)
7. `finally`: release this job's reserved output paths and delete the work dir.
8. Notify, then raise `JobFinished` exactly once.

### 4.4 Per-file conversion `RunFileAsync` (`:306-399`) — failure isolation

1. `source = Detect(path)`. If unknown: `Fail("error.unsupported_source", "." + ext)`.
2. Target:
   - intermediate mode (merge / TIFF): `"pdf"` or `"tiff"`
   - preset target `"*"` (`BuiltInData.SameAsSource`): the source's own id (so it needs a self edge)
   - otherwise `Preset.TargetFormat`
3. `route = planner.Plan(source, target)`. If none: `Fail("error.no_route", "src → target")`.
4. `State = Running`.
5. For each step `s` of the route:
   - allocator = the **final allocator** if this is the last step *and* this is a final output; otherwise a
     **temp allocator** in the work dir.
   - For **each input** produced by the previous step (fan-out: one PDF can become ten PNGs, and step 2 then runs
     ten times):
     - check cancellation
     - progress mapping: `fileProgress = (s + clamp(p, 0, 1)) / steps`, **only moves forward** (reports can arrive
       out of order from the thread pool)
     - acquire the engine gate, then `ConvertAsync(step)`, then release
   - `inputs = all outputs of this step`
6. On success: if final, append the outputs to `file.Outputs`, and if `Preset.Output.KeepDates`, copy the creation
   and modification times of the source onto every output (file or folder). A failure there is only a warning.
   `State = Skipped` when the final step produced **zero** outputs (every allocation returned null), otherwise
   `Done`. `Progress = 1`.
7. `OperationCanceledException` gives `Cancelled`. Any other exception gives
   `Fail("error.conversion_failed", ex.Message)`.

So **one bad file never fails the job**: every exception is caught per file and recorded on that `FileResult`.

### 4.5 Special modes

**Merge into one PDF** (`:116-154`)
- In parallel, convert every non-PDF file to PDF **in the work dir** (`finalOutput: false`). PDFs are used as they
  are. `parts[i]` keeps the input order.
- Inputs = the non-null parts. If there are none, or the job was cancelled, stop.
- The output is named after **the first file** of the job (index 1), extension `pdf`. If allocation returns null
  (Skip policy), mark **all** files `Skipped`.
- `pdfMerger.MergeAsync(inputs, output)`. The output is attached to the first file. Every file not `Failed` becomes
  `Done`.
- Quirks: the name comes from file 1 even if file 1 failed, and file 1 then holds the output while still being
  `Failed`. A merge exception escapes to `RunAsync` and fails the whole job.

**Multi-page TIFF** (`:160-193`): the same shape. TIFF sources are used as they are; others are converted with
`intermediate: "tiff"` (which can give several pages per file). All pages are flattened in order, then
`tiffMerger.MergeAsync(pages, output, Preset.Image)`.

**Combine into one archive** (`:196-271`)
- Inputs = sources that still exist as a **file or a directory**. A missing one is failed with "The file no longer
  exists." (folders can come from the CLI).
- One output, named after the first existing input, extension = the format's primary extension (`tar.gz` for tgz).
  Skip policy: everything `Skipped`.
- All inputs are `Running`, and they share one progress value (forward only).
- On success all are `Done` and the output is attached to the first input. Cancelled: all `Cancelled`. Exception:
  all `Failed` with `error.conversion_failed`.

### 4.6 Output allocators

**FinalAllocator** (`:456-474`): under a lock on the queue-wide `_reservedOutputs` set (case-insensitive):
- `path = OutputPathResolver.Resolve(rule, tokens, ext, exists, suffix)` where
  `exists(p) = file exists || directory exists || reserved.contains(p)`.
- null means skip. Otherwise add it to `reserved`, create the parent directory, return the path.
- Reservations are released when the job ends (by then the files are on disk).
- Tokens = `(sourcePath, PresetDisplayName, index (1-based position in the job), job.CreatedAt)`.

**TempAllocator** (`:477-486`): `workDir/<8 random hex>/<baseName><suffix>[.<ext>]`. Each output gets a **fresh
random subdirectory**, so intermediate names never collide and keep the source's base name. This matters for tools
like LibreOffice that name their output after the input.

### 4.7 Events

- `JobUpdated(job)`: raised on a background thread, very often (every progress tick). Handler exceptions are
  caught and logged. The UI coalesces them.
- `JobFinished(job)`: raised once per job. The app records history here (see 8).

### 4.8 Known bugs and fragile spots

- **Overwrite + duplicate names in one batch**: `Resolve` returns the candidate under `Overwrite` even when
  `exists()` is true **because of a reservation**. So `a.png` and `a.jpg` with target WEBP and Overwrite both get
  `a.webp`, and the files race. With `Skip`, the second one is skipped. With `Rename` (the default) it works
  (`a (2).webp`). Fix in the port: a reservation should always force renaming, whatever the policy.
- There is a time-of-check/time-of-use gap between allocation and writing for paths outside the batch (another
  program can create the file). It is acceptable, but converters should write to a temp file and rename.
- The job's work dir is under the shared temp dir with a 10-hex id; nothing cleans up leftovers after a crash.
- Jobs are strictly serial: a big video job blocks a later one-image drop.

---

## 5. Presets (data model)

Files: `src/Filee.Core/Presets/Preset.cs`, `OutputRule.cs`, `BuiltInData.cs`.

A preset is one slice of the ring: "convert to `TargetFormat` with these options". Presets are plain mutable
objects serialized directly to JSON. Editors work on a `Clone()` so Cancel can discard changes.

```text
Preset {
  Id: string = 12 hex chars of a new GUID     // referenced by toolbar profiles and watch rules
  Name: string = ""                            // user name; if empty, NameKey is localized
  NameKey: string? = null                      // e.g. "preset.merge_pdf"
  TargetFormat: string = "png"                 // a format id, or "*" = same as the source, or "folder" = extract
  Image:    ImageOptions
  Pdf:      PdfOptions
  Document: DocumentOptions
  Media:    MediaOptions
  Archive:  ArchiveOptions
  Output:   OutputRule
}
DisplayName = Name if not blank, else localize(NameKey) if set, else TargetFormat.upper()
   (Localization/ILocalizer.cs:28-31)

ImageOptions {
  Quality: int = 90                 // 1-100, lossy formats (JPG/WEBP/AVIF/HEIC)
  Resize: None|Percent|LongEdge|Width = None
  ResizePercent: int = 50
  LongEdge: int = 1920              // never upscales
  Width: int = 1200                 // never upscales
  Dpi: int? = null                  // null keeps the source value
  KeepMetadata: bool = true         // EXIF/XMP/ICC
  KeepColorProfile: bool = true     // keep ICC even when metadata is stripped (Display-P3 photos)
  Grayscale: bool = false
  Background: string = "#FFFFFF"    // fill for transparent pixels when the target has no alpha
  TiffCompression: None|Lzw|Zip = Lzw
  MultiPageTiff: bool = false
  WebpLossless: bool = false
  IcoSizes: int[] = [256,128,64,48,32,16]
}
PdfOptions {
  MergeIntoSingle: bool = false
  PageSize: FitImage|A4|Letter = FitImage     // image → PDF
  MarginMm: f64 = 0                          // ignored for FitImage
  RenderDpi: int = 150                       // PDF → image
  PageRange: string = ""                     // "1-3,5", empty = all
  SplitPages: bool = false                   // PDF → PDF, one file per page
  CombinePages: bool = false                 // PDF → one tall image
  TrimPageMargins: bool = false              // with CombinePages
  SplitTallImage: bool = false               // with CombinePages: split instead of downscale (JPG max 65,500 px, WebP 16,383 px)
}
DocumentOptions { PdfA: bool = false }
MediaOptions {
  Quality: High|Balanced|Small = Balanced
  MaxHeight: int = 0              // 0 keeps the source size, never upscales
  AudioBitrateKbps: int = 0       // 0 = a good default for the format
  RemoveAudio: bool = false
  AudioChannels: int = 0          // 0 keep, 1 mono, 2 stereo
  AudioSampleRate: int = 0        // WAV only, 8000..48000; 0 keep
  WavFormat: Pcm16|Pcm8|Pcm24|Pcm32|Float32 = Pcm16
}
ArchiveOptions { CombineIntoOne: bool = false, Level: Store|Fast|Normal|Maximum = Normal }
```

Core only stores these options. The engines interpret them (for example, mapping `MediaQuality` to an FFmpeg CRF).

### 5.1 Page range parsing (`Conversion/PageRange.cs`)

`Parse(expr, pageCount) -> sorted distinct 0-based indices`:
- `pageCount <= 0` gives `[]`. A blank expression gives all pages.
- Split on `,` (entries trimmed, empty ones dropped). Each part is one of `N`, `A-B`, `A-` (to the end), `-B` (from
  1). A non-numeric part is **ignored**. Reversed ranges are swapped. Values are clamped to `1..pageCount`.
- **If nothing valid remains, return all pages** (not an empty list).
- The first `-` is the separator, so negative numbers are not supported.

### 5.2 Built-in presets (`BuiltInData.cs:15-161`)

Ids are stable and referenced by migrations, so keep them exactly:

| id | name / key | target | non-default options |
|---|---|---|---|
| to-png | PNG | png | |
| to-jpg | JPG | jpg | Quality 90 |
| jpg-70 | JPG 70% | jpg | Quality 70, pattern `{name}_70` |
| to-webp | WEBP | webp | Quality 85 |
| to-tiff / to-bmp / to-ico / to-gif | | tiff / bmp / ico / gif | |
| to-avif | AVIF | avif | Quality 60 |
| to-heic | HEIC | heic | Quality 80 |
| half-size | preset.half_size | `*` | Resize Percent 50, `{name}_50` |
| grayscale | preset.grayscale | `*` | Grayscale, `{name}_gray` |
| tiff-300 | TIFF 300dpi | tiff | Dpi 300, RenderDpi 300, `{name}_300dpi` |
| to-pdf | PDF | pdf | |
| merge-pdf | preset.merge_pdf | pdf | MergeIntoSingle, `{name}_merged` |
| pdf-long-image | preset.long_image | png | CombinePages, `{name}_long` |
| pdf-split | preset.split_pdf | pdf | SplitPages, Location Subfolder, SubfolderName `{name}` |
| to-docx, to-hwpx, to-hwp, to-odt, to-rtf, to-txt, to-md, to-html | | (as named) | |
| to-xlsx, to-csv | | xlsx / csv | |
| to-jxl | JPEG XL | jxl | Quality 85 |
| to-psd, to-tga, to-icns, to-svg, to-eps, to-ods, to-pptx, to-odp | | | |
| to-epub, to-mobi, to-azw3, to-fb2, to-cbz | | | |
| to-mp4, to-webm, to-mov, to-mkv, to-avi | | | |
| mp4-720 | MP4 720p | mp4 | MaxHeight 720, Quality Small, `{name}_720p` |
| to-mp3, to-m4a, to-wav, to-flac, to-ogg, to-opus, to-aac | | | |
| mp3-128 | MP3 128k | mp3 | AudioBitrate 128, `{name}_128k` |
| to-zip, to-7z, to-tgz, to-tar | | | |
| extract | preset.extract | `folder` | |
| zip-all | preset.zip_all | zip | CombineIntoOne, `{name}_files` |
| to-dxf, to-dwg, to-ttf, to-otf, to-woff, to-woff2, to-eot | | | |

---

## 6. Output location, naming and collisions

Files: `src/Filee.Core/Presets/OutputRule.cs`, `src/Filee.Core/Presets/OutputPathResolver.cs`,
`src/Filee.Core/Settings/AppSettings.cs:149-165`.

### 6.1 OutputRule

```text
OutputRule {
  Location: Default|SameFolder|Subfolder|CustomFolder = Default
  SubfolderName: string = "converted"     // tokens allowed
  CustomFolder: string = ""
  KeepFoldersFrom: string = ""            // with CustomFolder: mirror subfolders below this root
  FileNamePattern: string = "{name}"      // no extension
  Renames: RenameStep[] = []
  Conflict: Rename|Overwrite|Skip = Rename
  KeepDates: bool = false
}
```

### 6.2 Resolving `Default` (done before the job is queued)

`AppSettings.WithDefaultOutput(preset)` is called by the app and the CLI just before enqueueing:
- If `Location == Default`: clone the preset and copy `DefaultOutput.Location` (if that is also `Default`, use
  `SameFolder`), `SubfolderName` and `CustomFolder` from settings. The defaults are `SameFolder`, `"converted"`, `""`.
- If the global `KeepFileDates` is on, force `KeepDates = true`.
- The name pattern and the conflict policy always stay the preset's own.

Inside the resolver, any leftover `Default` acts like `SameFolder`. So does `CustomFolder` with a blank folder.

### 6.3 `Resolve(rule, tokens, targetExtension, exists, suffix)` (`OutputPathResolver.cs:24-58`)

1. `sourceDir` = the directory of the absolute source path.
2. `dir`:
   - `Subfolder`: `sourceDir / Sanitize(ExpandPattern(SubfolderName), fallback "converted")`
   - `CustomFolder` (non-blank): `CustomFolder / SubfolderBelow(KeepFoldersFrom, sourceDir)`. That is the source
     dir's path relative to the root, or `""` if there is no root, the dir *is* the root, or it lies outside the
     root (`..` or a different drive).
   - otherwise: `sourceDir`
3. `name = Sanitize(ApplyRenames(ExpandPattern(FileNamePattern)), fallback = source name without ext) + suffix`.
   The suffix (such as `_p2`) is appended **after** sanitizing and renaming.
4. `candidate = dir / name[.ext]` (no dot when ext is `""`, which is the folder case).
5. `isSource` = the candidate equals the source path, compared **case-insensitively on every OS** (deliberate: a
   case-insensitive mount on Linux could otherwise overwrite the source).
6. If not the source and not `exists(candidate)`: return it.
7. If not the source and the policy is `Overwrite`: return the candidate. `Skip`: return null.
8. Otherwise (the Rename policy, **or the candidate is the source itself under any policy**): try
   `name (2)`, `name (3)`, ... until it neither exists nor equals the source. **Numbering starts at 2.** The number
   goes after the suffix: `doc_p2 (2).png`.

**The source file is never overwritten, whatever the policy.** A preset with `"*"` target and pattern `{name}`
saved in the same folder therefore gives `photo (2).jpg`.

### 6.4 Pattern tokens (`ExpandPattern`, `:113-126`)

A blank pattern is treated as `{name}`. Tokens are replaced **case-insensitively**, in this order:

| token | value |
|---|---|
| `{name}` | source name without its (possibly compound) extension |
| `{ext}` | source extension (original case, `tar.gz` style) |
| `{preset}` | the job's preset display name (localized) |
| `{date}` | `yyyyMMdd` of the job's `CreatedAt` (local time) |
| `{time}` | `HHmmss` of the job's `CreatedAt` |
| `{index}` | 1-based position of the file in the job |

Because the replacements run one after another, a source *name* that contains `{date}` would itself get
expanded. That is harmless and rare.

### 6.5 Sanitize (`:129-138`)

Replace every OS-invalid file name character, plus `< > : " / \ | ? *`, plus any control character, with `_`.
Then trim whitespace and trailing dots. If the result is empty, use the fallback. Filee uses the Windows-strict set
on every OS, so files stay portable. Note that it does not handle Windows reserved names (`CON`, `NUL`...).

### 6.6 Rename steps (`OutputRule.cs:76-146`)

Applied in order to the expanded pattern, before sanitizing. Every regex has a 250 ms timeout. A timeout or an
invalid pattern leaves the name unchanged.

| kind | effect |
|---|---|
| Replace | case-insensitive literal replace `Find` with `Replace` |
| Remove | case-insensitive literal removal of `Find` |
| Prefix / Suffix | `Replace + name` / `name + Replace` |
| SpacesToUnderscores | regex `\s+` replaced by `_` |
| RemoveCopyNumber | `\s*\(\d+\)$` removed |
| RemoveBrackets | `\s*(\([^)]*\)|\[[^\]]*\]|\{[^}]*\})` removed, then trim |
| RemoveLeadingNumber | `^\d+[\s._-]*` removed |
| RemoveSymbols | `[^\p{L}\p{N}\s._-]` removed |
| Lowercase / Uppercase | invariant case mapping |
| Regex | user regex `Find` replaced by `Replace` (`$1` group references), culture-invariant |

`IsBlank`: Replace, Remove and Regex steps with an empty `Find`, and Prefix and Suffix steps with an empty
`Replace`, do nothing. Watch rules drop blank steps.

---

## 7. Settings storage

Files: `src/Filee.Core/Settings/AppSettings.cs`, `UserDataStore.cs`, `FileeJsonContext.cs`, `TriggerGesture.cs`.

### 7.1 Location and files

- Data dir = `Environment.SpecialFolder.ApplicationData / "Filee"`: `%APPDATA%\Filee` on Windows,
  `~/Library/Application Support/Filee` on macOS, and **`$XDG_CONFIG_HOME/Filee` (usually `~/.config/Filee`) on
  Linux**. The app and the CLI can override it.
- Files: `settings.json`, `presets.json`, `profiles.json`, `history.json`, and a `logs/` folder (app only).
  Presets and profiles are kept apart from settings so they can be reset or exported on their own.

### 7.2 JSON conventions (`FileeJsonContext.cs:10-16`)

Indented, **camelCase** property names, **enums as strings** (the string enum converter writes the member name
unchanged, for example `"SameFolder"`, and reads it case-insensitively), nulls omitted on write, comments and
trailing commas accepted on read. Unknown properties are ignored, and missing ones take their defaults.
`ModifierKeys` is a flags enum, so it serializes as `"Ctrl, Alt"`.

For the Rust port, use serde `rename_all = "camelCase"` for fields, `#[serde(default)]` everywhere, and accept enum
names case-insensitively. We do not need to read Filee's files, so we are free to choose our own enum spelling.

### 7.3 Load (`UserDataStore.cs:55-76`)

1. Create the dir.
2. Read each file. If a file is **missing**, use the defaults (a fresh `AppSettings`, `BuiltInData` presets and
   profiles, an empty history). If it is **corrupt** (bad JSON or an IO error), log it, **copy it to `<file>.broken`**
   and use the defaults. The broken original is overwritten on the next save.
3. Remember the old `SchemaVersion`. Run `SettingsMigrations.Apply(settings)`. Run `ApplyToLibrary(oldSchema,
   presets, profiles)` **only if `profiles.json` existed**. On a fresh install the defaults are already current.
4. `EnsureConsistency()`: drop preset ids that point to missing presets, de-duplicate, cap at 12 per profile, and
   make sure at least one fallback ("mixed") profile exists (re-adding the built-in one if needed).
5. If the schema was older and `settings.json` existed, save it right away. Save presets and profiles if the library
   migration changed anything.

### 7.4 Save

`Write(path, value)` runs under a single save lock: serialize to `<path>.tmp`, then `File.Move(tmp, path,
overwrite)`, which is an **atomic replace**. `SaveSettings`, `SaveLibrary` (which runs `EnsureConsistency` first)
and `AddHistory` each rewrite the whole file. Events fire afterwards: `SettingsChanged`, `LibraryChanged`,
`HistoryChanged`. There is no fsync, so a power cut can still leave a zero-length file on some file systems.

### 7.5 Import / export

`PresetBundle { schemaVersion: 1, presets: Preset[], profiles: ToolbarProfile[] }`. Import **replaces items with the
same id and appends new ones** (imported profiles go to the end, so an imported fallback never takes over as the
catch-all). Then `SaveLibrary`. It returns the number of imported items.

### 7.6 AppSettings fields and defaults

| field | default | notes |
|---|---|---|
| schemaVersion | 9 | `CurrentSchemaVersion` |
| language | "auto" | "en", "ko", "zh-CN" |
| startWithSystem | true | |
| contextMenuEnabled | true | |
| checkForUpdates | true | |
| keepHistory | true | off = nothing new recorded |
| notifiedUpdateVersion | null | |
| notifiedEngineVersions | {} | |
| paused | false | tray "Pause" turns off the gestures |
| theme | Mode System, Accent `#7F77DD`, CornerRadius 16, ReduceMotion null (follow the OS) | |
| donut | OuterRadius 150, HoleRatio 0.42 (0.2-0.7), Opacity 0.96 (0.5-1) | |
| triggers | Drag + Ctrl (Alt on macOS) + Left button; KeyChord Ctrl+Alt+Space | see below |
| excludedProcesses | [] | |
| enginePriority | (list in section 2.2) | |
| firstRunCompleted | false | |
| successfulConversions / feedbackPromptShown | 0 / false | feedback card after 3 full successes |
| watchFolders | [] | `WatchRule[]` |
| defaultOutput | SameFolder, "converted", "" | |
| engineOwnCopies | {} | package id to a user-chosen binary path |
| libreOfficeForSlides | true | route slides to PDF through LibreOffice if it is installed |
| libreOfficeForHwp / ForDocuments / ForSheets | false | |
| keepFileDates | false | global override of `KeepDates` |

`TriggerGesture { id (8 hex), enabled true, kind Drag|Hold|KeyChord, modifiers flags Ctrl|Alt|Shift|Meta,
button None|Left|Right|Middle|X1|X2, key? (SharpHook KeyCode name without "Vc"), dragThreshold 0 (= system),
holdMilliseconds 450, scope FileManager|Anywhere = FileManager }`. This is UI and platform territory, listed here
only for completeness.

### 7.7 Migrations (`UserDataStore.cs:209-434`)

`Apply(settings)` handles engine priority changes for schemas 2-6 using `InsertBefore(list, id, before)` (no-op if
already present, appended if `before` is missing). It restores the default triggers if the list is empty, then sets
`schemaVersion = 9`. `ApplyToLibrary(from, ...)` adds new built-in presets and profiles per schema:

- v4: "text" profile, inserted before the fallback (unless the user already has an `md` profile).
- v5: spreadsheet and presentation presets and profiles (split from "office"); remove PDF-donut targets that only
  Word could produce.
- v6: the whole catalog. New presets are added; built-in profiles learn new extensions that no other profile has
  claimed yet; new profiles are inserted after "text" (with only unclaimed extensions); `to-docx` is put on the PDF
  donut and `zip-all` on the fallback donut.
- v7: the `to-hwp` preset plus a table of "offers" (preset X goes on profile Y after Z).
- v8: presets on `SameFolder` move to `Default` (which starts as SameFolder, so nothing moves on disk).
- v9: `pdf-long-image` and `to-heic` presets; offer `pdf-long-image` on the PDF donut after `to-jpg`.

`Offer(profile, preset, after)` inserts after `after` (or at the end), unless the profile is missing, full (12) or
already has it, or the preset does not exist. The pattern is worth copying: **migrations only add things and
respect what the user changed**.

---

## 8. History

Files: `src/Filee.Core/History/HistoryEntry.cs`, `WatchResults.cs`; recorded from
`src/Filee.App/Services/ConversionService.cs:151-156`.

```text
HistoryEntry {
  jobId, finishedAt (local DateTime), presetId, presetName (display), state: JobState,
  sources: string[], outputs: string[],
  watchRuleId: string?          // the watch folder the job came from
  failures: HistoryFailure[]    // { source, errorKey?, errorDetail? }
  preset: Preset?               // the preset AS RUN, kept only when something failed (for a later retry)
  errors: string[]              // legacy "file: error" lines from Filee <= 1.7
}
```

- `HistoryEntry.From(job)` is built on `JobFinished`. The app sets `watchRuleId`, then
  `store.AddHistory(entry)`.
- `AddHistory`: does nothing if `keepHistory` is off. Otherwise it **inserts at the front** (newest first), trims to
  **200 entries** (`MaxHistory`), and rewrites `history.json` atomically. `ClearHistory` empties the list.
- Retry: failed sources that still exist are re-queued with `entry.preset` (the exact preset the job ran with, with
  its output folder already resolved), so a retry behaves the same even after the preset has been edited or deleted.
- `WatchResults.Of(history, ruleId)`: walk the entries for that rule from newest to oldest. Each source path is
  decided by its **newest** entry only (a file that failed and later converted counts as converted). Returns
  `(convertedCount, failures)`. Paths are compared case-sensitively on Unix and case-insensitively on Windows
  (`Platform/FileSystemPaths.cs`).

---

## 9. Toolbar profiles (which presets the ring shows)

File: `src/Filee.Core/Profiles/ToolbarProfile.cs`.

```text
ToolbarProfile {
  const MaxSlices = 12
  id (12 hex), name = "", nameKey?, extensions: string[] (no dot),
  isFallback: bool       // a "mixed files" profile
  presetIds: string[]    // clockwise from 12 o'clock
}
```

`ProfileSelector.Select(profiles, files)`:
1. extensions = the distinct `ExtensionOf` of each file (case-insensitive).
2. The first **non-fallback** profile whose extensions include **every** file extension.
3. Otherwise the first **fallback** profile with a non-empty extension list that covers every extension.
4. Otherwise `CatchAll`: the first fallback with no extensions, then any fallback, then the first profile.

Built-in profile order: images, pdf, office, spreadsheets, presentations, hwp, text, ebooks, video, audio, vector,
archives, cad, fonts, mixed (fallback). Extensions come from `ExtensionsOf(category)`. The preset lists are in
`BuiltInData.cs:164-273`. For example, images = `to-png, to-jpg, to-webp, to-pdf, to-tiff, to-bmp, to-ico,
half-size` and mixed = `to-pdf, merge-pdf, zip-all, to-png, to-txt`.

Note that the ring shows presets **even when no installed engine can perform them**. Filtering by `CanConvert`, if
any, happens in the App layer.

---

## 10. Watch folders

Files: `src/Filee.Core/Watching/WatchRule.cs`, `FolderWatcher.cs`, `FileNameFilter.cs`; app wiring in
`src/Filee.App/Services/WatchFolderService.cs`, CLI in `src/Filee.Cli/Cli.cs:83-120`.

### 10.1 WatchRule

```text
WatchRule {
  id (8 hex), enabled = true, folder = "", presetId = "",
  includeSubfolders = false,
  outputFolder = ""                 // empty → <folder>/converted
  originals: Keep | MoveToOriginals = Keep    // MoveToOriginals → <folder>/originals
  include: string[] = []            // FileNameFilter patterns; empty = every file
  fileNamePattern = ""              // empty = the preset's
  renames: RenameStep[] = []        // appended after the preset's own renames
}
ResolvedOutputFolder = outputFolder.isBlank ? folder/"converted" : outputFolder
OriginalsFolder      = folder/"originals"
```

`rule.Apply(preset)` clones the preset and forces `Location = CustomFolder`, `CustomFolder =
ResolvedOutputFolder`, `KeepFoldersFrom = folder` (so subfolders are mirrored), then overrides the name pattern if
one is set and appends the non-blank renames. A watch folder therefore **never** uses the `Default` location.

### 10.2 FileNameFilter

- `Normalize(typed)`: trim; blank gives null. `/regex/` or anything containing `*` or `?` stays as it is. A known
  extension (`heic` or `.heic`, single part) becomes `*.heic`. Anything else becomes `*text*` (contains).
- `Of(patterns)`: compiled once and cached by the joined pattern text. Each pattern is a regex: `/.../` is used as
  is, otherwise the glob becomes `^...$` with `*` → `.*`, `?` → `.` and everything else escaped. Case-insensitive,
  250 ms timeout. Invalid patterns are silently dropped. If all are invalid, the filter lets everything through.
- `Matches(fileName)`: true if any pattern matches the bare file name. A timeout counts as no match.

### 10.3 FolderWatcher lifecycle

- `Start()`: fails if the folder does not exist (`Error` is set). Creates an OS watcher (`FileSystemWatcher`; on
  Linux this is inotify) on the folder, recursive if `includeSubfolders`, watching file name, size and
  last-write changes, with a 64 KB buffer. `Created`, `Changed` and `Renamed` all call `Notice(fullPath)`. A watcher
  **buffer overflow** triggers a full `ScanExisting()`.
- If `originals == MoveToOriginals`, the files **already in the folder** are scanned at start-up (the inbox model:
  whatever arrived while nobody was watching). With `Keep`, existing files are left alone, because they were
  presumably converted before.
- A **1 second timer** calls `ProcessPendingAsync()`.

### 10.4 "Wait until the copy has finished" (settle detection)

`Notice(path)`: if `ShouldIgnore`, drop it. Otherwise store or refresh a `Candidate(exists, length, lastWriteUtc,
stableSince)` in a concurrent map keyed by path. `Refresh` keeps the old candidate (and so its `stableSince`) when
the size **and** mtime are unchanged. Otherwise it creates a new candidate with `stableSince = now`.

Every tick (`ProcessPendingAsync`, `FolderWatcher.cs:128-176`):
1. **Non-reentrant**: `busy.WaitAsync(0)`. If the previous batch is still converting, return at once. New files wait
   for a later tick, so **batches are serial per watch folder**.
2. For each pending path: refresh it. If the file is gone, remove it. The file is ready only when **all** of these
   hold:
   - `now - stableSince >= settleTime`. The default settle time is **2 s on Windows and 5 s on macOS/Linux**
     (`:27`), configurable (the CLI has `--settle`).
   - `CanOpenExclusively(path)`: opening with `FileShare.None` succeeds. This works on Windows, where writers
     lock files. On Unix the open nearly always succeeds, which is why the settle time is longer there.
   - not `IsBeingCopiedByFinder` (macOS only: Finder sets the creation date to **24 January 1984** while a copy is
     in progress).
3. Remove ready paths from pending. **De-duplicate** with `handled.add("path|length|mtimeTicks")`. A file already
   converted with the same size and time is not converted again, which stops loops when the file is touched later.
4. If any files are ready: sort the paths, call `convert(ready, stopToken)` (the app routes this through the normal
   queue with `rule.Apply(preset)` and waits for the job to finish). Afterwards, if `MoveToOriginals`, move every
   source that ended `Done`. Raise `Converted(job)`.
5. Any exception is logged and the tick returns 0. `busy` is always released.

`ShouldIgnore(path, rule)` (`:180-203`) returns true for:
- an empty name, names starting with `~$` (Office lock files) or `.` (dotfiles and hidden files on Unix)
- `desktop.ini`, `thumbs.db`
- temporary extensions: `.tmp .temp .part .partial .crdownload .download .opdownload .!ut .filepart`
- anything **inside the output folder or the originals folder** (prevents loops)
- files in subfolders when `includeSubfolders` is off
- unknown formats (`Detect == null`)
- names the include filter rejects
- directories, Hidden or System attributes, or attribute read errors (the file is gone or unreadable)

`MoveToOriginals(rule, source)`: only for sources inside the watched folder and not already in originals. Target =
`originals/<relative path>`, parent directories created, numbered `name (2).ext`, `name (3).ext`... while the target
exists, then `File.Move`. Errors are logged and ignored.

`DisposeAsync`: cancel the stop token, dispose the OS watcher and the timer, then wait for a running batch to finish
(so originals still get moved).

The app (`WatchFolderService.ApplyAsync`) starts one watcher per enabled rule with a folder and a preset, and
restarts a watcher when its rule's "signature" changes. If the preset has been deleted, the convert callback logs a
warning and returns null.

### 10.5 Fragile spots

- `handled` grows without limit for the life of the watcher (a memory leak on very busy folders). Bound it with an
  LRU in the port.
- A file that **fails** is still marked handled, so it is not retried automatically. A retry happens only from the
  UI or history, or when the file changes.
- The app's convert callback ignores the stop token (`(files, _) => ...`), so stopping a watcher does not cancel a
  running job. It only waits for it.
- On Linux with inotify, a recursive watch on a large tree can hit `max_user_watches`.
- An exclusive-open check on Unix proves nothing. Only the settle time protects against half-copied files, so a
  copy that stalls for longer than 5 s will be converted half-done.

---

## 11. Localization and platform (Core side only)

- `ILocalizer` (`Localization/ILocalizer.cs`): `language`, `this[key]` (falls back to English, then to the key
  itself), `Format(key, args)` and a `LanguageChanged` event. The JSON translation files live in the App
  (`src/Filee.App/Assets/i18n`). Core stores only **keys**: `preset.*`, `profile.*`, `error.*`, `engine.reason.*`.
- `FileSystemPaths`: path comparison is case-insensitive on Windows and case-sensitive elsewhere. Note that
  `OutputPathResolver.PathsEqual` (source protection) and the queue's reservation set are **always**
  case-insensitive, on purpose.
- `IPlatformServices`: file manager hit-testing, process under the cursor, file manager selection, drag threshold,
  reduced motion, autostart, context menu, reveal in the file manager, keeping a window on top.
  `NullPlatformServices` is the fallback (drag threshold 4, everything else a no-op, reveal = `xdg-open <dir>` or
  `open <dir>`). This belongs to the platform/UI spec.

---

## 12. Porting to Rust

### 12.1 Crate layout and dependencies

- `morphring-core` (library, no UI): `format`, `route`, `preset`, `output`, `job`, `store`, `history`, `profile`,
  `watch`. Keep it free of Tauri, just like Filee.Core is free of Avalonia.
- Suggested crates:
  - `serde` + `serde_json` for persistence (`#[serde(rename_all = "camelCase", default)]`; enums as strings).
  - `directories` (or `dirs`) for the data dir: `ProjectDirs::from("", "", "MorphRing").config_dir()` gives
    `~/.config/morphring` on Linux.
  - `tempfile` for atomic writes (`NamedTempFile::new_in(dir)` + `persist(path)`) and for the per-job work dir
    (`tempfile::Builder::new().prefix("morphring-").tempdir()`; it deletes itself on drop).
  - `tokio` (rt-multi-thread, process, sync, fs, time) for the queue: `mpsc::unbounded_channel` for jobs,
    `Semaphore` per engine, `tokio::process::Command` with `kill_on_drop(true)` for cancellation, and
    `tokio_util::sync::CancellationToken` for job cancel. If we want to avoid async in v0.1.0, `std::thread` with
    `rayon` or a fixed pool plus a counting semaphore (`std::sync::{Mutex, Condvar}`) works too, because engines
    are just child processes.
  - `regex` for rename steps and filters. It has **no backtracking and linear-time matching**, so the 250 ms
    timeouts are not needed. Note that `regex` does not support look-around or backreferences. User regexes that
    use them fail to compile, so treat them as "invalid pattern, name unchanged", which is Filee's behaviour anyway.
    For case-insensitive literal replace, use `regex::escape` with `(?i)`.
  - `globset` or the hand-rolled glob-to-regex of section 10.2 (it is 10 lines, so hand-roll it).
  - `notify` (v6+) plus our own settle logic for watch folders. Do **not** use `notify-debouncer-*` as the
    settle logic, because Filee's rule is "size and mtime unchanged for N seconds", not "no events for N seconds".
    Use `notify::RecommendedWatcher` (inotify on Linux) and handle `EventKind::Create`, `Modify` and `Rename`.
    On an overflow or error event (`Flag::Rescan`), rescan.
  - `chrono` (or `time`) for the `{date}`/`{time}` tokens in local time, and `filetime` for `KeepDates` (mtime;
    creation time cannot be set on Linux, so skip it there).
  - `uuid` (or `rand`) for ids: `Uuid::new_v4().simple().to_string()[..N]`.
  - `thiserror` for errors (already in the scaffold).

### 12.2 Pitfalls

- **Extensions**: do not use `Path::extension()` for detection. Port `ExtensionOf` exactly: longest compound match
  first, then the last dot. Compare with `eq_ignore_ascii_case`. Watch out for non-UTF-8 paths: use
  `OsStr::to_string_lossy` for detection only and keep the real `PathBuf` for I/O.
- **Case-insensitive source protection**: compare canonical paths with ASCII case folding, like Filee does.
  `std::fs::canonicalize` fails when the file does not exist, so normalize the target's parent directory plus
  the file name instead (or use `std::path::absolute`).
- **Allocator locking**: keep the reservation set in a `Mutex<HashSet<PathBuf>>` owned by the queue, and hold the
  lock across `resolve + insert + create_dir_all`. **Fix the Overwrite bug**: if the candidate is in the
  reservation set, number it regardless of the policy.
- **Engine output naming**: LibreOffice (`soffice --convert-to`) always writes `<outdir>/<input stem>.<ext>` and
  cannot be given an output name. So always run it into a fresh temp dir and **move** the result to the allocated
  path. `fs::rename` fails across file systems (`/tmp` is tmpfs on Arch), so fall back to copy + remove. The
  scaffold already does this. Filee's `TempAllocator` (one random subdir per output) is the same idea.
- **Two LibreOffice processes at once** with the same profile clash. Run them with
  `-env:UserInstallation=file:///tmp/morphring-lo-<n>` or keep the soffice gate at 1 for v0.1.0. (This detail is
  from the engines side, but it affects the gate size we choose.)
- **Progress**: ffmpeg needs `-progress pipe:1` plus the duration to report progress. For v0.1.0, report 0 then 1
  per step. Keep the "forward only" clamp.
- **Child processes on cancel**: `kill_on_drop(true)`, and kill the process group (`setsid` / `pre_exec`) because
  `soffice` forks `soffice.bin`.
- **The planner hop limit**: use a state key `(format, hops)`, or since `MaxSteps = 3`, simply enumerate every path
  of up to 3 hops (a tiny BFS) and take the cheapest. Break ties deterministically: registration order, then edge
  order.
- **`"*"` target**: needs a self edge (`png -> png` on magick). Remember to declare self edges for engines that can
  re-encode, resize or grayscale.
- **Atomic write on Linux**: write the temp file *in the same directory*, `sync_all()`, then rename. Filee skips
  the fsync; we should not.
- **Serde defaults**: put `#[serde(default)]` on every struct, so that old or partial files load. Corrupt file:
  copy to `.broken`, use the defaults, log a warning.
- **Watch folders on Linux**: the exclusive-open check is a no-op on Unix. Optionally add a check that no process
  holds the file open for writing, by scanning `/proc/*/fd` (expensive). Settle time alone (5 s) is Filee's
  answer. Also skip `IN_CLOSE_WRITE`-less editors' temp files (`.swp`, `~` suffix, `.goutputstream-*` from GTK),
  which Filee does not list. Add them to the ignore list.

### 12.3 Suggested Rust shapes

```rust
pub struct Format { pub id: &'static str, pub name: &'static str, pub category: Category,
                    pub exts: &'static [&'static str], pub writable: bool }

pub struct Edge { pub from: &'static str, pub to: &'static str, pub cost: u32 }

pub trait Converter: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn edges(&self) -> &[Edge];
    fn max_parallelism(&self) -> usize;          // 0 = num_cpus
    fn status(&self) -> EngineStatus;
    fn convert(&self, step: &Step, out: &mut dyn OutputAllocator,
               progress: &dyn Fn(f64), cancel: &CancelToken) -> Result<Vec<PathBuf>, ConvertError>;
}

pub trait OutputAllocator { fn allocate(&mut self, ext: &str, suffix: Option<&str>) -> Option<PathBuf>; }

pub struct Route { pub steps: Vec<RouteStep>, pub cost: u32 }
pub fn plan(convs: &[&dyn Converter], penalty: impl Fn(&dyn Converter) -> u32,
            from: &str, to: &str) -> Option<Route>;
```

`OutputPathResolver::resolve` should be a **pure function** taking an `exists: impl Fn(&Path) -> bool` closure, as
in Filee, so it is unit-testable without touching the disk.

### 12.4 Tests to port first (pure logic, cheap)

`ExtensionOf` and `NameWithoutExtension` (compound, casing, `.tar.gz` alone), `PageRange::parse`, `plan` (self
edge, 2-hop, priority tie-break, hop cap), `resolve` (each location, each policy, source never overwritten,
numbering from 2, suffix before number, sanitize fallback), each rename step, `FileNameFilter::normalize` and
`matches`, `ProfileSelector::select`, and the settle state machine with a fake clock (Filee injects `TimeProvider`;
we inject a `now` closure).

---

## 13. v0.1.0 candidates

What is realistic to ship today, in priority order. Everything marked *defer* is out of scope for v0.1.0.

**Must-have (the CLI, and the core under the ring UI):**
1. **Format registry**: the full `Known` table as static data, plus `extension_of`, `name_without_extension`,
   `detect`. Pure data, about 1 hour, unlocks everything else.
2. **Converter trait + three engines** (ImageMagick `magick`, FFmpeg `ffmpeg`, LibreOffice `soffice`), each with
   an edge list including self edges where useful, and `status()` = binary found on PATH plus a `--version` probe.
3. **Route planner** (Dijkstra or brute-force up to 3 hops, priority penalty, self edges). Even with three
   engines it unlocks, for example, DOCX to PNG (soffice to PDF, then magick to PNG).
4. **Output path resolver**: SameFolder + Subfolder + CustomFolder, the `{name} {ext} {preset} {date} {time}
   {index}` tokens, Sanitize, the Rename (default) / Overwrite / Skip policies, numbering from `(2)`, and the
   never-overwrite-the-source rule. Pure and testable.
5. **Job runner**: one job = N files, files in parallel (bounded by CPU count and a per-engine semaphore: magick
   = cores, ffmpeg = 2, soffice = 1), per-file failure isolation with `FileState` and an error key/detail, the
   final `JobState` rule, a per-job temp work dir deleted at the end, cancel support. A single serial job queue
   is fine.
6. **Presets**: the `Preset` struct with `Image` (quality, resize, grayscale), `Media` (quality, maxHeight,
   audio bitrate) and `Output`, plus a trimmed built-in list of the presets our three engines can actually run
   (to-png/jpg/webp/avif/gif/tiff/bmp/ico, half-size, grayscale, to-pdf, to-docx/odt/txt/html, to-xlsx/csv/ods,
   to-pptx/odp, to-mp4/webm/mkv/mov, mp4-720, to-mp3/m4a/wav/flac/ogg/opus, mp3-128).

**Nice-to-have if time allows:**
7. **Settings store**: `settings.json` + `presets.json` in `~/.config/morphring/`, atomic write, `.broken` on
   corruption, `#[serde(default)]`, a `schemaVersion` field from day one (start at 1) so migrations can be added
   later.
8. **History**: an append-to-front list capped at 200 in `history.json`, recorded when a job finishes.
9. **Toolbar profiles + `ProfileSelector`**, so the Tauri ring can choose which slices to show (images / pdf /
   office / video / audio / mixed).

**Defer to v0.2+:**
- Watch folders (notify + settle + originals). It is valuable but has many edge cases. If it is pulled in, port
  `ShouldIgnore`, the size+mtime settle with a 5 s default, the handled-key de-duplication and the
  output/originals exclusion exactly as specified in section 10.
- PDF merge, multi-page TIFF, combine-into-archive and extract modes (they need Ghostscript / qpdf / archive
  engines).
- Rename steps (all 12 kinds), `KeepFoldersFrom`, `KeepDates`, the `Default` location and app-wide default output.
- Preset import and export bundles, schema migrations, engine own-copy paths, trigger gestures (UI side).
