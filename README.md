# MorphRing

Drag a file, pick a format, done. MorphRing is an offline file converter: drop a
file on it, a ring of target formats pops up, and you choose one. Images, audio,
video, PDFs, office documents and archives are converted on your own machine,
natively or by tools you already trust (ImageMagick, FFmpeg, LibreOffice, Pandoc,
Ghostscript, poppler). Nothing is uploaded anywhere.

MorphRing is a Rust re-implementation of [Filee](https://github.com/KnifeLemon/Filee)
by KnifeLemon. Filee did the hard thinking: the format table, the conversion graph
and its edge costs, the output naming rules and the CLI contract. MorphRing ports
that behaviour (and fixes a couple of bugs on the way) but contains no Filee code.
Thank you, KnifeLemon. See [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) for
Filee's licence.

## Status

- **v0.2.0** brings presets, `morphring watch` for folders, PDF, SVG and archive
  conversions, Pandoc and Ghostscript, and a settings and history window for the
  ring. It is the first release with Windows and macOS builds.
- **Linux is the main platform.** The CLI and the ring app (drop zone, ring,
  tray, Ctrl+drag, file manager entries) are used daily on Arch with Hyprland.
- **Windows and macOS are previews.** They build and pass the test suite on CI,
  but they are unsigned and parts of them have not been tried on a real machine
  yet (see [Not yet verified on real hardware](#not-yet-verified-on-real-hardware)).

## Engines

MorphRing does the planning; external programs do most of the converting.
Common images, SVG, PDF page tools (extract, split, merge, images to PDF) and
zip and tar archives are handled natively, with nothing to install. For the
rest, install the programs for the files you care about. Each is optional: a
missing one only disables the conversions that need it, and `morphring doctor`
tells you what is there and how to install what is not.

| Engine | Converts | Arch Linux | Debian / Ubuntu | Windows | macOS |
|---|---|---|---|---|---|
| ImageMagick 7 (`magick`) | images | `sudo pacman -S imagemagick` | `sudo apt install imagemagick` | `winget install -e --id ImageMagick.ImageMagick` | `brew install imagemagick` |
| FFmpeg (`ffmpeg`, `ffprobe`) | audio and video | `sudo pacman -S ffmpeg` | `sudo apt install ffmpeg` | `winget install -e --id Gyan.FFmpeg` | `brew install ffmpeg` |
| LibreOffice (`soffice`) | office documents | `sudo pacman -S libreoffice-fresh` | `sudo apt install libreoffice` | `winget install -e --id TheDocumentFoundation.LibreOffice` | `brew install --cask libreoffice` |
| poppler (`pdftoppm`, `pdftotext`) | PDF pages to images and text | `sudo pacman -S poppler` | `sudo apt install poppler-utils` | `scoop install poppler` | `brew install poppler` |
| Ghostscript (`gs`) | PDF, EPS and PS | `sudo pacman -S ghostscript` | `sudo apt install ghostscript` | `scoop install ghostscript` | `brew install ghostscript` |
| Pandoc (`pandoc`) | Markdown, HTML, EPUB and other markup | `sudo pacman -S pandoc-cli` | `sudo apt install pandoc` | `winget install -e --id JohnMacFarlane.Pandoc` | `brew install pandoc` |

MorphRing looks for the ImageMagick 7 `magick` binary. Older distributions that
still ship ImageMagick 6 (only `convert`) won't be picked up.

## Install

Every release on the [releases page](https://github.com/suraniharsh/morphring/releases)
comes with a `SHA256SUMS.txt`. Check your download against it before you run it:

```sh
sha256sum -c --ignore-missing SHA256SUMS.txt
```

On macOS use `shasum -a 256 -c --ignore-missing SHA256SUMS.txt`; on Windows,
`Get-FileHash <file>` in PowerShell prints the hash to compare by eye.

### Linux

The command line tool, from the tarball:

```sh
tar -xzf morphring-v0.2.0-x86_64-linux.tar.gz
install -Dm755 morphring-v0.2.0-x86_64-linux/morphring ~/.local/bin/morphring
```

The ring app, as a `.deb` (Debian, Ubuntu and friends):

```sh
sudo apt install ./morphring-app_0.2.0_amd64.deb
```

or as an AppImage, anywhere else:

```sh
chmod +x MorphRing_0.2.0_x86_64.AppImage
./MorphRing_0.2.0_x86_64.AppImage
```

On Arch Linux the CLI is in the AUR as
[`morphring-bin`](https://aur.archlinux.org/packages/morphring-bin):

```sh
yay -S morphring-bin
```

Ctrl+drag (hold Ctrl while dragging files out of a file manager, and the ring
opens under the cursor) reads `/dev/input`, which needs your user in the `input`
group (`sudo usermod -aG input $USER`, then log in again). Without it the main
window's drop zone is the way in.

### Windows (preview)

- **CLI:** unpack `morphring-v0.2.0-x86_64-windows.zip` and put `morphring.exe`
  somewhere on your `PATH`.
- **Ring app:** run `MorphRing_0.2.0_x64-setup.exe`. It installs for the current
  user only (no admin rights) and adds "Convert with MorphRing" to the Explorer
  context menu and to Send To.

The builds are not signed, so SmartScreen warns on first start: click
**More info**, then **Run anyway**. On Windows 11 the Explorer entry sits under
**Show more options** (Shift+F10).

### macOS (preview, Apple Silicon)

- **CLI:** unpack `morphring-v0.2.0-aarch64-macos.tar.gz` and copy `morphring`
  to a folder on your `PATH`, for example `/usr/local/bin`.
- **Ring app:** open `MorphRing_0.2.0_aarch64.dmg` and drag MorphRing to
  Applications.

The builds are ad-hoc signed, not notarized (there is no Apple Developer ID
behind them), so Gatekeeper blocks the first start:

1. Open MorphRing once; macOS says it cannot verify the developer. Click **Done**.
2. Open **System Settings > Privacy & Security**, scroll to Security and click
   **Open Anyway** next to MorphRing, then confirm.

Or remove the quarantine flag in a terminal instead:

```sh
xattr -dr com.apple.quarantine /Applications/MorphRing.app
xattr -d com.apple.quarantine /usr/local/bin/morphring
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
the release uses. The ring app lives in `app/` and builds with the Tauri CLI
(`npx @tauri-apps/cli@2.12.1 build` in `app/`).

### File manager integration

`morphring integrate install` adds "Convert with MorphRing" to your file manager;
`morphring integrate uninstall` takes it away again. Add `--dry-run` to see what
would change first. The entry starts the ring app, which `integrate` looks for
next to `morphring` and on `PATH`; name it with `--exe <path>` otherwise.

| OS | What `integrate install` adds |
|---|---|
| Linux | Context menu entries for Dolphin, Nautilus, Nemo and Thunar |
| Windows | An Explorer verb for files and folders, and a Send To shortcut (the setup.exe already does this) |
| macOS | A Finder Quick Action, `~/Library/Services/Convert with MorphRing.workflow` |

On Windows (in `cmd`; the installer puts the app in `%LOCALAPPDATA%\MorphRing`):

```bat
morphring integrate install --exe "%LOCALAPPDATA%\MorphRing\morphring-app.exe"
```

On macOS, point it at the program inside the app bundle:

```sh
morphring integrate install --exe /Applications/MorphRing.app/Contents/MacOS/morphring-app
```

On macOS, if the Quick Action does not show up, turn it on in
**System Settings > General > Login Items & Extensions > Finder**.

## Usage

```text
  morphring convert <files or folders...> --to <format> [options]
  morphring convert <files or folders...> --preset <preset> [options]
  morphring watch <folder> --to <format> [options]
  morphring formats [<format>]
  morphring presets
  morphring doctor [--json]
  morphring integrate install|uninstall
  morphring help [<command>]
  morphring version
```

`morphring --help` shows this with examples, and `morphring help convert` lists
every option of `convert`. `/?` works too, for old Windows habits.

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
such as JPG, WebP, AVIF and MP3. Use `--quiet` (long form only) to print nothing
but the files that failed, on stderr.

```text
$ morphring convert photo.png --to webp -q 80
ok    photo.png -> photo.webp
Converted 1 of 1 file(s).
```

### JSON for scripts

`--json` prints one JSON document on stdout with the preset (or `null`), the
target, the counts and every file's `source`, `state` (`done`, `skipped`, `failed`
or `cancelled`) and `outputs`.
Logs always go to stderr, so piping is safe:

```text
$ morphring convert photo.png --to webp --json | jq '{converted, skipped, failed}'
{
  "converted": 1,
  "skipped": 0,
  "failed": 0
}
```

### Presets

A preset is a target format plus options: `Half size` halves an image, `JPG 70%`
picks the quality, `Merge PDF` joins PDFs into one. The ring's slices are presets,
and the command line uses the same list:

```text
$ morphring presets
ID                       NAME                         TARGET
to-png                   PNG                          png
to-jpg                   JPG                          jpg
jpg-70                   JPG 70%                      jpg
...
half-size                Half size                    *

$ morphring convert photo.png --preset half-size
ok    photo.png -> photo_50.png
Converted 1 of 1 file(s).
```

`--preset` takes the id or the name (`--preset 'Half size'`). A `*` target keeps
the source's format. The presets live in `presets.json` in the config folder that
`morphring doctor` prints.

### Watch a folder

`morphring watch` converts every file that lands in a folder until you press
Ctrl+C. Outputs go to `<folder>/converted` unless you pass `-o`:

```text
$ morphring watch ~/Inbox --to webp
Watching /home/you/Inbox -> WEBP, saving to /home/you/Inbox/converted. Press Ctrl+C to stop.
ok    Inbox/photo.png -> Inbox/converted/photo.webp
```

A file is converted once it has not changed for 5 seconds (`--settle`), so
half-copied downloads are left alone. `--move-originals` moves each converted
source to `<folder>/originals` and also converts the files already waiting when
the watch starts; `--recursive`, `--preset` and `--keep-dates` work as in
`convert`. Watching is a command line feature; the ring app does not watch
folders.

### Keep file dates

`--keep-dates` gives each output the dates of its source (modified and accessed,
plus created where the system allows it), handy when a photo library sorts by
date. The ring app has the same switch in
its settings.

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
ok       image    0.2.0
ok       resvg    0.2.0
ok       pdf      0.2.0
ok       magick   7.1.2-34  /usr/bin/magick
ok       poppler  26.08.0   /usr/bin/pdftoppm
ok       archive  0.2.0
ok       ffmpeg   9.0.2     /usr/bin/ffmpeg
ok       pandoc   3.11      /usr/bin/pandoc
ok       ghostscript 10.08.0   /usr/bin/gs
ok       soffice  26.2.6.3  /usr/bin/soffice

config   /home/you/.config/morphring
data     /home/you/.local/share/morphring
temp     /tmp/morphring

10 of 10 engines available.
```

Your versions and paths will differ. The native engines (`image`, `resvg`,
`pdf`, `archive`) are built in and always there. A missing engine shows the command
that installs it on your system; `morphring doctor --json` gives the same report as
JSON.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Everything converted (skipped files count as fine) |
| 1 | Some files failed or the run was cancelled |
| 2 | Wrong arguments or a missing input |
| 3 | Nothing to convert (for example an empty folder) |

Ctrl+C stops a conversion cleanly: running tools are stopped, unfinished output is
removed and MorphRing exits with 1. Press it a second time to quit at once (130).

## The ring app

Drop files on the MorphRing window (or Ctrl+drag them on Linux, or use the file
manager entry) and a ring of presets for those files opens. Pick a slice and the
conversion runs; the ring shows its progress, a click on the cross (or Escape)
cancels it, and a click on the finished ring shows the first output in your file
manager. Jobs that run for a while also get a small toast with their progress.

The tray icon opens:

- **Settings:** where outputs go (next to the original, a subfolder or a folder
  of your choice), keep file dates, keep history.
- **History:** the last 200 conversions, with their files and results.

## Not yet verified on real hardware

These are things the tests and CI cannot see, collected from the `Unverified:`
notes in the commit history since v0.1.1. If you try one, an issue saying it works
(or doesn't) is very welcome.

macOS:

- The Gatekeeper flow when opening the dmg on a real Mac, and the ring's
  transparency there.
- Ring transparency in a bundled .app (tauri#13415).
- Files arriving through the Opened event from Finder (Open With, a drop on the
  Dock icon).
- The Finder Quick Action showing up in Finder (its workflow file is
  hand-written, not saved by Automator).

Windows:

- The ring staying on top, and not stealing focus, on a real Windows desktop.
- The Explorer verb and the Send To entry appearing after install.

Linux (Hyprland, checked only through tests so far):

- Converting a video, closing the ring, and the toast showing progress with a
  working Cancel.
- Escape during a large video's progress in the ring stopping it with no partial
  output.
- Dropping a png and the ring showing PNG, JPG, WEBP, PDF, TIFF, BMP, ICO and Half
  size, with Half size writing `<name>_50.png`.
- Tray > Settings, choosing the subfolder `out`, and `settings.json` showing it.
- Converting through the ring and tray > History showing the job.
- Dragging files onto the main window and clicking a ring slice by hand.

## Privacy

MorphRing makes no network connections. No telemetry, no analytics, no crash
reports, no update checks. Your files go from your disk, through programs on your
disk, back to your disk. If an update check ever arrives, it will be opt-in.

One caveat: Pandoc may fetch images that a document links to on the web while it
converts that document. MorphRing does not ask it to, but does not stop it yet
either.

## Development

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The golden-file tests run real conversions and skip (printing `SKIP: ...`) when an
engine is not installed, so a bare machine still passes. Install every engine in
the table above before trusting a green run.

The desktop app in `app/` is its own cargo workspace (it pulls in Tauri and
WebKitGTK), so the commands above never build it. Test it with
`cargo test --manifest-path app/src-tauri/Cargo.toml`.

Read these before sending a change:

- [docs/PLAN.md](docs/PLAN.md): roadmap, decisions and the v0.1.0 task list
- [docs/PLAN-v0.2.md](docs/PLAN-v0.2.md): the v0.2 task list
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): how the pieces fit together
- [docs/ENGINEERING.md](docs/ENGINEERING.md): how MorphRing is built and the rules it follows
- [docs/reverse-engineering/](docs/reverse-engineering/): what Filee does, in detail
- [docs/research/](docs/research/): the Rust ecosystem notes behind the design

## Licence

MorphRing is released under the MIT licence; see [LICENSE](LICENSE). It builds on
Filee (MIT) and drives external programs under their own licences; see
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
