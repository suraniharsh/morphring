//! File manager context menu entries for Linux.
//!
//! Writes one small file per file manager, each starting `<app> --convert %F`:
//!
//! | File manager | File (under `$HOME/.local/share`) | Mode |
//! |---|---|---|
//! | Dolphin (KDE) | `kio/servicemenus/morphring.desktop` | 755 |
//! | Nautilus (GNOME Files) | `nautilus/scripts/MorphRing` | 755 |
//! | Nemo (Cinnamon) | `nemo/actions/morphring.nemo_action` | 644 |
//! | Thunar (XFCE) | `Thunar/sendto/morphring.desktop` | 644 |
//!
//! Every file carries the [`MANAGED_MARKER`] line. [`install`] only replaces
//! files that carry it and [`uninstall`] only removes files that carry it, so
//! a file the user wrote by hand is never touched.
//!
//! Ported from Filee's `src/Filee.Platform.Linux/LinuxDesktopIntegration.cs`
//! (`SetContextMenu`, `DesktopArgument`, `ShellArgument`, `DesktopValue`).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The line that marks a file as written by MorphRing.
pub const MANAGED_MARKER: &str = "X-MorphRing-Managed=true";

/// The menu label shown by every file manager.
pub const LABEL: &str = "Convert with MorphRing";

// Freedesktop icon names are the closest thing Linux has to a universal
// language. "document-save-as" exists in every theme I have ever opened.
const ICON: &str = "document-save-as";

/// A file manager that gets a context menu entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileManager {
    Dolphin,
    Nautilus,
    Nemo,
    Thunar,
}

impl FileManager {
    /// Every supported file manager, Dolphin first.
    pub const ALL: [FileManager; 4] = [
        FileManager::Dolphin,
        FileManager::Nautilus,
        FileManager::Nemo,
        FileManager::Thunar,
    ];

    /// The display name of the file manager.
    pub fn name(self) -> &'static str {
        match self {
            FileManager::Dolphin => "Dolphin",
            FileManager::Nautilus => "Nautilus",
            FileManager::Nemo => "Nemo",
            FileManager::Thunar => "Thunar",
        }
    }

    /// Where the entry lives, relative to `$HOME/.local/share`.
    fn relative_path(self) -> &'static str {
        match self {
            FileManager::Dolphin => "kio/servicemenus/morphring.desktop",
            // Nautilus shows the file name as the menu item, so it gets a
            // proper name instead of a slug.
            FileManager::Nautilus => "nautilus/scripts/MorphRing",
            FileManager::Nemo => "nemo/actions/morphring.nemo_action",
            FileManager::Thunar => "Thunar/sendto/morphring.desktop",
        }
    }

    /// Whether the file must be executable (mode 755).
    fn executable(self) -> bool {
        // Plasma 6 refuses to load a local service menu that is not
        // executable, and a Nautilus script is, well, a script.
        matches!(self, FileManager::Dolphin | FileManager::Nautilus)
    }

    /// The file contents for this file manager, starting `exe`.
    pub fn contents(self, exe: &Path) -> Result<String, IntegrationError> {
        match self {
            FileManager::Dolphin => dolphin_service_menu(exe),
            FileManager::Nautilus => nautilus_script(exe),
            FileManager::Nemo => nemo_action(exe),
            FileManager::Thunar => thunar_sendto(exe),
        }
    }
}

/// One context menu file, written or removed (or planned, in a dry run).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuFile {
    pub manager: FileManager,
    pub path: PathBuf,
    pub contents: String,
    /// True for mode 755, false for mode 644.
    pub executable: bool,
}

/// Errors from building or writing context menu files.
#[derive(Debug, thiserror::Error)]
pub enum IntegrationError {
    #[error("invalid executable path {path:?}: {reason}")]
    InvalidExecutable { path: PathBuf, reason: &'static str },
    #[error(
        "{path} exists and was not written by MorphRing; move it away and run the command again"
    )]
    Unmanaged { path: PathBuf },
    #[error("could not write {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> IntegrationError + '_ {
    move |source| IntegrationError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// The Dolphin (KDE) service menu.
pub fn dolphin_service_menu(exe: &Path) -> Result<String, IntegrationError> {
    let command = desktop_command(exe)?;
    let name = desktop_value(LABEL);
    // Filee used application/octet-stream, which works because most types
    // inherit from it. all/allfiles is the catch-all KDE actually documents,
    // and it leaves folders out, which is what we want.
    Ok(format!(
        "[Desktop Entry]\n\
         Type=Service\n\
         MimeType=all/allfiles;\n\
         Actions=convert;\n\
         X-KDE-Protocols=file\n\
         {MANAGED_MARKER}\n\
         \n\
         [Desktop Action convert]\n\
         Name={name}\n\
         Icon={ICON}\n\
         Exec={command}\n"
    ))
}

/// The Nautilus (GNOME Files) script.
///
/// Nautilus passes the selected files as arguments, relative to the current
/// folder, so the script makes them absolute before handing them on.
pub fn nautilus_script(exe: &Path) -> Result<String, IntegrationError> {
    let exe = shell_argument(exe_str(exe)?);
    // `set --` + `shift` rewrites "$@" in place: each pass appends the fixed
    // path at the end and drops the original from the front. It reads like a
    // magic trick, and it is the only array POSIX sh gives us.
    Ok(format!(
        "#!/bin/sh\n\
         # {MANAGED_MARKER}\n\
         [ \"$#\" -gt 0 ] || exit 0\n\
         for file do\n\
         \x20 case \"$file\" in\n\
         \x20   /*) set -- \"$@\" \"$file\" ;;\n\
         \x20   *) set -- \"$@\" \"$PWD/$file\" ;;\n\
         \x20 esac\n\
         \x20 shift\n\
         done\n\
         exec {exe} --convert \"$@\"\n"
    ))
}

/// The Nemo (Cinnamon) action.
pub fn nemo_action(exe: &Path) -> Result<String, IntegrationError> {
    let command = desktop_command(exe)?;
    // Nemo expands % codes in Name too, so a literal % has to be doubled.
    let name = desktop_value(LABEL).replace('%', "%%");
    Ok(format!(
        "[Nemo Action]\n\
         Name={name}\n\
         Exec={command}\n\
         Selection=notnone\n\
         Extensions=nodirs;\n\
         UriScheme=file\n\
         Icon-Name={ICON}\n\
         {MANAGED_MARKER}\n"
    ))
}

/// The Thunar (XFCE) "Send To" entry.
pub fn thunar_sendto(exe: &Path) -> Result<String, IntegrationError> {
    let command = desktop_command(exe)?;
    let name = desktop_value(LABEL);
    // Filee also merged a custom action into Thunar's uca.xml. A sendto file
    // gets us into the menu without parsing someone else's XML, so that waits.
    Ok(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={name}\n\
         Exec={command}\n\
         Icon={ICON}\n\
         Terminal=false\n\
         {MANAGED_MARKER}\n"
    ))
}

/// Every menu file for `exe`, placed under `home`.
pub fn menu_files(exe: &Path, home: &Path) -> Result<Vec<MenuFile>, IntegrationError> {
    let data = data_home(home);
    FileManager::ALL
        .iter()
        .map(|&manager| {
            Ok(MenuFile {
                manager,
                path: data.join(manager.relative_path()),
                contents: manager.contents(exe)?,
                executable: manager.executable(),
            })
        })
        .collect()
}

/// Writes every context menu file under `home` and returns them.
///
/// Existing files are replaced only when they carry [`MANAGED_MARKER`]. With
/// `dry_run`, nothing is created or changed and the files that would be
/// written are returned.
pub fn install(exe: &Path, home: &Path, dry_run: bool) -> Result<Vec<MenuFile>, IntegrationError> {
    let files = menu_files(exe, home)?;
    // Check every target before writing any of them, so a refusal leaves the
    // desktop exactly as it was instead of three quarters integrated.
    for file in &files {
        if !is_managed_or_absent(&file.path)? {
            return Err(IntegrationError::Unmanaged {
                path: file.path.clone(),
            });
        }
    }
    if !dry_run {
        for file in &files {
            write_atomic(&file.path, &file.contents, file.executable)?;
        }
    }
    Ok(files)
}

/// Removes every context menu file under `home` that MorphRing wrote, and
/// returns the paths removed (or, with `dry_run`, the paths it would remove).
///
/// Files without [`MANAGED_MARKER`] are left alone.
pub fn uninstall(home: &Path, dry_run: bool) -> Result<Vec<PathBuf>, IntegrationError> {
    let data = data_home(home);
    let mut removed = Vec::new();
    for manager in FileManager::ALL {
        let path = data.join(manager.relative_path());
        if !path.exists() || !is_managed_or_absent(&path)? {
            continue;
        }
        if !dry_run {
            fs::remove_file(&path).map_err(io_error(&path))?;
        }
        removed.push(path);
    }
    Ok(removed)
}

// Filee honoured XDG_DATA_HOME. We take `home` as a parameter so tests never
// touch the environment (PLAN.md D9), and every file manager we target reads
// ~/.local/share anyway when XDG_DATA_HOME is unset, which is nearly always.
fn data_home(home: &Path) -> PathBuf {
    home.join(".local").join("share")
}

fn is_managed_or_absent(path: &Path) -> Result<bool, IntegrationError> {
    match fs::read(path) {
        Ok(bytes) => Ok(is_managed(&String::from_utf8_lossy(&bytes))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(io_error(path)(e)),
    }
}

/// True when `contents` carries [`MANAGED_MARKER`] on a line of its own
/// (optionally behind a `#` comment, as in the Nautilus script).
pub fn is_managed(contents: &str) -> bool {
    contents.lines().any(|line| {
        let line = line.trim();
        line.strip_prefix('#').map_or(line, str::trim_start) == MANAGED_MARKER
    })
}

/// Writes `contents` to `path` via a temp file in the same folder, so a file
/// manager never reads half a menu.
fn write_atomic(path: &Path, contents: &str, executable: bool) -> Result<(), IntegrationError> {
    let dir = path.parent().ok_or_else(|| IntegrationError::Io {
        path: path.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidInput, "path has no parent folder"),
    })?;
    fs::create_dir_all(dir).map_err(io_error(dir))?;
    let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(io_error(dir))?;
    temp.write_all(contents.as_bytes())
        .map_err(io_error(path))?;
    // NamedTempFile starts life as 0600, which nobody but us can read. Set
    // the final mode before the rename so the file is never briefly wrong.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o755 } else { 0o644 };
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(mode))
            .map_err(io_error(path))?;
    }
    #[cfg(not(unix))]
    let _ = executable;
    temp.as_file().sync_all().map_err(io_error(path))?;
    temp.persist(path).map_err(|e| io_error(path)(e.error))?;
    Ok(())
}

fn exe_str(exe: &Path) -> Result<&str, IntegrationError> {
    let invalid = |reason| IntegrationError::InvalidExecutable {
        path: exe.to_path_buf(),
        reason,
    };
    // Desktop files are UTF-8 by spec, so a path that is not cannot be named
    // in one. Rare on Linux, but OsStr makes me ask.
    let s = exe.to_str().ok_or_else(|| invalid("not valid UTF-8"))?;
    if s.trim().is_empty() {
        return Err(invalid("empty"));
    }
    if s.contains(['\0', '\r', '\n']) {
        return Err(invalid("contains a null character or line break"));
    }
    // A file manager starts us from who knows which folder, so a relative
    // path would resolve against a coin toss.
    if !exe.is_absolute() {
        return Err(invalid("not an absolute path"));
    }
    Ok(s)
}

/// `"<exe>" --convert %F`, escaped for an `Exec=` line.
fn desktop_command(exe: &Path) -> Result<String, IntegrationError> {
    Ok(format!("{} --convert %F", desktop_argument(exe)?))
}

/// Quotes `exe` as one `Exec=` argument.
///
/// An `Exec=` value goes through two parsers: the key-file reader (backslash
/// escapes, then `%` field codes) and the Exec quoting rules (`"`, `` ` ``,
/// `$` and `\` are backslash-escaped inside double quotes). So a backslash in
/// the path ends up quadrupled. That is the spec, not a bug.
fn desktop_argument(exe: &Path) -> Result<String, IntegrationError> {
    let s = exe_str(exe)?;
    // The Desktop Entry spec forbids '=' in Exec arguments, and KDE takes it
    // literally, so refuse instead of writing a menu that silently breaks.
    if s.contains('=') {
        return Err(IntegrationError::InvalidExecutable {
            path: exe.to_path_buf(),
            reason: "desktop launchers do not support '=' in an executable path",
        });
    }
    let mut quoted = String::with_capacity(s.len() + 2);
    quoted.push('"');
    for c in s.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    Ok(desktop_value(&quoted).replace('%', "%%"))
}

/// Single-quotes `value` for POSIX sh.
fn shell_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Escapes a key-file value (backslash, CR, LF, tab).
fn desktop_value(value: &str) -> String {
    value
        .replace('\\', r"\\")
        .replace('\r', r"\r")
        .replace('\n', r"\n")
        .replace('\t', r"\t")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const EXE: &str = "/usr/bin/morphring-app";

    fn exe() -> &'static Path {
        Path::new(EXE)
    }

    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn dolphin_service_menu_is_exact() {
        assert_eq!(
            dolphin_service_menu(exe()).unwrap(),
            "[Desktop Entry]\n\
             Type=Service\n\
             MimeType=all/allfiles;\n\
             Actions=convert;\n\
             X-KDE-Protocols=file\n\
             X-MorphRing-Managed=true\n\
             \n\
             [Desktop Action convert]\n\
             Name=Convert with MorphRing\n\
             Icon=document-save-as\n\
             Exec=\"/usr/bin/morphring-app\" --convert %F\n"
        );
    }

    #[test]
    fn nautilus_script_is_exact() {
        assert_eq!(
            nautilus_script(exe()).unwrap(),
            "#!/bin/sh\n\
             # X-MorphRing-Managed=true\n\
             [ \"$#\" -gt 0 ] || exit 0\n\
             for file do\n\
             \x20 case \"$file\" in\n\
             \x20   /*) set -- \"$@\" \"$file\" ;;\n\
             \x20   *) set -- \"$@\" \"$PWD/$file\" ;;\n\
             \x20 esac\n\
             \x20 shift\n\
             done\n\
             exec '/usr/bin/morphring-app' --convert \"$@\"\n"
        );
    }

    #[test]
    fn nemo_action_is_exact() {
        assert_eq!(
            nemo_action(exe()).unwrap(),
            "[Nemo Action]\n\
             Name=Convert with MorphRing\n\
             Exec=\"/usr/bin/morphring-app\" --convert %F\n\
             Selection=notnone\n\
             Extensions=nodirs;\n\
             UriScheme=file\n\
             Icon-Name=document-save-as\n\
             X-MorphRing-Managed=true\n"
        );
    }

    #[test]
    fn thunar_sendto_is_exact() {
        assert_eq!(
            thunar_sendto(exe()).unwrap(),
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Convert with MorphRing\n\
             Exec=\"/usr/bin/morphring-app\" --convert %F\n\
             Icon=document-save-as\n\
             Terminal=false\n\
             X-MorphRing-Managed=true\n"
        );
    }

    #[test]
    fn every_file_is_managed_and_runs_convert() {
        for file in menu_files(exe(), Path::new("/home/u")).unwrap() {
            assert!(is_managed(&file.contents), "{:?}", file.manager);
            let runs = match file.manager {
                FileManager::Nautilus => "--convert \"$@\"",
                _ => "--convert %F",
            };
            assert!(file.contents.contains(runs), "{:?}", file.manager);
        }
    }

    #[test]
    fn desktop_arguments_escape_both_parsing_layers() {
        // Same vector as Filee's DesktopArgumentsEscapeBothDesktopParsingLayers.
        assert_eq!(
            desktop_argument(Path::new("/tmp/a\"b$c`d\\e%f")).unwrap(),
            r#""/tmp/a\\"b\\$c\\`d\\\\e%%f""#
        );
        assert_eq!(shell_argument("/tmp/a'b $c"), r"'/tmp/a'\''b $c'");
        assert_eq!(desktop_value("label\nExec=bad"), r"label\nExec=bad");
    }

    #[test]
    fn spaces_unicode_and_quotes_stay_one_argument() {
        let exe = Path::new("/opt/Morph Ring's 앱/morphring-app");
        assert!(
            dolphin_service_menu(exe)
                .unwrap()
                .contains("Exec=\"/opt/Morph Ring's 앱/morphring-app\" --convert %F\n")
        );
        assert!(
            nautilus_script(exe)
                .unwrap()
                .contains("exec '/opt/Morph Ring'\\''s 앱/morphring-app' --convert \"$@\"\n")
        );
    }

    #[test]
    fn bad_executables_are_refused() {
        for bad in [
            "",
            "   ",
            "relative/morphring-app",
            "/opt/morph\nExec=bad",
            "/opt/morph\rring",
            "/opt/a=b/morphring-app",
        ] {
            let err = menu_files(Path::new(bad), Path::new("/home/u")).unwrap_err();
            assert!(
                matches!(err, IntegrationError::InvalidExecutable { .. }),
                "{bad:?}: {err}"
            );
        }
    }

    #[test]
    fn non_utf8_executable_is_refused() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let exe = Path::new(OsStr::from_bytes(b"/opt/\xffmorph"));
        assert!(matches!(
            dolphin_service_menu(exe),
            Err(IntegrationError::InvalidExecutable { .. })
        ));
    }

    #[test]
    fn marker_detection_needs_a_whole_line() {
        assert!(is_managed("a\nX-MorphRing-Managed=true\n"));
        assert!(is_managed("#!/bin/sh\n# X-MorphRing-Managed=true\n"));
        assert!(!is_managed("X-MorphRing-Managed=false\n"));
        assert!(!is_managed(
            "Name=X-MorphRing-Managed=true but not really\n"
        ));
        assert!(!is_managed(""));
    }

    #[test]
    fn install_writes_every_file_with_its_mode() {
        let home = TempDir::new().unwrap();
        let files = install(exe(), home.path(), false).unwrap();
        assert_eq!(files.len(), 4);
        let data = home.path().join(".local/share");
        let expect = [
            ("kio/servicemenus/morphring.desktop", 0o755),
            ("nautilus/scripts/MorphRing", 0o755),
            ("nemo/actions/morphring.nemo_action", 0o644),
            ("Thunar/sendto/morphring.desktop", 0o644),
        ];
        for ((rel, want_mode), file) in expect.iter().zip(&files) {
            let path = data.join(rel);
            assert_eq!(file.path, path);
            assert_eq!(fs::read_to_string(&path).unwrap(), file.contents);
            assert_eq!(mode(&path), *want_mode, "{rel}");
        }
        // No temp files left lying around next to the menus.
        assert_eq!(
            file_names(&data.join("kio/servicemenus")),
            ["morphring.desktop"]
        );
        assert_eq!(file_names(&data.join("nautilus/scripts")), ["MorphRing"]);
    }

    #[test]
    fn dry_run_touches_nothing() {
        let home = TempDir::new().unwrap();
        let files = install(exe(), home.path(), true).unwrap();
        assert_eq!(files.len(), 4);
        assert!(file_names(home.path()).is_empty());

        install(exe(), home.path(), false).unwrap();
        let removed = uninstall(home.path(), true).unwrap();
        assert_eq!(removed.len(), 4);
        assert!(removed.iter().all(|p| p.exists()));
    }

    #[test]
    fn reinstall_replaces_managed_files() {
        let home = TempDir::new().unwrap();
        install(exe(), home.path(), false).unwrap();
        let files = install(Path::new("/opt/new/morphring-app"), home.path(), false).unwrap();
        for file in files {
            let text = fs::read_to_string(&file.path).unwrap();
            assert!(text.contains("/opt/new/morphring-app"));
            assert!(!text.contains(EXE));
        }
    }

    #[test]
    fn install_refuses_unmanaged_file_and_writes_nothing() {
        let home = TempDir::new().unwrap();
        let data = home.path().join(".local/share");
        let theirs = data.join("nemo/actions/morphring.nemo_action");
        fs::create_dir_all(theirs.parent().unwrap()).unwrap();
        fs::write(&theirs, "[Nemo Action]\nName=Mine\n").unwrap();

        let err = install(exe(), home.path(), false).unwrap_err();
        assert!(matches!(&err, IntegrationError::Unmanaged { path } if *path == theirs));
        assert_eq!(
            fs::read_to_string(&theirs).unwrap(),
            "[Nemo Action]\nName=Mine\n"
        );
        assert!(!data.join("kio").exists());
        assert!(matches!(
            install(exe(), home.path(), true),
            Err(IntegrationError::Unmanaged { .. })
        ));
    }

    #[test]
    fn uninstall_removes_only_managed_files() {
        let home = TempDir::new().unwrap();
        let files = install(exe(), home.path(), false).unwrap();
        let theirs = &files[3].path;
        fs::write(theirs, "[Desktop Entry]\nName=Hand made\n").unwrap();

        let removed = uninstall(home.path(), false).unwrap();
        assert_eq!(
            removed,
            [
                files[0].path.clone(),
                files[1].path.clone(),
                files[2].path.clone()
            ]
        );
        assert!(removed.iter().all(|p| !p.exists()));
        assert!(theirs.exists());
        assert!(uninstall(home.path(), false).unwrap().is_empty());
    }

    #[test]
    fn nautilus_script_makes_relative_names_absolute() {
        // Run the generated script with a stand-in for the app that prints
        // its arguments, one per line.
        let dir = TempDir::new().unwrap();
        let fake = dir.path().join("fake app");
        fs::write(&fake, "#!/bin/sh\nfor a do printf '%s\\n' \"$a\"; done\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let script = dir.path().join("MorphRing");
        write_atomic(&script, &nautilus_script(&fake).unwrap(), true).unwrap();

        let out = std::process::Command::new(&script)
            .args(["a b.png", "/abs/c.png"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(out.status.success());
        let cwd = dir.path().to_str().unwrap();
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            format!("--convert\n{cwd}/a b.png\n/abs/c.png\n")
        );
    }
}
