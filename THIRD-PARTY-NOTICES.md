# Third-party notices

MorphRing is MIT-licensed (see `LICENSE`). This file credits the work it builds on
and lists the external programs it can drive. Each component remains under its own
license.

## Filee

MorphRing is a Rust re-implementation of Filee
(https://github.com/KnifeLemon/Filee), a drag-to-convert offline file converter.
MorphRing contains no Filee code, but it ports Filee's behaviour: the format table,
the conversion-graph edge costs, the output naming rules and the CLI contract.
Filee is distributed under the MIT License, reproduced in full below.

```text
MIT License

Copyright (c) 2026 Filee contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## External programs (not bundled)

MorphRing does not ship, link against or redistribute any of the programs below.
When they are installed, MorphRing finds them on `PATH` and runs them as separate
processes. Their licenses apply to the copies you install yourself.

| Program | License | Project |
|---|---|---|
| FFmpeg (incl. ffprobe) | LGPL-2.1-or-later, or GPL-2.0-or-later depending on the build | https://ffmpeg.org |
| ImageMagick | ImageMagick License | https://imagemagick.org |
| LibreOffice (`soffice`) | MPL-2.0 | https://www.libreoffice.org |
| Poppler (`pdftoppm`, `pdftotext`, `pdfinfo`) | GPL-2.0-or-later | https://poppler.freedesktop.org |
| Ghostscript (`gs`, `gswin64c`) | AGPL-3.0-or-later | https://www.ghostscript.com |
| Pandoc | GPL-2.0-or-later | https://pandoc.org |

Poppler, Ghostscript and Pandoc are copyleft. Running them as separate programs,
the way Filee does, keeps their licenses with the copies you install and out of
MorphRing's own code. Any program added to this list later follows the same
rule: run as a separate process, found on `PATH` or in its usual install folder,
never bundled.

## Rust crates

MorphRing links the Rust crates below (and their own dependencies). Each one
keeps its license; "MIT OR Apache-2.0" means you may pick either.

### Conversion engines and core

| Crate | Used for | License |
|---|---|---|
| `image` | built-in raster engine, images into PDFs | MIT OR Apache-2.0 |
| `resvg`, `usvg` | built-in SVG engine, SVG parsing for SVG to PDF | Apache-2.0 OR MIT |
| `krilla`, `krilla-svg` | writing PDFs from images and SVG | MIT OR Apache-2.0 |
| `lopdf` | PDF page ranges, splitting and merging | MIT |
| `zip` | ZIP archives | MIT |
| `tar` | TAR archives | MIT OR Apache-2.0 |
| `flate2` | gzip for TAR.GZ and SVGZ | MIT OR Apache-2.0 |
| `notify` | `morphring watch` | CC0-1.0 |
| `filetime` | `--keep-dates` | MIT OR Apache-2.0 |
| `dunce` | plain Windows paths for tools | CC0-1.0 OR MIT-0 OR Apache-2.0 |
| `chrono` | ZIP dates, history timestamps | MIT OR Apache-2.0 |
| `serde`, `serde_json`, `thiserror`, `anyhow`, `tracing`, `tracing-subscriber`, `tempfile`, `which`, `regex`, `glob`, `url`, `directories`, `libc` | plumbing | MIT OR Apache-2.0 (`tracing*`, `which`: MIT) |
| `clap`, `ctrlc`, `walkdir`, `indexmap`, `pathdiff` | the CLI | MIT OR Apache-2.0 (`walkdir`: Unlicense OR MIT) |

### Desktop integration (per operating system)

| Crate | Used for | License |
|---|---|---|
| `windows-sys` | Windows: Job Objects that end a tool's whole process tree | MIT OR Apache-2.0 |
| `winreg` | Windows: the Explorer menu entry under HKCU | MIT |
| `mslnk` | Windows: the Send To shortcut | MIT |
| `evdev` | Linux app: the Ctrl+drag gesture | Apache-2.0 OR MIT |

### The ring app

| Crate | Used for | License |
|---|---|---|
| `tauri` | the app shell and its webview windows | Apache-2.0 OR MIT |
| `tauri-plugin-single-instance`, `tauri-plugin-opener`, `tauri-plugin-dialog` | one running app, opening results, the folder picker | Apache-2.0 OR MIT |

The manifests also declare crates for engines that are still being written
(spreadsheets, documents, e-books, fonts, CAD, 3D models, the opt-in engine
downloader). They are added to these tables when code that uses them lands.

`cargo deny` enforces the license allow-list in `deny.toml` in CI (MIT,
Apache-2.0, BSD, ISC, Zlib, MPL-2.0, Unicode, CC0-1.0, plus CDLA-Permissive-2.0
and bzip2-1.0.6 for two dependencies of planned features), so no crate under
any other license can be linked. A complete, generated list of every crate in
the build will come with `cargo about` in a later release.
