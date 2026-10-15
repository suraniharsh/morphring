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

Any program added to this list later follows the same rule: run as a separate
process, found on `PATH`, never bundled.

## Rust crates

A full list of the licenses of the Rust crates compiled into MorphRing will be
generated with `cargo about` in a later release. Until then, `cargo deny` enforces
the license allow-list (MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0, Unicode) in CI,
so no crate under any other license can be linked.
