# Changelog

All notable changes to MorphRing are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-10-17

Presets, watch folders on the command line, PDF and archive support, a
settings and history window for the ring, and the first Windows and macOS
previews.

### Added

- Presets: the ring and the command line share one list of presets (target
  format plus options such as quality, size or codec), ported from Filee's
  built-in set, stored in `presets.json` and `profiles.json` in the config
  folder.
- `morphring convert --preset <id or name>` converts with a preset instead of
  `--to`.
- `morphring presets` lists every preset with its id, name and target format.
- `morphring watch <folder> --to <format>` (or `--preset`) converts every file
  that arrives in a folder until Ctrl+C, with `-o/--output`, `--recursive`,
  `--move-originals`, `--settle <seconds>`, `--keep-dates` and `--quiet`.
- `--keep-dates` on `convert` and `watch` gives each output the dates of its
  source.
- Settings (`settings.json`): default output location (same folder, a
  subfolder or a custom folder), keep file dates and keep history.
- Conversion history (`history.json`), capped at 200 jobs.
- PDF engines: page rendering and text extraction with poppler (`pdftoppm`,
  `pdftotext`), page extraction, splitting and merging with lopdf, and
  Ghostscript (`gs`) for PDF, EPS and PS.
- SVG: native rendering with resvg and native SVG to PDF with krilla.
- Archives: zip and tar extraction with Filee's safety rules (no path
  traversal, size and entry limits) and packing files into zip and tar.
- Pandoc engine (`pandoc`) for Markdown, HTML, reStructuredText, LaTeX, EPUB
  and other markup and document formats.
- Native engines that need no installed program: images (common raster
  formats), images to PDF, SVG, PDF page tools and archives.
- FFmpeg: Filee's remux matrix (copy streams when the container allows it),
  hvc1 tagging for HEVC in MP4 and MOV, and preset options mapped to FFmpeg.
- ImageMagick: preset options (size, quality, background and more) mapped to
  `magick`.
- Engines that report no progress get an estimated one, and a tool that stops
  making progress is stopped.
- Desktop app: a settings window and a history window from the tray, a toast
  with progress for long-running jobs, a cancel button on the progress ring
  (and Escape), slices built from your presets, and a click on the finished
  ring reveals the first output.
- Desktop app on Linux: hold Ctrl while dragging files out of a file manager
  and the ring opens under the cursor (needs read access to `/dev/input`, for
  example through the `input` group). Mice and keyboards plugged in later are
  picked up.
- Windows preview: the CLI as a zip (`morphring-v0.2.0-x86_64-windows.zip`) and
  the ring app as a per-user NSIS installer (`MorphRing_0.2.0_x64-setup.exe`).
  `morphring integrate install` adds a "Convert with MorphRing" Explorer verb
  for files and folders and a Send To entry. Running tools are stopped with a
  Job Object on cancel, and engines are found in their usual install folders.
- macOS preview (Apple Silicon): the CLI as a tarball
  (`morphring-v0.2.0-aarch64-macos.tar.gz`) and the ring app as an ad-hoc
  signed dmg (`MorphRing_0.2.0_aarch64.dmg`). `morphring integrate install`
  adds a "Convert with MorphRing" Finder Quick Action, and the app accepts
  files from Open With and the Dock.
- Missing-engine hints for Windows (winget, Scoop) and macOS (Homebrew).
- `/?` as an alias for `--help`, as in Filee.
- An AUR package, `morphring-bin`, for the CLI.

### Changed

- Engines are tried in their final v0.2 order: the native image, SVG and PDF
  engines, ImageMagick, poppler, the native archive engine, FFmpeg, Pandoc,
  Ghostscript and LibreOffice. The format registry also gains Filee's newer
  format ids and categories.
- The minimum supported Rust version is now 1.93 (it was 1.85).
- `convert --json` has a `preset` field (the preset's name, or `null` under
  `--to`), and `convert --quiet` prints nothing but the failed and stopped
  files, on stderr, with no summary, matching Filee.
- The ring has a new look: ink, cream and one tomato slice.

### Fixed

- Ring progress arrives in order when files convert in parallel.
- Ring animations always finish when frames stall.
- SVG text renders on machines without Microsoft fonts.
- Renames and version probes are retried when Windows holds a file lock.
- The Windows binaries link the C runtime statically, so they need no Visual
  C++ Redistributable.

### Known issues

- The Windows builds are unsigned: SmartScreen warns on first start ("More
  info", then "Run anyway").
- The macOS builds are ad-hoc signed, not notarized: Gatekeeper blocks the
  first start. Allow it under System Settings > Privacy & Security > "Open
  Anyway", or run `xattr -dr com.apple.quarantine /Applications/MorphRing.app`
  (and the same on the `morphring` binary).
- Pandoc may download images that a document links to on the web while it
  converts. Convert documents with remote images only if that is acceptable.
- On Windows 11 the Explorer entry sits under "Show more options"
  (Shift+F10), not in the short menu.
- `morphring presets export`, `import` and `reset` are listed in the help but
  not available yet.
- The Windows and macOS builds are previews and have not been tried on real
  hardware yet; see the README for the list.

## [0.1.1] - 2026-10-16

The desktop ring app arrives as a preview, with Linux packages.

### Added

- The MorphRing desktop app (`morphring-app`), as a preview: a drop zone
  window, a click-mode conversion ring that opens centred on the screen on
  Wayland, a tray icon, and a single running instance that receives files from
  later launches.
- `morphring-app --convert <files>` opens the ring for the given files.
- The file manager entries added by `morphring integrate` now open the ring.
- Linux `.deb` and `.AppImage` downloads of the desktop app, next to the CLI
  tarball, all listed in `SHA256SUMS.txt`.

### Fixed

- The ring no longer closes on its own on focus-follows-mouse compositors such
  as Hyprland.

### Known issues

- The ring cannot cancel a running job yet.
- The ring uses the default quality and output naming; there are no settings
  for them in the app yet.
- The desktop app is untested on Windows and macOS.

## [0.1.0] - 2026-10-15

The first release: the `morphring` command line tool, Linux first.

### Added

- `morphring convert <files or folders...> --to <format>`: converts files,
  folders (`--recursive` for sub folders) and wildcards to one target format,
  in parallel, with `-o/--output` for an output folder, `-q/--quality` (1 to 100)
  for lossy targets, `--skip` and `--overwrite` for existing outputs, `--quiet`
  and `--json` for scripts. Exit codes 0, 1, 2 and 3 follow Filee's CLI contract.
- `morphring formats [<format>]`: lists every known format with its extensions,
  or the targets a single format can become.
- `morphring doctor [--json]`: shows which engines are installed, their versions
  and paths, and the config, data and temp folders.
- `morphring help [<command>]`, `--help`, `morphring version` and `--version`,
  which work without any engine installed.
- `morphring integrate install|uninstall` (with `--dry-run`): adds or removes a
  "Convert with MorphRing" entry in the Dolphin, Nautilus, Nemo and Thunar
  context menus. The entry starts the desktop app, so it only becomes useful
  once that app ships.
- ImageMagick 7 engine (`magick`) for images, with quality mapping.
- FFmpeg engine (`ffmpeg`, `ffprobe`) for audio and video, with ffprobe-based
  probing and progress parsing.
- LibreOffice engine (`soffice`) for office documents, with one isolated user
  profile per worker and a process-group kill on cancel.
- Filee's format registry and an exact route planner that finds the cheapest
  conversion of up to three hops over Filee's edge costs.
- Output naming ported from Filee (`photo (2).jpg` instead of clobbering), with a
  per-job reservation set. This fixes Filee's Overwrite bug, where two files in
  the same job could be handed the same output path.
- Missing-engine hints: a route that needs an absent tool fails with the command
  that installs it on the detected distribution.
- Parallel job runner with per-engine concurrency limits, intermediate files in
  a per-job temp folder, and clean Ctrl+C handling (running tools stopped,
  partial outputs removed; a second Ctrl+C exits at once with 130).
- CI workflow running `cargo fmt`, `cargo clippy`, `cargo test` and `cargo deny`.
- Release workflow that builds a `x86_64-linux` tarball with `SHA256SUMS.txt`
  and opens a draft GitHub release from this changelog on `v*` tags.

### Known limitations

- Linux first. Windows and macOS are untested.
- No presets, watch folders or PDF input yet.
- No desktop app in this release. The ring window in `app/` is a work in
  progress and is not part of the release tarball; v0.1.1 brings the ring
  preview.
- Engines are not bundled: install ImageMagick 7, FFmpeg and LibreOffice
  yourself for the conversions you need.

### Credits

MorphRing is a Rust re-implementation of
[Filee](https://github.com/KnifeLemon/Filee) by KnifeLemon (MIT). Its format
table, conversion graph and edge costs, output naming rules and CLI contract are
ported from Filee. See [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

[Unreleased]: https://github.com/suraniharsh/morphring/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/suraniharsh/morphring/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/suraniharsh/morphring/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/suraniharsh/morphring/releases/tag/v0.1.0
