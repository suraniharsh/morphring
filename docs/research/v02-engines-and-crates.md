# v0.2 engines and crates: PDF, native images, archives, watch folders, dates, pandoc

Status: R&D recommendation for MorphRing v0.2 (PLAN.md section 9, with notes for v0.4)
Date: 2026-10-16
Author: R&D team

Question: which crates and external tools should back the v0.2 engines (PDF, `image`, `resvg`, archives, `watch`, `--keep-dates`, pandoc), and how does each map to Filee's behaviour in `docs/reverse-engineering/`?

This updates `docs/research/rust-conversion-ecosystem.md` (2026-10-15). Versions were checked again today against the crates.io API, the crate sources (downloaded and read, not built), the RustSec advisory DB and the tools on the dev machine.

## TL;DR

| Job | Pick for v0.2 | Fallback / later | Not chosen |
|---|---|---|---|
| Image to PDF | **`krilla` 0.8.2** (in-process) | - | `lopdf` (hand-built XObjects), `pdf-writer` (too low level), `printpdf` |
| PDF to PDF (page range, split, merge) | **`lopdf` 0.45.0** (in-process) | `qpdf` CLI (not installed) | `krilla`'s `pdf` feature (pages become XObjects, links and annotations are lost) |
| PDF to image | **`pdftoppm`** (poppler 26.08.0, installed) | Ghostscript 10.08 (`gs`, installed); `pdfium-render` 0.9.4 for Windows/macOS in v0.4 | `hayro` 0.8.0 (watch), `mupdf` (AGPL) |
| PDF to text | **`pdftotext`** (poppler, installed) | `pdf-extract` 0.12.1 behind a feature for v0.4 | `pdf_oxide` (78 deps, a release every few days) |
| Raster images | **`image` 0.25.10** behind `img`, cheaper than magick for PNG/JPEG/GIF/BMP/TIFF/ICO/QOI/PNM/TGA | magick for everything else | `magick_rust` |
| SVG to raster | **`resvg` 0.47.0** behind `svg` (pinned to match `krilla-svg`) | magick | - |
| SVG to PDF | **`krilla-svg` 0.8.1** | - | `svg2pdf` 0.13.0 (stale: usvg 0.45) |
| ZIP / TAR / TGZ | **`zip` 9.0.3, `tar` 0.4.46, `flate2` 1.1.10** with our own port of `ArchiveSafety` | `bsdtar` for exotic formats (v0.3) | `ZipArchive::extract`, `tar::Archive::unpack` |
| Watch folders | **`notify` 8.2.0** raw watcher + our own pure settle state machine | `notify::PollWatcher` for network shares | `notify-debouncer-full` 0.7.0 |
| `--keep-dates` | **`filetime` 0.2.29** (mtime, atime) | std `FileTimesExt::set_created` (Windows/macOS, v0.4) | - |
| Pandoc | **External `pandoc`** (3.11, installed), Filee's tables verbatim | - | Linking anything (pandoc is GPL) |

Two housekeeping changes come with this: `deny.toml` must allow **`CC0-1.0`** (the licence of `notify`), and the workspace `rust-version` must go from the v0.1 value (the edition 2024 minimum) to **1.92** (`krilla`, `krilla-svg` and `hayro` need 1.92; `image`, `zip`, `lopdf` and `notify` need 1.88). The pinned toolchain is stable 1.98, so this changes nothing for CI.

## 1. Verified versions (crates.io API, 2026-10-16)

| Crate | Version | Published | Licence | MSRV | Downloads | Notes |
|---|---|---|---|---|---|---|
| `lopdf` | 0.45.0 | 2026-09-08 | MIT | 1.88 | 24.5M | Must be >= 0.42 (RUSTSEC-2026-0187). Merge is only an example (`examples/merge.rs`), not an API. `LoadOptions` caps stream decompression (`examples/decompression_bomb.rs`). |
| `krilla` | 0.8.2 | 2026-06-04 | MIT OR Apache-2.0 | 1.92 | 2.8M | Default features `raster-images` (png, zune-jpeg, gif, image-webp) and `simple-text`. JPEG is embedded as-is with `DCTDecode` (`src/stream.rs:280`). Optional `pdf` feature embeds pages of other PDFs through `hayro-write`. |
| `krilla-svg` | 0.8.1 | 2026-06-04 | MIT OR Apache-2.0 | 1.92 | 2.3M | Depends on `usvg`/`resvg` ^0.47 and `tiny-skia` 0.12. |
| `pdf-writer` | 0.15.0 | 2026-05-27 | MIT OR Apache-2.0 | - | 5.6M | What `krilla` builds on. |
| `printpdf` | 0.12.8 | 2026-09-05 | MIT | 1.88 | 3.5M | Alternative to `krilla`. |
| `svg2pdf` | 0.13.0 | 2025-03-04 | MIT OR Apache-2.0 | - | 1.7M | Still on `usvg` 0.45, `pdf-writer` 0.12. Superseded by `krilla-svg`. |
| `pdfium-render` | 0.9.4 | 2026-09-06 | MIT OR Apache-2.0 | 1.61 | 3.3M | Loads `libpdfium` at run time. No `libpdfium` on this machine or in Arch repos. |
| `hayro` | 0.8.0 | 2026-10-04 | Apache-2.0 OR MIT | 1.92 | 3.5M | Pure Rust, `forbid(unsafe_code)`. README: "still very far away" from feature-complete; no encrypted PDFs, no blending/isolation, no knockout groups. |
| `pdf-extract` | 0.12.1 | 2026-09-16 | MIT | - | 6.4M | Built on `lopdf`. Basic layout. |
| `pdf_oxide` | 0.3.78 | 2026-09-08 | MIT OR Apache-2.0 | 1.88 | 1.3M | Claims speed and a 3,830-PDF pass rate. 78 dependencies, very fast release cadence. |
| `mupdf` | 0.8.0 | 2026-06-22 | **AGPL-3.0** | - | 1.8M | Never link. |
| `image` | 0.25.10 | 2026-03-10 | MIT OR Apache-2.0 | 1.88 | 215M | Has `Limits`, `ImageDecoder::orientation()` and `DynamicImage::apply_orientation`. WebP encode is lossless only. |
| `resvg` / `usvg` | 0.48.1 (0.47.0 recommended) | 2026-08-02 (0.47.0: 2026-02-09) | Apache-2.0 OR MIT | 1.85 | 31M | Default features: `svgz`, `text`, `system-fonts`, `memmap-fonts`, `raster-images`. |
| `tiny-skia` | 0.12.0 | 2026-02-02 | BSD-3-Clause | - | 56M | Pulled by resvg and krilla-svg. |
| `zip` | 9.0.3 | 2026-10-13 | MIT | 1.88 | 302M | Must be >= 2.3.0 (RUSTSEC-2025-0168, symlink path traversal in `extract`). Default features include AES, bzip2, lzma, ppmd, zstd, xz. |
| `tar` | 0.4.46 | 2026-05-18 | MIT OR Apache-2.0 | 1.63 | 256M | Must be >= 0.4.45 (RUSTSEC-2026-0067 chmod through symlinks in `unpack_in`; RUSTSEC-2026-0068 PAX size mismatch). |
| `flate2` | 1.1.10 | 2026-08-28 | MIT OR Apache-2.0 | 1.67 | 755M | Pure Rust backend by default. |
| `notify` | 8.2.0 | 2026-08-30 | **CC0-1.0** | 1.88 | 172M | inotify / FSEvents / ReadDirectoryChangesW / kqueue, plus `PollWatcher`. inotify `Q_OVERFLOW` becomes `EventKind::Other` with `Flag::Rescan` (`src/inotify.rs:212`). |
| `notify-debouncer-full` | 0.7.0 | 2026-05-02 | MIT OR Apache-2.0 | 1.88 | 18M | Time-based debounce plus rename stitching. |
| `filetime` | 0.2.29 | 2026-05-12 | MIT OR Apache-2.0 | 1.75 | 414M | Sets atime/mtime only (reads creation time, cannot set it). Opens directories on Windows with `FILE_FLAG_BACKUP_SEMANTICS`, so it works on folders. |

Tools on the dev machine (`command -v`, 2026-10-16): `pdftoppm` and `pdftotext` 26.08.0, `gs` 10.08.0, `pandoc` 3.11, `magick` 7, `soffice`. Not installed: `qpdf`, `mutool`, `libpdfium`, calibre.

## 2. PDF

### 2.1 Image to PDF (Filee: `PdfSharpConverter`, reverse-engineering/02 section 9.1)

| Option | Fit for Filee's rules | Verdict |
|---|---|---|
| `krilla` | `Image::from_jpeg` passes JPEG bytes through untouched, which is exactly Filee's "a jpg source with no alpha stays JPEG". PNG with alpha becomes an image plus soft mask, so Filee's "never palette PNG" workaround for PDFsharp is not needed. Page size is free (`PageSettings::from_wh`), so FitImage, A4 (595.28 x 841.89) and Letter (612 x 792) with landscape switching and margins are a few lines. Has PDF/A validators for Filee's `Document.PdfA` later. | **Pick** |
| `lopdf` | Can do it, but we would hand-build image XObjects, colour spaces and SMasks. That is the work krilla already did. | No |
| `pdf-writer` | Same, one level lower. | No |
| `printpdf` | Workable, smaller community, overlaps krilla. | No |
| `magick img... out.pdf` | Works today without Ghostscript, but re-encodes JPEGs and has no page-size control. | Keep as the v0.1 path until krilla lands |

Port these Filee rules exactly: source DPI from the image density, else 96 (Filee's fallback turns a 3000 px photo with no density into a 31-inch page, reverse-engineering/02 line 842; the PDF task should decide between keeping 96 for parity and capping FitImage pages, for example at A4's long side, and record the choice with a test); JPEG quality `max(preset, 85)` when re-encoding; one page per selected frame; progress `(i+1)/frames`. The `image` crate decodes frames for krilla when the `img` feature is on; without it, image to PDF stays on magick.

### 2.2 PDF to PDF and merge (Filee: `PdfSharpConverter` + `IPdfMerger`)

| Option | Notes | Verdict |
|---|---|---|
| `lopdf` | Object-level copy: annotations, links, form fields and page content survive. `delete_pages` covers page-range extraction; split is "load once, keep one page, save" per page; merge follows `examples/merge.rs` (renumber objects, rebuild the page tree, drop the old catalogs). About 150 lines for us to own, plus tests. Load through `LoadOptions` with a decompression cap. | **Pick** |
| `krilla` with `pdf` feature | Draws each source page as a form XObject (`Surface::draw_pdf_page`). Links, annotations, outlines and form fields do not come along. Fine for imposition, wrong for merge. | No |
| `qpdf --pages` / `--split-pages` | Best fidelity, but not installed here and adds an engine to discover. | Later fallback only if lopdf merges fail on a real corpus |

Filee rules to port: `PageRange.Parse` (reverse-engineering/02 section 9.4: 1-based, `3`, `2-5`, `7-`, `-4`, swapped and clamped ranges, nothing valid means all pages); `SplitPages` suffix `_p{page+1}`; merge appends every page of every input in order. `lopdf` also gives the page count that pdftoppm and Ghostscript need (Filee uses PDFsharp for that and falls back to all pages if it cannot open the file; keep the fallback).

### 2.3 PDF to image (Filee: `PdfiumConverter`, section 9.2; Ghostscript, section 8.2)

| Option | Licence and shipping | Quality | Parallelism | Verdict |
|---|---|---|---|---|
| `pdftoppm` (poppler) | GPL, run as a process. Installed on Linux; Windows/macOS users must install it. | Very good; same engine as most Linux viewers. | Separate processes, so `max_parallelism = 0` is fine. | **Pick for v0.2** |
| Ghostscript `gs` | AGPL, run as a process. Installed. | Very good. `pngalpha` keeps transparency (Filee uses it for EPS/PS). | Processes. | **Fallback**, and the engine for EPS/PS/AI (Filee section 8.2) |
| `pdfium-render` | Crate is MIT/Apache; `libpdfium` (BSD/Apache) must ship next to the binary, about 5-6 MB per OS from pdfium-binaries. | What Filee uses. | **Must be 1** (Filee: PDFium is not thread-safe). | **v0.4** for Windows/macOS, fetched by the pinned-SHA engine downloader |
| `hayro` | Pure Rust, no system deps. | Experimental by its own README. | In-process. | **Watch**; try behind an off-by-default `pdf-render-native` feature against a corpus |
| `mupdf` | AGPL if linked. | Excellent. | - | Never |
| magick | Needs `gs` anyway. | - | - | Never route PDF through magick (reverse-engineering/02 line 926) |

Mapping pdftoppm to Filee: `pdftoppm -r <clamp(RenderDpi,36,1200)> -png|-jpeg|-tiff -f N -l M in.pdf <prefix>`. pdftoppm only takes one contiguous range, so run one process per contiguous run from `PageRange.Parse`, then rename its zero-padded outputs to Filee's `_p{n}` suffix (no suffix when only one page is selected; `-singlefile` for that case). pdftoppm renders on white paper, which matches Filee's PDFium "white background". For targets pdftoppm cannot write (webp, avif, gif...), render PNG then chain to the image engine; the planner does this already through costs. Multi-page TIFF/GIF and `CombinePages` stacking (200 MP cap, trim margins, split tall) need an image library and stay in a later task.

### 2.4 PDF to text (Filee: `PdfTextConverter`, section 9.3)

| Option | Notes | Verdict |
|---|---|---|
| `pdftotext` (poppler) | Installed, robust reading order. Run without `-layout` (one logical line per text line), `-enc UTF-8`. | **Pick** |
| `pdf-extract` | Pure Rust on top of lopdf; basic layout. | Feature `pdf-text-native` for Windows/macOS in v0.4 |
| `pdf_oxide` | Promising benchmarks, but 78 dependencies, a large surface (bindings, MCP) and a 0.3.x version that moves weekly. | Re-evaluate at v0.5 |
| Port Filee's PdfPig layout analyser (~1,100 lines) | Columns, running heads, CJK spacing. | v1.0 parity item, not v0.2 |

Filee parity to keep: output is **UTF-8 with BOM**; error "The PDF contains no text (it may be scanned...)" when the result is empty or whitespace; blank line between paragraphs (post-process pdftotext's form feeds and blank lines).

## 3. Native image and SVG engines (cargo features)

### 3.1 `image` (feature `img`)

- `default-features = false` and enable only `png`, `jpeg`, `gif`, `bmp`, `tiff`, `ico`, `qoi`, `pnm`, `tga`. Leave out `webp` encode (lossless only), `avif` (rav1e is slow; decode needs dav1d) and `exr`/`hdr`. Those stay on magick.
- Give native edges a lower cost than magick's 10 (for example 8) so the planner prefers them, the same way Filee's built-in Markdown edges (cost 4-5) beat Pandoc.
- Port the parts of Filee's `ApplyOptions` (reverse-engineering/02 section 4.6) that apply to these formats: EXIF orientation (`ImageDecoder::orientation` + `apply_orientation`), JPEG max side 65,500 (`resize` when larger), flatten alpha onto `Background` (default `#FFFFFF`) for jpg/ppm, JPEG quality `clamp(q,1,100)`, TIFF compression, first frame only for single-frame targets, Netpbm `.pbm/.pgm` naming. Anything else in a preset (sharpen, strip metadata, colour profiles) means the job routes to magick: the native engine should **decline** a step it cannot honour rather than ignore the option.
- Always set `image::Limits` (Filee's caps: 100 MP for rasterised SVG, 200 MP for PDF stacks are good defaults).

### 3.2 `resvg` (feature `svg`)

- Pin `resvg`/`usvg` to **0.47.0** so the tree is shared with `krilla-svg` 0.8.1 (which requires ^0.47). Using 0.48.1 next to krilla-svg builds two copies of usvg, fontdb and rustybuzz. Move both together when krilla-svg moves.
- Filee rules (reverse-engineering/02 section 8.1): gunzip when the file starts `1F 8B` (resvg's `svgz` feature does it), error on empty or infinite bounds, `scale = max(dpi/96, 1024/longEdge)` with caps 16,384 px per side and 100 MP, svg <-> svgz by `<svg` sniffing in the first 4 KB plus `flate2`.
- Security parity: usvg never fetches over the network. Its default `ImageHrefResolver` reads local files relative to `resources_dir`; set `resources_dir` to the SVG's folder, as Filee only allows local references.
- SVG to PDF uses `krilla-svg`, which draws text as real glyphs with font subsetting. This avoids the "whole font embedded" problem that made Filee outline its text (reverse-engineering/02 line 693).

## 4. Archives (feature `archive`)

Crates: `zip` 9.0.3 with `default-features = false, features = ["deflate", "time"]` (add `bzip2`, `zstd`, `lzma` in `archive-extra`), `tar` 0.4.46, `flate2` 1.1.10. Pin the minimums in `Cargo.toml`: `zip >= 2.3`, `tar >= 0.4.45`.

Do **not** use the one-call extractors. `ZipArchive::extract` creates symlinks (inside the root), overwrites, is not atomic and has no size or count caps; its `enclosed_name` also accepts `foo/../bar`, which Filee rejects. `tar::Archive::unpack` follows the same pattern. Instead write one loop per format that enforces Filee's `ArchiveSafety.cs` (reverse-engineering/04 section 1.7):

| Filee rule | Rust implementation |
|---|---|
| `MaxEntries = 1_000_000` | Check `ZipArchive::len()` before extracting; count tar entries while streaming. |
| Refuse encrypted entries | `ZipFile::encrypted()`; message "is password-protected. ..." verbatim. |
| `IsUnsafePath`: leading `/` or `\`, drive letter, any `..` segment | Our own check on the raw name, stricter than `enclosed_name`. |
| `ResolveEntryPath`: sanitize segments, Windows device names, must stay under root | Our own function, shared with the zip and tar loops; pure and unit-tested. |
| Links | Skip symlink and hardlink entries (`ZipFile::is_symlink`, tar `EntryType::Symlink/Link`) instead of creating them; then `RemoveLinks(root)` as Filee does. |
| Disk check: sum of sizes <= free space - 64 MiB | Sum declared sizes; free space via `libc::statvfs` on Unix (already a dependency), `GetDiskFreeSpaceExW` in v0.4. Skip if unknown, as Filee does. |
| `CopyLimited`: error when more than the declared size arrives | `io::copy(&mut entry.take(size + 1), ...)` and fail if `size + 1` bytes came through. |
| Staging folder next to output, then rename; `ContentRoot` unwrap of a single top folder | Same, with our existing output allocator; the unwrap is ours (zip's `extract_unwrapped_root_dir` cannot be combined with our loop). |
| Writers: ZIP with UTF-8 names (bit 11), DOS dates clamped to 1980..2107; PAX TAR 0755/0644; TGZ through gzip; skip symlinks when packing | `zip::write` sets the UTF-8 flag for non-ASCII names; clamp `DateTime` ourselves. `tar::Header::new_ustar` + `Builder::append_pax_extensions` for long names. `flate2::write::GzEncoder`. |
| Atomic write `.<name>.filee-<8hex>.tmp` then rename | Same pattern as our other engines (temp file then rename). |

## 5. Watch folders (`notify`)

Filee (reverse-engineering/01 section 10) does not debounce on events. It uses events only to **notice** paths, and a 1 s tick decides readiness: size and mtime unchanged for the settle time (5 s on Linux/macOS, 2 s on Windows), dedupe on `path|length|mtime`, one batch at a time per folder, overflow means a full rescan.

- Use **`notify::RecommendedWatcher` directly** (`RecursiveMode::Recursive` only when `includeSubfolders`), and send `Create`, `Modify` and `Rename` paths into a channel. On an event with `Flag::Rescan`, rescan the folder, which is Filee's overflow behaviour.
- Put the settle logic in `core` as a **pure state machine** fed `(path, size, mtime, now)` and ticked with an injected clock, so all of Filee's rules (ignore list: `~$`, dotfiles, `.part`/`.crdownload`/..., the output and originals folders; `MoveToOriginals` numbering) get unit tests without touching the file system.
- Fix Filee's known flaw: cap the `handled` set (LRU, for example 10,000 keys) instead of letting it grow. PLAN.md already caps history at 200.
- Offer `PollWatcher` for paths where inotify does not work (NFS/SMB); Linux users with large trees can hit `max_user_watches`, so report that error clearly.
- `notify-debouncer-full` is not a fit: it debounces on a timeout after the last event, which is not the same as "size and mtime stable for N seconds", and it adds a second timing layer to test.

## 6. `--keep-dates`

Filee copies **creation and modification** times from the source onto every output, file or folder, and a failure is only a warning (reverse-engineering/01 line 321).

- `filetime::set_file_times(out, atime, mtime)` from the source metadata. It works on directories on all three OSes.
- Creation time: Linux cannot set a birth time, so it is skipped there. On Windows and macOS use std's `FileTimes::set_created` through `std::os::windows::fs::FileTimesExt` / `std::os::darwin::fs::FileTimesExt` (stable since Rust 1.75) in v0.4.
- Apply it after the rename into place, and log a warning on error, never fail the job.

## 7. Pandoc engine

Pandoc 3.11 is installed (reverse-engineering/03 still says it is not; that note is out of date). It is GPL, so run it as a process only, which matches Filee.

Port reverse-engineering/03 section 4 verbatim:

- Readers: `md->markdown, html->html, docx->docx, odt->odt, rtf->rtf, rst->rst, tex->latex`. Writers: `md->gfm, html->html, docx->docx, odt->odt, rtf->rtf, txt->plain, rst->rst, tex->latex, epub->epub`.
- Edges: `{docx,odt,html,rtf}->md` and `md->{docx,odt,html,rtf,txt}` cost 10; `html<->{docx,odt}` cost 14; `{rst,tex}->` everything else cost 10; `{md,html,docx,odt}->{rst,tex}` cost 10. **No PDF output.**
- Command: `pandoc <in> -f <reader> -t <writer> -o <out>`, plus `--standalone` for html/tex and `--extract-media=<outdir>/<outstem>_files` for md/html/rst/tex; `current_dir` = the input's folder; timeout 2 min; success means the output exists; error `"Pandoc failed (exit N). <stderr>"`.
- Edges only appear when `pandoc` is found (the same discovery as our other engines). Version regex `^pandoc(?:\.exe)? (\d+)\.(\d+)`.

## 8. Proposed feature layout for `crates/engines`

```toml
[features]
default = ["img", "svg", "pdf", "archive"]
img     = ["dep:image"]                                  # native raster, cost 8
svg     = ["dep:resvg", "dep:flate2"]                    # resvg 0.47 (shared usvg with krilla-svg)
pdf     = ["dep:lopdf", "dep:krilla", "dep:krilla-svg"]  # image->pdf, svg->pdf, page range/split/merge
archive = ["dep:zip", "dep:tar", "dep:flate2"]
archive-extra     = ["zip/bzip2", "zip/zstd", "zip/lzma"]
pdf-render-native = ["dep:hayro"]                        # experimental, off by default
pdf-text-native   = ["dep:pdf-extract"]                  # v0.4, for hosts without poppler
```

`notify` and `filetime` are not engine features: `notify` belongs where `watch` lives (the CLI, plus the app later), and `filetime` in the job runner. External engines (`pdftoppm`, `pdftotext`, `gs`, `pandoc`) need no feature; they are discovered at run time.

## 9. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| `notify` is CC0-1.0, which `deny.toml` does not allow | CI `cargo deny check licenses` fails | Add `CC0-1.0` to `allow` (it is a public-domain dedication, more permissive than MIT), in the same commit that adds notify |
| MSRV jumps to 1.92 (krilla, hayro) | Users on older toolchains cannot build | Bump `rust-version` to 1.92 in the workspace; CI uses 1.98 |
| lopdf merge is our code, not a library API | Broken merges on odd PDFs (inherited resources, object streams, encrypted input) | Port the example with tests on a small corpus (linearized, object streams, encrypted, rotated pages); refuse encrypted input with a clear message |
| Hostile PDFs (deep nesting, flate bombs) crash or exhaust memory | DoS in the app | lopdf >= 0.42 (nesting fix), `LoadOptions` decompression limits; external tools run with timeouts and the stall watchdog |
| poppler and gs are missing on Windows/macOS | PDF to image and to text unavailable there | Missing-engine hints in v0.2; v0.4 adds pdfium (downloaded with pinned SHA-256) and `pdf-text-native` |
| Archive extraction bugs (zip-slip, symlink tricks, PAX size confusion) | Files written outside the target | Pin `zip >= 2.3`, `tar >= 0.4.45`; never create links; our own path check is stricter than the crates'; fuzz-style tests with crafted archives (`../`, absolute, drive letters, symlink then file, lying sizes) |
| Native `image` output differs from magick (metadata, colour profiles) | Users see a change in results | Native engine declines steps with options it does not implement; golden tests compare dimensions and alpha, not bytes |
| resvg 0.48 vs krilla-svg's ^0.47 | Two copies of usvg and fontdb, a bigger binary | Pin 0.47 until krilla-svg moves |
| Pandoc fetches `http(s)` images when writing docx/odt/epub | A network call, against ENGINEERING.md principle 5 | Pandoc's `--sandbox` blocks it, but on Arch's pandoc build it fails for docx ("Could not find data file 'data/data/docx/[Content_Types].xml'", tested 2026-10-16) and it also blocks local images. Document the behaviour; revisit with a self-contained pandoc build in v0.4 |
| Pandoc edges overlap LibreOffice | Unexpected engine choice | Keep Filee's costs (pandoc 10/14, LibreOffice 25) so routes match Filee |
| inotify watch limits and network shares | Watch folder silently misses files | Report `max_user_watches` errors; offer `PollWatcher` |

## 10. Sources

- crates.io API, 2026-10-16: `https://crates.io/api/v1/crates/<name>` and `/<version>/dependencies` for every crate above.
- Crate sources read from `https://static.crates.io/crates/<name>/<name>-<version>.crate`: krilla 0.8.2 (`Cargo.toml` features, `src/stream.rs`, `src/surface.rs`, `src/pdf.rs`), lopdf 0.45.0 (`src/processor.rs`, `examples/merge.rs`, `examples/decompression_bomb.rs`), zip 9.0.3 (`src/read/extract.rs`, `src/read/zipfile.rs`), tar 0.4.46 (`src/archive.rs`, `src/entry.rs`, `src/pax.rs`), notify 8.2.0 (`src/inotify.rs`), notify-debouncer-full 0.7.0, filetime 0.2.29 (`src/lib.rs`, `src/windows.rs`), resvg 0.48.1, image 0.25.10 (`src/io/limits.rs`, `src/images/dynimage.rs`), hayro 0.8.0 (README), pdf-extract 0.12.1, pdf_oxide 0.3.78 (README).
- RustSec: RUSTSEC-2026-0187 (lopdf nesting stack overflow, fixed 0.42.0), RUSTSEC-2025-0168 (zip symlink traversal, fixed 2.3.0), RUSTSEC-2026-0067 and RUSTSEC-2026-0068 (tar chmod through symlinks and PAX size, fixed 0.4.45): https://github.com/rustsec/advisory-db/tree/main/crates
- Project pages: https://github.com/LaurenzV/krilla, https://github.com/LaurenzV/hayro, https://github.com/typst/pdf-writer, https://github.com/typst/svg2pdf, https://github.com/J-F-Liu/lopdf, https://github.com/ajrcarey/pdfium-render, https://github.com/bblanchon/pdfium-binaries, https://github.com/jrmuizel/pdf-extract, https://github.com/yfedoseev/pdf_oxide, https://github.com/image-rs/image, https://github.com/linebender/resvg, https://github.com/zip-rs/zip2, https://github.com/composefs/tar-rs, https://github.com/rust-lang/flate2-rs, https://github.com/notify-rs/notify, https://github.com/alexcrichton/filetime, https://pandoc.org/MANUAL.html
- Rust std: `std::fs::FileTimes` and `FileTimesExt::set_created` (Windows, Apple), https://doc.rust-lang.org/std/fs/struct.FileTimes.html
- Local tools (2026-10-16): `pdftoppm -v` 26.08.0, `gs --version` 10.08.0, `pandoc --version` 3.11; `pandoc --sandbox` md to docx test in the scratchpad.
- Filee behaviour: `docs/reverse-engineering/01-core.md` (sections 10 and job step 6), `02-engines-media-image.md` (sections 8, 9, 11), `03-engines-office-docs.md` (section 4), `04-engines-archive-cad-font.md` (sections 1.5-1.7); Filee files `src/Filee.Engines/Pdf/PdfSharpConverter.cs`, `PdfiumConverter.cs`, `PdfTextConverter.cs`, `src/Filee.Core/Conversion/PageRange.cs`, `src/Filee.Engines/Archives/ArchiveSafety.cs`, `src/Filee.Core/Watching/FolderWatcher.cs`, `src/Filee.Engines/Office/PandocConverter.cs`.
