//! macOS Finder integration: a Quick Action.
//!
//! [`install`] writes an Automator service bundle,
//! `~/Library/Services/Convert with MorphRing.workflow`, with two files:
//!
//! | File | What it says |
//! |---|---|
//! | `Contents/Info.plist` | `NSServices`: the menu item, `public.item` files, Finder only |
//! | `Contents/document.wflow` | one Run Shell Script action, input as arguments, `/bin/sh` |
//!
//! The script is `/usr/bin/open -a MorphRing "$@"`, so a running app gets the
//! files through `RunEvent::Opened` instead of a second instance starting
//! (`docs/research/macos-support.md` section 6).
//!
//! Both files carry [`MANAGED_MARKER`] as an XML comment. [`install`] only
//! replaces files that carry it and [`uninstall`] only removes files that
//! carry it, the same deal the Linux menus get.
//!
//! The generation and file writing are plain Rust and run on every OS, so a
//! Linux test can write the bundle into a temporary home. Only the
//! `pbs -update` nudge after a real install is macOS only.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::integration::{Change, IntegrationError, LABEL};

/// The comment that marks a Quick Action file as written by MorphRing.
pub const MANAGED_MARKER: &str = "<!-- X-MorphRing-Managed=true -->";

/// What the user may still have to do by hand after [`install`].
pub const ENABLE_HINT: &str = "If 'Convert with MorphRing' does not show up under Quick Actions, \
     turn it on in System Settings > General > Login Items & Extensions > Finder.";

// RISK: these templates are hand-written, not saved by Automator on a Mac as
// the research note would have liked. They are modelled on what Automator
// 2.10 writes for a Finder Quick Action, `plutil -lint` checks them on CI,
// but whether Finder actually lists the result can only be seen on a real
// Mac. If it ever refuses, save the same workflow in Automator once and diff.
const INFO_PLIST: &str = include_str!("../resources/quick-action/Info.plist");
const DOCUMENT_WFLOW: &str = include_str!("../resources/quick-action/document.wflow");

/// The spot in [`DOCUMENT_WFLOW`] where the shell command goes.
const COMMAND_SLOT: &str = "{{command}}";

/// One file of the Quick Action bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowFile {
    /// Where the file goes.
    pub path: PathBuf,
    /// What goes in it.
    pub contents: String,
}

/// The bundle folder, `<home>/Library/Services/Convert with MorphRing.workflow`.
pub fn workflow_dir(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Services")
        .join(format!("{LABEL}.workflow"))
}

/// The `Info.plist` that tells the Services menu about the action.
pub fn info_plist() -> String {
    lf(INFO_PLIST)
}

/// The `document.wflow` whose one action hands the selected files to the
/// app that `exe` belongs to.
pub fn document_wflow(exe: &Path) -> Result<String, IntegrationError> {
    let command = xml_escape(&shell_command(exe)?);
    Ok(lf(DOCUMENT_WFLOW).replace(COMMAND_SLOT, &command))
}

/// The template with any CRLF folded back to LF.
///
/// A Windows checkout with autocrlf hands `include_str!` CRLF templates, and
/// a plist should not change its line endings depending on which OS happened
/// to compile the binary. One `replace` beats one more `.gitattributes` rule.
fn lf(template: &str) -> String {
    template.replace("\r\n", "\n")
}

/// The shell script the action runs.
///
/// When `exe` sits inside an app bundle (the usual
/// `/Applications/MorphRing.app/Contents/MacOS/morphring-app`), the bundle is
/// named by path, so `--exe` really picks the app. Otherwise LaunchServices
/// looks MorphRing up by name.
pub fn shell_command(exe: &Path) -> Result<String, IntegrationError> {
    let invalid = |reason| IntegrationError::InvalidExecutable {
        path: exe.to_path_buf(),
        reason,
    };
    let s = exe.to_str().ok_or_else(|| invalid("not valid UTF-8"))?;
    if s.trim().is_empty() {
        return Err(invalid("empty"));
    }
    let app = exe
        .ancestors()
        .find(|dir| dir.extension().is_some_and(|ext| ext == "app"))
        .and_then(Path::to_str)
        .map_or_else(|| "MorphRing".to_owned(), shell_quote);
    // Automator runs the script with a PATH of about four folders, so `open`
    // gets its full name rather than a hopeful lookup.
    Ok(format!("/usr/bin/open -a {app} \"$@\""))
}

/// Every file of the Quick Action for `exe`, placed under `home`.
pub fn workflow_files(exe: &Path, home: &Path) -> Result<Vec<WorkflowFile>, IntegrationError> {
    let contents = workflow_dir(home).join("Contents");
    Ok(vec![
        WorkflowFile {
            path: contents.join("Info.plist"),
            contents: info_plist(),
        },
        WorkflowFile {
            path: contents.join("document.wflow"),
            contents: document_wflow(exe)?,
        },
    ])
}

/// Writes the Quick Action starting `exe` under `home`, and returns the files
/// plus a [`Change::Note`] on enabling it in System Settings.
///
/// Existing files are replaced only when they carry [`MANAGED_MARKER`]. With
/// `dry_run`, nothing is written and the same changes are returned.
pub fn install(exe: &Path, home: &Path, dry_run: bool) -> Result<Vec<Change>, IntegrationError> {
    let files = workflow_files(exe, home)?;
    // Refuse before writing anything, so a stranger's workflow with our name
    // never ends up half ours.
    for file in &files {
        if !is_managed_or_absent(&file.path)? {
            return Err(IntegrationError::Unmanaged {
                path: file.path.clone(),
            });
        }
    }
    if !dry_run {
        for file in &files {
            write_atomic(&file.path, &file.contents)?;
        }
        refresh_services();
    }
    let mut changes: Vec<Change> = files.into_iter().map(|f| Change::File(f.path)).collect();
    changes.push(Change::Note(ENABLE_HINT.to_owned()));
    Ok(changes)
}

/// Removes the Quick Action files under `home` that MorphRing wrote, and
/// returns them (or, with `dry_run`, the ones it would remove). The bundle
/// folders go too once they are empty.
pub fn uninstall(home: &Path, dry_run: bool) -> Result<Vec<Change>, IntegrationError> {
    let dir = workflow_dir(home);
    let contents = dir.join("Contents");
    let mut removed = Vec::new();
    for name in ["Info.plist", "document.wflow"] {
        let path = contents.join(name);
        if !path.exists() || !is_managed_or_absent(&path)? {
            continue;
        }
        if !dry_run {
            fs::remove_file(&path).map_err(io_error(&path))?;
        }
        removed.push(Change::File(path));
    }
    if !dry_run && !removed.is_empty() {
        // remove_dir only takes empty folders, which is exactly the manners we
        // want: anything the user tucked in there keeps the bundle alive.
        let _ = fs::remove_dir(&contents);
        let _ = fs::remove_dir(&dir);
    }
    Ok(removed)
}

/// True when `contents` carries [`MANAGED_MARKER`] on a line of its own.
pub fn is_managed(contents: &str) -> bool {
    contents.lines().any(|line| line.trim() == MANAGED_MARKER)
}

fn is_managed_or_absent(path: &Path) -> Result<bool, IntegrationError> {
    match fs::read(path) {
        Ok(bytes) => Ok(is_managed(&String::from_utf8_lossy(&bytes))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(io_error(path)(e)),
    }
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> IntegrationError + '_ {
    move |source| IntegrationError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Writes `contents` to `path` via a temp file in the same folder, so the
/// Services menu never reads half a plist.
fn write_atomic(path: &Path, contents: &str) -> Result<(), IntegrationError> {
    let dir = path.parent().ok_or_else(|| IntegrationError::Io {
        path: path.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidInput, "path has no parent folder"),
    })?;
    fs::create_dir_all(dir).map_err(io_error(dir))?;
    let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(io_error(dir))?;
    temp.write_all(contents.as_bytes())
        .map_err(io_error(path))?;
    // NamedTempFile is born 0600. A plist nobody else can read is a plist
    // pbs may quietly skip, so give it the usual 0644 before the rename.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o644))
            .map_err(io_error(path))?;
    }
    temp.as_file().sync_all().map_err(io_error(path))?;
    temp.persist(path).map_err(|e| io_error(path)(e.error))?;
    Ok(())
}

/// Asks the pasteboard server to re-read the Services, so the Quick Action
/// can show up without a re-login.
#[cfg(target_os = "macos")]
fn refresh_services() {
    // The platform crate has no engines dependency (and should not grow one
    // for a single fire-and-forget call), so this is a plain Command. Its
    // failure is ignored on purpose: the files are written either way, and
    // the worst case is the re-login the hint already covers.
    let _ = std::process::Command::new("/System/Library/CoreServices/pbs")
        .arg("-update")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Anywhere but macOS there is no pasteboard server to poke.
#[cfg(not(target_os = "macos"))]
fn refresh_services() {}

/// Single-quotes `value` for `/bin/sh`.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Escapes `value` for an XML text node.
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe() -> &'static Path {
        Path::new("/Applications/MorphRing.app/Contents/MacOS/morphring-app")
    }

    #[test]
    fn the_command_names_the_bundle_when_there_is_one() {
        assert_eq!(
            shell_command(exe()).unwrap(),
            r#"/usr/bin/open -a '/Applications/MorphRing.app' "$@""#
        );
        assert_eq!(
            shell_command(Path::new("/usr/local/bin/morphring-app")).unwrap(),
            r#"/usr/bin/open -a MorphRing "$@""#
        );
        assert_eq!(
            shell_command(Path::new("/Users/o'brien/Tom & Jerry.app/Contents/MacOS/x")).unwrap(),
            r#"/usr/bin/open -a '/Users/o'\''brien/Tom & Jerry.app' "$@""#
        );
        assert!(matches!(
            shell_command(Path::new(" ")),
            Err(IntegrationError::InvalidExecutable { .. })
        ));
    }

    #[test]
    fn crlf_templates_come_out_as_lf() {
        assert_eq!(lf("<a>\r\n\t<b/>\r\n</a>\n"), "<a>\n\t<b/>\n</a>\n");
    }

    #[test]
    fn the_templates_carry_the_marker_and_the_label() {
        let info = info_plist();
        let wflow = document_wflow(exe()).unwrap();
        assert!(is_managed(&info));
        assert!(is_managed(&wflow));
        assert!(info.contains(&format!("<string>{LABEL}</string>")));
        assert!(info.contains("<string>public.item</string>"));
        assert!(info.contains("<key>NSRequiredContext</key>"));
        assert!(!wflow.contains(COMMAND_SLOT));
        assert!(
            wflow
                .contains("<string>/usr/bin/open -a '/Applications/MorphRing.app' \"$@\"</string>")
        );
        assert!(wflow.contains("<string>/bin/sh</string>"));
        assert!(wflow.contains("<key>inputMethod</key>\n\t\t\t\t\t<integer>1</integer>"));
        // Exactly one action, or Automator gets ideas.
        assert_eq!(wflow.matches("<key>ActionName</key>").count(), 1);
        // Whatever the checkout did to line endings, the output is LF only.
        assert!(!info.contains('\r'));
        assert!(!wflow.contains('\r'));
    }

    #[test]
    fn xml_specials_in_the_path_are_escaped() {
        let wflow =
            document_wflow(Path::new("/Users/a/<Tom & Jerry>.app/Contents/MacOS/x")).unwrap();
        assert!(wflow.contains("'/Users/a/&lt;Tom &amp; Jerry&gt;.app'"));
    }

    #[test]
    fn install_writes_the_bundle_and_uninstall_removes_it() {
        let home = tempfile::TempDir::new().unwrap();
        let dir = workflow_dir(home.path());
        let info = dir.join("Contents").join("Info.plist");
        let wflow = dir.join("Contents").join("document.wflow");
        let files = vec![Change::File(info.clone()), Change::File(wflow.clone())];
        let mut expected = files.clone();
        expected.push(Change::Note(ENABLE_HINT.to_owned()));
        assert!(dir.ends_with("Library/Services/Convert with MorphRing.workflow"));

        // A dry run plans everything and touches nothing.
        assert_eq!(install(exe(), home.path(), true).unwrap(), expected);
        assert!(fs::read_dir(home.path()).unwrap().next().is_none());
        assert!(uninstall(home.path(), false).unwrap().is_empty());

        assert_eq!(install(exe(), home.path(), false).unwrap(), expected);
        assert_eq!(fs::read_to_string(&info).unwrap(), info_plist());
        assert_eq!(
            fs::read_to_string(&wflow).unwrap(),
            document_wflow(exe()).unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&wflow).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o644);
        }
        // Installing twice is fine: the files are ours.
        assert_eq!(install(exe(), home.path(), false).unwrap(), expected);

        assert_eq!(uninstall(home.path(), true).unwrap(), files);
        assert!(info.exists());
        assert_eq!(uninstall(home.path(), false).unwrap(), files);
        assert!(!dir.exists());
        assert!(dir.parent().unwrap().exists());
        assert!(uninstall(home.path(), false).unwrap().is_empty());
    }

    #[test]
    fn a_workflow_we_did_not_write_is_left_alone() {
        let home = tempfile::TempDir::new().unwrap();
        let contents = workflow_dir(home.path()).join("Contents");
        fs::create_dir_all(&contents).unwrap();
        let theirs = contents.join("document.wflow");
        fs::write(&theirs, "<plist>handmade with love</plist>\n").unwrap();

        let err = install(exe(), home.path(), false).unwrap_err();
        assert!(matches!(err, IntegrationError::Unmanaged { ref path } if *path == theirs));
        // The refusal came before any write, Info.plist included.
        assert!(!contents.join("Info.plist").exists());

        assert!(uninstall(home.path(), false).unwrap().is_empty());
        assert_eq!(
            fs::read_to_string(&theirs).unwrap(),
            "<plist>handmade with love</plist>\n"
        );
    }

    /// The generated files must be plists macOS itself accepts.
    #[cfg(target_os = "macos")]
    #[test]
    fn plutil_accepts_both_files() {
        let home = tempfile::TempDir::new().unwrap();
        for file in workflow_files(exe(), home.path()).unwrap() {
            write_atomic(&file.path, &file.contents).unwrap();
            let out = std::process::Command::new("/usr/bin/plutil")
                .arg("-lint")
                .arg(&file.path)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "plutil -lint {}: {}{}",
                file.path.display(),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}
