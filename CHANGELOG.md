# Changelog

All notable changes to MorphRing are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/suraniharsh/morphring/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/suraniharsh/morphring/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/suraniharsh/morphring/releases/tag/v0.1.0
