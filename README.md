# MorphRing

Drag a file, pick a format, done. MorphRing is an offline file converter: drop a
file on it, a ring of target formats pops up, and you choose one. Images, audio,
video and office documents are converted on your own machine by tools you already
trust (ImageMagick, FFmpeg, LibreOffice). Nothing is uploaded anywhere.

MorphRing is a Rust re-implementation of [Filee](https://github.com/KnifeLemon/Filee)
by KnifeLemon. Filee did the hard thinking: the format table, the conversion graph
and its edge costs, the output naming rules and the CLI contract. MorphRing ports
that behaviour (and fixes a couple of bugs on the way) but contains no Filee code.
Thank you, KnifeLemon. See [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) for
Filee's licence.

## Status

- **v0.1.0 is the command line tool** (`morphring`), Linux first. It converts
  files, lists formats and checks which engines are installed.
- **The desktop ring follows in v0.1.1**: a drop zone, the ring menu, a tray icon and
  file manager integration. Until then the ring lives in `app/` and is a work in
  progress, so please don't judge it yet.

Windows and macOS are untested for now. The code tries to be polite to them, but
nobody has checked.

## Install

MorphRing does the planning; external programs do the converting. Install the ones
for the files you care about. Each is optional: a missing one only disables the
conversions that need it, and `morphring doctor` tells you what is there.

| Engine | Converts | Arch Linux | Debian / Ubuntu |
|---|---|---|---|
| ImageMagick 7 (`magick`) | images | `sudo pacman -S imagemagick` | `sudo apt install imagemagick` |
| FFmpeg (`ffmpeg`, `ffprobe`) | audio and video | `sudo pacman -S ffmpeg` | `sudo apt install ffmpeg` |
| LibreOffice (`soffice`) | office documents | `sudo pacman -S libreoffice-fresh` | `sudo apt install libreoffice` |

MorphRing looks for the ImageMagick 7 `magick` binary. Older distributions that
still ship ImageMagick 6 (only `convert`) won't be picked up.

### From a release tarball

Download `morphring-v0.1.0-x86_64-linux.tar.gz` and `SHA256SUMS.txt` from the
[releases page](https://github.com/suraniharsh/morphring/releases), then:

```sh
sha256sum -c SHA256SUMS.txt
tar -xzf morphring-v0.1.0-x86_64-linux.tar.gz
install -Dm755 morphring-v0.1.0-x86_64-linux/morphring ~/.local/bin/morphring
```

### From source

You need a stable Rust toolchain (1.93 or newer; `rust-toolchain.toml` picks it for
you with rustup).

```sh
git clone https://github.com/suraniharsh/morphring.git
cd morphring
cargo install --locked --path crates/cli
```

`--locked` builds with the exact dependency versions in `Cargo.lock`, the same ones
the release uses.

## Usage

```text
  morphring convert <files or folders...> --to <format> [options]
  morphring formats [<format>]
  morphring doctor [--json]
  morphring integrate install|uninstall
  morphring help [<command>]
  morphring version
```

`morphring --help` shows this with examples, and `morphring help convert` lists
every option of `convert`.

### Convert one file

```text
$ morphring convert photo.png --to jpg
ok    photo.png -> photo.jpg
Converted 1 of 1 file(s).
```

The source is never touched. The output lands next to it, and if `photo.jpg` already
exists you get a numbered copy instead of a clobbered file:

```text
$ morphring convert photo.png --to jpg
ok    photo.png -> photo (2).jpg
Converted 1 of 1 file(s).
```

Pass `--skip` to leave existing outputs alone, or `--overwrite` to replace them.

```text
$ morphring convert photo.png --to jpg --skip
skip  photo.png (the output file already exists)
Converted 0 of 1 file(s).
```

### Convert a folder into another folder

```text
$ morphring convert recordings --to mp3 -o mp3s
ok    recordings/interview.wav -> mp3s/interview.mp3
ok    recordings/voice-memo.wav -> mp3s/voice-memo.mp3
Converted 2 of 2 file(s).
```

Files are converted in parallel, so the `ok` lines come in the order the files
finish. Add `--recursive` to include sub folders. Wildcards work too:
`morphring convert '*.heic' --to jpg`.

### Quality

`-q` means **quality**, not quiet. It takes 1 to 100 and applies to lossy targets
such as JPG, WebP, AVIF and MP3. Use `--quiet` (long form only) to print just the
summary and errors.

```text
$ morphring convert photo.png --to webp -q 80
ok    photo.png -> photo.webp
Converted 1 of 1 file(s).
```

### JSON for scripts

`--json` prints one JSON document on stdout with the target, the counts and every
file's `source`, `state` (`done`, `skipped`, `failed` or `cancelled`) and `outputs`.
Logs always go to stderr, so piping is safe:

```text
$ morphring convert photo.png --to webp --json | jq '{converted, skipped, failed}'
{
  "converted": 1,
  "skipped": 0,
  "failed": 0
}
```

### What can a format become?

```text
$ morphring formats wav
WAV (wav, wave) converts to:
  mp3, m4a, flac, ogg, opus
```

`morphring formats` with no argument lists every format MorphRing knows, grouped by
kind, with its file extensions.

### Which engines are installed?

```text
$ morphring doctor
ok       magick   7.1.2-32  /usr/bin/magick
ok       ffmpeg   9.0.2     /usr/bin/ffmpeg
ok       soffice  26.2.6.3  /usr/bin/soffice

config   /home/you/.config/morphring
data     /home/you/.local/share/morphring
temp     /tmp/morphring

3 of 3 engines available.
```

Your versions and paths will differ. A missing engine shows the command that installs
it; `morphring doctor --json` gives the same report as JSON.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Everything converted (skipped files count as fine) |
| 1 | Some files failed or the run was cancelled |
| 2 | Wrong arguments or a missing input |
| 3 | Nothing to convert (for example an empty folder) |

Ctrl+C stops a conversion cleanly: running tools are stopped, unfinished output is
removed and MorphRing exits with 1. Press it a second time to quit at once (130).

## Privacy

MorphRing makes no network connections. No telemetry, no analytics, no crash
reports, no update checks. Your files go from your disk, through programs on your
disk, back to your disk. If an update check ever arrives, it will be opt-in.

## Development

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The golden-file tests run real conversions and skip (printing `SKIP: ...`) when an
engine is not installed, so a bare machine still passes. Install all three engines
before trusting a green run.

The desktop app in `app/` is its own cargo workspace (it pulls in Tauri and
WebKitGTK), so the commands above never build it.

Read these before sending a change:

- [docs/PLAN.md](docs/PLAN.md): roadmap, decisions and the v0.1.0 task list
- [docs/ENGINEERING.md](docs/ENGINEERING.md): how MorphRing is built and the rules it follows
- [docs/reverse-engineering/](docs/reverse-engineering/): what Filee does, in detail
- [docs/research/](docs/research/): the Rust ecosystem notes behind the design

## Licence

MorphRing is released under the MIT licence; see [LICENSE](LICENSE). It builds on
Filee (MIT) and drives external programs under their own licences; see
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
