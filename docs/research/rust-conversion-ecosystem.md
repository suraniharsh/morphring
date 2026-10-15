# Rust conversion ecosystem: native crates vs. external engines

Status: R&D recommendation for MorphRing v0.1.0 and later
Date: 2026-10-15
Author: R&D team

Question: which conversions can MorphRing do in-process with Rust crates, and which should it hand off to an external engine (ffmpeg, ImageMagick, LibreOffice, pandoc, Ghostscript, calibre)?

## TL;DR

- **Do it natively when the format is small, well specified, and a mature permissively licensed crate exists.** That covers common raster images, SVG rendering, ZIP/TAR, CSV/XLSX, and Markdown to HTML.
- **Shell out when the format is huge or the reference implementation *is* the engine.** That means audio/video (ffmpeg), word processor and slide layout (LibreOffice), markup-to-markup document conversion (pandoc), PostScript/EPS and PDF rasterising (Ghostscript), and Kindle formats (calibre).
- **Use ImageMagick as a fallback for raster images**, not the main path. It covers formats the `image` crate does not handle well (HEIC, lossy WebP, JPEG XL, PSD, AVIF decode). The `image` crate stays the fast, dependency-free default.
- **Never link GPL or AGPL code into the binary.** Calling an external program as a separate process keeps MorphRing MIT/Apache. This rules out linking `mupdf` (AGPL-3.0), the `epub` crate (GPL-3.0) and linking `ffmpeg-next` against a GPL ffmpeg build.

This is the same split Filee uses. It runs Magick.NET, PDFsharp, PDFium and its own small readers and writers in-process, and runs separate processes for heavy engines. See the engine list in `src/Filee.Engines/EngineRegistry.cs:32-54`: in-process converters come first and external ones last (`FfmpegConverter`, `PandocConverter`, `GhostscriptConverter`, `CalibreConverter`, `LibreOfficeConverter`). Downloadable engines are pinned with SHA-256 hashes in `src/Filee.Engines/Infrastructure/engines.json`.

## How Filee does it (for reference, not to copy)

| Area | Filee approach | Files |
|---|---|---|
| Raster images | Magick.NET (bundled ImageMagick, Q8) in-process | `src/Filee.Engines/Magick/*.cs` |
| HEIC write | Hand-written HEIF container + HEVC stream | `src/Filee.Engines/Heic/HeifFile.cs`, `HeicWriter.cs` |
| SVG | Svg.Skia | `src/Filee.Engines/Vector/SvgRenderer.cs` |
| PDF merge/split | PDFsharp | `src/Filee.Engines/Pdf/PdfSharpConverter.cs` |
| PDF to image | PDFium (via PDFtoImage) | `src/Filee.Engines/Pdf/PdfiumConverter.cs` |
| PDF to text | PdfPig + own layout code (about 1,100 lines) | `src/Filee.Engines/Pdf/PdfDocumentReader*.cs` |
| PS/EPS/AI | Ghostscript CLI (`pdfwrite`, `ps2write`, `eps2write`, `pngalpha`) | `src/Filee.Engines/Vector/GhostscriptConverter.cs:85-155` |
| Archives | SharpCompress + own 7z/ALZ/EGG/bzip2/lzip code | `src/Filee.Engines/Archives/*.cs` |
| Fonts | Own SFNT/CFF/WOFF/WOFF2/EOT code | `src/Filee.Engines/Fonts/*.cs` |
| Ebooks | Own EPUB/FB2 reader and writer; calibre for legacy formats | `src/Filee.Engines/Ebooks/*.cs`, `CalibreConverter.cs:22-38` |
| Office | OpenXML SDK, ExcelDataReader, LibreOffice, pandoc | `src/Filee.Engines/Office/*.cs` |
| Media | ffmpeg CLI | `src/Filee.Engines/Media/FfmpegConverter.cs`, `FfmpegRunner.cs` |

The lesson: Filee spent thousands of lines on niche formats (Korean ALZ/EGG/HWP, its own WOFF2 transforms). We should not try that for v0.1.0. Crates already cover most of it.

## Crate survey (verified on crates.io, 2026-10-15)

"Updated" is the date of the last crates.io publish. Downloads are all-time.

### Images and vector graphics

| Crate | Version | Updated | License | Notes | Verdict |
|---|---|---|---|---|---|
| `image` | 0.25.10 | 2026-03-10 | MIT OR Apache-2.0 | Decodes and encodes PNG, JPEG, GIF, BMP, ICO, TIFF, TGA, PNM, QOI, HDR, EXR, DDS (decode). WebP encoding is **lossless only** through `image-webp`. AVIF encoding goes through `ravif`; AVIF decoding needs the `avif-native` feature (dav1d, a C library). 215M downloads. MSRV 1.88. | **v0.1 core** |
| `image-webp` | 0.2.4 | 2025-08-27 | MIT OR Apache-2.0 | Pure Rust. README: "This crate only supports lossless encoding." | Used by `image` |
| `webp` | 0.3.1 | 2025-08-29 | MIT OR Apache-2.0 | Binds libwebp. Gives lossy WebP encoding. | Later, if lossy WebP is needed without magick |
| `ravif` | 0.13.0 | 2026-01-19 | BSD-3-Clause | Pure Rust AVIF encoder (rav1e). Slow but works everywhere. | Through `image` |
| `libavif` | 0.14.0 | 2024-07-05 | BSD-2-Clause | Binding, stale since 2024. | Avoid |
| `jxl-oxide` | 0.12.6 | 2026-05-29 | MIT OR Apache-2.0 | Pure Rust JPEG XL **decoder** only. | Later (JXL input) |
| `libheif-rs` | 3.0.0 | 2026-08-18 | MIT (wrapper; libheif is LGPL-3.0) | Needs system libheif (installed: 1.23.5). HEVC encoding carries patent and licence issues. | Later; use magick for v0.1 |
| `zune-jpeg` | 0.5.15 | 2026-09-08 | MIT/Apache/Zlib | Fast JPEG decoder; `image` already uses it. | Indirect |
| `oxipng` | 10.2.1 | 2026-09-02 | MIT | Lossless PNG optimiser. | Later ("optimize" preset) |
| `resvg` / `usvg` | 0.48.1 | 2026-08-02 | Apache-2.0 OR MIT | The reference Rust SVG renderer. Renders SVG to a pixmap. | **v0.1 core** (SVG to PNG/JPEG/WebP) |
| `svg2pdf` | 0.13.0 | 2025-03-04 | MIT OR Apache-2.0 | SVG to PDF from the Typst team. | v0.2 |
| `magick_rust` | 2.1.1 | 2026-06-21 | Apache-2.0 | Binds MagickWand. Build depends on the exact ImageMagick version on the system. | **Avoid.** Shell out to `magick` instead |

### PDF

| Crate | Version | Updated | License | Notes | Verdict |
|---|---|---|---|---|---|
| `lopdf` | 0.45.0 | 2026-09-08 | MIT | Low-level object model. Good for merge, split, rotate, page extraction and metadata. 24M downloads. | **v0.1** (merge/split) or v0.2 |
| `pdf-writer` | 0.15.0 | 2026-05-27 | MIT OR Apache-2.0 | Low-level, fast PDF writer (Typst). | Indirect |
| `krilla` | 0.8.2 | 2026-06-04 | MIT OR Apache-2.0 | High-level PDF creation (Typst). Good for "images to PDF". | v0.2 |
| `printpdf` | 0.12.8 | 2026-09-05 | MIT | Simpler PDF creation API. | Alternative to krilla |
| `hayro` | 0.8.0 | 2026-10-04 | Apache-2.0 OR MIT | Pure Rust PDF renderer to PNG/SVG. README calls it "experimental, work-in-progress". Passes 1,400+ test PDFs (pdf.js and PDFBox suites). Cannot render knockout groups or non-embedded CID fonts. MSRV 1.92. | **Watch.** Good v0.3 candidate for zero-dependency PDF to image |
| `pdfium-render` | 0.9.4 | 2026-09-06 | MIT OR Apache-2.0 | Wraps Google PDFium. This is what Filee uses. Needs a `libpdfium` shared library at runtime (not packaged on Arch; AUR or pdfium-binaries). | v0.2+ if hayro is not good enough |
| `pdf-extract` | 0.12.1 | 2026-09-16 | MIT | PDF to plain text. Quality is fair; layout is basic. | v0.2 (PDF to TXT) |
| `mupdf` | 0.8.0 | 2026-06-22 | **AGPL-3.0** | Excellent engine, but linking it makes MorphRing AGPL. | **Do not link** |

### Archives and compression

| Crate | Version | Updated | License | Notes | Verdict |
|---|---|---|---|---|---|
| `zip` | 9.0.3 | 2026-10-13 | MIT | De facto standard. 301M downloads. Supports deflate, bzip2, zstd, lzma and AES. | **v0.1** |
| `tar` | 0.4.46 | 2026-05-18 | MIT OR Apache-2.0 | Standard. Pair it with compressors. | **v0.1** |
| `flate2` | 1.1.10 | 2026-08-28 | MIT OR Apache-2.0 | gzip (pure Rust `miniz_oxide` backend by default). | **v0.1** (.tar.gz) |
| `zstd` | 0.14.2 | 2026-10-10 | BSD-3-Clause | Binds libzstd and builds it from vendored C. | v0.1 optional |
| `liblzma` | 0.4.8 | 2026-08-09 | MIT OR Apache-2.0 | Maintained successor to the stale `xz2` (last release 2022). | v0.1 optional (.tar.xz) |
| `bzip2` | 0.6.1 | 2025-10-16 | MIT OR Apache-2.0 | Has a pure Rust backend (`libbz2-rs-sys`). | v0.2 |
| `sevenz-rust2` | 0.24.0 | 2026-10-11 | Apache-2.0 | Pure Rust 7z read and write. Maintained fork of `sevenz-rust` (stale since 2024). MSRV 1.93. | v0.2 |
| `unrar` | 0.5.8 | 2025-02-19 | MIT/Apache wrapper around the **non-free UnRAR licence** | Can only extract. The UnRAR licence forbids writing a RAR compressor. | Later, extract only; review licence |
| `compress-tools` | 0.16.1 | 2026-04-23 | MIT OR Apache-2.0 | Binds libarchive (installed: 3.8.9). Reads almost anything, including RAR and ISO. | Later fallback for exotic inputs |

### Spreadsheets, documents, Markdown, email

| Crate | Version | Updated | License | Notes | Verdict |
|---|---|---|---|---|---|
| `calamine` | 0.36.1 | 2026-07-27 | MIT | Reads XLSX, XLSM, XLSB, XLS and ODS. Read only. | **v0.1** (sheet to CSV/JSON) |
| `rust_xlsxwriter` | 0.99.1 | 2026-09-14 | MIT OR Apache-2.0 | Writes XLSX only. Very complete and well documented. | **v0.1** (CSV to XLSX) |
| `csv` | 1.4.0 | 2025-10-17 | MIT OR Apache-2.0 | Standard. | **v0.1** |
| `docx-rs` | 0.4.22 | 2026-07-21 | MIT | Mostly a DOCX *writer*. Its reader is limited. | v0.2 (MD/TXT to DOCX without pandoc) |
| `docx-rust` | 0.1.11 | 2026-01-22 | MIT | Reads and writes DOCX; smaller project. | Watch |
| `pulldown-cmark` | 0.13.4 | 2026-05-20 | MIT | Fast CommonMark to HTML. | **v0.1** (MD to HTML) |
| `comrak` | 0.56.0 | 2026-10-06 | BSD-2-Clause | GFM (tables, task lists), AST access. Filee uses Markdig, which is GFM-like. | Pick this one if GFM parity matters |
| `mail-parser` | 0.11.9 | 2026-09-09 | Apache-2.0 OR MIT | EML parsing (Filee's `EmlConverter`). | v0.3 |
| `dxf` | 0.6.2 | 2026-10-03 | MIT | DXF read and write (Filee's CAD uses ACadSharp). | v0.4+ |

### Fonts

| Crate | Version | Updated | License | Notes | Verdict |
|---|---|---|---|---|---|
| `read-fonts` / `write-fonts` / `skrifa` (fontations) | 0.45.1 / 0.54.1 / 0.48.0 | 2026-10 | MIT OR Apache-2.0 | Google's Rust font stack, used in Chrome. Very active. Reading and writing SFNT tables. | v0.3 (TTF/OTF work) |
| `allsorts` | 0.17.0 | 2026-05-13 | Apache-2.0 | Parses OpenType, **WOFF and WOFF2**, subsets, and does CFF to TTF. Closest to Filee's `Fonts/` folder. | v0.3 (WOFF/WOFF2 to TTF/OTF) |
| `woff` | 0.6.3 | 2026-04-14 | Apache-2.0 OR MIT | Compresses and decompresses WOFF and WOFF2 by wrapping Google's C++ reference code (built with `cc`). | v0.3 (TTF to WOFF2 output) |
| `woff2` | 0.3.0 | 2022-05-05 | Apache-2.0 | Decompress only; stale. | Avoid |
| `ttf-parser` | 0.25.1 | 2024-11-29 | MIT OR Apache-2.0 | Read only; quiet since 2024 (fontations is the successor). | Indirect only |

### Ebooks

| Crate | Version | Updated | License | Notes | Verdict |
|---|---|---|---|---|---|
| `epub` | 2.1.5 | 2025-10-29 | **GPL-3.0** | EPUB reader. Linking it makes MorphRing GPL. | **Do not use** |
| `epub-builder` | 0.8.3 | 2026-04-10 | MPL-2.0 | EPUB writer. MPL is file-level copyleft and fine to link unmodified. | v0.3 (MD/HTML/TXT to EPUB) |
| (own reader) | n/a | n/a | n/a | EPUB is ZIP + XHTML + OPF. A small reader on `zip` + `quick-xml` is about 300 lines, like Filee's `Ebooks/EpubReader.cs`. | v0.3 |

### Audio and video

| Crate | Version | Updated | License | Notes | Verdict |
|---|---|---|---|---|---|
| `ffmpeg-sidecar` | 2.6.0 | 2026-09-30 | MIT | Runs the `ffmpeg` binary and parses progress and events from its output. No linking. | **v0.1** (or hand-roll with `std::process`) |
| `ffmpeg-next` | 9.0.0 | 2026-08-05 | WTFPL wrapper, but links libav* (LGPL or GPL) | Needs ffmpeg dev headers at build time, has an ABI tied to the ffmpeg version, and is a cross-compile headache. | **Avoid** |
| `symphonia` | 0.6.1 | 2026-08-13 | MPL-2.0 | Pure Rust audio *decoding* (MP3, FLAC, AAC, Vorbis, WAV). No encoders. | Later, for probing duration and tags only |

## External engines

Checked on the dev machine (Arch, 2026-10-15). Note: **pandoc and Ghostscript are already installed.** The project brief said they were not.

| Engine | Installed | Licence (process boundary keeps us clean) | Use for |
|---|---|---|---|
| `ffmpeg` | n9.0.2 | LGPL/GPL | All audio and video, GIF to MP4, audio extraction, cover art |
| `magick` | 7.1.2-32 Q16-HDRI | ImageMagick (Apache-like) | Image fallback: HEIC/HEIF, lossy WebP, JXL, PSD, RAW, AVIF decode, EPS through gs |
| `soffice` | yes | MPL-2.0 | DOCX/ODT/RTF/DOC to PDF, PPTX/ODP to PDF, XLSX to ODS and back, DOC to DOCX |
| `pandoc` | yes | GPL-2.0+ | MD/HTML/DOCX/ODT/EPUB/RST/LaTeX to each other (not PDF output without a TeX engine) |
| `gs` | yes | AGPL-3.0 | PS/EPS/AI to PDF and PNG, PDF to PNG, PDF compression |
| `ebook-convert` (calibre) | **missing** | GPL-3.0 | MOBI/AZW3 and other legacy ebook formats |
| `heif-enc`, `avifenc`, `bsdtar` | yes | various | Optional single-purpose helpers; magick and our crates cover them |

Rules for every engine call:

1. Find the binary on `PATH` first, then in a user-configured path. Report "engine missing" as a typed error that the UI can show as an "install X" hint. Filee does this with `EngineEnvironment.cs` and the installer.
2. Pass arguments as an argv list, never through a shell string. This blocks injection through file names. Put `--` (or the engine's equivalent) before input paths that may start with `-`.
3. Write to a temporary file in the destination folder, then rename it into place. A cancelled job never leaves a half-written output.
4. Run soffice with its own `-env:UserInstallation=file:///tmp/...` profile. Otherwise a running LibreOffice window blocks headless conversion or makes it fail silently.
5. Set a timeout and support kill-on-cancel for each job.
6. Record the engine version in history and logs (Filee: `Infrastructure/EngineVersions.cs`).

## Recommendation per category

| Category | v0.1.0 (today) | v0.2-v0.3 | Later |
|---|---|---|---|
| Common raster (PNG/JPEG/GIF/BMP/TIFF/ICO/WebP-lossless/QOI/TGA) | **Native `image`** | Add `oxipng` for optimising | — |
| AVIF output | Native `image` + `ravif` (slow; use speed 6-8) | — | — |
| HEIC, lossy WebP, JXL, PSD, RAW, AVIF input | **Shell out to `magick`** | `jxl-oxide` for JXL input | `libheif-rs`, `webp` behind cargo features |
| SVG to raster | **Native `resvg`** | `svg2pdf` for SVG to PDF | — |
| Images to PDF | `magick` (quick win) | Native `krilla` or `printpdf` | — |
| PDF to image | **`gs`** (`pngalpha`, like Filee's `GhostscriptConverter.cs:155`) | Evaluate `hayro` against our test PDFs | `pdfium-render` as an option |
| PDF merge/split/rotate | Out of scope (not a "convert" action) | Native `lopdf` | — |
| PDF to text | Out of scope | `pdf-extract` | Better layout code |
| PS/EPS/AI | `gs` if time allows | `gs` | — |
| Office documents (DOCX/ODT/RTF/PPTX to PDF and so on) | **Shell out to `soffice`** | Same | — |
| Markdown | **Native `pulldown-cmark` or `comrak`** for MD to HTML; `pandoc` for MD to DOCX/ODT/EPUB | `docx-rs` to make MD to DOCX work without pandoc | — |
| Spreadsheets | **Native**: `calamine` (read) + `csv` + `rust_xlsxwriter` (write) for XLSX/ODS/XLS to CSV and CSV to XLSX | `soffice` for to-ODS and to-PDF | — |
| Archives | **Native `zip` + `tar` + `flate2`** (zip, tar, tar.gz); `liblzma` and `zstd` if cheap | `sevenz-rust2` (7z), `bzip2` | `compress-tools` (libarchive) or `unrar` for RAR extraction only |
| Audio/video | **Shell out to `ffmpeg`** (`ffmpeg-sidecar` or our own runner) | Presets (quality, codec) | `symphonia` for metadata |
| Fonts | Out of scope | `allsorts` + `woff` (TTF/OTF/WOFF/WOFF2) | fontations for advanced work |
| Ebooks | Out of scope (or EPUB through pandoc) | Own EPUB reader + `epub-builder` | calibre for MOBI/AZW3 |
| Email, CAD, HWP, ICNS | Out of scope | — | `mail-parser`, `dxf`, own ICNS |

### Why this split for v0.1.0

- Everything marked native for v0.1 is mature: either a de facto standard with tens of millions of downloads (`image`, `zip`, `tar`, `flate2`, `csv`, `pulldown-cmark`) or a well-known focused crate (`resvg`, `calamine`, `rust_xlsxwriter`). All are MIT or Apache and all are pure Rust or vendor their C code. `cargo build` works on Linux, Windows and macOS with no system packages.
- Everything shelled out is already installed on the dev machine, so it can be tested today.
- The engine abstraction is the same for both kinds, so a route can move from "external" to "native" later without UI changes. Filee's `RoutePlanner` (`src/Filee.Core/Conversion/RoutePlanner.cs`) picks the cheapest edge. We should give each engine a cost and prefer native routes when both exist.

### Suggested cargo feature layout

```toml
[features]
default = ["img", "svg", "archive", "sheet", "markdown"]
img      = ["dep:image"]          # png, jpeg, gif, webp(lossless), avif(enc), tiff, bmp, ico
svg      = ["dep:resvg"]
archive  = ["dep:zip", "dep:tar", "dep:flate2"]
archive-extra = ["dep:liblzma", "dep:zstd", "dep:sevenz-rust2"]
sheet    = ["dep:calamine", "dep:rust_xlsxwriter", "dep:csv"]
markdown = ["dep:comrak"]
pdf      = ["dep:lopdf"]          # v0.2
fonts    = ["dep:allsorts", "dep:woff"]  # v0.3
```

External engines need no feature flag. They are found at runtime.

## Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Licence leakage from linking GPL/AGPL crates (`epub`, `mupdf`, GPL ffmpeg through `ffmpeg-next`) | MorphRing would have to become GPL or AGPL | Run `cargo deny` with an allow-list (MIT, Apache-2.0, BSD-2/3, Zlib, ISC, MPL-2.0, Unicode-3.0) in CI from day one |
| Bundling AGPL Ghostscript or GPL calibre/pandoc in Windows or macOS installers | Distribution obligations | Do not bundle for v0.1. Detect on PATH and offer an "install" link. Later, download on demand like Filee's `engines.json` (pinned URL + SHA-256) |
| `image` AVIF decode needs dav1d (C) and NASM for speed | Build friction on Windows | Leave `avif-native` off; route AVIF input through magick |
| `ravif` encoding is slow on large images | UI looks stuck | Run conversions on worker threads with progress and cancel; default to encoder speed 6-8 |
| `hayro` is experimental (by its own README) | Wrong renders | Keep gs as the default; add hayro behind a flag and compare on a corpus |
| `pdfium-render` needs a runtime shared library that Arch does not package | Missing-library errors at runtime | Defer; prefer hayro or gs |
| soffice headless hangs if a user profile is locked | Jobs hang | Use a private `-env:UserInstallation`, a timeout and kill on cancel |
| Engine output differs between versions (ffmpeg flags, magick policy.xml blocking PDF/PS) | Flaky conversions | Detect and record versions; catch magick "not authorized" errors and route PDF/PS to gs instead |
| Malicious inputs (zip bombs, path traversal in archives, huge images) | Disk fill, file overwrite, OOM | Enforce archive entry path checks and size caps (Filee: `Archives/ArchiveSafety.cs`, `BoundedStream.cs`); set `image::Limits` |
| Crate churn (`image` 0.25 to 0.26, `zip` major bumps: now at 9.x) | Upgrade work | Pin versions in `Cargo.lock`; upgrade each release |
| MSRV: `hayro` and `krilla` need 1.92, `sevenz-rust2` needs 1.93 | None for us (cargo 1.98) | Set `rust-version = "1.88"` for the core crate while the v0.1 crates allow it |

## Sources

- crates.io API, checked 2026-10-15: `https://crates.io/api/v1/crates/<name>` for version, publish date, licence, MSRV and downloads of every crate listed above.
- image: https://github.com/image-rs/image (feature list from crates.io 0.25.10: `avif`, `avif-native`, `webp`, and others)
- image-webp lossless-only encoding: https://github.com/image-rs/image-webp
- resvg: https://github.com/linebender/resvg
- hayro status and limitations: https://github.com/LaurenzV/hayro
- krilla: https://github.com/LaurenzV/krilla
- svg2pdf / pdf-writer: https://github.com/typst/svg2pdf, https://github.com/typst/pdf-writer
- lopdf: https://github.com/J-F-Liu/lopdf
- pdfium-render: https://github.com/ajrcarey/pdfium-render
- zip: https://github.com/zip-rs/zip2
- sevenz-rust2: https://github.com/hasenbanck/sevenz-rust
- calamine: https://github.com/tafia/calamine
- rust_xlsxwriter: https://github.com/jmcnamara/rust_xlsxwriter
- docx-rs: https://github.com/bokuweb/docx-rs
- allsorts: https://github.com/yeslogic/allsorts
- fontations: https://github.com/googlefonts/fontations
- woff: https://github.com/bodoni/woff
- epub-builder: https://github.com/crowdagger/epub-builder
- ffmpeg-sidecar: https://github.com/nathanbabcock/ffmpeg-sidecar
- symphonia: https://github.com/pdeljanov/Symphonia
- cargo-deny: https://github.com/EmbarkStudios/cargo-deny
- Filee source: `src/Filee.Engines/EngineRegistry.cs`, `src/Filee.Engines/Infrastructure/engines.json`, `src/Filee.Engines/Vector/GhostscriptConverter.cs`, `src/Filee.Engines/Ebooks/CalibreConverter.cs`, `src/Filee.Engines/Archives/ArchiveSafety.cs`
