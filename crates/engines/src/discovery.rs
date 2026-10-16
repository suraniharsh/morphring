//! Finding the external programs the engines need, reading their versions and
//! telling the user how to install the ones that are missing.
//!
//! Ported from Filee's `src/Filee.Engines/Infrastructure/EngineEnvironment.cs`
//! (system lookup only: no bundled copies and no downloads) and the version
//! probe in `src/Filee.Engines/Media/FfmpegConverter.cs`.

use std::collections::HashMap;
use std::ffi::OsStr;
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
        match self {
            Tool::Magick => &[&["magick"]],
            Tool::Ffmpeg => &[&["ffmpeg", "ffprobe"]],
            Tool::Soffice => &[&["soffice"]],
            Tool::Pdftoppm => &[&["pdftoppm", "pdftotext", "pdfinfo"]],
            Tool::Ghostscript if cfg!(windows) => GS_WINDOWS,
            Tool::Ghostscript => &[&["gs"]],
            Tool::Pandoc => &[&["pandoc"]],
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
    let program = locate_in(tool, path_var.as_deref(), &system_folders(tool))?;
    let version = probe_version(tool, &program).unwrap_or_else(|| UNKNOWN_VERSION.to_owned());
    tracing::debug!(tool = tool.id(), program = %program.display(), %version, "found tool");
    Some(Found { program, version })
}

/// Searches the folders of `path_var` (a `PATH`-style list), then `extra`, and
/// returns the main program from the first folder that holds **all** of the
/// tool's programs. Tools with several [`Tool::program_sets`] try each set in
/// turn, best first. Nothing is cached and `PATH` is not read from the environment.
pub fn locate_in(tool: Tool, path_var: Option<&OsStr>, extra: &[PathBuf]) -> Option<PathBuf> {
    tool.program_sets()
        .iter()
        .find_map(|programs| locate_set(programs, path_var, extra))
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

/// The fixed folders searched after `PATH`, in Filee's order.
pub fn system_folders(tool: Tool) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = Vec::new();
    if cfg!(unix) {
        folders.extend(
            [
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/usr/bin",
                "/bin",
                "/opt/local/bin",
            ]
            .map(PathBuf::from),
        );
    }
    if tool == Tool::Soffice {
        folders.extend(libreoffice_folders());
    }
    folders
}

fn libreoffice_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if cfg!(target_os = "macos") {
        folders.push(PathBuf::from(
            "/Applications/LibreOffice.app/Contents/MacOS",
        ));
        if let Some(home) = directories::BaseDirs::new() {
            folders.push(
                home.home_dir()
                    .join("Applications/LibreOffice.app/Contents/MacOS"),
            );
        }
    }
    if cfg!(unix) {
        folders.push(PathBuf::from("/usr/lib/libreoffice/program"));
        folders.push(PathBuf::from("/usr/lib64/libreoffice/program"));
        // The tarballs from libreoffice.org unpack to /opt/libreoffice26.2 and
        // friends. Newest first, so the one you installed last wins, which is
        // usually the one you meant.
        let mut opt: Vec<PathBuf> = std::fs::read_dir("/opt")
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("libreoffice")
            })
            .map(|entry| entry.path().join("program"))
            .collect();
        opt.sort_by(|a, b| b.cmp(a));
        folders.extend(opt);
    }
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(root) = std::env::var_os(var) {
                folders.push(PathBuf::from(root).join("LibreOffice").join("program"));
            }
        }
    }
    folders
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

/// [`install_hint`] for the machine we are running on: Homebrew on macOS,
/// the distribution's package manager on Linux (from `/etc/os-release`), and
/// the download page anywhere else.
pub fn install_hint_here(tool: Tool) -> String {
    if cfg!(target_os = "macos") {
        return brew_hint(tool);
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
            assert_eq!(locate_in(Tool::Ffmpeg, path.as_deref(), &[]), Some(ffmpeg));
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
                    path.as_deref(),
                    &[fixed.path().to_path_buf()]
                ),
                Some(soffice)
            );
            assert_eq!(locate_in(Tool::Soffice, None, &[]), None);
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
                locate_in(Tool::Pdftoppm, path.as_deref(), &[]),
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
        fn locate_in_ignores_files_that_are_not_executable() {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("magick"), "not a program").unwrap();
            let path = join_path([dir.path()]);
            assert_eq!(locate_in(Tool::Magick, path.as_deref(), &[]), None);
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
