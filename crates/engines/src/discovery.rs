//! Finding the external programs the engines need, reading their versions and
//! telling the user how to install the ones that are missing.
//!
//! Ported from Filee's `src/Filee.Engines/Infrastructure/EngineEnvironment.cs`
//! (system lookup only: no bundled copies and no downloads) and the version
//! probe in `src/Filee.Engines/Media/FfmpegConverter.cs`.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{LazyLock, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use morphring_core::EngineStatus;
use regex::Regex;

/// An external program package that an engine depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    /// ImageMagick 7 (`magick`).
    Magick,
    /// FFmpeg (`ffmpeg` together with `ffprobe`).
    Ffmpeg,
    /// LibreOffice (`soffice`).
    Soffice,
    /// Poppler's command line tools: `pdftoppm`, with `pdftotext` and
    /// `pdfinfo` in the same folder.
    Pdftoppm,
    /// Ghostscript (`gs`; `gswin64c` or `gswin32c` on Windows).
    Ghostscript,
    /// Pandoc (`pandoc`).
    Pandoc,
}

// Ghostscript on Windows ships as gswin64c (console, 64-bit) or gswin32c, and a
// plain `gs` only shows up when someone brought their own MSYS habits along.
const GS_WINDOWS: &[&[&str]] = &[&["gswin64c"], &["gswin32c"], &["gs"]];

// LibreOffice on Windows installs soffice.exe (the GUI launcher, which detaches
// and takes the exit code with it) and soffice.com (the console wrapper that
// actually tells us how it went). We want the chatty one.
const SOFFICE_WINDOWS: &[&[&str]] = &[&["soffice.com"], &["soffice"]];

/// An operating system family, for the parts of discovery that differ per OS:
/// program names, the fixed folders searched after `PATH`, and install hints.
///
/// Every function that takes an `Os` is pure with respect to it, so the
/// Windows and macOS rules can be checked on any machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Os {
    /// Linux and the other Unix-likes that are not macOS.
    Linux,
    /// Windows.
    Windows,
    /// macOS.
    MacOs,
}

impl Os {
    /// The OS this build targets.
    pub const fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }
}

impl Tool {
    /// Every tool, in a stable order.
    pub const ALL: [Tool; 6] = [
        Tool::Magick,
        Tool::Ffmpeg,
        Tool::Soffice,
        Tool::Pdftoppm,
        Tool::Ghostscript,
        Tool::Pandoc,
    ];

    /// The engine id that uses this tool, such as `"magick"` or `"poppler"`.
    pub const fn id(self) -> &'static str {
        match self {
            Tool::Magick => "magick",
            Tool::Ffmpeg => "ffmpeg",
            Tool::Soffice => "soffice",
            Tool::Pdftoppm => "poppler",
            Tool::Ghostscript => "ghostscript",
            Tool::Pandoc => "pandoc",
        }
    }

    /// The product name shown to users, such as `"ImageMagick"` or `"Poppler"`.
    pub const fn display_name(self) -> &'static str {
        match self {
            Tool::Magick => "ImageMagick",
            Tool::Ffmpeg => "FFmpeg",
            Tool::Soffice => "LibreOffice",
            Tool::Pdftoppm => "Poppler",
            Tool::Ghostscript => "Ghostscript",
            Tool::Pandoc => "Pandoc",
        }
    }

    /// The programs that must all sit in one folder, without the platform's
    /// executable suffix. The first one is the program that is run.
    ///
    /// This is the preferred entry of [`program_sets`](Self::program_sets).
    pub const fn programs(self) -> &'static [&'static str] {
        self.program_sets()[0]
    }

    /// Every set of programs that can stand in for this tool, best first.
    /// Each set follows the [`programs`](Self::programs) rule; most tools
    /// have exactly one.
    pub const fn program_sets(self) -> &'static [&'static [&'static str]] {
        self.program_sets_on(Os::current())
    }

    /// [`program_sets`](Self::program_sets) for `os` instead of the machine we
    /// run on.
    pub const fn program_sets_on(self, os: Os) -> &'static [&'static [&'static str]] {
        match (self, os) {
            // Only ever magick. Windows keeps a `convert.exe` in System32 that
            // converts FAT volumes to NTFS, and "I asked for a PNG and got a
            // reformatted drive" is not a bug report anyone wants to read.
            (Tool::Magick, _) => &[&["magick"]],
            (Tool::Ffmpeg, _) => &[&["ffmpeg", "ffprobe"]],
            (Tool::Soffice, Os::Windows) => SOFFICE_WINDOWS,
            (Tool::Soffice, _) => &[&["soffice"]],
            (Tool::Pdftoppm, _) => &[&["pdftoppm", "pdftotext", "pdfinfo"]],
            (Tool::Ghostscript, Os::Windows) => GS_WINDOWS,
            (Tool::Ghostscript, _) => &[&["gs"]],
            (Tool::Pandoc, _) => &[&["pandoc"]],
        }
    }

    /// Arguments that make the program print its version and exit.
    const fn version_args(self) -> &'static [&'static str] {
        match self {
            Tool::Magick => &["-version"],
            Tool::Ffmpeg => &["-hide_banner", "-version"],
            Tool::Soffice | Tool::Ghostscript | Tool::Pandoc => &["--version"],
            Tool::Pdftoppm => &["-v"],
        }
    }

    /// Which stream the version command prints to.
    const fn version_stream(self) -> Stream {
        match self {
            // pdftoppm -v writes its banner to stderr, because of course it does:
            // it is the only thing it can print without a PDF to complain about.
            Tool::Pdftoppm => Stream::Stderr,
            _ => Stream::Stdout,
        }
    }
}

/// One of a child's output streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stream {
    Stdout,
    Stderr,
}

/// The product name shown to users, such as `"FFmpeg"`.
pub const fn display_name(tool: Tool) -> &'static str {
    tool.display_name()
}

/// A tool that was found on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// Full path of the main program, for example `/usr/bin/ffmpeg`.
    pub program: PathBuf,
    /// The version it reports, or `"unknown"` when it could not be read.
    pub version: String,
}

impl Found {
    /// The path of another program from the same folder, for example
    /// `companion("ffprobe")` next to `ffmpeg`.
    pub fn companion(&self, name: &str) -> PathBuf {
        self.program
            .with_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX))
    }
}

/// Version string used when a program was found but would not tell us its version.
pub const UNKNOWN_VERSION: &str = "unknown";

/// How long a version probe may run before it is killed.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

// One entry per tool, filled on first use. Engines call status() for every
// planner run, and nobody wants to wait for soffice --version (a whole second of
// an office suite clearing its throat) more than once per process.
static CACHE: OnceLock<Mutex<HashMap<Tool, Option<Found>>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<Tool, Option<Found>>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Finds `tool` on this machine and reads its version. The result, found or
/// not, is cached for the life of the process (see [`forget`]).
pub fn find(tool: Tool) -> Option<Found> {
    if let Some(hit) = cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&tool)
    {
        return hit.clone();
    }
    // The lookup runs without the lock held, so a slow soffice probe never makes
    // the magick lookup wait. Two threads racing here both probe once and agree.
    let found = discover(tool);
    cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(tool, found.clone());
    found
}

/// Path of the main program of `tool`, or `None` when it is not installed.
/// For [`Tool::Ffmpeg`] the folder is guaranteed to hold `ffprobe` too.
pub fn locate(tool: Tool) -> Option<PathBuf> {
    find(tool).map(|found| found.program)
}

/// The engine status for `tool`: where it is and its version, or how to install it.
pub fn status(tool: Tool) -> EngineStatus {
    match find(tool) {
        Some(Found { program, version }) => EngineStatus::Available {
            path: program,
            version,
        },
        None => EngineStatus::Missing {
            hint: install_hint_here(tool),
        },
    }
}

/// The status of an engine built into MorphRing itself: always available, with
/// no program path and MorphRing's own version.
pub fn builtin_status() -> EngineStatus {
    EngineStatus::Available {
        path: PathBuf::new(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

/// Clears the cache, so the next lookup searches the machine again.
pub fn forget() {
    cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

fn discover(tool: Tool) -> Option<Found> {
    let path_var = std::env::var_os("PATH");
    let program = locate_in(
        tool,
        Os::current(),
        path_var.as_deref(),
        &system_folders(tool),
    )?;
    let version = probe_version(tool, &program).unwrap_or_else(|| UNKNOWN_VERSION.to_owned());
    tracing::debug!(tool = tool.id(), program = %program.display(), %version, "found tool");
    Some(Found { program, version })
}

/// Searches the folders of `path_var` (a `PATH`-style list), then `extra`, and
/// returns the main program from the first folder that holds **all** of the
/// tool's programs on `os` (see [`Tool::program_sets_on`]). Tools with several
/// sets try each set in turn, best first. Nothing is cached and `PATH` is not
/// read from the environment.
///
/// The file checks themselves (the executable bit, `PATHEXT`) follow the
/// machine we run on, so on Linux a Windows lookup finds files literally named
/// `gswin64c` or `soffice.com`.
///
/// The result never carries a `\\?\` verbatim prefix, which ImageMagick,
/// soffice and older FFmpeg builds refuse in an argv.
pub fn locate_in(
    tool: Tool,
    os: Os,
    path_var: Option<&OsStr>,
    extra: &[PathBuf],
) -> Option<PathBuf> {
    tool.program_sets_on(os)
        .iter()
        .find_map(|programs| locate_set(programs, path_var, extra))
        .map(|program| dunce::simplified(&program).to_path_buf())
}

/// [`locate_in`] for one set of programs.
fn locate_set(programs: &[&str], path_var: Option<&OsStr>, extra: &[PathBuf]) -> Option<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));

    // which handles PATHEXT and the executable bit for us. We only use it to
    // collect candidate folders; the "everything in one folder" rule comes after,
    // because an ffmpeg without its ffprobe is like a car without a dashboard:
    // it runs, but you have no idea what it is doing.
    let mut folders: Vec<PathBuf> = Vec::new();
    if let Some(hits) = path_var.and_then(|p| which::which_in_all(programs[0], Some(p), &cwd).ok())
    {
        folders.extend(hits.filter_map(|hit| hit.parent().map(Path::to_path_buf)));
    }
    folders.extend(extra.iter().cloned());

    let mut seen = Vec::new();
    for folder in folders {
        if seen.contains(&folder) {
            continue;
        }
        if let Some(program) = programs_in(programs, &folder) {
            return Some(program);
        }
        seen.push(folder);
    }
    None
}

/// The main program inside `folder` when every program is there and executable.
fn programs_in(programs: &[&str], folder: &Path) -> Option<PathBuf> {
    let mut main = None;
    for name in programs {
        let hit = which::which_in(name, Some(folder), folder).ok()?;
        main.get_or_insert(hit);
    }
    main
}

/// The fixed folders searched after `PATH` on this machine, in Filee's order.
pub fn system_folders(tool: Tool) -> Vec<PathBuf> {
    system_folders_in(tool, Os::current(), |name| std::env::var_os(name))
}

/// The fixed folders searched after `PATH` for `tool` on `os`, best first.
///
/// `env` answers environment lookups (`ProgramFiles`, `HOME`, ...), so tests can
/// hand in a pretend Windows box. Folders with a version in their name, such as
/// `gs\gs10.08.0\bin` or `ImageMagick-7.1.2-Q16-HDRI`, are listed from disk
/// with the newest version first.
pub fn system_folders_in<F>(tool: Tool, os: Os, env: F) -> Vec<PathBuf>
where
    F: Fn(&str) -> Option<OsString>,
{
    match os {
        Os::Linux | Os::MacOs => unix_folders(tool, os, &env),
        Os::Windows => windows_folders(tool, &env),
    }
}

fn unix_folders(tool: Tool, os: Os, env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    // A Finder-launched app gets PATH=/usr/bin:/bin:/usr/sbin:/sbin and not a
    // byte more, so Homebrew's folders have to be on this list or a Mac user
    // with every tool installed would be told to install every tool.
    let mut folders: Vec<PathBuf> = [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/opt/local/bin",
    ]
    .map(PathBuf::from)
    .into();
    if tool != Tool::Soffice {
        return folders;
    }
    if os == Os::MacOs {
        folders.push(PathBuf::from(
            "/Applications/LibreOffice.app/Contents/MacOS",
        ));
        if let Some(home) = env("HOME").filter(|h| !h.is_empty()) {
            folders.push(PathBuf::from(home).join("Applications/LibreOffice.app/Contents/MacOS"));
        }
        return folders;
    }
    folders.push(PathBuf::from("/usr/lib/libreoffice/program"));
    folders.push(PathBuf::from("/usr/lib64/libreoffice/program"));
    // The tarballs from libreoffice.org unpack to /opt/libreoffice26.2 and
    // friends. Newest first, so the one you installed last wins, which is
    // usually the one you meant.
    folders.extend(
        versioned_children(Path::new("/opt"), "libreoffice")
            .into_iter()
            .map(|dir| dir.join("program")),
    );
    folders
}

fn windows_folders(tool: Tool, env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let var = |name: &str| env(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    // Both Program Files, 64-bit first. On a 32-bit Windows (do those still
    // exist?) the two variables point at the same place, which dedup handles.
    let mut program_files: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(var)
        .collect();
    program_files.dedup();

    let mut folders = Vec::new();
    for root in &program_files {
        match tool {
            Tool::Soffice => folders.push(root.join("LibreOffice").join("program")),
            Tool::Ghostscript => folders.extend(
                versioned_children(&root.join("gs"), "gs")
                    .into_iter()
                    .map(|dir| dir.join("bin")),
            ),
            Tool::Magick => folders.extend(versioned_children(root, "ImageMagick-")),
            Tool::Pandoc => folders.push(root.join("Pandoc")),
            Tool::Ffmpeg | Tool::Pdftoppm => {}
        }
    }
    // The pandoc MSI installs per user by default, into LOCALAPPDATA.
    if tool == Tool::Pandoc
        && let Some(local) = var("LOCALAPPDATA")
    {
        folders.push(local.join("Pandoc"));
    }
    // The package managers' shim folders. They are on PATH for a fresh shell,
    // but an app started before the install still has yesterday's PATH.
    if let Some(local) = var("LOCALAPPDATA") {
        folders.push(local.join("Microsoft").join("WinGet").join("Links"));
    }
    if let Some(profile) = var("USERPROFILE") {
        folders.push(profile.join("scoop").join("shims"));
    }
    if let Some(data) = var("ProgramData") {
        folders.push(data.join("chocolatey").join("bin"));
    }
    folders
}

/// The subfolders of `root` whose name starts with `prefix` (ASCII case
/// ignored), newest version first.
fn versioned_children(root: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut found: Vec<(Vec<u64>, PathBuf)> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let head = name.get(..prefix.len())?;
            head.eq_ignore_ascii_case(prefix)
                .then(|| (version_key(&name[prefix.len()..]), entry.path()))
        })
        .collect();
    // Numbers compare as numbers: plain string order would crown gs9.56 over
    // gs10.08, and Ghostscript 9 has had quite enough time on the throne.
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    found.into_iter().map(|(_, path)| path).collect()
}

/// The digit runs in `text` as numbers, for example `[7, 1, 2, 33, 16]` from
/// `7.1.2-33-Q16-HDRI`.
fn version_key(text: &str) -> Vec<u64> {
    text.split(|c: char| !c.is_ascii_digit())
        .filter(|run| !run.is_empty())
        .map(|run| run.parse().unwrap_or(u64::MAX))
        .collect()
}

/// Compiles a pattern written in this file.
// The patterns are literals that the tests below exercise, so a typo blows up in
// cargo test long before it could reach a user.
#[allow(clippy::expect_used, reason = "literal patterns covered by unit tests")]
fn static_regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static regex is valid")
}

static FFMPEG_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    // Filee's pattern: "ffmpeg version 9.0.2-full_build-www.gyan.dev" gives
    // 9.0.2, Arch's "n9.0.2" loses its n, and a git build keeps its whole
    // "N-12345-gabc" because there is no number to dig out of it.
    static_regex(r"version\s+n?(\d+(?:\.\d+)+|\S+)")
});
static MAGICK_VERSION: LazyLock<Regex> =
    LazyLock::new(|| static_regex(r"ImageMagick\s+(\d+(?:\.\d+)+(?:-\d+)?)"));
static SOFFICE_VERSION: LazyLock<Regex> =
    LazyLock::new(|| static_regex(r"LibreOffice(?:Dev)?\s+(\d+(?:\.\d+)+)"));
static PDFTOPPM_VERSION: LazyLock<Regex> =
    LazyLock::new(|| static_regex(r"pdftoppm version (\d+(?:\.\d+)+)"));
// gs --version prints the bare number and nothing else, which is refreshingly
// un-Ghostscript-like for a program whose manual is longer than most novels.
static GS_VERSION: LazyLock<Regex> = LazyLock::new(|| static_regex(r"(?m)^\s*(\d+(?:\.\d+)+)\s*$"));
// Major and minor are what a minimum-version check needs; the rest of the
// number (3.1.11.1, pandoc likes its dots) rides along in group 3 for doctor.
static PANDOC_VERSION: LazyLock<Regex> =
    LazyLock::new(|| static_regex(r"(?m)^pandoc(?:\.exe)? (\d+)\.(\d+)((?:\.\d+)*)"));

/// Reads the version from what the tool's version command printed (stderr for
/// `pdftoppm`, stdout for the rest), for example `9.0.2` from
/// `ffmpeg version n9.0.2 Copyright ...`.
pub fn parse_version(tool: Tool, stdout: &str) -> Option<String> {
    let regex = match tool {
        Tool::Ffmpeg => &*FFMPEG_VERSION,
        Tool::Magick => &*MAGICK_VERSION,
        Tool::Soffice => &*SOFFICE_VERSION,
        Tool::Pdftoppm => &*PDFTOPPM_VERSION,
        Tool::Ghostscript => &*GS_VERSION,
        Tool::Pandoc => {
            let caps = PANDOC_VERSION.captures(stdout)?;
            let rest = caps.get(3).map_or("", |m| m.as_str());
            return Some(format!("{}.{}{rest}", &caps[1], &caps[2]));
        }
    };
    regex
        .captures(stdout)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_owned())
}

fn probe_version(tool: Tool, program: &Path) -> Option<String> {
    let text = run_probe(
        program,
        tool.version_args(),
        tool.version_stream(),
        PROBE_TIMEOUT,
    )?;
    parse_version(tool, &text)
}

/// Runs `program args` and returns what it printed on `stream` when it exits
/// successfully within `timeout`.
// TODO(T09): switch to process::run once the process runner lands; it was being
// written in the same wave, so this is a deliberately tiny stand-in. A version
// probe prints a few lines and exits, so one pipe reader and no group kill.
fn run_probe(program: &Path, args: &[&str], stream: Stream, timeout: Duration) -> Option<String> {
    tracing::debug!(program = %program.display(), ?args, "probing version");
    let (stdout, stderr) = match stream {
        Stream::Stdout => (Stdio::piped(), Stdio::null()),
        Stream::Stderr => (Stdio::null(), Stdio::piped()),
    };
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .ok()?;

    // Read stdout on its own thread and hand it over through a channel. If the
    // program leaves a grandchild holding the pipe open, recv_timeout lets us
    // walk away instead of joining a thread that never finishes.
    let mut pipe: Box<dyn Read + Send> = match stream {
        Stream::Stdout => Box::new(child.stdout.take()?),
        Stream::Stderr => Box::new(child.stderr.take()?),
    };
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });

    let deadline = Instant::now() + timeout;
    let exit = loop {
        match child.try_wait() {
            Ok(Some(exit)) => break exit,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                tracing::debug!(program = %program.display(), "version probe timed out");
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !exit.success() {
        return None;
    }
    let left = deadline.saturating_duration_since(Instant::now());
    let buf = rx.recv_timeout(left.max(Duration::from_millis(100))).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Linux distribution families we know the package commands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Arch,
    Debian,
}

/// Picks the family from the `ID` and `ID_LIKE` fields of an os-release file.
fn family(os_release: &str) -> Option<Family> {
    let mut id = Vec::new();
    let mut like = Vec::new();
    for line in os_release.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
        match key.trim() {
            "ID" => id.push(value.to_ascii_lowercase()),
            "ID_LIKE" => like.extend(value.split_whitespace().map(str::to_ascii_lowercase)),
            _ => {}
        }
    }
    // ID beats ID_LIKE: Manjaro says ID=manjaro, ID_LIKE=arch, and Ubuntu says
    // ID=ubuntu, ID_LIKE=debian. Either way we land on the right package manager.
    id.iter()
        .chain(like.iter())
        .find_map(|name| match name.as_str() {
            "arch" | "archarm" => Some(Family::Arch),
            "debian" | "ubuntu" => Some(Family::Debian),
            _ => None,
        })
}

/// How to install `tool` on the Linux distribution described by `os_release`
/// (the contents of `/etc/os-release`), for example `pacman -S ffmpeg`.
/// Unknown systems get the project's download page.
pub fn install_hint(tool: Tool, os_release: &str) -> String {
    match family(os_release) {
        Some(Family::Arch) => {
            let package = match tool {
                Tool::Magick => "imagemagick",
                Tool::Ffmpeg => "ffmpeg",
                Tool::Soffice => "libreoffice-fresh",
                Tool::Pdftoppm => "poppler",
                Tool::Ghostscript => "ghostscript",
                // Arch split pandoc in two; the program lives in pandoc-cli and
                // plain "pandoc" is not a package any more.
                Tool::Pandoc => "pandoc-cli",
            };
            format!("pacman -S {package}")
        }
        Some(Family::Debian) => {
            let package = match tool {
                Tool::Magick => "imagemagick",
                Tool::Ffmpeg => "ffmpeg",
                Tool::Soffice => "libreoffice",
                Tool::Pdftoppm => "poppler-utils",
                Tool::Ghostscript => "ghostscript",
                Tool::Pandoc => "pandoc",
            };
            format!("sudo apt install {package}")
        }
        None => download_hint(tool),
    }
}

/// The macOS install command (Homebrew) for `tool`.
pub fn brew_hint(tool: Tool) -> String {
    match tool {
        Tool::Magick => "brew install imagemagick".to_owned(),
        Tool::Ffmpeg => "brew install ffmpeg".to_owned(),
        Tool::Soffice => "brew install --cask libreoffice".to_owned(),
        Tool::Pdftoppm => "brew install poppler".to_owned(),
        Tool::Ghostscript => "brew install ghostscript".to_owned(),
        Tool::Pandoc => "brew install pandoc".to_owned(),
    }
}

fn download_hint(tool: Tool) -> String {
    let url = match tool {
        Tool::Magick => "https://imagemagick.org/script/download.php",
        Tool::Ffmpeg => "https://ffmpeg.org/download.html",
        Tool::Soffice => "https://www.libreoffice.org/download/",
        Tool::Pdftoppm => "https://poppler.freedesktop.org/",
        Tool::Ghostscript => "https://ghostscript.com/releases/",
        Tool::Pandoc => "https://pandoc.org/installing.html",
    };
    format!("download it from {url}")
}

/// The Windows install command for `tool`: winget where the package exists,
/// Scoop where it does not (Poppler and Ghostscript have no winget package).
pub fn winget_hint(tool: Tool) -> String {
    let id = match tool {
        Tool::Magick => "ImageMagick.ImageMagick",
        Tool::Ffmpeg => "Gyan.FFmpeg",
        Tool::Soffice => "TheDocumentFoundation.LibreOffice",
        Tool::Pandoc => "JohnMacFarlane.Pandoc",
        // Artifex publishes mutool on winget and not Ghostscript, and a winget
        // id that does not exist is worse than no hint at all.
        Tool::Pdftoppm => return "scoop install poppler".to_owned(),
        Tool::Ghostscript => return "scoop install ghostscript".to_owned(),
    };
    format!("winget install -e --id {id}")
}

/// How to install `tool` on `os`. `os_release` (the contents of
/// `/etc/os-release`) picks the package manager on Linux and is ignored
/// elsewhere.
pub fn install_hint_for(tool: Tool, os: Os, os_release: &str) -> String {
    match os {
        Os::Windows => winget_hint(tool),
        Os::MacOs => brew_hint(tool),
        Os::Linux => install_hint(tool, os_release),
    }
}

/// [`install_hint_for`] the machine we are running on: winget (or Scoop) on
/// Windows, Homebrew on macOS, the distribution's package manager on Linux
/// (from `/etc/os-release`), and the download page anywhere else.
pub fn install_hint_here(tool: Tool) -> String {
    let os = Os::current();
    if os != Os::Linux {
        return install_hint_for(tool, os, "");
    }
    // /usr/lib/os-release is the fallback the os-release spec itself blesses,
    // for distros that keep /etc minimal.
    let os_release = std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
        .unwrap_or_default();
    install_hint(tool, &os_release)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[cfg(unix)]
    fn join_path<I, P>(folders: I) -> Option<OsString>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<OsStr>,
    {
        std::env::join_paths(folders).ok()
    }

    #[test]
    fn parse_version_reads_real_strings_from_the_dev_machine() {
        assert_eq!(
            parse_version(
                Tool::Ffmpeg,
                "ffmpeg version n9.0.2 Copyright (c) 2000-2026 the FFmpeg developers\nbuilt with gcc 16 (GCC)\n"
            )
            .as_deref(),
            Some("9.0.2")
        );
        assert_eq!(
            parse_version(
                Tool::Magick,
                "Version: ImageMagick 7.1.2-32 Q16-HDRI x86_64 68f8d115a:20260926 https://imagemagick.org\n"
            )
            .as_deref(),
            Some("7.1.2-32")
        );
        assert_eq!(
            parse_version(Tool::Soffice, "LibreOffice 26.2.6.3 620(Build:3)\n\n").as_deref(),
            Some("26.2.6.3")
        );
        assert_eq!(
            parse_version(
                Tool::Ffmpeg,
                "ffmpeg version N-12345-gabcdef Copyright (c) 2000-2026"
            )
            .as_deref(),
            Some("N-12345-gabcdef")
        );
    }

    #[test]
    fn parse_version_drops_build_suffixes_and_rejects_noise() {
        assert_eq!(
            parse_version(
                Tool::Ffmpeg,
                "ffmpeg version 9.0.2-full_build-www.gyan.dev Copyright"
            )
            .as_deref(),
            Some("9.0.2")
        );
        assert_eq!(parse_version(Tool::Magick, "command not found"), None);
        assert_eq!(parse_version(Tool::Soffice, ""), None);
    }

    #[test]
    fn install_hint_follows_the_distribution() {
        assert_eq!(install_hint(Tool::Ffmpeg, "ID=arch"), "pacman -S ffmpeg");
        assert_eq!(
            install_hint(Tool::Ffmpeg, "ID=ubuntu\nID_LIKE=debian"),
            "sudo apt install ffmpeg"
        );
        assert_eq!(
            install_hint(Tool::Soffice, "ID=arch"),
            "pacman -S libreoffice-fresh"
        );
        assert_eq!(
            install_hint(Tool::Magick, "ID=arch"),
            "pacman -S imagemagick"
        );
        assert_eq!(
            install_hint(Tool::Soffice, "ID=debian"),
            "sudo apt install libreoffice"
        );
    }

    #[test]
    fn install_hint_reads_quoted_and_derived_ids() {
        assert_eq!(
            install_hint(
                Tool::Ffmpeg,
                "NAME=\"Manjaro Linux\"\nID=manjaro\nID_LIKE=arch\n"
            ),
            "pacman -S ffmpeg"
        );
        assert_eq!(
            install_hint(Tool::Magick, "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n"),
            "sudo apt install imagemagick"
        );
    }

    #[test]
    fn install_hint_falls_back_to_the_download_page() {
        assert_eq!(
            install_hint(Tool::Ffmpeg, "ID=fedora"),
            "download it from https://ffmpeg.org/download.html"
        );
        assert!(install_hint(Tool::Soffice, "").contains("libreoffice.org"));
        assert_eq!(brew_hint(Tool::Soffice), "brew install --cask libreoffice");
    }

    #[test]
    fn parse_version_reads_the_v02_tools() {
        let pdftoppm = "pdftoppm version 26.08.0\nCopyright 2005-2026 The Poppler Developers - http://poppler.freedesktop.org\n";
        assert_eq!(
            parse_version(Tool::Pdftoppm, pdftoppm).as_deref(),
            Some("26.08.0")
        );
        assert_eq!(
            parse_version(Tool::Ghostscript, "10.08.0\n").as_deref(),
            Some("10.08.0")
        );
        assert_eq!(
            parse_version(Tool::Pandoc, "pandoc 3.11\nFeatures: +server +lua\n").as_deref(),
            Some("3.11")
        );
        assert_eq!(
            parse_version(Tool::Pandoc, "pandoc.exe 3.1.11.1\r\n").as_deref(),
            Some("3.1.11.1")
        );
        assert_eq!(parse_version(Tool::Pandoc, "pandoc-citeproc 0.17"), None);
        assert_eq!(parse_version(Tool::Ghostscript, "GPL Ghostscript"), None);
        assert_eq!(parse_version(Tool::Pdftoppm, ""), None);
    }

    #[test]
    fn install_hints_cover_the_v02_tools() {
        assert_eq!(install_hint(Tool::Pdftoppm, "ID=arch"), "pacman -S poppler");
        assert_eq!(
            install_hint(Tool::Pdftoppm, "ID=debian"),
            "sudo apt install poppler-utils"
        );
        assert_eq!(
            install_hint(Tool::Ghostscript, "ID=arch"),
            "pacman -S ghostscript"
        );
        assert_eq!(
            install_hint(Tool::Ghostscript, "ID=ubuntu"),
            "sudo apt install ghostscript"
        );
        assert_eq!(
            install_hint(Tool::Pandoc, "ID=arch"),
            "pacman -S pandoc-cli"
        );
        assert_eq!(
            install_hint(Tool::Pandoc, "ID=debian"),
            "sudo apt install pandoc"
        );
        assert_eq!(brew_hint(Tool::Pdftoppm), "brew install poppler");
        assert_eq!(brew_hint(Tool::Ghostscript), "brew install ghostscript");
        assert_eq!(brew_hint(Tool::Pandoc), "brew install pandoc");
        assert!(install_hint(Tool::Pandoc, "ID=fedora").contains("pandoc.org"));
    }

    #[test]
    fn v02_tools_have_ids_and_programs() {
        assert_eq!(Tool::Pdftoppm.id(), "poppler");
        assert_eq!(Tool::Ghostscript.id(), "ghostscript");
        assert_eq!(Tool::Pandoc.id(), "pandoc");
        assert_eq!(
            Tool::Pdftoppm.programs(),
            ["pdftoppm", "pdftotext", "pdfinfo"]
        );
        assert_eq!(Tool::Pandoc.programs(), ["pandoc"]);
        if cfg!(windows) {
            assert_eq!(Tool::Ghostscript.programs(), ["gswin64c"]);
            assert_eq!(Tool::Ghostscript.program_sets().len(), 3);
        } else {
            assert_eq!(Tool::Ghostscript.program_sets(), [["gs"]]);
        }
        for tool in Tool::ALL {
            assert!(!tool.program_sets().is_empty(), "{tool:?}");
        }
    }

    #[test]
    fn display_names_match_the_products() {
        assert_eq!(display_name(Tool::Ffmpeg), "FFmpeg");
        assert_eq!(display_name(Tool::Magick), "ImageMagick");
        assert_eq!(display_name(Tool::Soffice), "LibreOffice");
    }

    #[test]
    fn companion_sits_next_to_the_program() {
        let found = Found {
            program: PathBuf::from("/opt/ff/bin/ffmpeg"),
            version: UNKNOWN_VERSION.to_owned(),
        };
        assert_eq!(
            found.companion("ffprobe"),
            PathBuf::from(format!(
                "/opt/ff/bin/ffprobe{}",
                std::env::consts::EXE_SUFFIX
            ))
        );
    }

    #[test]
    fn soffice_searches_the_libreoffice_program_folder() {
        let folders = system_folders(Tool::Soffice);
        if cfg!(target_os = "linux") {
            assert!(folders.contains(&PathBuf::from("/usr/lib/libreoffice/program")));
            assert!(folders.contains(&PathBuf::from("/usr/bin")));
        }
        assert!(
            !system_folders(Tool::Ffmpeg)
                .iter()
                .any(|f| f.ends_with("program"))
        );
    }

    #[test]
    fn locate_finds_ffmpeg_with_ffprobe_beside_it() {
        if which::which("ffmpeg").is_err() {
            eprintln!("SKIP: ffmpeg not found");
            return;
        }
        let program = locate(Tool::Ffmpeg).expect("ffmpeg is on PATH, so locate finds it");
        let folder = program.parent().unwrap();
        assert!(
            which::which_in("ffprobe", Some(folder), folder).is_ok(),
            "ffprobe missing next to {}",
            program.display()
        );
        match status(Tool::Ffmpeg) {
            EngineStatus::Available { path, version } => {
                assert_eq!(path, program);
                assert_ne!(version, UNKNOWN_VERSION);
            }
            EngineStatus::Missing { hint } => panic!("ffmpeg reported missing: {hint}"),
        }
    }

    #[test]
    fn installed_v02_tools_report_a_real_version() {
        for tool in [Tool::Pdftoppm, Tool::Ghostscript, Tool::Pandoc] {
            if which::which(tool.programs()[0]).is_err() {
                eprintln!("SKIP: {} not found", tool.programs()[0]);
                continue;
            }
            match status(tool) {
                EngineStatus::Available { version, .. } => {
                    assert_ne!(version, UNKNOWN_VERSION, "{tool:?}");
                }
                // pdftoppm alone is not enough (pdfinfo and pdftotext must sit
                // beside it), so Missing is a fair answer for a half install.
                EngineStatus::Missing { hint } => eprintln!("{tool:?} missing: {hint}"),
            }
        }
    }

    #[test]
    fn windows_hints_use_winget_and_scoop() {
        let hint = |tool| install_hint_for(tool, Os::Windows, "ID=arch");
        assert_eq!(
            hint(Tool::Magick),
            "winget install -e --id ImageMagick.ImageMagick"
        );
        assert_eq!(hint(Tool::Ffmpeg), "winget install -e --id Gyan.FFmpeg");
        assert_eq!(
            hint(Tool::Soffice),
            "winget install -e --id TheDocumentFoundation.LibreOffice"
        );
        assert_eq!(
            hint(Tool::Pandoc),
            "winget install -e --id JohnMacFarlane.Pandoc"
        );
        assert_eq!(hint(Tool::Pdftoppm), "scoop install poppler");
        assert_eq!(hint(Tool::Ghostscript), "scoop install ghostscript");
    }

    #[test]
    fn macos_hints_use_brew_for_everything() {
        for tool in Tool::ALL {
            let hint = install_hint_for(tool, Os::MacOs, "ID=debian");
            assert!(hint.starts_with("brew install "), "{tool:?}: {hint}");
            assert_eq!(hint, brew_hint(tool));
        }
        assert_eq!(
            install_hint_for(Tool::Soffice, Os::MacOs, ""),
            "brew install --cask libreoffice"
        );
        assert_eq!(
            install_hint_for(Tool::Ffmpeg, Os::Linux, "ID=arch"),
            "pacman -S ffmpeg"
        );
    }

    #[test]
    fn magick_is_never_convert_on_any_os() {
        for os in [Os::Linux, Os::Windows, Os::MacOs] {
            assert_eq!(Tool::Magick.program_sets_on(os), [["magick"]], "{os:?}");
            for tool in Tool::ALL {
                assert!(
                    tool.program_sets_on(os)
                        .iter()
                        .flat_map(|set| set.iter())
                        .all(|name| !name.starts_with("convert")),
                    "{tool:?} on {os:?}"
                );
            }
        }
    }

    #[test]
    fn windows_program_names_differ_where_they_should() {
        assert_eq!(
            Tool::Soffice.program_sets_on(Os::Windows),
            [["soffice.com"], ["soffice"]]
        );
        assert_eq!(Tool::Soffice.program_sets_on(Os::MacOs), [["soffice"]]);
        assert_eq!(
            Tool::Ghostscript.program_sets_on(Os::Windows)[0],
            ["gswin64c"]
        );
        assert_eq!(Tool::Ghostscript.program_sets_on(Os::Linux), [["gs"]]);
        assert_eq!(
            Tool::ALL.map(|t| t.program_sets()),
            Tool::ALL.map(|t| t.program_sets_on(Os::current()))
        );
    }

    fn fake_env(vars: &[(&str, &Path)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let vars: Vec<(String, OsString)> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.as_os_str().to_owned()))
            .collect();
        move |name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    }

    #[test]
    fn windows_folders_cover_program_files_newest_first() {
        let pf = tempfile::TempDir::new().unwrap();
        let x86 = tempfile::TempDir::new().unwrap();
        for dir in [
            "gs/gs9.56.1/bin",
            "gs/gs10.08.0/bin",
            "ImageMagick-7.0.10-Q16",
            "ImageMagick-7.1.2-Q16-HDRI",
            "LibreOffice/program",
        ] {
            std::fs::create_dir_all(pf.path().join(dir)).unwrap();
        }
        let local = Path::new("C:\\Users\\u\\AppData\\Local");
        let env = fake_env(&[
            ("ProgramFiles", pf.path()),
            ("ProgramFiles(x86)", x86.path()),
            ("LOCALAPPDATA", local),
        ]);

        let soffice = system_folders_in(Tool::Soffice, Os::Windows, &env);
        assert_eq!(soffice[0], pf.path().join("LibreOffice").join("program"));
        assert_eq!(soffice[1], x86.path().join("LibreOffice").join("program"));

        let gs = system_folders_in(Tool::Ghostscript, Os::Windows, &env);
        assert_eq!(
            gs[..2],
            [
                pf.path().join("gs").join("gs10.08.0").join("bin"),
                pf.path().join("gs").join("gs9.56.1").join("bin"),
            ]
        );

        let magick = system_folders_in(Tool::Magick, Os::Windows, &env);
        assert_eq!(
            magick[..2],
            [
                pf.path().join("ImageMagick-7.1.2-Q16-HDRI"),
                pf.path().join("ImageMagick-7.0.10-Q16"),
            ]
        );
        assert!(magick.contains(&local.join("Microsoft").join("WinGet").join("Links")));

        // No Unix folders on Windows, and no LibreOffice folder for FFmpeg.
        let ffmpeg = system_folders_in(Tool::Ffmpeg, Os::Windows, &env);
        assert!(!ffmpeg.contains(&PathBuf::from("/usr/bin")));
        assert!(!ffmpeg.iter().any(|f| f.ends_with("program")));
        // An empty environment just means fewer places to look, not a panic.
        assert!(system_folders_in(Tool::Soffice, Os::Windows, |_| None).is_empty());
    }

    #[test]
    fn macos_folders_include_homebrew_and_the_libreoffice_bundle() {
        let env = fake_env(&[("HOME", Path::new("/Users/u"))]);
        for tool in Tool::ALL {
            let folders = system_folders_in(tool, Os::MacOs, &env);
            assert_eq!(folders[0], PathBuf::from("/opt/homebrew/bin"), "{tool:?}");
            assert!(
                folders.contains(&PathBuf::from("/usr/local/bin")),
                "{tool:?}"
            );
        }
        let soffice = system_folders_in(Tool::Soffice, Os::MacOs, &env);
        assert!(soffice.contains(&PathBuf::from(
            "/Applications/LibreOffice.app/Contents/MacOS"
        )));
        assert!(soffice.contains(&PathBuf::from(
            "/Users/u/Applications/LibreOffice.app/Contents/MacOS"
        )));
        assert!(!soffice.iter().any(|f| f.starts_with("/usr/lib")));
        let linux = system_folders_in(Tool::Soffice, Os::Linux, &env);
        assert!(linux.contains(&PathBuf::from("/usr/lib/libreoffice/program")));
        assert!(!linux.iter().any(|f| f.starts_with("/Applications")));
    }

    #[test]
    fn version_key_orders_numbers_as_numbers() {
        assert_eq!(version_key("7.1.2-33-Q16-HDRI"), [7, 1, 2, 33, 16]);
        assert!(version_key("10.08.0") > version_key("9.56.1"));
        assert!(version_key("26.2") > version_key("7.6"));
        assert_eq!(version_key("-dev"), Vec::<u64>::new());
    }

    /// CI installs the engines on Windows and macOS; this proves discovery
    /// finds them there (and prints a doctor-style line for the log).
    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn installed_engines_are_found_on_this_os() {
        let mut wanted = vec![Tool::Magick, Tool::Ffmpeg];
        if cfg!(target_os = "macos") {
            wanted.extend([Tool::Pdftoppm, Tool::Ghostscript, Tool::Pandoc]);
        }
        let on_ci = std::env::var_os("CI").is_some();
        for tool in wanted {
            match status(tool) {
                EngineStatus::Available { path, version } => {
                    eprintln!("{}: {} ({version})", tool.id(), path.display());
                    assert!(
                        !path.to_string_lossy().starts_with(r"\\?\"),
                        "{}",
                        path.display()
                    );
                    if tool == Tool::Magick {
                        assert_eq!(path.file_stem().and_then(OsStr::to_str), Some("magick"));
                    }
                }
                EngineStatus::Missing { hint } if on_ci => {
                    panic!("{} not found on CI; hint: {hint}", tool.id())
                }
                EngineStatus::Missing { hint } => {
                    eprintln!("SKIP: {} not installed ({hint})", tool.id())
                }
            }
        }
    }

    #[cfg(unix)]
    mod fake_tools {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        #[test]
        fn locate_in_needs_every_program_in_one_folder() {
            let half = tempfile::TempDir::new().unwrap();
            let full = tempfile::TempDir::new().unwrap();
            script(half.path(), "ffmpeg", "exit 0");
            let ffmpeg = script(full.path(), "ffmpeg", "exit 0");
            script(full.path(), "ffprobe", "exit 0");

            let path = join_path([half.path(), full.path()]);
            assert_eq!(
                locate_in(Tool::Ffmpeg, Os::Linux, path.as_deref(), &[]),
                Some(ffmpeg)
            );
        }

        #[test]
        fn locate_in_falls_back_to_extra_folders() {
            let empty = tempfile::TempDir::new().unwrap();
            let fixed = tempfile::TempDir::new().unwrap();
            let soffice = script(fixed.path(), "soffice", "exit 0");

            let path = join_path([empty.path()]);
            assert_eq!(
                locate_in(
                    Tool::Soffice,
                    Os::Linux,
                    path.as_deref(),
                    &[fixed.path().to_path_buf()]
                ),
                Some(soffice)
            );
            assert_eq!(locate_in(Tool::Soffice, Os::Linux, None, &[]), None);
        }

        #[test]
        fn locate_in_wants_pdftotext_and_pdfinfo_beside_pdftoppm() {
            let half = tempfile::TempDir::new().unwrap();
            let full = tempfile::TempDir::new().unwrap();
            script(half.path(), "pdftoppm", "exit 0");
            script(half.path(), "pdftotext", "exit 0");
            let pdftoppm = script(full.path(), "pdftoppm", "exit 0");
            script(full.path(), "pdftotext", "exit 0");
            script(full.path(), "pdfinfo", "exit 0");

            let path = join_path([half.path(), full.path()]);
            assert_eq!(
                locate_in(Tool::Pdftoppm, Os::Linux, path.as_deref(), &[]),
                Some(pdftoppm.clone())
            );
            let found = Found {
                program: pdftoppm,
                version: UNKNOWN_VERSION.to_owned(),
            };
            assert_eq!(found.companion("pdfinfo"), full.path().join("pdfinfo"));
        }

        #[test]
        fn probe_reads_pdftoppm_from_stderr() {
            let dir = tempfile::TempDir::new().unwrap();
            let fake = script(
                dir.path(),
                "pdftoppm",
                "echo 'pdftoppm version 24.02.0' >&2; echo 'not on stdout'",
            );
            assert_eq!(
                probe_version(Tool::Pdftoppm, &fake).as_deref(),
                Some("24.02.0")
            );
        }

        #[test]
        fn magick_lookup_ignores_a_convert_on_path() {
            // Stand-in for C:\\Windows\\System32, home of the FAT-to-NTFS convert.exe.
            let system32 = tempfile::TempDir::new().unwrap();
            script(system32.path(), "convert", "exit 0");
            script(system32.path(), "convert.exe", "exit 0");
            let path = join_path([system32.path()]);
            for os in [Os::Windows, Os::Linux, Os::MacOs] {
                assert_eq!(
                    locate_in(
                        Tool::Magick,
                        os,
                        path.as_deref(),
                        &[system32.path().to_path_buf()]
                    ),
                    None,
                    "{os:?}"
                );
            }
            let magick = script(system32.path(), "magick", "exit 0");
            assert_eq!(
                locate_in(Tool::Magick, Os::Windows, path.as_deref(), &[]),
                Some(magick)
            );
        }

        #[test]
        fn windows_lookup_prefers_soffice_com_and_gswin64c() {
            let program = tempfile::TempDir::new().unwrap();
            script(program.path(), "soffice", "exit 0");
            let com = script(program.path(), "soffice.com", "exit 0");
            let extra = [program.path().to_path_buf()];
            assert_eq!(
                locate_in(Tool::Soffice, Os::Windows, None, &extra),
                Some(com)
            );

            let gui_only = tempfile::TempDir::new().unwrap();
            let exe = script(gui_only.path(), "soffice", "exit 0");
            assert_eq!(
                locate_in(
                    Tool::Soffice,
                    Os::Windows,
                    None,
                    &[gui_only.path().to_path_buf()]
                ),
                Some(exe)
            );

            // A gs on PATH loses to a gswin64c in Program Files on Windows,
            // and wins everywhere else.
            let msys = tempfile::TempDir::new().unwrap();
            let gs = script(msys.path(), "gs", "exit 0");
            let gs_bin = tempfile::TempDir::new().unwrap();
            let gswin = script(gs_bin.path(), "gswin64c", "exit 0");
            let path = join_path([msys.path()]);
            let extra = [gs_bin.path().to_path_buf()];
            assert_eq!(
                locate_in(Tool::Ghostscript, Os::Windows, path.as_deref(), &extra),
                Some(gswin)
            );
            assert_eq!(
                locate_in(Tool::Ghostscript, Os::Linux, path.as_deref(), &extra),
                Some(gs)
            );
        }

        #[test]
        fn locate_in_ignores_files_that_are_not_executable() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("magick"), "not a program").unwrap();
            let path = join_path([dir.path()]);
            assert_eq!(
                locate_in(Tool::Magick, Os::Linux, path.as_deref(), &[]),
                None
            );
        }

        #[test]
        fn probe_reads_the_version_a_program_prints() {
            let dir = tempfile::TempDir::new().unwrap();
            let fake = script(
                dir.path(),
                "magick",
                "echo 'Version: ImageMagick 7.0.1-2 Q16'",
            );
            assert_eq!(
                probe_version(Tool::Magick, &fake).as_deref(),
                Some("7.0.1-2")
            );
        }

        #[test]
        fn probe_gives_up_on_a_failing_or_hanging_program() {
            let dir = tempfile::TempDir::new().unwrap();
            let failing = script(dir.path(), "failing", "echo 'LibreOffice 1.2'; exit 3");
            assert_eq!(
                run_probe(&failing, &[], Stream::Stdout, Duration::from_secs(5)),
                None
            );

            let hanging = script(dir.path(), "hanging", "exec sleep 30");
            let started = Instant::now();
            assert_eq!(
                run_probe(&hanging, &[], Stream::Stdout, Duration::from_millis(200)),
                None
            );
            assert!(started.elapsed() < Duration::from_secs(3));
        }
    }
}
