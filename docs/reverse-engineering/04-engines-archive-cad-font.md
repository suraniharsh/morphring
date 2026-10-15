# 04 — Engines: Archives, CAD, Fonts

Reverse-engineering spec of three Filee engines:

- `src/Filee.Engines/Archives/**` (engine id `archive`)
- `src/Filee.Engines/Cad/**` (engine id `cad`)
- `src/Filee.Engines/Fonts/**` (engine id `font`)

All paths are relative to the Filee root. Line numbers refer to the snapshot we read. The supporting docs are
`docs/ENGINES.md` ("Archives", "CAD drawings", "Fonts").

## At a glance: managed code or external tool?

| Engine | Pure managed (in-process) | External / third-party |
|---|---|---|
| Archives | ZIP write, TAR write (PAX), TAR.GZ write, ALZ reader, EGG reader (store/deflate/bzip2), own bzip2 decoder, lzip container parsing, all safety checks | **Bundled 7-Zip console** (`7z.exe`/`7zz`, separate process) for all other reads, for 7Z/TAR.BZ2/TAR.XZ/GZ/BZ2/XZ writes, and for LZMA decoding (EGG LZMA blocks, lzip members) |
| CAD | Scene builder, curve math, text layout, hatch pattern generation, page fitting, renderer (on SkiaSharp) | **ACadSharp** (MIT .NET library) parses DWG/DXF and writes DXF/DWG; **SkiaSharp** draws PDF/SVG/bitmap; **Magick.NET** encodes raster outputs |
| Fonts | Everything: SFNT read/write, WOFF 1, WOFF2 (with glyf/loca and hmtx transforms), EOT, CFF parser, Type 2 charstring interpreter, cubic-to-quadratic, glyf builder | Only zlib and Brotli, which ship with .NET |

The font engine is the cleanest port target (no processes, no native libraries). The archive engine is mostly
orchestration around 7-Zip plus a strict safety layer. The CAD engine is the biggest (about 3,700 lines) and
depends on a DWG parser that has no Rust equivalent.

---

# Part 1 — Archive engine

## 1.1 Files and responsibilities

| File | Lines | Role |
|---|---|---|
| `Archives/ArchiveConverter.cs` | 147 | The engine (`IConverter` + `IFileCombiner`): edge table, extract-to-folder, archive→archive, "compress into one archive" |
| `Archives/ArchiveExtractor.cs` | 120 | Multi-stage unpacking (tar.gz → tar → files, rpm → cpio.xz → cpio → files), dispatch to 7-Zip or own readers |
| `Archives/ArchivePacker.cs` | 334 | Writing archives: ZIP/TAR/TGZ in-process, others via 7-Zip, atomic temp-file-and-rename |
| `Archives/SevenZip.cs` | 218 | Locating and running 7-Zip, parsing `7z l -slt`, mapping errors to user messages |
| `Archives/ArchiveSafety.cs` | 155 | Zip-slip checks, entry-count cap, disk-space check, link removal, Windows-safe names, error messages |
| `Archives/ArchiveFiles.cs` | 160 | Size-capped copy, unique names, "single top folder" rule, folder move/merge, CRC pass-through stream |
| `Archives/AlzReader.cs` | 216 | ALZ (ALZip, Korean) reader |
| `Archives/EggReader.cs` | 322 | EGG (ALZip's newer format) reader |
| `Archives/LzipReader.cs` | 122 | lzip (.lz, .tar.lz) — rewrites members as `.lzma` for 7-Zip |
| `Archives/Bzip2Decoder.cs` | 420 | Managed bzip2 decoder, standard and ALZ framing |
| `Archives/Crc32.cs` | 42 | CRC-32 IEEE, reflected poly `0xEDB88320` |
| `Archives/BoundedStream.cs` | 42 | Read window over one entry's compressed bytes |

## 1.2 Formats and conversion edges

Format ids come from `src/Filee.Core/Formats/FormatRegistry.cs:175-205`. Detection there is **by extension only**
(`FormatRegistry.Detect`, line 241). Multi-part extensions matter: `tgz` = `.tar.gz|.tgz|.taz`, `tbz2` =
`.tar.bz2|.tbz2|.tar.bz|.tbz`, `txz` = `.tar.xz|.txz`, `tar7z` = `.tar.7z`, `tlz` = `.tar.lz|.tlz`,
`tz` = `.tar.z|.tz`, `lha` = `.lha|.lzh`, `bz2` = `.bz2|.bz`. The pseudo-format `folder` (`FormatRegistry.Folder`)
has an empty extension.

Readable (`ArchiveExtractor.cs:13-23`):

- Via 7-Zip: `zip jar 7z rar tar tgz tbz2 txz tar7z tz gz bz2 xz lzma z cab cpio deb rpm dmg iso img lha arj`
- Built in: `alz egg lz tlz` (lz/tlz still need 7-Zip's LZMA decoder)

Writable (`ArchiveConverter.cs:17`, `ArchivePacker.cs:20-23`):

- Archive targets: `zip 7z tar tgz tbz2 txz` plus `folder`
- Single-file targets: `gz bz2 xz` (hold one file, no names)
- Built-in writers (no 7-Zip needed): `zip tar tgz`

Edges (`ArchiveConverter.cs:30-38`):

1. every readable format → every archive target and `folder`;
2. every single-file source (`gz bz2 xz lzma z lz`) → every single-file target (`gz bz2 xz`).

Engine properties: `Id = "archive"`, `DisplayName = "Archives (7-Zip)"`, `MaxParallelism = 2` ("7-Zip already
uses several threads"). `GetStatus()` is *unavailable* when 7-Zip is not found, even though ZIP/TAR writing and the
ALZ reader would work without it; `CanCombineInto` (line 108) is more precise and allows ZIP/TAR/TGZ without 7-Zip.

## 1.3 Data model

```text
SevenZipEntry(Path, IsFolder, Size, IsEncrypted, SymbolicLink?, HardLink?) { Mode? }
  IsSymbolicLink = SymbolicLink non-empty
                 || Mode has a space-separated token of length 10 starting with 'l'   (e.g. "lrwxrwxrwx")
PackItem(SourcePath, EntryName)                 // a file or folder and its name inside the archive
PackFile(Path, EntryName, IsFolder, Length, Modified)   // flattened, folders before their content
ArchiveLevel = Store | Fast | Normal (default) | Maximum   // src/Filee.Core/Presets/Preset.cs:245
```

Level mapping (`SevenZip.cs:99-105`, `ArchivePacker.cs:153-159`):

| ArchiveLevel | In-process (Deflate/GZip) | 7-Zip |
|---|---|---|
| Store | NoCompression | `-mx0` |
| Fast | Fastest | `-mx1` |
| Normal | Optimal | `-mx5` |
| Maximum | SmallestSize | `-mx9` |

## 1.4 Control flow

### Extract to folder (target `folder`) — `ArchiveConverter.ExtractToFolderAsync`, lines 84-106

1. `output = step.Output.Allocate("")` (the output rule picks the folder name and handles conflicts; null means
   "skip").
2. Staging folder **next to the output** (same volume, so the final move is a rename):
   `<outdir>/.<outname>.filee-<8 hex>`, marked Hidden.
3. `extractor.ExtractAsync(input, from, staging)` with a time-based progress estimate (0.02 → 0.95, assuming
   ~60 MB/s, min 0.5 s; `TypicalDuration`, line 142).
4. `ArchiveFiles.ContentRoot(unpacked)`: if the unpacked folder holds exactly one entry and it is a directory, use
   that directory (so `photos.zip` containing `photos/...` gives `photos/...`, not `photos/photos/...`).
5. `ArchiveFiles.MoveFolder(contentRoot, output)`:
   - a *file* at the target → error "A file named X is in the way of the extracted folder";
   - target missing → `Directory.Move`; on IOException across volumes fall back to merge;
   - target exists (preset overwrite mode) → `MergeInto`: refuse if target is a reparse point (link); move files
     with overwrite; recurse into folders; a file in the way of a folder is deleted; copy the folder mtime.
6. `finally`: delete the staging folder (errors ignored).

### Archive → archive — `ArchiveConverter.ConvertAsync`, lines 46-78

1. Scratch: `<work>/archive-<8 hex>`.
2. Extract (progress 0.02 → 0.45).
3. Items = top-level entries of the unpacked folder, sorted ordinal, each packed under its own name.
4. `output = step.Output.Allocate(primaryExtension)`; pack with `ArchivePacker.PackAsync(... level, scratch,
   progress 0.5 → 1)`.
5. `finally`: delete scratch.

Note: archive→archive does **not** apply the single-top-folder rule; the structure is kept.

### Compress into one archive — `CombineAsync`, lines 113-139

All dropped files (any type) go into one archive. `UniqueEntries`: each path keeps its file name; later
duplicates (case-insensitive) become `stem (2).ext`, `stem (3).ext`, ... Trailing separators are trimmed first so
folders work. 7-Zip is only located when the format is not ZIP/TAR/TGZ.

### Multi-stage extraction — `ArchiveExtractor.ExtractAsync`, lines 37-59

```text
current = archive, fmt = format, previous = null
for stage = 0..:
    folder = scratch/x<stage>; mkdir
    ExtractOnce(current, fmt, folder)
    RemoveLinks(folder)                      // delete symlinks/junctions without following
    delete previous stage folder (if any)
    if stage+1 < 3 and InnerArchive(ORIGINAL format, folder) is (path, innerFmt):
        previous = folder; current = path; fmt = innerFmt; continue
    return folder
```

`InnerArchive` (lines 88-106) only applies when the **original** format is TAR-wrapped
(`tgz tbz2 txz tar7z tz tlz`) or `rpm`, and the stage produced exactly one regular file:

- the file is a TAR (`.tar` extension, or `"ustar"` at bytes 257..262) → `tar`;
- for `rpm`: format by extension (`FormatRegistry.Detect`) or raw extension, accepted if in
  `cpio gz bz2 xz lzma z zst`.

Max 3 stages (`MaxStages`), enough for `rpm → cpio.xz → cpio → files`.

`ExtractOnce` (lines 61-85) dispatch:

| Format | Handler |
|---|---|
| `alz` | `AlzReader.Extract` (sync, on thread pool) |
| `egg` | `EggReader.ExtractAsync` (7-Zip optional, only for LZMA blocks) |
| `lz`, `tlz` | `LzipReader.DecompressAsync` → output file named `<name without ext>` (+ `.tar` for tlz); 7-Zip required |
| everything else | 7-Zip: `ListAsync` → `ArchiveSafety.CheckListing` → `ExtractAsync` excluding every symlink entry |

## 1.5 Exact 7-Zip invocations — `SevenZip.cs`

Locating (line 40): only Filee's **bundled** copy, `EngineEnvironment.FindBundled("7zip")` then `7z.exe` on
Windows, `7zz` elsewhere. A system 7-Zip is never used. Timeout: 6 hours (only kills hung processes).
Every read passes a dummy password `-pfilee-no-password` so 7-Zip never blocks on a console prompt; encrypted
archives then fail with "Wrong password", which maps to the "password-protected" message.

List:

```text
7zz l -slt -sccUTF-8 -bsp0 -pfilee-no-password -- <abs archive>
```

Accept exit 0 or 1. Parse (lines 108-156): ignore everything until a line starting with `----------`; then blocks
of `Key = Value` lines separated by blank lines (a line ending in ` =` is an empty value). A block without `Path`
is dropped. Fields used: `Path`, `Folder` (`+`) or `Attributes` starting with `D`, `Size`, `Encrypted` (`+`),
`Symbolic Link`, `Hard Link`, `Mode` (fallback `Attributes`).

Extract:

```text
7zz x -y -aou -sccUTF-8 -scsUTF-8 -bsp0 -bb0 -pfilee-no-password -o<abs outdir>
    [-spd -x@<work>/exclude-<guid>.txt]  -- <abs archive>
```

- `-aou` renames clashing names (`A.txt` vs `a.txt` on case-insensitive FS) instead of overwriting.
- The exclude list holds the symlink entry paths, UTF-8 without BOM, one per line; `-spd` disables wildcards so
  `*` in a name is literal.
- Success: exit 0 or 1, or exit 2 when every error line mentions `symbolic link` or `Dangerous link`.

Add (create):

```text
7zz a -t<type> -mx<0|1|5|9> -ssw -sse -y -sccUTF-8 -scsUTF-8 -bsp0 -bb0 <abs archive> @<work>/items-<guid>.txt
```

- `<type>` ∈ `7z zip tar gzip bzip2 xz`. `-ssw` packs files open in other programs; `-sse` fails instead of
  silently skipping unreadable files. **No `--`** here, because it would disable the `@list` file.
- 7-Zip stores each listed path under its own file name. When an item's entry name differs (a renamed duplicate),
  `NamedAsAsync` (ArchivePacker lines 102-122) first copies it into `<work>/rename-<8hex>/<EntryName>` (folders
  copied recursively, links skipped, mtimes preserved).
- Success: exit 0 or 1 **and** the archive file exists.

Error mapping (`Failure`, lines 171-184), searching stderr+stdout case-insensitively:

| Text | User message |
|---|---|
| `Wrong password`, `encrypted archive` | "X is password-protected. Filee cannot unpack encrypted archives." |
| `Cannot open the file as`, `Is not archive`, `Can not open the file as` | "X is not a valid archive, or not the kind its extension says." |
| `Unexpected end`, `Data Error`, `CRC Failed`, `Headers Error`, `Missing volume` | "X is damaged or incomplete (<first 3 error lines>)." |
| otherwise | "7-Zip could not read X (exit N). <error lines>" |

`ErrorLines` keeps lines starting with `ERROR`, `Open ERROR`, `WARNING`, or lines under an `ERRORS:` /
`WARNINGS:` heading until a blank line.

## 1.6 Writing — `ArchivePacker.PackAsync`, lines 30-84

Atomic write: build into `<outdir>/.<outname>.filee-<8hex>.tmp`, then `File.Move(temp, output, overwrite)`;
`finally` deletes the temp. A failed or cancelled run never leaves a half archive and never destroys an existing
one.

| Target | How |
|---|---|
| `zip` | In-process `ZipArchive`, UTF-8 names **with the language-encoding flag (bit 11)**, folders as `name/` entries, mtimes clamped to DOS range 1980-01-01 .. 2107-12-31 in local time |
| `tar` | In-process PAX TAR (`TarEntryFormat.Pax`): folders mode 0755 with trailing `/`, files mode 0644, mtime from source |
| `tgz` | Same TAR written through `GZipStream` at the mapped level |
| `tbz2`, `txz` | Write a plain TAR to `<work>/pack-<guid>.tar` (progress 0 → 0.3), then `7zz a -tbzip2` / `-txz` on that one file |
| `7z` | `7zz a -t7z` on the items |
| `gz`, `bz2`, `xz` | Exactly one item that is a regular file, else error "A .gz file holds a single file. Choose TAR.GZ to compress several files or folders."; `7zz a -tgzip|-tbzip2|-txz` |

`Expand` (lines 228-268): files are taken as-is; folders recurse with children sorted ordinal; **reparse points
(symlinks/junctions) are skipped, never followed**; a missing source path throws `FileNotFoundException`. Entry
names use `/` and have slashes trimmed.

Progress: in-process writers count bytes and report every ≥1 %; 7-Zip runs report nothing, so a time estimate is
used (~40 MB/s, from `max(from, 0.05)` to 0.95).

## 1.7 Safety layer — `ArchiveSafety.cs`

- `MaxEntries = 1_000_000` (entry bombs).
- Disk check: sum of non-folder sizes must be ≤ free space − 64 MiB on the target volume. Skipped when the root is
  unknown or `DriveInfo` throws (network shares).
- `IsUnsafePath(entry)`: true if it starts with `/` or `\`, starts with `X:` (ASCII letter + colon), or any segment
  split on `/` or `\` equals `..`. Empty path is safe.
- `CheckListing` refuses: too many entries; any `Encrypted = +`; unsafe `Path`; unsafe `Hard Link` target. Then the
  disk check.
- `ResolveEntryPath` (own readers only): unsafe → error; split on separators, drop empty and `.` segments, sanitize
  each segment (invalid chars `< > : " / \ | ? *`, platform-invalid chars and control chars → `_`; trim trailing
  dots/spaces; empty → `_`; Windows device names `CON PRN AUX NUL COM1-9 LPT1-9` as the part before the first dot get
  a `_` prefix); final full path must start with `root + separator` (case-insensitive) or error.
- `RemoveLinks(root)`: recursively delete every reparse point (symlink or junction) without following it. Runs
  after every extraction stage.
- `ArchiveFiles.CopyLimited`: 64 KiB buffer; throws "the data unpacks to more than the archive declares" when more
  than the declared size arrives (lying headers / decompression bombs in own readers).
- `ArchiveFiles.UniquePath`: `name.ext` → `name_1.ext`, `name_2.ext` ... (mirrors 7-Zip's `-aou`).
- `FormatSize`: 1024-based, `"N bytes"` or one decimal below 10 (`"1.4 GB"`), invariant culture.

Messages (reuse verbatim for parity): "is password-protected. Filee cannot unpack encrypted archives.", "was not
unpacked: the entry \"E\" points outside the target folder.", "is damaged or incomplete (D).", "has more than
1,000,000 entries; Filee does not unpack it.", "unpacks to S, but only F is free on R."

## 1.8 Own readers

### ALZ — `AlzReader.cs`

All little-endian. Signatures as u32 LE:

| Value | Bytes | Meaning | Action |
|---|---|---|---|
| `0x015A4C41` | `ALZ\x01` | archive header, + 4 unknown bytes | must be first; file ≥ 8 bytes |
| `0x015A4C42` | `BLZ\x01` | local file entry | parse entry |
| `0x015A4C43` | `CLZ\x01` | central directory marker | skip 12 bytes |
| `0x035A4C43` | `CLZ\x03` | split marker | ignore |
| `0x015A4C45` | `ELZ\x01` | comment | size = u32 at `fileLength - 12` (includes the 4 signature bytes); skip `size - 4` |
| `0x025A4C43` | `CLZ\x02` | end of archive | stop |
| `0x015A4C53` | `SLZ\x01` | split archive part | error "part of a split ALZ archive" |
| other | | | error "unknown ALZ header" |

Running out of input without `CLZ\x02` keeps what was found (CRC checks catch damage). EOF mid-header → "the archive
ends too early".

Entry header: `nameLen u16, attributes u8 (0x10 = folder), dosTime u32, descriptor u8, unknown u8`. Descriptor bit 0
= encrypted → refuse. Size-field width from `descriptor & 0xF0`: `0x00→0, 0x10→1, 0x20→2, 0x40→4, 0x80→8` bytes,
anything else is damage. If width > 0: `method u8 (0 store, 1 ALZ-bzip2, 2 deflate), unknown u8, crc u32,
packedSize (width bytes LE, zero-extended), size (width bytes)`. Then `name[nameLen]`, then `packedSize` bytes of
data. `nameLen == 0` or a negative size is damage; data must fit in the file.

Names: strict UTF-8 if valid, else **CP949** (Korean). DOS time → local `DateTime` (2-second steps; invalid fields
give null).

Extraction: disk check; for each entry, resolve path; folders are created and their mtime set **after** all files;
files go to `UniquePath`, decoded through a CRC stream with `CopyLimited(size)`, then length and CRC must match
("checksum error in NAME"). Deflate is raw deflate (no zlib header).

### ALZ bzip2 framing — `Bzip2Decoder.cs`

ALZ method 1 is bzip2 **without** the `BZh` stream header: each block starts with the 32-bit marker
`DLZ\x01` (`0x444C5A01`, read big-endian as bits) **in place of** the 48-bit block magic, the block CRC and the
randomised bit; the stream ends with `DLZ\x02`. Block size is always 900k (level 9). Blocks have no CRC to verify.

The standard path (used by EGG method 2) accepts concatenated streams, tolerates a missing `BZ` (starts at `h`),
checks block CRCs and the combined stream CRC, and refuses randomised blocks. Limits: code length ≤ 20, max 18002
selectors, output capped by `maxOutput`.

### EGG — `EggReader.cs`

All little-endian. Magics:

| Magic | Name |
|---|---|
| `0x41474745` | `EGGA` archive header: version u16, header id u32, reserved u32, extra fields, END |
| `0x0A8590E3` | FILE: file id u32, size u64, extra fields, END |
| `0x02B50C13` | BLOCK: method u8, hint u8, size u32, packed u32, crc u32, END, then `packed` data bytes |
| `0x08E28222` | END |
| `0x0A8591AC` | file name field |
| `0x2C86950B` | Windows info field |
| `0x1EE922E5` | POSIX info field |
| `0x08D1470F` | encryption field → refuse |
| `0x24F5A262` | split field → refuse |
| `0x24E5A060` | solid field → refuse |

Extra field: `magic u32, flags u8, size u16 (u32 if flags bit 0)`, data. File name field: if `flags & 0x04` →
encrypted; if `flags & 0x08` a code page u16 precedes the name (unknown code page → CP949), else UTF-8; if
`flags & 0x10` a parent id u32 precedes the name (ignored; names are full paths). Windows info: FILETIME i64 + attr
u8 (`0x10` folder). POSIX info: mode u32, folder when `(mode & 0xF000) == 0x4000`. Unknown records between entries
are skipped as extra fields.

Validation: every entry needs a name; a trailing `/` or `\` marks a folder; for files the block sizes must sum to
the entry size.

Block methods: 0 store, 1 raw deflate, 2 standard bzip2, 3 **AZO** (ESTsoft proprietary → "Save it again in ALZip as
ZIP or 7Z"), 4 **LZMA**. Each block is CRC- and size-checked individually; then the whole file size.

LZMA trick (lines 124-155): the block holds ZIP-style LZMA: `version u16, propSize u16, 5 property bytes`, raw data.
Filee writes a classic `.lzma` file = `props[5] + unpackedSize u64 LE + data` to `<work>/egg-<8hex>/block.lzma`, runs
`7zz x` on it, and reads back `out/block`.

### lzip — `LzipReader.cs`

Member layout: `"LZIP"`, version byte `1`, coded dictionary byte; raw LZMA stream (lc=3 lp=0 pb=2, always with end
marker); 20-byte trailer `crc u32, dataSize u64, memberSize u64` (LE).

1. Find members **from the end backwards** (each trailer gives its member size) and verify the header of each.
2. Disk check on the sum of data sizes.
3. For each member write `<work>/lzip-<8hex>/member.lzma` = `0x5D` (props) + dictionary u32 + `-1` as i64
   (unknown size, until end marker) + member body; run `7zz x`; read `out/member`; copy with size cap and CRC;
   compare with trailer.
4. On any error delete the partial output; set output mtime from the input.

Dictionary size: `size = 1 << (b & 0x1F); size -= (size / 16) * ((b >> 5) & 7)`.

## 1.9 Clever or fragile points

- **Bundled 7-Zip only** for reproducible behaviour; the dummy password prevents hanging on prompts.
- Symlinks are excluded at extraction time **and** removed afterwards (belt and braces); packing never follows links.
- Staging next to the output gives an instant atomic move; temp-and-rename for archive writes.
- `IsSymbolicLink` sniffs the Unix mode string because some formats have no explicit link field.
- Fragile: RPM payload detection is extension-based; `GetStatus` reports the whole engine unavailable without
  7-Zip; exit code 2 is accepted only when every error is about links (string matching on English 7-Zip output).
- The progress bar for 7-Zip is a time estimate, not real progress (`-bsp0` turns progress output off).

---

# Part 2 — CAD engine

## 2.1 Files and responsibilities

| File | Lines | Role |
|---|---|---|
| `Cad/CadConverter.cs` | 120 | Engine: edges, DWG↔DXF, render to PDF/SVG/raster |
| `Cad/CadFile.cs` | 176 | Read DWG/DXF and write DWG/DXF via ACadSharp, with fix-ups |
| `Cad/CadScene.cs` | 98 | Display list (primitives) |
| `Cad/CadSceneBuilder.cs` | 919 | Walk entities, resolve layers/colours/weights/linetypes, expand blocks |
| `Cad/CadSceneBuilder.Text.cs` | 312 | TEXT/ATTRIB/MTEXT layout |
| `Cad/CadSceneBuilder.Hatch.cs` | 254 | HATCH boundaries, gradients, pattern lines |
| `Cad/CadTextCodes.cs` | 300 | `%%` specials, `\U+`, `\M+`, MTEXT formatting language parser |
| `Cad/CadCurves.cs` | 164 | NURBS (de Boor), centripetal Catmull-Rom, bulge arcs |
| `Cad/CadGeometry.cs` | 282 | `Vec2`, `Vec3`, `Affine3`, OCS, `Figure` (lines + cubic Béziers), `Box2` |
| `Cad/CadFonts.cs` | 185 | Typeface resolution, per-character CJK fallback, measuring |
| `Cad/CadRenderer.cs` | 432 | Page layout and drawing with SkiaSharp |

## 2.2 Edges and engine properties

- `dwg → dxf`, `dxf → dwg`
- `dwg|dxf → pdf, svg` and every raster format in `ImageEncoder.Writable` (PNG, JPG, WEBP, TIFF, BMP, GIF, ICO, AVIF)
- `Id = "cad"`, always available (library). `MaxParallelism = clamp(cores / 2, 1, 4)` because a loaded drawing
  takes many times its file size in memory (a 55 MB DXF peaks around 800 MB).

## 2.3 Reading and writing — `CadFile.cs`

- **DWG read**: first check the signature: bytes 0-1 `AC` and byte 2 an ASCII digit, else "The file is not a DWG
  drawing". Then `DwgReader.Read(path, { Failsafe = true })`. Supported: R14 (AC1014) .. 2018 (AC1032).
- **DXF read**: `DxfReader` with `{ Failsafe = true, CreateDefaults = true }` (adds layer 0, standard styles, model
  space for minimal writers). ASCII and binary auto-detected.
- **Korean code page fix** (`OpenDxf`, lines 140-165): scan the first 64 KiB as ASCII for `$DWGCODEPAGE`; if
  `ANSI_949` appears within the next 64 characters, rewrite those 8 bytes to `KCS5601` in an in-memory copy so the
  library decodes CP949.
- Errors: unsupported version → "This DWG version is not supported (...)"; any other parse exception (except I/O,
  cancellation, OOM; truncation counts as damage) → "The file is not a valid DWG drawing or is damaged: ...".
- **Write**: always **AC1032 (AutoCAD 2018)**; DXF is written ASCII (2018 DXF is UTF-8). Before writing: remove stray
  `SEQEND` entities from every block record (R12 leftovers); for DWG also remove dimensions whose `Measurement`
  throws (degenerate), with a warning. On failure the partial file is deleted.

## 2.4 Scene model — `CadScene.cs`

Everything is resolved to plain 2D WCS geometry (top view, Z dropped) before rendering:

```text
Figure { Start: Vec2, Segments: [Line(P) | Cubic(C1, C2, P)], Closed }
StrokePrimitive    { Color, Figures[], WidthMm, WorldWidth (constant-width polylines, 0 otherwise), Dashes? (drawing units; >0 dash, <0 gap, 0 dot) }
FillPrimitive      { Color, Figures[] (even-odd), GradientColor?, GradientAngle }
HatchLinesPrimitive{ Color, Boundary[] (clip, even-odd), Lines[(A,B)], WidthMm }
TextPrimitive      { Text, Typeface, Origin, XAxis, YAxis }   // glyph (gx, gy) at size 1, y down → Origin + gx·XAxis + gy·YAxis
InfiniteLinePrimitive { Origin, Direction, IsRay, WidthMm, Dashes? }
PointPrimitive     { Location, WidthMm }
CadScene { Items[], Extents: Box2, Skipped: sorted map type→count, Truncated, Space: "model" | layout name }
```

The page is fitted to `Extents` of what is actually drawn; the header's EXTMIN/EXTMAX are ignored.

## 2.5 Scene building — `CadSceneBuilder.cs`

Limits: `MaxPrimitives = 1_000_000`, `MaxDepth = 16` (also stops self-referencing blocks),
`MaxArrayItems = 10_000` (MINSERT rows × cols), hatch pattern segments 20,000 per hatch and 2,000,000 per drawing.
Exceeding sets `Truncated` (logged as a warning). Cancellation is polled every 1024 entities.

Flow (`Build`, line 49): draw model space; if nothing was added, draw `*Paper_Space` (the active layout) and record
its name. Viewports are never followed. If the scene is still empty the converter fails with either "The drawing
only contains objects Filee cannot draw (TYPES)" or "The drawing has nothing visible to draw: ...".

Per entity: skip invisible; compute effective layer; INSERT handled before the visibility check (a **frozen** layer
hides the whole insert, an **OFF** layer only hides contents on layer 0); otherwise skip unless the layer is
on + thawed + plottable (Defpoints never plots). Any exception in one entity is caught and counted as
`"<TYPE> (invalid)"` so one bad entity never loses the drawing.

**Property resolution** (lines 763-843):

- Effective layer: inside a block, entities on layer `"0"` (or with no layer) take the insert's layer.
- Colour: BYBLOCK → the context's block colour (root context: black); BYLAYER → layer colour; still by-something or
  index 257 → black; convert ACI / true colour to RGB; **if min(r,g,b) ≥ 224 draw black** (ACI 7 is white on
  AutoCAD's black screen, the page is white).
- Line weight (mm): BYLAYER → layer weight; BYBLOCK → block weight; `W0` → 0.1 mm; 1..211 → value/100 mm but at
  least 0.1; Default and everything else → **0.18 mm** (thinner than AutoCAD's 0.25 on purpose).
- Line type: null/BYLAYER → layer's; BYBLOCK → block's. Dashes only when the type has ≥ 2 finite segments with at
  least one negative; scaled by `LTSCALE × entity ltscale × PlanScale(transform)`. Shapes/text in complex line
  types are left as empty space.
- POINTs are hidden when header `PDMODE == 1`.

**Blocks** (`DrawInsert`, lines 210-266): transform =
`parent × OCS(normal) × T(insertPoint) × Rz(rotation)`, then for each MINSERT cell
`× T(col·colSpacing, row·rowSpacing, 0) × S(x, y, z) × T(−basePoint)`. The child context inherits the insert's
layer, resolved colour, width and line type (for BYBLOCK), depth + 1. Attributes are drawn in the parent's
coordinates but with the inherited BYBLOCK/layer properties; hidden attributes are skipped. `ATTDEF`s draw their
tag only at depth 0.

**Dimensions**: draw their anonymous block in place (it is already in WCS) with inherited properties; if the
block is missing or empty call ACadSharp's `UpdateBlock()` to generate it; otherwise skip as
`DIMENSION (no block)`. TABLE also goes through its block.

**Entities handled**: LINE; ARC/CIRCLE (in OCS, as cubic Bézier arcs); ELLIPSE (WCS; minor = normal × major ×
ratio; partial supported); LWPOLYLINE and POLYLINE 2D/3D (bulges; constant width → wide stroke with butt caps and
miter joins; varying width → filled bands, arcs sampled every 10°; spline-fit polylines drop frame control points;
mesh-flagged polylines skipped); SPLINE (NURBS from control points/knots/weights, else Catmull-Rom through fit
points); TEXT/ATTRIB/MTEXT; LEADER (arrowhead size = `ArrowSize × DIMSCALE`); MULTILEADER (lines, landing, arrow,
text or block content); HATCH; SOLID/TRACE (corner order 1-2-4-3); 3DFACE (visible edges); POINT; XLINE/RAY (clipped
to the page at render time); MLINE (each element offset along miters); polyface / polygon mesh / MESH (edges).
VIEWPORT and SEQEND are ignored silently. Everything else is counted in `Skipped` (3DSOLID, REGION, BODY, IMAGE,
OLE, underlays, WIPEOUT, SHAPE, TOLERANCE, proxies).

Arrowhead (line 741): filled triangle, length `size`, half-width `size / 6`.

**Hatch** (`CadSceneBuilder.Hatch.cs`): boundary loops from line/arc/ellipse/spline/polyline edges in the hatch OCS
(clockwise edge arcs store mirrored angles: run from −start to −end). Solid → `FillPrimitive`; gradient → first
colour + second colour (or first blended 80 % toward white), angle. Pattern → generate each pattern line family
across the boundary bounding box (`k` from floor to ceil of the projected range / spacing; dashes start at the base
point); if more than 20,000 segments (or the global 2 M budget is used), fall back to a fill of the colour blended
75 % toward white. Pattern line width is `min(resolved, 0.18 mm)`.

**Text** (`CadSceneBuilder.Text.cs`): height = entity height, else style height, else 2.5. `em = height /
capHeight(typeface)`. Horizontal alignment Left/Center/Right/Middle/Aligned/Fit, vertical Baseline/Bottom/Middle/Top;
justified text anchors at the alignment point; Aligned scales height to fit between the two points, Fit scales only
the width factor. Mirror flags flip axes. Oblique angle is normalised to (−π, π] and clamped to ±1.48 rad.
MTEXT: parsed into paragraphs of formatted runs, greedy word-wrap at the rectangle width (CJK characters are
individual wrap units; spaces at a break vanish), first baseline at `−lineHeight`, then `lineHeight × 5/3 × spacing`
per line; attachment point shifts X per line (left/centre/right) and Y for the block (top/middle/bottom). Extents
use −0.2 em below and 0.95 em above the baseline.

**Text codes** (`CadTextCodes.cs`): `%%d`→°, `%%c`→Ø, `%%p`→±, `%%%`→%, `%%u/o/k` dropped, `%%nnn` (up to 3 digits)
→ char code; `\U+XXXX`; `\M+nXXXX` with n=1..5 → code pages 932, 950, 949, 1361, 936; tabs → space (TEXT) or 4 spaces
(MTEXT). MTEXT: `{}` push/pop state, `\P \N \X` and newlines → new paragraph, `\~` → space, `\\ \{ \}` escapes,
`\H` height (`x` suffix = relative), `\W` width factor, `\Q` oblique (±85°), `\C` ACI (0..256), `\c` true colour
(24-bit), `\f`/`\F` font (`family|b1|i0|...`, `.shx` families → default font), `\S` stacked text shown inline as
`top/bottom` (split on `/`, `#` or `^`, `\;` is an escaped semicolon), `\A \T \p \q` argument skipped, other codes
(`\L \O \K` ...) dropped.

**Fonts** (`CadFonts.cs`): SHX fonts are never available, so: style file name → family via a small table
(`arial`→Arial, `malgun`→Malgun Gothic, `simsun`→SimSun, ...; extension stripped; `.shx` → none); else the first
installed of Noto Sans, Arial, Helvetica, Liberation Sans, DejaVu Sans, Segoe UI. Lines are split into runs by glyph
coverage, falling back per character to Noto Sans KR, Noto Sans CJK KR, Malgun Gothic, Apple SD Gothic Neo,
NanumGothic, Noto Sans SC/CJK SC, Microsoft YaHei, PingFang SC, Yu Gothic, MS Gothic, then the system matcher with
languages `ko zh ja`. Whitespace stays in the current run. Measurement uses an unhinted 64 pt font; zero advances
fall back to `0.6 em × chars`. Cap height falls back to 0.7 em, descent to 0.21 em.

## 2.6 Geometry details — `CadGeometry.cs`, `CadCurves.cs`

- `Affine3` = images of X, Y, Z axes + translation; `Multiply(inner)` composes "inner first".
- `Ocs(normal)`: AutoCAD arbitrary axis algorithm — if |Nx| and |Ny| < 1/64 then `Ax = Wy × N` else `Ax = Wz × N`;
  `Ay = N × Ax`. Identity when N = +Z.
- `PlanScale` = `sqrt(|det of projected XY|)`, or the longest projected axis when the plane is edge-on.
- `Figure.ArcTo(c, u, v, t0, t1)`: split into ≤ 90° pieces, cubic control distance `k = 4/3 · tan(step/4)`; exact
  under any affine map (non-uniform block scales).
- Bounding boxes include exact Bézier extrema (derivative roots), not control points.
- `SampleNurbs`: de Boor on homogeneous points; invalid knots → clamped uniform; invalid weights → 1; degree clamped
  to [1, n−1]; per non-empty span 1 point (degree 1) or `clamp(4000 / spans, 1, 32)`; max 4000 samples.
- `ThroughPoints`: centripetal Catmull-Rom (α = 0.5), `clamp(4000 / segments, 1, 10)` points per segment, open ends
  mirror the neighbour.
- `BulgeArc(p0, p1, b)`: centre = `p0 + chord/2 + perp(chord) · (1 − b²) / (4b)`, sweep = `4 · atan(b)`.

## 2.7 Rendering — `CadRenderer.cs`

Page layout (`PageLayout`): uniform scale fitting the extents into the printable area, centred, Y flipped.
Degenerate extents get a minimum size (`max(w, size·1e-3)`; a single point becomes 1×1).

| Output | Page | Units | Background |
|---|---|---|---|
| PDF | A3 297×420 mm (A4 210×297 or Letter 215.9×279.4 if the preset says so), landscape if width ≥ height, margin = preset margin (capped at short side / 4) else 10 mm | points (72/25.4 per mm) | none |
| SVG | drawing aspect ratio clamped to 1:20 .. 20:1, long edge 420 mm, 6 mm margin | 96 px/inch | white rect |
| Raster | same as SVG at `dpi = clamp(preset.Pdf.RenderDpi, 36, 1200)` (default 150) | pixels | white |

Raster limits: long edge ≤ 12,000 px and ≤ 80 megapixels (resolution reduced to fit). The bitmap is encoded to PNG
and handed to Magick.NET, which sets density and applies the normal image options (resize, grayscale, quality)
before writing the target format. PDF metadata: Title = input file stem, Creator "Filee", RasterDpi 300.

Drawing: primitives in order. Consecutive strokes with the same colour, width and cap style are merged into one path
(max 4096 points per batch). Line-weight strokes use round caps and joins; wide polylines use butt caps and miter
joins; a wide stroke is used only when its world width on paper exceeds the pen width. Dashes are applied **by
cutting the path** (not a path effect) so PDF, SVG and raster agree; patterns shorter than 0.8 mm on paper or
producing more than 20,000 pieces are drawn solid. Fills use even-odd; gradients are a linear gradient across the
bounds along the angle. Hatch lines are clipped to the boundary path. Text is drawn at a font size equal to the em
height on the page with only the direction in the matrix (viewers handle that better). Points are dots of
`max(weight, 0.3 mm)` diameter. Infinite lines are clipped to the projections of the page corners.

## 2.8 Clever or fragile points

- One display list for three outputs keeps PDF, SVG and PNG identical.
- Extents from real geometry make drawings far from the origin still fill the page.
- Near-white → black colour rule; Default line weight 0.18 mm.
- Heavy dependence on ACadSharp for DWG (reading R14..2018) — the hardest piece to replace.
- Font availability changes the output (CJK fallback depends on installed fonts).

---

# Part 3 — Font engine

## 3.1 Files and responsibilities

| File | Lines | Role |
|---|---|---|
| `Fonts/FontConverter.cs` | 44 | Engine: every format → every other, progress slices |
| `Fonts/FontFile.cs` | 63 | Sniff container by content, choose writer, convert CFF when needed |
| `Fonts/SfntFont.cs` | 194 | SFNT container model; read TTF/OTF/TTC(first font); write with checksums |
| `Fonts/Woff.cs` | 132 | WOFF 1.0 encode/decode (zlib per table) |
| `Fonts/Woff2.cs` | 259 | WOFF 2.0 encode/decode (Brotli, transforms), WOFF2 collections (first font) |
| `Fonts/Woff2Transforms.cs` | 418 | glyf/loca transform (version 0) and hmtx transform (version 1), both directions |
| `Fonts/Eot.cs` | 168 | EOT encode (v0x00020001, uncompressed) and decode; `NameTable` reader |
| `Fonts/TrueTypeGlyph.cs` | 359 | glyf glyph parse/serialize, `GlyfTable` (loca read/build), `FontTables` field helpers |
| `Fonts/BigEndian.cs` | 200 | Bounds-checked BE reader/writer, UIntBase128, 255UInt16 |
| `Fonts/CffToTrueType.cs` | 326 | CFF → TrueType: build glyf/loca/maxp/post, fix head/hhea/hmtx/vhea/vmtx |
| `Fonts/Cff/CffFont.cs` | 361 | CFF v1 parser: INDEXes, DICTs, charsets, private dicts, subrs, FDArray/FDSelect, FontMatrix |
| `Fonts/Cff/Type2Interpreter.cs` | 475 | Type 2 charstring interpreter → cubic contours |
| `Fonts/Cff/CubicToQuadratic.cs` | 159 | cu2qu-style cubic → quadratic spline |
| `Fonts/Cff/CffStandardData.cs` | 92 | 391 CFF standard strings, Standard Encoding, 258 Mac glyph names |

Engine: `Id = "font"`, always available, `MaxParallelism = 0` (= host default). Edges: all 20 ordered pairs of
`ttf otf woff woff2 eot`. Progress: 0.2 after load, outline conversion / Brotli mapped to 0.2..0.8, 0.9 after encode.

## 3.2 Loading (by content, not extension) — `FontFile.Load`

1. First u32 BE `wOFF` (`0x774F4646`) → WOFF; `wOF2` (`0x774F4632`) → WOFF2.
2. `IsSfnt`: ≥ 12 bytes and u32 BE ∈ {`0x00010000`, `OTTO`, `true`, `ttcf`}.
3. `IsEot`: ≥ 82 bytes, u16 LE at 34 = `0x504C`, u32 LE at 8 ∈ {`0x00010000`, `0x00020001`, `0x00020002`}.
4. Else "This file is not a font Filee can read (TTF, OTF, WOFF, WOFF2 or EOT)."

The in-memory model is just `SfntFont { Flavor: u32, Tables: map<tag, bytes> }`. Apple `true` is normalised to
`0x00010000`. A TTC returns its first font (nested collections refused).

## 3.3 Saving — `FontFile.Save`

| Target | Action |
|---|---|
| `otf` | `font.Write()` as is (a TrueType-flavoured OTF is valid OpenType; **no TT→CFF conversion**) |
| `ttf` | convert CFF → TrueType if the font has `CFF `/`CFF2` and no `glyf`, then write |
| `eot` | same conversion, then EOT wrapper |
| `woff` | WOFF 1 of the font as is |
| `woff2` | WOFF2 of the font as is |

So OTF→TTF and OTF/WOFF(CFF)→EOT are the only conversions that touch outlines. CFF2 → TTF/EOT fails with a clear
message; CFF2 → WOFF/WOFF2/OTF works (tables copied).

## 3.4 SFNT writer — `SfntFont.Write`, lines 134-172

- Table directory sorted by tag (ordinal); `searchRange = 16 · 2^floor(log2 n)`, `entrySelector = floor(log2 n)`,
  `rangeShift = 16n − searchRange`.
- Table **data** order: TrueType order `head hhea maxp OS/2 hmtx LTSH VDMX hdmx cmap fpgm prep cvt loca glyf kern
  name post gasp PCLT`, or CFF order `head hhea maxp OS/2 name cmap post CFF`; other tables after, by tag; `DSIG`
  last.
- Each table 4-byte aligned, zero-padded. Checksum = sum of BE u32 (last word zero-padded); for `head` computed with
  `checkSumAdjustment` (offset 8) zeroed.
- Finally `head.checkSumAdjustment = 0xB1B0AFBA − checksum(whole file)`.

## 3.5 WOFF 1.0 — `Woff.cs`

Encode: normalise first by `Read(Write(font))` so the checksum adjustment matches what a decoder rebuilds. Header
(44 bytes): `wOFF, flavor, length, numTables, reserved 0, totalSfntSize, majorVersion, minorVersion, 20 zero bytes`
(no metadata, no private data). Directory entries (20 bytes): `tag, offset, compLength, origLength,
origChecksum`, tags sorted. Each table is zlib-compressed at max level; if that is not smaller the raw table is
stored. Data 4-byte aligned. Version = `head.fontRevision` (16.16) → major = floor, minor = round(frac × 1000).

Decode: validate flavor; for each entry, bounds-check and `compLength ≤ origLength`; equal lengths mean stored.
Metadata/private blocks dropped.

## 3.6 WOFF 2.0 — `Woff2.cs`, `Woff2Transforms.cs`

Encode:

1. Clone, **drop DSIG**.
2. If TrueType outlines and `glyf`, `loca`, `head` exist: try the glyf/loca transform (returns null for damaged
   glyphs or glyphs using the cubic-glyf flag `0x80`; then stored untransformed with version 3). On success set
   `head.indexToLocFormat` to the reconstructed loca format and **set head.flags bit 11** ("losslessly
   transformed"). Then try the hmtx transform (drop lsb arrays equal to xMin; null when neither array can be dropped).
3. Directory: tags sorted; flags byte = `knownTagIndex (0..62) or 63` | `version << 6`; arbitrary tags spelled out
   as u32; `origLength` as UIntBase128; `transformLength` only for transformed tables. Transformed loca has
   transformLength 0. Untransformed glyf/loca use version 3; transformed hmtx uses version 1.
4. Concatenate table data; Brotli quality 11, window 22, fed in 256 KiB chunks for progress/cancel (~10 s per MB).
5. Header (48 bytes): `wOF2, flavor, length, numTables, reserved, totalSfntSize, totalCompressedSize, major,
   minor, 20 zero bytes`; pad to 4.

Decode: parse directory (transform rules as above; any other transform → error), handle WOFF2 collections by taking
the first font's table indices (255UInt16 encoded), Brotli-decompress to exactly the summed length, copy
untransformed tables, reconstruct glyf+loca (section 5.1 of the spec: 7 sub-streams nContour, nPoints, flags, glyph,
composite, bbox, instruction, plus optional overlap bitmap) and set `indexToLocFormat`, reconstruct hmtx using the
xMins.

Known tag list (63 entries) is in `Woff2.cs:22-29` and must match the W3C table exactly.

## 3.7 EOT — `Eot.cs`

Encode (always version `0x00020001`, flags 0 = no MTX compression, no XOR, not a subset), all little-endian:

```text
EOTSize u32 (patched), FontDataSize u32, Version u32, Flags u32,
PANOSE[10] (OS/2 @32), Charset u8 = 1, Italic u8 (OS/2.fsSelection bit 0), Weight u32 (OS/2.usWeightClass),
fsType u16 (OS/2 @8), MagicNumber u16 = 0x504C,
UnicodeRange1-4 u32 (OS/2 @42..), CodePageRange1-2 u32 (OS/2 @78..), CheckSumAdjustment u32 (head @8),
Reserved1-4 u32 = 0, Padding1 u16 = 0,
FamilyName (name id 1), Padding2, StyleName (id 2), Padding3, VersionName (id 5), Padding4, FullName (id 4),
Padding5 u16 = 0, RootStringSize u16 = 0,
FontData
```

Each name = `u16 byteLength + UTF-16LE bytes`. Name lookup prefers Windows/en-US (3, 0x409), then any Windows, then
Unicode platform, then anything. OS/2 fields read big-endian with 0 defaults when the table is short.

Decode: refuse MTX-compressed (`flags & 0x4`); check `82 ≤ EOTSize ≤ file length` and
`FontDataSize ≤ EOTSize − 82`; the font is always the **last** `FontDataSize` bytes before `EOTSize` regardless of
version; XOR every byte with `0x50` if `flags & 0x10000000`.

## 3.8 CFF → TrueType — `CffToTrueType.cs` and `Cff/*`

1. Refuse `CFF2`. Parse `CFF ` (version 1 only; Type 1 charstrings (`CharstringType != 2`) refused; CID-keyed fonts
   supported via FDArray/FDSelect; FontMatrix per FD).
2. Glyph counts must match `maxp.numGlyphs`.
3. Tolerance = `max(unitsPerEm, 16) × 0.001` (1/1000 em, as fontTools `otf2ttf`).
4. Per glyph: run the Type 2 interpreter (hints parsed only to skip; subroutines with bias, nesting ≤ 10; flex/hflex/
   flex1/hflex1; arithmetic and storage operators; `random` returns a fixed value for reproducibility; `endchar`
   with 4 args = **seac** accent composition; CFF2 operators → error). If the FD's FontMatrix × upem is not
   identity, transform the contours.
5. Build the glyph: cubic segments → quadratic off-curve points (cu2qu: try n = 1..100 equal pieces, smallest that
   fits; straight cubics become lines); round with `floor(x + 0.5)`; drop consecutive duplicate on-curve points and a
   closing point equal to the start; skip contours with < 3 points; **reverse each contour** (keep point 0, then the
   rest backwards) because PostScript outer contours run counter-clockwise and TrueType clockwise; > 65535 points →
   error. No contours → empty glyph.
6. New font: clone, remove `CFF `, `VORG`, `DSIG`; flavor `0x00010000`.
7. `glyf`/`loca` via `GlyfTable.Build(glyphs, longFormat: false, alignment: 2)` — short loca unless the total exceeds
   the short-loca limit, then long.
8. `head`: indexToLocFormat, global bbox (xMin/yMin/xMax/yMax at offsets 36..42) from all outlines,
   `glyphDataFormat = 0`.
9. `maxp` v1.0: numGlyphs, maxPoints, maxContours, composites 0, maxZones 1, maxStackElements 1, rest 0.
10. `post` v2.0 with CFF glyph names when every name is 1..255 printable ASCII chars (standard Mac names by index
    0..257, custom names appended as Pascal strings, deduplicated), else v3.0 (CID fonts). The 32-byte header
    (italic angle, underline, isFixedPitch ...) is kept.
11. `hmtx`: advances kept, every lsb set to the new glyph xMin; `hhea` advanceWidthMax, minLeftSideBearing,
    minRightSideBearing, xMaxExtent recomputed.
12. If `VORG`, `vhea`, `vmtx` exist: top side bearing = `VORG origin − yMax`; `vhea` extremes recomputed.
13. If neither `gasp` nor `prep` exists, add `gasp` = `[0,1, 0,1, 0xFFFF, 0x000F]` (smooth + grid-fit at all sizes)
    and `prep` = `B8 01 FF 85 B0 04 8D` (PUSHW 511, SCANCTRL, PUSHB 4, SCANTYPE): dropout control for unhinted fonts.
    PostScript hints are not converted.

Every other table (cmap, name, OS/2, GSUB, GPOS, kern ...) is copied unchanged.

## 3.9 Clever or fragile points

- Content sniffing ignores wrong extensions.
- Round-trips are byte-exact for tables (tests in `tests/Filee.Engines.Tests/FontTests.cs`).
- The WOFF2 glyf transform falls back silently to untransformed tables, so encoding never fails on odd glyphs.
- Brotli quality 11 is slow (about a minute for a 4.6 MB CJK font) — needs progress and cancellation.
- No TT → CFF direction: "TTF → OTF" only rewraps.
- No font collection output; WOFF/WOFF2 extended metadata is dropped.

---

# Porting to Rust

## Archives

| Need | Rust option | Notes |
|---|---|---|
| ZIP write/read | `zip` crate | Set the UTF-8 flag for non-ASCII names (the crate does this when names are not ASCII); clamp DOS times to 1980..2107 |
| TAR write/read | `tar` crate | Use `HeaderMode`/PAX extensions for long and UTF-8 names (`Builder::append_data` + `PaxExtensions`); mode 0644/0755 |
| gzip / bzip2 / xz | `flate2`, `bzip2` (or pure-Rust `bzip2-rs`/`libbz2-rs-sys`), `xz2` or `liblzma` | Gives TAR.GZ/BZ2/XZ and GZ/BZ2/XZ **without** 7-Zip |
| zstd | `zstd` | Free bonus (`.tar.zst` is common on Arch Linux) |
| 7z | `sevenz-rust2` (pure Rust, read and write) | Or shell out to `7zz`/`7z` if present |
| RAR, ISO, CAB, CPIO, DEB, LHA, XAR ... | Shell out to `bsdtar` (libarchive) — installed on this machine — or `7z` when present; `compress-tools` crate wraps libarchive | RAR5 via unrar license issues: prefer external tool |
| LZMA for lzip | `lzma-rs` or `xz2` raw LZMA decoder | Lets us read lzip natively — no `.lzma` rewrite trick needed |
| ALZ / EGG | Port `AlzReader`/`EggReader` directly (small, self-contained); the `unalz` crate exists (MIT) | ALZ bzip2 framing needs a custom bzip2 block decoder — port `Bzip2Decoder.cs` or patch a pure-Rust decoder to accept `DLZ\x01` framing |
| CP949 names | `encoding_rs` (`EUC_KR` is a superset of CP949 in the WHATWG spec) | |
| Free disk space | `fs4::available_space` or `sysinfo` | |
| Atomic writes | `tempfile::NamedTempFile::new_in(outdir)` + `persist` | Mirrors the `.filee-xxxx.tmp` pattern |

Pitfalls:

- **Zip-slip**: port `IsUnsafePath` and `ResolveEntryPath` verbatim and apply them to *every* reader, including
  `zip`/`tar` crates (the `tar` crate's `unpack_in` already refuses `..`, but check symlinks and hard links yourself;
  `zip::ZipArchive::extract` sanitises names via `enclosed_name()` — use that).
- **Never create symlinks** from archives on Linux: skip `EntryType::Symlink`/`Link` in `tar`, and Unix-mode symlinks
  in ZIP (`unix_mode() & 0o170000 == 0o120000`). Then still walk the output and delete any symlink
  (`symlink_metadata().file_type().is_symlink()`), never following.
- On Linux the "reparse point" checks map to `is_symlink()`; Windows device-name sanitising is still worth keeping
  for archives that will be copied to Windows later.
- Size cap: wrap every decoder in a `Read` adapter that errors after the declared size (decompression bombs).
- Hidden staging folder: on Linux a leading dot is enough.
- If we shell out (7z/bsdtar), use `std::process::Command` with args (no shell), pass a dummy password to 7z, kill on
  cancellation, and match error strings only as a fallback.

## CAD

| Need | Rust option | Notes |
|---|---|---|
| DXF read/write | `dxf` crate (pure Rust, R12..2018, ASCII + binary) | Good coverage of entities; check MTEXT, HATCH, SPLINE, INSERT attributes |
| DWG read | **No mature Rust crate.** Options: LibreDWG `dwg2dxf` (GPL-3, external process), ODA File Converter (free, proprietary, external) | Treat DWG as "external tool required" |
| DWG write | LibreDWG `dxf2dwg` (experimental) or ODA | Defer |
| Raster | `tiny-skia` | Already in the cargo cache |
| Vector PDF | `pdf-writer` or `krilla` | Must cut dashes ourselves (as Filee does) for identical output |
| SVG | write strings directly (`svg` crate optional) | |
| Text | `fontdb` + `rustybuzz`/`cosmic-text`, or `skrifa` outlines as paths | CJK fallback list from `CadFonts.cs`; consider drawing text as outlines to avoid font embedding |
| Curves | `kurbo` (arcs → cubic, Bézier bounds) | `Affine3`/OCS are small enough to port by hand |

Pitfalls: the Y flip; the arbitrary axis algorithm; BYBLOCK/BYLAYER resolution through nested inserts; ACI colour
table (256 entries) must be copied; near-white → black; MINSERT and depth limits; fit to drawn extents, not header
extents.

## Fonts

| Need | Rust option | Notes |
|---|---|---|
| Table-level SFNT read/write | Port `SfntFont`/`BigEndian` (≈400 lines) or `read-fonts` + `write-fonts` (fontations) | A hand port keeps byte-exact round-trips and the exact table order |
| zlib (WOFF) | `flate2` with `Compression::best()` | |
| Brotli (WOFF2) | `brotli` crate (pure Rust encoder + decoder), quality 11, lgwin 22 | Encode in chunks for progress |
| WOFF2 transforms | Port `Woff2Transforms.cs`; or shell out to `woff2_compress`/`woff2_decompress` (Google, installed here) | Crates `woff2` / `woofwoof` exist but verify encoder support and licence before relying on them |
| CFF parsing + charstrings | `skrifa` (draws CFF outlines via an `OutlinePen`, handles subrs, seac, CID) | Replaces `CffFont` + `Type2Interpreter` (~840 lines) |
| Cubic → quadratic | `kurbo::CubicBez::to_quads` / `kurbo::cubics_to_quadratic_splines` (cu2qu port) | Tolerance `max(upem,16) × 0.001` |
| glyf building | `write-fonts` `SimpleGlyph` / `GlyfLocaBuilder`, or port `TrueTypeGlyph` | Remember contour reversal and rounding `floor(x + 0.5)` |
| EOT | Port `Eot.cs` (tiny) | |

Pitfalls: `head.checkSumAdjustment` must be computed over the final file; WOFF encoder must normalise first;
`indexToLocFormat` must match the loca actually written; DSIG must be dropped whenever tables change; post v2 name
rules; skrifa's pen gives already-scaled outlines unless asked for unscaled font units (use `Size::unscaled()`).

---

# v0.1.0 candidates

Ranked by value / effort for a release today, Linux first, no bundled 7-Zip.

1. **Fonts: TTF/OTF ↔ WOFF ↔ WOFF2 (rewrap only)** — pure Rust, no external tool. SFNT read/write with checksums
   (~250 lines), WOFF 1 with `flate2`, WOFF2 **untransformed** (all tables version 0, glyf/loca version 3) with the
   `brotli` crate. Untransformed WOFF2 is valid per spec and every browser loads it; the glyf transform can come in
   v0.2. WOFF2 *decode* needs the reconstruct path only for fonts made by other encoders — either port
   `ReconstructGlyf` or shell out to `woff2_decompress` when the glyf entry is transformed.
2. **Archives: extract to folder** for ZIP, TAR, TAR.GZ, TAR.BZ2, TAR.XZ, TAR.ZST, GZ/BZ2/XZ/ZST (single file) using
   `zip`, `tar`, `flate2`, `bzip2`, `xz2`, `zstd` — with the full safety layer (zip-slip, no symlinks, entry cap,
   disk check, size cap), staging folder + atomic move, and the single-top-folder rule.
3. **Archives: create** ZIP (UTF-8 flag), TAR (PAX), TAR.GZ/BZ2/XZ, GZ/BZ2/XZ (single file) in-process; level mapping
   Store/Fast/Normal/Maximum; temp-file + rename; "Compress into one archive" with `name (2).ext` de-duplication.
4. **Archives: everything else via `bsdtar`** (installed): 7Z, RAR, ISO, CAB, CPIO, DEB, RPM, LHA → folder. Locate the
   tool at startup and report the edge unavailable if missing. Keep the "list first, refuse unsafe entries" step
   (`bsdtar -tvf` gives names and link markers).
5. **Archive → archive** = extract to temp + pack (falls out of 2-4 for free).
6. **Fonts: EOT encode/decode (uncompressed)** — under 150 lines, trivial once SFNT exists. Only for TrueType
   flavoured input in v0.1.0; refuse CFF input with a clear message.

Defer to later releases:

- CFF → TrueType (OTF → TTF, OTF → EOT): doable with `skrifa` + `kurbo` + `write-fonts`, but needs metric fix-ups and
  testing; v0.2.
- WOFF2 glyf/hmtx transforms (smaller files): v0.2.
- ALZ, EGG, lzip readers (niche, Korean audience): v0.3; lzip is easy with `lzma-rs` if wanted earlier.
- **CAD entirely**: DWG needs an external converter we do not have; DXF → SVG/PNG/PDF is a large renderer. A minimal
  DXF → SVG (LINE, ARC, CIRCLE, LWPOLYLINE, TEXT, INSERT, layer colours) with the `dxf` crate is a possible v0.3
  stretch goal.
