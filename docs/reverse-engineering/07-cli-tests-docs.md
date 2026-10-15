# 07 — Filee CLI, tests, docs and licenses: reverse-engineering spec

Scope: `src/Filee.Cli/**`, `tests/**`, `README.md`, `CHANGELOG.md`, `CONTRIBUTING.md`, `docs/**` and
`THIRD-PARTY-NOTICES.md` of Filee (C#/.NET 10, MIT, version 1.10.3 per `Directory.Build.props:17`). All paths are
relative to the Filee repository root. Line numbers refer to the snapshot we reviewed.

The CLI is small (657 lines in 6 files). Almost everything it does is delegated to `Filee.Core` (job queue, output
naming, watcher; see `01-core.md`) and `Filee.Engines` (the converters; see `02`–`04`). This document pins down the
**exact user-visible contract**: commands, flags, messages, `--json` shape and exit codes. It also covers what the test
suite treats as required behaviour, the full format list, the release history, and the licenses we have to respect.

| File | Lines | What it is |
|---|---|---|
| `src/Filee.Cli/Cli.cs` | 330 | Command dispatch, convert/watch/formats/presets, output printing, JSON, input expansion |
| `src/Filee.Cli/CliOptions.cs` | 129 | Hand-written argument parser (no library) |
| `src/Filee.Cli/Help.cs` | 81 | Usage texts and version string |
| `src/Filee.Cli/CliHost.cs` | 44 | Builds settings + engines + job queue without a UI |
| `src/Filee.Cli/EnglishTexts.cs` | 30 | English strings from the app's embedded `en.json` |
| `src/Filee.Cli/Program.cs` | 16 | `main`: UTF-8 console, two-stage Ctrl+C |
| `tests/Filee.Cli.Tests/CliTests.cs` | 228 | 13 end-to-end CLI tests (real engines) |

---

## 1. How the binary is shipped and named

- The assembly is `filee-cli` (`src/Filee.Cli/Filee.Cli.csproj:9`). It is published **into the same folder as the GUI
  app**, so the two share one .NET runtime and the bundled engines (`docs/ARCHITECTURE.md:77-80`).
- It cannot be called `filee.exe`, because it sits next to `Filee.exe` and Windows file systems ignore case. So:
  - Windows: `cli\filee.cmd` contains `@"%~dp0..\filee-cli.exe" %*` (`build/build-installer.ps1:103-110`). The
    installer can add that `cli` folder to the system PATH (`installer/Filee.iss:15,334`).
  - Linux: `install.sh` symlinks `~/.local/bin/filee` → `<install_dir>/filee-cli` (`installer/linux/install.sh:23-27`).
    `uninstall.sh` removes the link only if it still points there (`installer/linux/uninstall.sh:19`).
  - macOS: the same symlink, to `Filee.app/Contents/MacOS/filee-cli` (`installer/macos/install.sh:18`).
- The CLI reads the **app's own settings folder** (`CliHost.cs:26-29` → `UserDataStore.DefaultDirectory`, which is
  `SpecialFolder.ApplicationData/Filee`): `%APPDATA%\Filee`, `~/Library/Application Support/Filee` or
  `~/.config/Filee` (README "Build from source"). The user's presets, engine priority, the paths they picked with
  "use my copy" and the LibreOffice switches all apply to the CLI. Side effect: `store.Load()` **creates that folder**
  and may write schema migrations back to disk (`src/Filee.Core/Settings/UserDataStore.cs:55-75`). The CLI never
  writes history.

**For MorphRing:** a single `morphring` binary has no name clash on Linux. Keep the "CLI and GUI share one config
folder" idea (`~/.config/morphring`).

---

## 2. Process entry (`Program.cs`)

```text
Console.OutputEncoding = UTF-8
cancel = new CancellationTokenSource
on Ctrl+C:
    if cancel already requested → return without e.Cancel  (the runtime kills the process: 2nd Ctrl+C = hard exit)
    else e.Cancel = true (keep the process alive) and cancel.Cancel()
exit code = await Cli.RunAsync(args, stdout, stderr, cancel.Token)
```

- First Ctrl+C: cooperative. A running `convert` job is cancelled (files become `Cancelled`) and the command exits
  with code **1**. `watch` stops and exits with code **0**.
- Second Ctrl+C: the process dies immediately. Its exit code is whatever the OS gives a SIGINT-killed process (130 in a
  shell). Partial outputs may be left behind (engines clean up on cooperative cancel only).
- `Cli.RunAsync` takes the writers and a `createHost` factory as parameters, so tests can run it in-process with
  `StringWriter`s and a temporary settings folder (`Cli.cs:22-24`, `CliTests.cs:27-33`). **Copy this design:** make
  `run(args, stdout, stderr, cancel, host_factory) -> i32` a library function and keep `main` tiny.

---

## 3. Argument parser (`CliOptions.cs`)

The parser is hand-written on purpose ("Kept dependency-free", `CliOptions.cs:1`). Exact rules:

### 3.1 Data model

```text
CliOptions {
  Command: string = "help"
  Inputs: List<string>            // positional arguments, in order
  To: string?                     // -t / --to
  Preset: string?                 // -p / --preset
  Output: string?                 // -o / --output
  Recursive: bool                 // -r / --recursive
  Quality: int? (1..=100)         // -q / --quality   (NOTE: -q is quality, NOT quiet)
  Conflict: ConflictPolicy?       // --overwrite → Overwrite, --skip → Skip (last one wins)
  Json: bool                      // --json
  Quiet: bool                     // --quiet (no short form)
  MoveOriginals: bool             // --move-originals
  KeepDates: bool                 // --keep-dates
  SettleSeconds: double? (>= 0)   // --settle
  Error: string?                  // first error message wins
}
Commands = ["convert", "watch", "formats", "presets", "help", "version"]
```

### 3.2 Algorithm

1. No arguments → command `help`.
2. `first = args[0].ToLowerInvariant()`. `-h`, `--help`, `/?` → `help`; `-v`, `--version` → `version`; anything else
   is used as the command name. Commands are **case-insensitive** (`CONVERT` works).
3. If the command is not in `Commands` → error `Unknown command '<args[0] as typed>'.` and stop parsing. So
   `filee --to jpg` gives "Unknown command '--to'".
4. Loop over `args[1..]`, matching each one lower-cased (so `--TO` works):
   - Options that take a value consume the **next** argument whatever it looks like (`--to --json` sets
     To = "--json"). If there is no next argument: error `<arg as typed> needs a value.` (for example
     `-t needs a value.`).
   - `--quality`: `int.TryParse` and 1–100, else `--quality must be a number from 1 to 100.`
   - `--settle`: invariant-culture float, `>= 0`, else `--settle must be a number of seconds.`
   - `-h` / `--help` anywhere after the command: insert the command name at the front of `Inputs`, return a copy
     with Command = `help` **immediately** (the remaining arguments are not parsed and the final validation is
     skipped). So `filee convert --help` prints the convert help. Note: an error set *before* the `-h` survives
     the copy, so `filee convert --bogus --help` still fails with "Unknown option".
   - Unknown option: anything starting with `--`, or exactly two characters starting with `-` →
     `Unknown option '<arg>'.` Everything else is a positional input. Quirks: `-abc` (single dash, 3+ chars) is an
     **input file**, `-` alone is an input file (and later "not found: -"). Negative numbers are inputs too.
   - Errors use `??=`: the **first** error is kept, later ones are ignored.
5. Final validation, only when there is no error yet:
   - `convert` with no inputs → `Give at least one file or folder to convert.`
   - `convert` or `watch` where **both or neither** of `--to`/`--preset` are given →
     `Use either --to <format> or --preset <preset>.`
   - `watch` without exactly one input → `Give exactly one folder to watch.`

   Order matters: `convert --to jpg` with no inputs reports "at least one file" (test, `CliTests.cs:108`);
   `convert a.png` with neither flag reports "Use either --to…" (test line 104).
6. Options are accepted by **every** command and silently ignored where they mean nothing: `--json` on `watch`,
   `--move-originals` / `--settle` on `convert`, any flag on `formats`, `presets`, `version`, extra inputs after
   `formats <format>` or `version`.

### 3.3 Error output

On any parse error (`Cli.cs:27-32`), to **stderr**:

```text
filee: <message>
Run 'filee help' for usage.
```

and exit code **2**. Nothing is written to stdout.

---

## 4. Exit codes (`Cli.cs:1-3,17-20`)

| Code | Name | When |
|---|---|---|
| 0 | `Success` | help, version, formats, presets; convert where no file failed (skipped files are fine); watch stopped with Ctrl+C |
| 1 | `SomeFailed` | convert where at least one file is `Failed` **or `Cancelled`** (Ctrl+C during convert gives 1) |
| 2 | `UsageError` | parse error; unknown format; read-only target format; unknown preset; any missing input; `formats <unknown>`; watch folder that can't be watched |
| 3 | `NothingToDo` | convert where the inputs expanded to zero files (for example a folder with no known formats) |

Note: a wildcard that matches nothing counts as a **missing input** (2), not "nothing to do" (3). Only a folder that
has no known files gives 3.

---

## 5. Commands

### 5.1 `help [<command>]` and `version`

- Run **before** the host is created, so they need no engines and no settings folder (test
  `Help_and_version_need_no_engines`, `CliTests.cs:171-181`).
- `help` writes `Help.Text(Inputs[0])` to stdout: `convert`, `watch`, `formats` and `presets` have their own text;
  anything else (or nothing) gives the general text. The full texts are in `Help.cs:21-80`; reproduce them nearly
  word for word (see §10.1).
- `version` prints `filee <version>`. The version is the assembly's InformationalVersion with any `+<git sha>` suffix
  removed (`Help.cs:9-10`), `?` if missing. Test: output starts with `"filee "`.

### 5.2 `convert <inputs...> (--to <format> | --preset <preset>) [options]`

Control flow (`Cli.cs:53-82`):

```text
preset = ResolvePreset(options)                  // §6; on failure: message already printed → exit 2
if --output: preset.Output.Location = CustomFolder; CustomFolder = full path of --output
missing = []
files = ExpandInputs(inputs, recursive, missing) // §7
for each m in missing: stderr "filee: not found: <m as typed>"
if missing not empty → exit 2                    // nothing is converted, even the inputs that exist
if files empty: stderr "filee: no files to convert." → exit 3
job = RunJob(files, preset, printer = (json or quiet) ? none : stdout)
done   = count(state == Done)
failed = count(state in {Failed, Cancelled})
if json: print JSON document (§5.2.2)            // --json wins over --quiet
elif not quiet:
    failed == 0 ? "Converted {done} of {total} file(s)."
                : "Converted {done} of {total} file(s), {failed} failed."
exit failed == 0 ? 0 : 1
```

The summary line does not mention skipped files: with `--skip` and every output present you get
"Converted 0 of 3 file(s)." and exit code 0.

#### 5.2.1 Per-file lines (human mode)

`RunJobAsync` (`Cli.cs:120-148`) subscribes to `JobQueue.JobUpdated` (raised from worker threads). On every update it
prints, under a lock, each file whose state is no longer `Pending`/`Running` and that has not been printed yet. After
the job ends it runs the same check once more to catch stragglers. Consequences:

- Lines appear **as files finish**, so with parallel conversion the order is not the input order.
- Each file is printed exactly once.

Line formats (`Cli.cs:150-166`). The column is padded so the paths line up: "ok" + 4 spaces, "skip" + 2, "stop" + 2,
"fail" + 2.

```text
ok    <src> -> <out1>, <out2>, ...
skip  <src> (the output file already exists)
stop  <src> (cancelled)
fail  <src>: <message>[ (<detail>)]
```

- `<src>` and `<out>` go through `Shorten`: the path relative to the current directory when it is inside it, else
  the full path (`Cli.cs:169-173`).
- `<message>` = English text of `ErrorKey`, or `conversion failed` when there is no key (a cancelled file has none).
  The keys the queue sets and their English texts (`src/Filee.App/Assets/i18n/en.json:55-57`):

  | ErrorKey | English text | Typical detail |
  |---|---|---|
  | `error.unsupported_source` | This file type isn't supported | `.xyz` (dot + extension) |
  | `error.no_route` | No engine can do this conversion | `txt → mp3` (format ids, U+2192 arrow) |
  | `error.conversion_failed` | The engine reported an error | the exception message |

  ` (<detail>)` is added only when the detail is not empty or whitespace.
- Merge/one-archive presets put the single output on the **first** file only. The other files are `Done` with no
  outputs, so they print `ok    b.jpg -> ` with an empty list. That's a quirk; don't copy it.

#### 5.2.2 `--json` document (`Cli.cs:175-204`)

Written once, after the job finishes, as one indented JSON object (2-space indent, `Utf8JsonWriter`), then a newline.
No per-file lines, no summary.

```json
{
  "preset": "WEBP",
  "target": "webp",
  "converted": 1,
  "skipped": 0,
  "failed": 0,
  "files": [
    {
      "source": "/abs/path/a.png",
      "state": "done",
      "outputs": [
        "/abs/path/a.webp"
      ]
    },
    {
      "source": "/abs/path/notes.txt",
      "state": "failed",
      "outputs": [],
      "error": "No engine can do this conversion (txt → mp3)"
    }
  ]
}
```

| Field | Type | Meaning |
|---|---|---|
| `preset` | string | Display name of the preset: its `Name`, else the English text of `NameKey`, else `TargetFormat` upper-cased. For `--to webp` that is `"WEBP"` |
| `target` | string | `Preset.TargetFormat` as stored: a format id, or `"*"` (same as source, for example `--preset half-size`) or `"folder"` (extract) |
| `converted` | int | files in state `done` |
| `skipped` | int | files in state `skipped` |
| `failed` | int | files in state `failed` **plus** `cancelled` |
| `files[]` | array | one per input file, **in input order** (unlike the human lines) |
| `files[].source` | string | **absolute** path (not shortened) |
| `files[].state` | string | enum name lower-cased: `pending`, `running`, `done`, `skipped`, `failed`, `cancelled` (only the last four happen after a run) |
| `files[].outputs` | string[] | absolute output paths. Several for multi-page outputs (`name_p1.png`…), empty for failures, skips and the non-first files of a merge |
| `files[].error` | string | **only** when the state is `failed` or `cancelled`. Same text as the human line: `message (detail)` |

Details:

- `Utf8JsonWriter` uses the default `JavaScriptEncoder`, which escapes all non-ASCII characters and HTML-sensitive
  ones (`'`, `"`, `<`, `>`, `&`, `+`) as `\uXXXX`. A file named `한글 report's.png` comes out as
  `"\uD55C\uAE00 report\u0027s.png"`. Every JSON parser decodes that to the same string, so emitting raw UTF-8 (what
  `serde_json` does) is compatible.
- Test `Json_output_lists_every_file` (`CliTests.cs:64-77`): `--to .webp` (leading dot accepted), `converted == 1`,
  `files[0].state == "done"`, `files[0].outputs[0]` ends with `a.webp`.
- Test `Case_distinct_inputs_are_both_converted_and_exact_duplicates_are_removed` (`CliTests.cs:183-206`): inputs
  `photo.png PHOTO.png photo.png` on a case-sensitive file system give 2 files, 2 converted, 2 distinct outputs that
  exist.
- Test `Unicode_spaces_and_quotes_stay_in_a_single_input_argument` (`CliTests.cs:208-227`): source
  `한글 report's "quoted" 100%.png` (no `"` on Windows), output folder `결과 output's`. The output's parent must equal
  the `-o` folder exactly, the name must keep `한글 report's`, and the **source must still exist**.

### 5.3 `watch <folder> (--to | --preset) [options]`

Control flow (`Cli.cs:84-117`):

```text
preset = ResolvePreset(options)                         // exit 2 on failure
rule = WatchRule {
  Folder           = full path of inputs[0]
  IncludeSubfolders = --recursive
  OutputFolder     = --output ? full path : ""          // "" → <Folder>/converted
  Originals        = --move-originals ? MoveToOriginals : Keep
}
preset.Output.Location = CustomFolder; CustomFolder = rule.ResolvedOutputFolder
preset.Output.KeepFoldersFrom = rule.Folder             // Inbox/2025/a.jpg → converted/2025/a.pdf
settle = --settle ? seconds : null (watcher default)
watcher = FolderWatcher(rule, convert = files → RunJob(files, preset, quiet ? none : stdout), settle)
if !watcher.Start(): stderr "filee: <watcher.Error>" → exit 2   // "Folder not found: <path>"
unless quiet: stdout "Watching <Folder> → <preset display name>, saving to <output folder>. Press Ctrl+C to stop."
wait until Ctrl+C → dispose watcher (lets a running batch finish moving originals) → exit 0
```

- Each batch prints the same per-file lines as `convert` (§5.2.1). There is no per-batch summary, `--json` is
  ignored, and failed conversions **do not** change the exit code (always 0 after Ctrl+C).
- The folder must exist: `Watch_needs_an_existing_folder` expects exit 2 and "not found" in stderr (case-insensitive).
- Watcher behaviour (core spec `01-core.md`, `src/Filee.Core/Watching/FolderWatcher.cs`). Summary:
  - OS file events (create, change, rename; buffer 64 KiB; on overflow rescan the folder) put the file in a
    `pending` map with its size, mtime and "stable since" time.
  - A 1-second timer checks the pending files. A file is ready when its size and mtime have not changed for
    `settle`, it can be opened exclusively (in practice Windows only) and, on macOS, its creation date is not
    1984-01-24 (Finder's copy-in-progress marker).
  - It is converted once per (path, size, mtime) key. Ready files go to one job per tick, sorted by path. Only one
    batch runs at a time; new files wait for the next tick.
  - Ignored: names starting with `~$` or `.`, `desktop.ini`, `thumbs.db`, extensions `.tmp .temp .part .partial
    .crdownload .download .opdownload .!ut .filepart`, anything inside the output or `originals` folder, subfolders
    unless `-r`, unknown formats, hidden or system attributes.
  - With `--move-originals`, files already in the folder are converted at start, and each `Done` source moves to
    `<folder>/originals/<same relative path>` (numbered ` (2)` on clash). With Keep, existing files are left alone.
  - Test `Watch_converts_files_that_arrive_until_cancelled` (`CliTests.cs:140-157`): `--settle 0.2`, a PNG written
    500 ms after start shows up as `<folder>/converted/arrived.jpg` within 10 s; after cancel the exit code is 0.
- **Doc/code mismatch (fragile):** `Help.cs:76` says `--settle` defaults to 2 seconds, but
  `FolderWatcher.DefaultSettleTime` is 2 s on Windows and **5 s on Linux/macOS** (`FolderWatcher.cs:27`), and a test
  pins that (`FolderWatcherTests.cs:43-44`). On Linux, a still-growing file can be opened exclusively, so the longer
  wait is the only protection. MorphRing should use 5 s on Linux and say so in the help.
- No include patterns, name rules or renames from the CLI. Those exist only for the app's watch folders.

### 5.4 `formats [<format>]` (`Cli.cs:289-319`)

Without an argument, it lists every known format except the `folder` pseudo-format, grouped by `FormatCategory` in
order of first appearance. The heading is the enum name (`Image:`, `Vector:`, `Pdf:`, `Document:`, `Hwp:`,
`Spreadsheet:`, `Presentation:`, `Text:`, `Ebook:`, `Video:`, `Audio:`, `Archive:`, `Cad:`, `Font:`). Each line:

```text
  {id,-10} {DisplayName}{" (read only)" if !Writable}  .{ext1} .{ext2} ...
```

followed by an empty line and `Run 'filee formats <format>' to see what a format converts to.` Example:

```text
Image:
  png        PNG  .png
  jpg        JPG  .jpg .jpeg .jpe .jfif
  xcf        XCF (read only)  .xcf
```

With an argument (looked up like `--to`, §6.1):

```text
<DisplayName> (<ext1>, <ext2>, ...) converts to:
  <id>, <id>, ...                          or "  (nothing with the engines installed now)"
With optional engines (install them in Filee, Settings → Engines):     ← only if the list is non-empty
  <id>, <id>, ...
```

- Targets considered: every writable known format except the source itself and `folder`, in registry order.
- "now" uses the planner over **available** engines. "later" uses a planner that **assumes every registered engine
  is installed** (`Catalog.CreatePlanner(all ids)`), minus the "now" ones.
- Same-format re-encodes (half size, MP4 720p) are not listed, because the source format is excluded.
- An unknown name gives `filee: unknown format '<name>'.` and exit 2 (without the "Run 'filee formats'" hint).
- Test `Formats_and_presets_are_listed`: `formats heic` output contains `jpg`.

### 5.5 `presets` (`Cli.cs:321-329`)

```text
{"ID",-24} {"NAME",-28} TARGET
{preset.Id,-24} {display name,-28} {preset.TargetFormat}     ← one per preset in the user's presets.json order

Use one with: filee convert <files> --preset <id or name>
```

Built-in presets with a `NameKey` show the English text (`Half size`, `Grayscale`, `Merge PDF`, `Split PDF`,
`Long image`, `Extract`, `One ZIP`; `en.json:103-109`). Test: output contains `to-png`.

---

## 6. Resolving the preset (`Cli.cs:206-250`)

### 6.1 `--to <format>`

```text
key = text.Trim().TrimStart('.').ToLowerInvariant()
format = FormatRegistry.FindById(key) ?? FormatRegistry.FindByExtension(key)
```

- So `jpg`, `JPG`, `.jpg`, `jpeg` and ` .Jpeg ` all mean JPG. `tar.gz` finds `tgz` through its extension. Ids win over
  extensions; the only overlap is `raw`, which is both an id and an extension of the same format.
- Unknown: `filee: unknown format '<to>'. Run 'filee formats' for the list.` → exit 2.
- Read-only (for example `raw`, `xcf`, `pages`, `rar`): `filee: Filee reads <DisplayName> but can't write it.` → exit 2.
- Otherwise `preset = new Preset { TargetFormat = format.Id }` with **all default options** (image quality 90, no
  resize, keep metadata, background `#FFFFFF`, PDF render 150 dpi, media Balanced, archive Normal, name `{name}`,
  conflict Rename, location Default).
- Undocumented: `--to folder` works (the `folder` format is "writable"), so `filee convert a.zip --to folder`
  extracts the archive.

### 6.2 `--preset <id or name>`

The first preset whose `Id` matches case-insensitively, else the first whose **display name** matches
case-insensitively (`"merge pdf"` finds `merge-pdf`). Not found: `filee: no preset '<name>'. Run 'filee presets' for
the list.` → exit 2. The preset is **cloned**, so the CLI never changes saved presets.

### 6.3 Overrides, in this order

1. `Settings.WithDefaultOutput(preset)` (`src/Filee.Core/Settings/AppSettings.cs:149-165`): if the preset's location
   is `Default`, use the app's default save location (factory default: `SameFolder`, next to the source). If the
   app-wide "keep file dates" switch is on, set `KeepDates`.
2. `--quality N` → `preset.Image.Quality = N` (applied even when the target isn't an image; harmless).
3. `--overwrite` / `--skip` → `preset.Output.Conflict`.
4. `--keep-dates` → `preset.Output.KeepDates = true`.
5. (convert) `-o DIR` → `Location = CustomFolder`, `CustomFolder = abs(DIR)`. The folder is created on demand, when
   the first output is allocated (`JobQueue.FinalAllocator`). `KeepFoldersFrom` stays empty, so with `-r` and `-o`
   **every file lands flat in DIR** and same-named files get ` (2)`. Only `watch` keeps subfolders.

---

## 7. Input expansion (`Cli.cs:259-287`)

```text
option = recursive ? AllDirectories : TopDirectoryOnly
for each input in order:
  if input contains '*' or '?':
      folder = dirname(input) or "."
      pattern = basename(input)                      // wildcards only in the LAST path segment
      matches = folder exists ? EnumerateFiles(folder, pattern, option) : []
      if matches empty → missing += input
      files += matches                               // NOT filtered by known format
  elif directory exists(input):
      files += EnumerateFiles(input, "*", option) where FormatRegistry.Detect(f) != null   // known formats only
  elif file exists(input):
      files += input                                 // any extension; unknown ones fail later as unsupported
  else:
      missing += input
return files.map(GetFullPath).Distinct(OS comparer)  // first occurrence wins, order kept
```

- The wildcard path exists because `cmd.exe` and PowerShell pass `*.heic` through unexpanded. On Linux the shell
  expands globs first, but a quoted `'*.png'` still works.
- `--recursive` also makes wildcards recurse (`-r '*.png'` matches `*.png` in all subfolders).
- Dedupe is **ordinal (case-sensitive) on Linux/macOS** and case-insensitive on Windows
  (`src/Filee.Core/Platform/FileSystemPaths.cs`). `GetFullPath` makes the path absolute and normalizes `.`/`..`
  without resolving symlinks.
- Directory enumeration order is whatever the file system returns (unsorted). With parallel conversion this barely
  matters, but `{index}` in name patterns and JSON order depend on it. **MorphRing should sort** (an improvement).
- Hidden files in a folder are included if their extension is known (only `watch` skips dot files).

---

## 8. What happens inside the job (the parts that affect CLI behaviour)

Full detail is in `01-core.md`; this is what a CLI implementer needs (`src/Filee.Core/Conversion/JobQueue.cs`):

- Temp work folder: `<tmp>/Filee/<job id (10 hex chars)>`, deleted at the end.
- Normal mode: files run in parallel with `MaxDegreeOfParallelism = CPU count`. Each engine also has a semaphore
  (`MaxParallelism`, for example FFmpeg 2). One file failing never stops the others.
- Per file: detect the source format by extension (`error.unsupported_source` if unknown), target = preset target
  (or the source format for `*`), plan a route of at most **3 hops** (`error.no_route` if none), run each step;
  intermediate files go to the temp folder and only the last step writes to the final location. An exception gives
  `error.conversion_failed` with the message. A cancel gives `Cancelled`.
- An engine that writes **zero outputs** on the final step, because the allocator returned null under the Skip
  policy, marks the file `Skipped`.
- `KeepDates`: after success, copy the source's creation time and mtime to each output (file or folder). On Linux
  only the mtime can be set; the test skips creation time there (`JobQueueTests.cs:48-51`). The CLI test
  `Keep_dates_copies_the_dates_of_the_originals` checks the mtime (2018-05-04 12:00 UTC) on `old.jpg`.
- Special modes, chosen from the preset: merge into one PDF (`Pdf.MergeIntoSingle`, more than one file, target pdf),
  one multi-page TIFF (`Image.MultiPageTiff`), one archive (`Archive.CombineIntoOne`). Reachable from the CLI only
  through `--preset` (`merge-pdf`, `zip-all`, or a user preset).

### 8.1 Output path rules (`src/Filee.Core/Presets/OutputPathResolver.cs`)

```text
dir = SameFolder  → source folder
      Subfolder   → source folder / Sanitize(Expand(SubfolderName), "converted")
      Custom      → CustomFolder / (subpath of source folder below KeepFoldersFrom, or "")
name = Sanitize(Renames(Expand(FileNamePattern)), fallback = source name without ext) + suffix
candidate = dir / name.ext        (no ".ext" when the extension is empty: a folder, for extract)
if candidate != source (case-INSENSITIVE compare, always) and !exists → candidate
if != source and Overwrite → candidate
if != source and Skip → null (skip)
else try "name (2).ext", "name (3).ext", ... until free and != source
```

- Tokens (case-insensitive): `{name}`, `{ext}`, `{preset}`, `{date}` = yyyyMMdd, `{time}` = HHmmss (job creation
  time, local), `{index}` = 1-based position in the batch. A blank pattern means `{name}`.
- Sanitize replaces `< > : " / \ | ? *`, control characters and the OS's invalid characters with `_`, trims spaces,
  strips trailing dots; empty → fallback. Test: `{name}_{preset}_{date}_{index}:?` → `a_PNG_20260930_3__.png`.
- Two-part extensions count as one: `backup.tar.gz` → name `backup`, `{ext}` = `tar.gz`.
- **The source is never overwritten**, under any policy, even when the names differ only in case.
- "Exists" includes paths already reserved by other files of the running batch, so two parallel files named
  `same.jpg` and `same.png` → webp get `same.webp` and `same (2).webp`.

---

## 9. Full format list (`src/Filee.Core/Formats/FormatRegistry.cs:52-255`)

170 real formats + the `folder` pseudo-format; 56 are read-only (RO). The first extension is the one used for
output. Lookup is case-insensitive. Every extension belongs to exactly one format (test `Extensions_are_unique`).
Two-part extensions (`tar.gz` and friends) are checked first, longest first, and only when the file name is longer
than the extension plus its dot.

| Category | id: extensions (RO = read only) |
|---|---|
| Image | png: png · jpg: jpg jpeg jpe jfif · webp · tiff: tiff tif · bmp: bmp dib · gif · ico · avif · heic: heic heif · jxl ("JPEG XL") · jp2: jp2 j2k jpf jpx ("JPEG 2000") · psd · psb · tga: tga icb vda vst · ppm: ppm pgm pbm pnm · icns · xcf (RO) · raw (RO): dng cr2 cr3 crw nef nrw arw srf sr2 orf rw2 raf pef dcr kdc erf mos mrw x3f 3fr fff iiq mef rwl srw raw |
| Vector | svg · svgz · eps: eps epsf epsi · ps · ai (RO) · emf (RO) · wmf (RO) · cdr (RO) · cgm (RO) · vsd (RO): vsd vsdx · odg |
| Pdf | pdf |
| Document | docx · docm · dotx · dotm · doc · dot (RO) · odt · rtf · wps (RO) · wpd (RO) · lwp (RO) · abw (RO): abw zabw · pages (RO) · sdw (RO) · pub (RO) · eml (RO) |
| Hwp | hwpx · hwp |
| Spreadsheet | xlsx · xlsm · xltx · xls: xls xlt · ods · csv · tsv: tsv tab · et (RO) · numbers (RO) · sdc (RO) |
| Presentation | pptx · pptm · potx · ppsx · ppt: ppt pps pot · odp · key (RO, "Keynote") · dps (RO) · sda (RO): sda sdd |
| Text | txt · md: md markdown ("Markdown") · html: html htm xhtml · rst ("reStructuredText") · tex: tex latex ("LaTeX") |
| Ebook | epub · mobi · azw3 · azw (RO) · azw4 (RO) · prc (RO) · fb2 · cbz · cbr (RO) · cb7 (RO) · cbt (RO) · cbc (RO) · htmlz · txtz · chm (RO) · lit · lrf · pdb · pml · rb · snb · tcr · oeb (RO): oeb opf |
| Video | mp4 · m4v · mov: mov qt · mkv · webm · avi: avi divx · wmv: wmv asf · flv: flv f4v · mpg: mpg mpeg mpe m1v m2v ("MPEG") · ts · m2ts: m2ts mts m2t · 3gp: 3gp 3gpp · 3g2 · ogv · vob · mod (RO): mod tod · dv: dv dif · mxf · rm (RO): rm rmvb · swf (RO) · wtv (RO): wtv dvr-ms dvr · cavs (RO) |
| Audio | mp3 · m4a · m4b · aac · wav: wav wave · flac · ogg: ogg oga · opus · wma · aiff: aiff aif aifc · ac3 · amr · au: au snd · caf · weba · mka · mp2: mp2 mpa · dss (RO) · voc |
| Archive | zip · 7z · rar (RO) · tar · tgz: tar.gz tgz taz ("TAR.GZ") · tbz2: tar.bz2 tbz2 tar.bz tbz · txz: tar.xz txz · tar7z (RO): tar.7z · tlz (RO): tar.lz tlz · tz (RO): tar.z tz · gz · bz2: bz2 bz · xz · lz (RO) · lzma (RO) · z (RO) · jar (RO) · cab (RO) · cpio (RO) · deb (RO) · rpm (RO) · dmg (RO) · iso (RO) · img (RO) · lha (RO): lha lzh · arj (RO) · alz (RO) · egg (RO) · folder (pseudo: extract target, extension "") |
| Cad | dwg · dxf |
| Font | ttf · otf · woff · woff2 · eot |

When no display name is given above, it is the id upper-cased (`TAR.GZ`-style names are given explicitly). Note that
`ts` (MPEG transport stream) clashes with TypeScript files, and `.img`, `.mod`, `.rb`, `.pdb`, `.ps` and `.key` are
generic extensions that will misdetect some files. Filee accepts that risk; MorphRing can too, but folder expansion
(§7) will then pick up `*.rb` Ruby files as "ebooks". Content sniffing would be a later improvement.

### 9.1 Built-in presets (`src/Filee.Core/Presets/BuiltInData.cs:278-424`), 68 in total

`to-<id>` with the format name and default options, for: png, jpg (q90), webp (q85), tiff, bmp, ico, gif, avif (q60),
heic (q80), pdf, docx, hwpx, hwp, odt, rtf, txt, md, html, xlsx, csv, jxl (q85), psd, tga, icns, svg, eps, ods, pptx,
odp, epub, mobi, azw3, fb2, cbz, mp4, webm, mov, mkv, avi, mp3, m4a, wav, flac, ogg, opus, aac, zip, 7z, tgz, tar,
dxf, dwg, ttf, otf, woff, woff2, eot. Plus the special ones:

| id | name | target | options |
|---|---|---|---|
| `jpg-70` | JPG 70% | jpg | quality 70, pattern `{name}_70` |
| `half-size` | Half size | `*` | resize 50 %, pattern `{name}_50` |
| `grayscale` | Grayscale | `*` | grayscale, pattern `{name}_gray` |
| `tiff-300` | TIFF 300dpi | tiff | DPI 300, PDF render 300, pattern `{name}_300dpi` |
| `merge-pdf` | Merge PDF | pdf | merge into one, pattern `{name}_merged` |
| `pdf-long-image` | Long image | png | stack PDF pages, pattern `{name}_long` |
| `pdf-split` | Split PDF | pdf | split pages, Subfolder `{name}` |
| `mp4-720` | MP4 720p | mp4 | max height 720, quality Small, pattern `{name}_720p` |
| `mp3-128` | MP3 128k | mp3 | audio 128 kbit/s, pattern `{name}_128k` |
| `extract` | Extract | folder | — |
| `zip-all` | One ZIP | zip | combine into one, pattern `{name}_files` |

---

## 10. Help texts and docs

### 10.1 Help text (verbatim content to mirror, `Help.cs`)

General:

```text
Filee: convert files offline from the command line.

Usage:
  filee convert <files or folders...> --to <format> [options]
  filee convert <files or folders...> --preset <preset> [options]
  filee watch <folder> --to <format> [options]
  filee formats [<format>]
  filee presets
  filee help [<command>]
  filee version

Examples:
  filee convert photo.heic --to jpg
  filee convert *.png --to webp --quality 80 -o converted
  filee convert D:\Scans --recursive --preset to-pdf
  filee watch D:\Inbox --to pdf --move-originals

Presets and engines are the ones in the Filee app. Exit codes: 0 all converted, 1 some failed,
2 wrong arguments or a missing input, 3 nothing to convert.
```

`convert` options: `-t, --to <format>`, `-p, --preset <preset>`, `-o, --output <folder>` ("Save there instead of
next to the sources"), `-r, --recursive`, `-q, --quality <1-100>` ("Image quality for JPG, WebP, AVIF, …"),
`--overwrite` ("Replace existing output files (default: add a number)"), `--skip`, `--keep-dates`, `--json`,
`--quiet` ("Print nothing but errors").

`watch` options: `-t`, `-p`, `-o` ("default: <folder>\converted"), `-r` ("their files keep the sub folder in the
output"), `--move-originals` ("Move converted sources to <folder>\originals; files already waiting are converted at
start"), `--settle <seconds>` ("default 2", which is wrong on Unix, see §5.3), `--keep-dates`, `--quiet`. The watch help
doesn't list `--quality`, `--overwrite` or `--skip`, but they work.

### 10.2 README / docs facts worth keeping

- README "Command line" (lines 154-177) repeats the examples, the four commands and the exit codes, and points to
  `https://filee.sh/cli/` for every option.
- README claims "180 formats"; the registry has 170 + folder (the marketing count probably includes extensions or
  pseudo-targets like "720p"). Don't copy the number; generate ours from the registry.
- `docs/ARCHITECTURE.md:75-85`: the CLI composes the same pipeline as the app (`UserDataStore`, `EngineRegistry`,
  `ConverterCatalog`, `JobQueue`) with no UI, and `filee watch` runs one `FolderWatcher` by itself.
- `docs/ENGINES.md`: engine-by-engine coverage. The parts relevant to Linux v0.1.0:
  - External engines are looked up in this order (`ENGINES.md:176-188`): a copy the user picked → Filee's own
    download → a copy on the system (PATH, Homebrew, usual Unix folders). **On Linux, LibreOffice and Ghostscript come
    only from the system**, which is exactly MorphRing's v0.1.0 model.
  - FFmpeg codec table per target (`ENGINES.md` FFmpeg section): MP4/MOV/MKV… = H.264 (x264 medium, yuv420p) + AAC
    with `+faststart`; WEBM = VP9 + Opus; AVI = MPEG-4 XVID + MP3; GIF = two-pass palettegen/paletteuse;
    MP3 = LAME VBR V0/V2/V5 by quality; CRF 18/23/28 for High/Balanced/Small; remux with `-c copy` when the codecs fit.
    At most 2 FFmpeg jobs at once; killed after 10 minutes without output.
  - LibreOffice: **one profile folder per parallel worker** (`-env:UserInstallation`), because two conversions
    sharing a profile fail silently (`ENGINES.md` LibreOffice section). This matters for us right away.
- `docs/ADDING-A-CONVERTER.md` and `ADDING-A-LANGUAGE.md` are contributor guides (one converter class + edges + test;
  i18n json keys kept identical across languages, checked by a test).

### 10.3 CONTRIBUTING.md conventions worth copying

- Every file starts with a short comment saying what it is for; public items get doc comments; comments explain
  *why* for anything non-obvious. (Our version: `//!` module docs + `///`, and a bit of humour is welcome.)
- UI strings are never in code; every language file must have the same keys (enforced by a test).
- Tests are split into pure logic (fast), real-engine tests that **skip automatically when an engine is missing**,
  and UI tests. CI runs `dotnet format --verify-no-changes`; for us that's `cargo fmt --check` + `clippy -D warnings`.
- Release: bump the version, tag `vX.Y.Z`, CI builds artifacts with SHA-256 sums and puts the CHANGELOG section in the
  release notes.

---

## 11. CHANGELOG history (what shipped when)

The CHANGELOG starts at 1.3.0. Each section begins with a hidden `<!-- update level: required|recommended|optional
ko:/en:/zh: one-liners -->` block that the app's update notice reads (`CHANGELOG.md:9-16`). That's a neat trick for a
later MorphRing updater.

| Version | User-visible changes (condensed) |
|---|---|
| **1.3.0** | **CLI introduced**: `convert` (files, wildcards, folders, `--to`/preset), `watch`, `formats`, `presets`, `--json`, exit codes 0/1/2/3. Watch folders page in the app (Keep or move to `originals`). History on/off. Fix: donut stays topmost |
| 1.3.1 | Per-preset "Keep the original file dates" and **CLI `--keep-dates`**. HWP preset. More presets on the donuts |
| 1.4.0 | Self-installing updates. **Default save location** (next to original / subfolder / fixed folder), app-wide keep-dates switch; **the CLI follows both**. "Use my copy…" for engines; **CLI uses the same copies** |
| 1.5.0 | Engines already on the PC are found automatically (PATH, standard folders); order: user's copy → own download → system. Multi-page TIFF option. Donut drag-arrange gap |
| 1.5.1 / 1.5.2 | Donut animation stall fix; drag logging; Opus/Ogg album art via METADATA_BLOCK_PICTURE |
| 1.6.0 | Audio: 32/48/64 kbit/s, channels, WAV sample rate and bit depth. EPS/PS → PNG keeps transparency (Ghostscript `pngalpha`). PDF → DOCX tab stops and line breaks |
| 1.7.0 | **macOS and Linux previews** (X11 gestures; Wayland: drop zone + file manager actions). Search boxes. **Unix watch folders wait for paused copies** (the 5 s settle) |
| 1.7.1 / 1.7.2 | Donut highlight and animation robustness |
| 1.8.0 | Failed files list + Retry. Watch folders: include patterns, name rules (rename steps), preview, recent results |
| 1.8.1 | Sponsor links |
| 1.9.0 | Engine update notices. LibreOffice for slides → PDF (default on), HWP → PDF (opt-in). Fixes: update picks the latest release, PPTX picture backgrounds, rhwp 0.8.7 |
| 1.10.0 | PDF → one long image. Photos → video in full colour (PNG route, not GIF). **MOV ↔ MP4 remux without re-encoding**. Images → HEIC (FFmpeg x265 tiles + own HEIF writer) |
| 1.10.1 | Long image trim/split; resize to a width; oversized images are scaled to fit instead of failing |
| 1.10.2 | Single progress card; variable CFF2 fonts replaced to avoid blank PDF pages |
| 1.10.3 | Update notice levels. LibreOffice for Word/Excel → PDF (opt-in). Excel page breaks to PDF. Per-script fonts in PDFs. HWP boxes and equations |

Lesson for us: Filee's CLI has barely changed since 1.3.0. Its surface is stable and small, so it is a safe target to
clone exactly for v0.1.0.

---

## 12. The test suite as a behaviour spec

Framework: xUnit v3. Five projects. Tests that need a missing engine skip themselves (`Assert.Skip`). Tests that need
a case-sensitive volume skip on case-insensitive ones.

| Project | Tests (approx.) | What it pins down |
|---|---|---|
| `tests/Filee.Cli.Tests` | 13 (7 theory rows in one) | The CLI contract (§5). **Port all of them** |
| `tests/Filee.Core.Tests` | ~110 | Output naming, routes, job queue, watcher, profiles, settings migrations |
| `tests/Filee.Engines.Tests` | ~330 | Real conversions on generated inputs (Magick, FFmpeg, LibreOffice, own writers) |
| `tests/Filee.App.Tests` | ~125 | Donut geometry/highlight/progress, gestures, i18n, updater, headless rendering |
| `tests/Filee.Platform.Tests` | 13 | Linux desktop files/file manager actions, macOS services; skip on other OSes |

### 12.1 CLI tests (`tests/Filee.Cli.Tests/CliTests.cs`), one by one

Setup: a temp input folder and a temp settings folder per test. PNG inputs are 40×30 orange images made with Magick.
`Run(args)` calls `Cli.RunAsync` in-process and captures stdout, stderr and the exit code.

| # | Test | Asserts |
|---|---|---|
| 1 | `Converts_files_and_wildcards_into_an_output_folder` | `convert <dir>/*.png --to jpg -o <dir>/out --quality 70` → 0, empty stderr, stdout has `Converted 2 of 2`, `out/a.jpg` and `out/b.jpg` exist |
| 2 | `Keep_dates_copies_the_dates_of_the_originals` | source mtime 2018-05-04 12:00 UTC; `--keep-dates` → 0; `old.jpg` next to the source has that mtime |
| 3 | `Json_output_lists_every_file` | `--to .webp --json` → JSON parses, `converted` 1, `files[0].state` `done`, output ends with `a.webp` |
| 4 | `A_folder_converts_the_files_filee_knows` | folder with `a.png` + `notes.unknownformat`, `--preset to-jpg` → 0, `Converted 1 of 1` |
| 5 | `A_file_that_cannot_be_converted_fails_with_exit_code_1` | `notes.txt --to mp3` → 1, stdout contains `fail` |
| 6 | `Wrong_arguments_explain_themselves_with_exit_code_2` | exit 2 with stderr containing (case-insensitive): no target → `--to`; both → `--to`; `--to nosuchformat` → `unknown format`; `--preset nosuchpreset` → `no preset`; no input → `at least one file`; `--quality 300` → `--quality`; `frobnicate` → `Unknown command` |
| 7 | `A_missing_input_is_reported` | missing file → 2, stderr `not found` |
| 8 | `Watch_needs_an_existing_folder` | → 2, stderr `not found` (case-insensitive) |
| 9 | `Watch_converts_files_that_arrive_until_cancelled` | `watch <dir> --to jpg --settle 0.2`; file arrives → `<dir>/converted/arrived.jpg` within 10 s; cancel → 0 |
| 10 | `Formats_and_presets_are_listed` | `formats heic` → 0, contains `jpg`; `presets` → 0, contains `to-png` |
| 11 | `Help_and_version_need_no_engines` | `help convert` → 0, contains `--to`; `--version` → 0, starts with `filee ` |
| 12 | `Case_distinct_inputs_are_both_converted_and_exact_duplicates_are_removed` | `photo.png PHOTO.png photo.png` → 2 files, 2 distinct existing outputs, empty stderr |
| 13 | `Unicode_spaces_and_quotes_stay_in_a_single_input_argument` | Hangul, spaces, `'`, `"`, `%` in source and output folder names survive end to end |

### 12.2 Core tests that matter for the CLI

- `OutputPathResolverTests` (10): default `{name}`; `tar.gz` as one extension; empty extension → folder with
  ` (2)`; numbering by default; Overwrite returns the existing path and Skip returns null; **source never overwritten
  for all 3 policies**; token expansion and sanitizing; Subfolder `{name}` with suffix `_p1` → `report/report_p1.pdf`;
  custom folder; a case alias of the source (`PHOTO.jpg` vs `photo.jpg`) is never overwritten.
- `JobQueueTests` (14): every file converted, progress 1; keep-dates (mtime always, creation time except on Linux);
  multi-step intermediates stay out of the output folder (`doc_p1.png`, `doc_p2.png` only); per-file failure
  isolation (`CompletedWithErrors`, `error.unsupported_source`); `error.no_route`; engine exception →
  `error.conversion_failed`; `*` target re-encodes in the source format; merge order with existing PDFs merged
  as-is; multi-page TIFF order; combine packs unknown types without needing a route, uses the two-part extension
  (`a.tar.gz`) and reports failures on every file; same-name parallel files don't collide; background `Enqueue`.
- `RoutePlannerTests` (9): direct edge; 2-step route; cheaper wins; engine priority breaks ties; same format needs an
  explicit self edge; impossible → null; **max 3 hops** (a→d OK, a→e null); unavailable engines are excluded; a
  planner can assume optional engines are installed (used by `formats <fmt>`).
- `FolderWatcherTests` (10): default settle 2 s on Windows, 5 s elsewhere; converts once settled, not again for the
  same size and time; a growing file waits; a file held open waits (Windows-style lock); ignore list (crdownload, `~$`,
  dot files, unknown, `converted/`, `originals/`, `sub/` without `-r`); move-to-originals converts existing files and
  empties the folder; a missing folder is reported; case-distinct files with identical metadata are both converted;
  a `Converted` folder (capital C) is not mistaken for the output folder `converted`.
- `SmallParsersTests`: page ranges (`""`→all, `"2-4"`, `"4-"`, `"-2"`, `"5,1, 3"` sorted, `"4-2"` reversed OK,
  `"9-12"` out of range → all, `"x, 2"` → just 2); format detection (`a.JPEG`→jpg, `b.tif`→tiff, `.md`→md); unique
  extensions; built-in preset ids unique; profiles reference existing presets; preset targets are known formats or `*`.
- `DefaultOutputTests` (9): presets on Default follow the settings' save location; the factory default is the source
  folder; a preset with its own location ignores it; the app-wide keep-dates switch turns dates on for every preset.

### 12.3 Engine tests relevant to v0.1.0 (Linux, ffmpeg + magick + soffice)

- `MediaTests` (34): one FFmpeg step per media conversion; video → image goes through one still frame (a tenth into
  the video), not a GIF; photos → video through a full-colour PNG; every writable audio/video format is a target;
  audio never becomes video; quality, bitrate and size options reach the encoder arguments; remux with `-c copy` when
  the codecs fit (the iPhone HDR MOV test); GIF palette pass with size limits; odd sizes and surround audio encode;
  max height scales down only; portrait video is rotated upright; remove audio; tags and cover art kept; progress
  rises; **cancel kills ffmpeg and deletes the partial output**.
- `ImageAndPdfTests` (20): PNG → each format; animated GIF ↔ WebP; animated → still takes the first frame;
  multi-page TIFF → one file per page; strip metadata but keep the ICC profile; JPG flattens transparency onto the
  background and applies quality; resize by long edge, percent and width (never upscales); ICO holds several sizes;
  images ↔ PDF; PDF → multi-page TIFF; long image stack, trim and split; an over-tall JPG is scaled down instead of
  failing; merge images into a PDF; split a PDF by page range.
- `LibreOfficeRouteTests` / `LibreOfficeFormatTests`: which formats go through soffice, depending on the switches.
- `PortableEngineTests` (8): Unix engine lookup. FFmpeg needs **both `ffmpeg` and `ffprobe` in the same folder**;
  tar unpacking keeps the exec bit and blocks path escapes and symlink escapes.
- `OwnCopyTests`: a user-picked engine path wins over the downloaded one; versions are read from each program's
  output.

---

## 13. Third-party licenses we must respect (`THIRD-PARTY-NOTICES.md`)

Filee itself is **MIT** (`LICENSE`), so we may reimplement its design and port its logic as long as we keep its
copyright notice wherever we copy substantial code or text. Practical rules for MorphRing:

1. **Credit Filee.** Put an "Inspired by / ported from Filee (MIT, © KnifeLemon)" line in our README and a
   `THIRD-PARTY-NOTICES.md` entry. If we copy text verbatim (help strings, the format table), include Filee's MIT
   notice.
2. **External engines run as separate processes.** That is how Filee keeps GPL/AGPL tools at arm's length ("These
   programs run as separate processes"). We do the same: call `ffmpeg`, `magick`, `soffice`, `gs`, `pandoc`,
   `ebook-convert` as subprocesses and never link them.
   - FFmpeg: LGPL-2.1+, but most distro builds are GPL (`--enable-gpl`). As a subprocess this is fine. If we ever
     **bundle** a build, we must ship its license and a source offer.
   - Ghostscript: **AGPL-3.0**. Subprocess only, never bundled in v0.1.0.
   - LibreOffice: MPL-2.0. Pandoc: GPL-2.0+. calibre: GPL-3.0. H2Orestart: GPL-3.0. All subprocess-only.
   - ImageMagick: ImageMagick License (Apache-2.0-like). Fine as a subprocess, and also fine to link (for example the
     `magick_rust` crate), as long as the notice is kept.
   - 7-Zip: LGPL-2.1+ with the **unRAR restriction**: RAR may only be unpacked, never used to build a RAR-compatible
     archiver. Filee "only reads RAR files"; we keep `rar` read-only too.
   - rhwp: MIT (HWP rendering, a later release).
3. **Libraries Filee uses and their licenses.** These guide our crate choices (prefer MIT/Apache/BSD/zlib):
   Avalonia, CommunityToolkit, PDFsharp, PDFtoImage, SkiaSharp, Svg.Skia, HarfBuzzSharp, SharpCompress, Unhwp,
   ExcelNumberFormat, ExcelDataReader, MimeKitLite, ACadSharp, AngleSharp, pypandoc-hwpx, Microsoft.Extensions are MIT;
   Markdig BSD-2; PdfPig Apache-2.0; PDFium BSD-3/Apache-2.0; Magick.NET Apache-2.0; SharpHook MIT (libuiohook
   GPL-3.0 with linking exception / LGPL-3.0); cu2qu (fontTools) Apache-2.0. Fonts: Noto (OFL-1.1); icons: Material
   Design Icons (Apache-2.0).
4. **Code that is adapted, not just linked:** Filee credits adapted code (Svg.Skia text shaping, pypandoc-hwpx AST
   mapping and the `blank.hwpx` template, cu2qu's algorithm). If we port those parts later, we carry those notices
   too.
5. Bundled assets: if we ship Noto fonts or Material icons in the Tauri app, include OFL-1.1 / Apache-2.0 texts.

---

## 14. Clever bits and fragile bits

Clever:

- `RunAsync(args, out, err, token, hostFactory)` makes the whole CLI testable in-process, fast, with no subprocess.
- `help`/`version` short-circuit before any engine probing (probing can be slow).
- Ids and extensions both work as `--to` values, and a leading dot is fine.
- One settings store shared by CLI and GUI: presets made in the GUI are usable from scripts.
- Wildcard expansion inside the program for shells that don't glob.
- `--json` gives stable `files[]` input order even though the human output streams in completion order.
- The source can never be overwritten (case-insensitive check even on case-sensitive file systems).

Fragile or wrong (fix in MorphRing):

- `-q` means `--quality`, while most CLIs use `-q` for quiet. Keep it for parity, but document it loudly.
- `--settle` help says 2 s; the real default on Unix is 5 s.
- Options that don't apply are silently accepted (`--json` on watch, `--move-originals` on convert).
- Human output order is non-deterministic; folder expansion order is file-system order.
- The merge/combine "ok" line prints an empty output list for the non-first files.
- A first Ctrl+C during convert exits with 1 (indistinguishable from "a file failed"); a second Ctrl+C leaves temp
  folders and partial files behind.
- Watch never reports failures in its exit code and has no JSON mode, so it's hard to supervise from scripts.
- Extension-only detection (`.ts`, `.rb`, `.img`, `.key`, `.pdb`) misfires on code and disk images during folder
  expansion.

---

## 15. Porting to Rust

### 15.1 Shape

```text
crates/core   (lib)  formats registry, preset model, output path resolver, route planner, job runner, watcher
crates/cli    (bin "morphring")
    src/main.rs        → tiny: install Ctrl+C handler, std::process::exit(cli::run(args, stdout, stderr, cancel, host))
    src/cli.rs         → run(): parse, dispatch, printing, JSON
    src/options.rs     → parser (clap derive or hand-rolled, see below)
    src/help.rs        → texts
    src/host.rs        → load config dir, build engine catalog, job runner
```

Write `pub fn run(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write, cancel: CancelToken,
host: impl FnOnce() -> Result<Host>) -> i32` so integration tests can call it in-process with `Vec<u8>` buffers and a
`tempfile::TempDir` config folder, exactly like `CliTests.Run`.

### 15.2 Parser: clap or hand-rolled?

- **clap 4 (derive)** is the idiomatic choice. Its usage-error exit code is already **2**. Mapping:
  - `#[arg(short='t', long="to")] to: Option<String>`, `preset` likewise, in an `ArgGroup` with
    `required = true, multiple = false` for convert/watch. Note that clap's message differs from Filee's; tests
    only check substrings, so make sure the error mentions `--to`.
  - `#[arg(short='q', long, value_parser = clap::value_parser!(u8).range(1..=100))] quality`.
  - `--settle`: custom `value_parser` (`f64`, `>= 0`, finite).
  - `--overwrite` / `--skip`: `ArgGroup` with `multiple = false`, or `overrides_with` for last-wins like Filee.
  - `-v/--version`: disable clap's `-V`, add `-v` and a `version` subcommand that prints `morphring <ver>`
    (`env!("CARGO_PKG_VERSION")`).
  - `help <cmd>`: clap's built-in `help` subcommand handles it.
  - Wrap clap errors to print `morphring: <msg>` + `Run 'morphring help' for usage.` to stderr, and return 2:
    `Cli::try_parse_from(args)` and map `ErrorKind::DisplayHelp | DisplayVersion` to exit 0 on stdout.
  - Case-insensitive commands/flags (`CONVERT`, `--TO`) are not supported by clap. Drop that quirk; nobody relies
    on it.
- **Hand-rolled** (~120 lines, a direct port of §3.2) gives byte-exact messages. Choose this only if exact message
  parity matters. Recommendation: **clap** for v0.1.0, keeping the documented messages for the *semantic* errors
  (unknown format, read-only, no preset, not found, no files), which are ours anyway.

### 15.3 Crates

| Need | Crate | Notes |
|---|---|---|
| CLI parsing | `clap` 4 (derive) | exit code 2 on usage errors by default |
| JSON | `serde` + `serde_json` | `#[derive(Serialize)]` structs keep field order; `to_writer_pretty` gives 2-space indent like Filee; `#[serde(skip_serializing_if = "Option::is_none")] error` |
| Absolute paths | `std::path::absolute` (stable since 1.79) | like `GetFullPath`: no symlink resolution. Avoid `canonicalize` (fails on missing paths, resolves links) |
| Relative display | `pathdiff::diff_paths` | for `Shorten` (§5.2.1) |
| Wildcards | `glob` (or `globset` + `walkdir`) | only needed when a shell didn't expand; keep "last segment only" semantics; `-r` → `**/pattern` |
| Recursion | `walkdir` | sort entries (`sort_by_file_name`) for deterministic order |
| Dedupe | `indexmap::IndexSet<PathBuf>` | keeps first occurrence and order; on Linux compare exactly (case-sensitive) |
| Ctrl+C | `ctrlc` (with `termination` feature) | first hit: set an `AtomicBool`/cancel token; second hit: `std::process::exit(130)` |
| Parallel jobs | `std::thread::scope` + a work queue, or `rayon` | `available_parallelism()`; one `Semaphore` per engine (ffmpeg 2) |
| Subprocesses | `std::process::Command` | **kill on cancel**: poll `try_wait` with the cancel flag; delete partial output |
| Dates | `filetime::set_file_mtime` | creation time can't be set on Linux, so skip it (Filee's test does too) |
| Config dir | `directories::ProjectDirs` or `dirs::config_dir()` | `~/.config/morphring` |
| Watch | `notify` 6+ | events only *nominate* files; keep Filee's own 1 s tick + settle logic (don't trust debouncers to know when a copy finished) |
| Temp work dir | `tempfile::TempDir` | auto-removed even on panic; Filee used `<tmp>/Filee/<jobid>` |
| Exit code | `std::process::ExitCode::from(u8)` | 0/1/2/3 |
| Tests | `assert_cmd` + `predicates`, or in-process `run()` | plus `tempfile`; skip tests when `ffmpeg`/`magick`/`soffice` are missing (`which` crate) |

### 15.4 Pitfalls

- **Non-UTF-8 paths.** .NET strings are always Unicode; Linux paths are bytes. For JSON, use
  `to_string_lossy()` (document it), and never `unwrap()` on `to_str()`. Keep `OsString`/`PathBuf` everywhere
  else, so a Hangul/space/quote filename survives (port test 13).
- **`-q` is quality.** Don't let clap's habits turn it into quiet.
- **`--json` beats `--quiet`;** `--quiet` still prints errors to stderr.
- **Missing inputs abort before converting anything** (exit 2), even if other inputs exist.
- **Wildcard with no match = missing (2); folder with no known files = nothing to do (3).**
- **Folder inputs filter by known extension; explicit files and wildcard matches do not** (they fail later as
  `unsupported`, exit 1).
- **The output reservation set must be shared across threads** (Mutex<HashSet<PathBuf>>) and checked together with
  `exists()`, or two parallel `same.*` files will race for `same.webp`.
- **Never overwrite the source**: compare candidate and source case-insensitively (Filee does this even on Linux).
- **Cancelled counts as failed** in both the exit code and JSON `failed`.
- **Second Ctrl+C**: exit immediately, but try to kill child processes first (spawn children in their own process
  group, and `killpg` on the way out) to avoid orphaned `ffmpeg`/`soffice`.
- **LibreOffice**: use a separate `-env:UserInstallation=file:///tmp/.../profile-N` per parallel worker, or
  conversions silently fail. A desktop soffice that is already running also grabs the default profile.
- **Settle default 5 s on Linux;** `--settle 0` must be allowed (Filee accepts `>= 0`).
- **ExpandInputs `-o` with `-r`** flattens subfolders in Filee. Decide deliberately; parity says flatten, and the
  ` (2)` numbering keeps it safe.
- **Watch must ignore its own output and `originals` folders**, including when `-o` points inside the watched folder,
  or outputs loop back in forever.

### 15.5 Suggested Rust data shapes

```rust
pub enum FileState { Pending, Running, Done, Skipped, Failed, Cancelled }   // serde rename_all = "lowercase"

#[derive(Serialize)]
pub struct JsonReport<'a> {
    preset: &'a str, target: &'a str,
    converted: usize, skipped: usize, failed: usize,
    files: Vec<JsonFile<'a>>,
}
#[derive(Serialize)]
pub struct JsonFile<'a> {
    source: String,              // absolute, lossy UTF-8
    state: FileState,
    outputs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,       // "message (detail)"
}

pub mod exit { pub const SUCCESS: i32 = 0; pub const SOME_FAILED: i32 = 1;
               pub const USAGE: i32 = 2; pub const NOTHING_TO_DO: i32 = 3; }
```

---

## 16. v0.1.0 candidates (ship today)

Realistic for one day with ffmpeg, magick and soffice already installed:

1. **`morphring convert <inputs...> --to <format>`** with `-o/--output`, `-r/--recursive`, `-q/--quality`,
   `--overwrite`/`--skip`, `--keep-dates` (mtime only), `--json`, `--quiet`, and exit codes 0/1/2/3 exactly as in §4.
2. **Input expansion** (§7): files, folders (known extensions only), in-program wildcards, dedupe, sorted order.
3. **Output path resolver** (§8.1): `{name}` pattern, ` (2)` numbering, overwrite/skip, never touch the source, plus a
   shared reservation set for parallel files. Port the 10 `OutputPathResolverTests` as unit tests.
4. **Format registry** (§9): the full 170-entry table as static data (it's just data; cheap to include in full),
   with a `supported_now` check driven by which engines are on PATH.
5. **Three subprocess engines:** ffmpeg (video/audio ↔, video → gif), magick (raster ↔ raster, images → PDF), and
   soffice (`--headless --convert-to`, docx/odt/xlsx/pptx/… → pdf and between office formats, with a per-worker
   profile). Route planner with the 3-hop limit is optional on day one; direct edges cover most cases.
   PDF → image through magick needs Ghostscript, which is **not installed**: leave it out or report `no_route`.
6. **`morphring formats [<format>]`**: list output in Filee's layout. `formats <fmt>` can say "now" vs "with optional
   engines" by planning with all engines assumed present.
7. **`morphring help [<cmd>]` / `version`**, with help texts adapted from §10.1 (examples with Linux paths, the
   correct 5 s settle default, the `-q` note).
8. **Per-file lines + summary** in the exact `ok/skip/stop/fail` format, and the `--json` document from §5.2.2.
9. **Two-stage Ctrl+C**: cooperative cancel (kill children, delete partial output, exit 1), then a hard exit.
10. **Port the CLI integration tests** that fit v0.1.0 scope: 1, 2 (mtime), 3, 4 (with `--to jpg` instead of a
    preset), 5, 6 (minus `--preset` rows if presets are deferred), 7, 10 (formats only), 11, 12, 13. Skip any test
    whose engine is missing.

Stretch, or v0.2.0:

- `--preset` with the 68 built-in presets as static data (read-only, no `presets.json` yet) and `morphring presets`.
  Small and valuable: `half-size`, `jpg-70`, `mp4-720` and `mp3-128` just set fields on the same preset struct.
- `morphring watch` (notify + 1 s tick + 5 s settle + ignore list + `--move-originals`), and test 9.
- User presets in `~/.config/morphring/presets.json` shared with the Tauri app; default save location setting.
- Merge PDF / one ZIP / multi-page TIFF modes; name patterns with tokens and rename steps.
- Ghostscript, Pandoc and calibre engines; content sniffing to avoid `.ts`/`.rb` misdetection.
