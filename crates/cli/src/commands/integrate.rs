//! `morphring integrate install|uninstall`: adds or removes the "Convert with
//! MorphRing" entry in the file manager context menus, each starting the
//! MorphRing app with `--convert`. On Linux that is Dolphin, Nautilus, Nemo
//! and Thunar; `morphring_platform::integration` picks the backend per OS.
//!
//! Like Filee, only release builds change the desktop on their own. Other
//! builds need `--allow`, so `cargo run` never rewires the developer's file
//! manager by accident. `--dry-run` changes nothing and so needs no permission.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use morphring_platform::integration;

use crate::args::IntegrateArgs;
use crate::ctx::Ctx;

/// The file name of the MorphRing desktop app, which the menus start.
pub const APP_BINARY: &str = "morphring-app";

/// Exit code for wrong arguments, as everywhere else.
const EXIT_USAGE: i32 = 2;

/// What `integrate` should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Install,
    Uninstall,
}

impl Action {
    /// Parses `install` or `uninstall`, ignoring case.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "install" => Some(Action::Install),
            "uninstall" => Some(Action::Uninstall),
            _ => None,
        }
    }
}

/// The parts of the outside world `integrate` needs, read once by the caller
/// so tests can hand in a temporary home instead (D9).
#[derive(Debug, Clone, Default)]
pub struct Host {
    /// The user's home folder; the menus go under `<home>/.local/share`.
    pub home: Option<PathBuf>,
    /// True in release builds, which may change the desktop without `--allow`.
    pub release_build: bool,
    /// The MorphRing app found next to this binary or on `PATH`, if any.
    pub app: Option<PathBuf>,
}

impl Host {
    /// Reads the home folder, the build profile and the app location from
    /// the running process. Only `lib.rs` calls this; tests never do.
    pub fn from_process() -> Self {
        let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
        let current_exe = std::env::current_exe().ok();
        let path = std::env::var_os("PATH");
        Self {
            home,
            release_build: !cfg!(debug_assertions),
            app: find_app(current_exe.as_deref(), path.as_deref()),
        }
    }
}

/// Finds the MorphRing app: next to `current_exe` first (an unpacked release
/// tarball keeps both binaries together), then in every folder of `path`.
pub fn find_app(current_exe: Option<&Path>, path: Option<&OsStr>) -> Option<PathBuf> {
    let beside = current_exe
        .and_then(Path::parent)
        .map(|dir| dir.join(APP_BINARY));
    let on_path = path
        .into_iter()
        .flat_map(std::env::split_paths)
        // An empty PATH entry means "the current folder", which is exactly
        // the kind of surprise a context menu should not be built on.
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(APP_BINARY));
    beside.into_iter().chain(on_path).find(|p| p.is_file())
}

/// Runs `morphring integrate` against `host`. Returns the exit code: 0 done,
/// 1 refused or failed, 2 wrong arguments.
pub fn run(args: &IntegrateArgs, ctx: &mut Ctx, host: &Host) -> i32 {
    let action = match args.action.as_deref() {
        None => return usage(ctx, "Give an action: install or uninstall."),
        Some(s) => match Action::parse(s) {
            Some(action) => action,
            None => {
                return usage(
                    ctx,
                    &format!("Unknown action '{s}'. Use install or uninstall."),
                );
            }
        },
    };
    if args.exe.is_some() && action == Action::Uninstall {
        return usage(ctx, "--exe only applies to install.");
    }

    let Some(home) = host.home.as_deref() else {
        return fail(ctx, "cannot find your home folder.");
    };
    // A dry run only reads, so it is the one thing a debug build may do
    // without being told twice.
    if !args.dry_run && !host.release_build && !args.allow {
        return fail(
            ctx,
            "this is not a release build, so it does not change the desktop on its own. \
             Add --allow to do it anyway, or --dry-run to see what would change.",
        );
    }

    match action {
        Action::Install => install(args, ctx, host, home),
        Action::Uninstall => uninstall(args, ctx, home),
    }
}

fn install(args: &IntegrateArgs, ctx: &mut Ctx, host: &Host, home: &Path) -> i32 {
    let exe = match &args.exe {
        // Relative to where the user typed it, not to wherever a file manager
        // happens to start us from later.
        Some(exe) => ctx.cwd.join(exe),
        None => match &host.app {
            Some(app) => app.clone(),
            None => {
                return fail(
                    ctx,
                    &format!(
                        "cannot find the MorphRing app ({APP_BINARY}) next to this program or on PATH. \
                         Install it, or name it with --exe <path>."
                    ),
                );
            }
        },
    };
    if !exe.is_file() {
        return fail(ctx, &format!("{} is not a file.", exe.display()));
    }

    let changes = match integration::install(&exe, home, args.dry_run) {
        Ok(changes) => changes,
        Err(e) => return fail(ctx, &e.to_string()),
    };
    if changes.is_empty() {
        // Only a backend that is still a stub says nothing at all, and an
        // empty "now opens ..." would be a promise we cannot keep.
        let _ = writeln!(
            ctx.out,
            "MorphRing cannot add a context menu entry on this system yet."
        );
        return 0;
    }
    let verb = if args.dry_run { "Would write" } else { "Wrote" };
    for change in &changes {
        let _ = match (change, integration::surface_of(change)) {
            // A note is advice, not something we wrote, so it gets no verb.
            (integration::Change::Note(note), _) => writeln!(ctx.out, "{note}"),
            (_, Some(surface)) => writeln!(ctx.out, "{verb} {change} ({surface})"),
            (_, None) => writeln!(ctx.out, "{verb} {change}"),
        };
    }
    if !args.dry_run {
        let _ = writeln!(
            ctx.out,
            "'{}' now opens {} in {}.",
            integration::LABEL,
            exe.display(),
            english_list(&integration::surfaces())
        );
        let _ = writeln!(
            ctx.out,
            "Restart the file manager if the entry does not show up yet."
        );
    }
    0
}

fn uninstall(args: &IntegrateArgs, ctx: &mut Ctx, home: &Path) -> i32 {
    let removed = match integration::uninstall(home, args.dry_run) {
        Ok(removed) => removed,
        Err(e) => return fail(ctx, &e.to_string()),
    };
    if removed.is_empty() {
        let _ = writeln!(ctx.out, "No MorphRing context menu entries to remove.");
        return 0;
    }
    let verb = if args.dry_run {
        "Would remove"
    } else {
        "Removed"
    };
    for change in &removed {
        let _ = writeln!(ctx.out, "{verb} {change}");
    }
    0
}

/// "Dolphin, Nautilus, Nemo and Thunar".
fn english_list(names: &[&str]) -> String {
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => names.concat(),
    }
}

fn usage(ctx: &mut Ctx, message: &str) -> i32 {
    let _ = writeln!(ctx.err, "morphring: {message}");
    let _ = writeln!(ctx.err, "Run 'morphring help integrate' for usage.");
    EXIT_USAGE
}

fn fail(ctx: &mut Ctx, message: &str) -> i32 {
    let _ = writeln!(ctx.err, "morphring: {message}");
    1
}

#[cfg(test)]
mod tests {
    use std::fs;

    use morphring_core::CancelToken;
    use tempfile::TempDir;

    use super::*;

    /// A fake world: a temp home and a fake app binary inside it.
    struct World {
        dir: TempDir,
    }

    impl World {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            fs::create_dir_all(dir.path().join("home")).unwrap();
            fs::create_dir_all(dir.path().join("bin")).unwrap();
            // Not a real app, but the menus only need a path that exists.
            fs::write(dir.path().join("bin").join(APP_BINARY), "#!/bin/sh\n").unwrap();
            Self { dir }
        }

        fn home(&self) -> PathBuf {
            self.dir.path().join("home")
        }

        fn app(&self) -> PathBuf {
            self.dir.path().join("bin").join(APP_BINARY)
        }

        fn host(&self, release_build: bool) -> Host {
            Host {
                home: Some(self.home()),
                release_build,
                app: Some(self.app()),
            }
        }

        // Only the Linux tests look at menu files.
        #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
        fn menu(&self, relative: &str) -> PathBuf {
            self.home().join(".local/share").join(relative)
        }

        // Only the Linux tests look at menu files.
        #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
        fn dolphin(&self) -> PathBuf {
            self.menu("kio/servicemenus/morphring.desktop")
        }

        // Only the Linux tests look at menu files.
        #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
        fn all_menus(&self) -> [PathBuf; 4] {
            [
                self.dolphin(),
                self.menu("nautilus/scripts/MorphRing"),
                self.menu("nemo/actions/morphring.nemo_action"),
                self.menu("Thunar/sendto/morphring.desktop"),
            ]
        }

        /// Runs integrate with `argv` (after `integrate`) through the real
        /// clap tree, so the flags are tested as a user types them.
        fn run(&self, argv: &[&str], host: &Host) -> (i32, String, String) {
            use clap::Parser;

            let cli = crate::args::Cli::try_parse_from(
                ["morphring", "integrate"].iter().chain(argv.iter()),
            )
            .unwrap();
            let Some(crate::args::Command::Integrate(args)) = cli.command else {
                panic!("not integrate")
            };
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = {
                let mut ctx = Ctx {
                    out: &mut out,
                    err: &mut err,
                    cwd: self.dir.path().to_path_buf(),
                    cancel: CancelToken::new(),
                    quit: CancelToken::new(),
                    engines: Vec::new(),
                    config_dir: None,
                };
                run(&args, &mut ctx, host)
            };
            (
                code,
                String::from_utf8(out).unwrap(),
                String::from_utf8(err).unwrap(),
            )
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn release_build_installs_every_menu_pointing_at_the_app() {
        let world = World::new();
        let (code, out, err) = world.run(&["install"], &world.host(true));
        assert_eq!((code, err.as_str()), (0, ""));
        for menu in world.all_menus() {
            assert!(menu.is_file(), "{}", menu.display());
            assert!(out.contains(&menu.display().to_string()), "{out}");
        }
        let dolphin = fs::read_to_string(world.dolphin()).unwrap();
        assert!(dolphin.contains(&format!("Exec=\"{}\" --convert %F", world.app().display())));
        assert!(dolphin.contains(morphring_platform::context_menu::MANAGED_MARKER));
        assert!(out.contains("Dolphin, Nautilus, Nemo and Thunar"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dolphin_menu_is_executable() {
        use std::os::unix::fs::PermissionsExt;

        let world = World::new();
        assert_eq!(world.run(&["install"], &world.host(true)).0, 0);
        let mode = fs::metadata(world.dolphin()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn debug_build_refuses_without_allow_and_writes_nothing() {
        let world = World::new();
        for action in ["install", "uninstall"] {
            let (code, out, err) = world.run(&[action], &world.host(false));
            assert_eq!(code, 1, "{action}");
            assert_eq!(out, "");
            assert!(err.contains("--allow"), "{err}");
        }
        assert!(!world.home().join(".local").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn allow_flag_lets_a_debug_build_install() {
        let world = World::new();
        let (code, _, err) = world.run(&["install", "--allow"], &world.host(false));
        assert_eq!((code, err.as_str()), (0, ""));
        assert!(world.dolphin().is_file());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dry_run_needs_no_permission_and_changes_nothing() {
        let world = World::new();
        let (code, out, err) = world.run(&["install", "--dry-run"], &world.host(false));
        assert_eq!((code, err.as_str()), (0, ""));
        assert_eq!(out.matches("Would write ").count(), 4, "{out}");
        assert!(!world.home().join(".local").exists());

        world.run(&["install", "--allow"], &world.host(false));
        let (code, out, _) = world.run(&["uninstall", "--dry-run"], &world.host(false));
        assert_eq!(code, 0);
        assert_eq!(out.matches("Would remove ").count(), 4, "{out}");
        assert!(world.all_menus().iter().all(|m| m.is_file()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn uninstall_removes_only_what_we_wrote() {
        let world = World::new();
        assert_eq!(world.run(&["install"], &world.host(true)).0, 0);
        // The user hand-wrote their own Nemo action with our file name.
        let nemo = world.menu("nemo/actions/morphring.nemo_action");
        fs::write(&nemo, "[Nemo Action]\nName=Mine\n").unwrap();

        let (code, out, err) = world.run(&["uninstall"], &world.host(true));
        assert_eq!((code, err.as_str()), (0, ""));
        assert_eq!(out.matches("Removed ").count(), 3, "{out}");
        assert!(!world.dolphin().exists());
        assert_eq!(
            fs::read_to_string(&nemo).unwrap(),
            "[Nemo Action]\nName=Mine\n"
        );

        let (code, out, _) = world.run(&["uninstall"], &world.host(true));
        assert_eq!(code, 0);
        assert_eq!(out, "No MorphRing context menu entries to remove.\n");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn install_refuses_to_replace_a_file_we_did_not_write() {
        let world = World::new();
        fs::create_dir_all(world.dolphin().parent().unwrap()).unwrap();
        fs::write(world.dolphin(), "[Desktop Entry]\n").unwrap();
        let (code, out, err) = world.run(&["install"], &world.host(true));
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(err.contains("was not written by MorphRing"), "{err}");
        assert_eq!(
            fs::read_to_string(world.dolphin()).unwrap(),
            "[Desktop Entry]\n"
        );
        assert!(!world.menu("nautilus").exists());
    }

    // Not on Windows: there a real install writes the real HKCU verb, and
    // tests never get to touch that one.
    #[cfg(not(windows))]
    #[test]
    fn install_twice_is_fine() {
        let world = World::new();
        assert_eq!(world.run(&["install"], &world.host(true)).0, 0);
        assert_eq!(world.run(&["install"], &world.host(true)).0, 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn exe_flag_overrides_the_app_and_resolves_against_cwd() {
        let world = World::new();
        fs::write(world.dir.path().join("my app"), "").unwrap();
        let host = Host {
            app: None,
            ..world.host(true)
        };
        let (code, _, err) = world.run(&["install", "--exe", "my app"], &host);
        assert_eq!((code, err.as_str()), (0, ""));
        let dolphin = fs::read_to_string(world.dolphin()).unwrap();
        let expected = world.dir.path().join("my app");
        assert!(dolphin.contains(&format!("Exec=\"{}\" --convert %F", expected.display())));
    }

    #[test]
    fn missing_app_or_home_is_reported() {
        let world = World::new();
        let no_app = Host {
            app: None,
            ..world.host(true)
        };
        let (code, _, err) = world.run(&["install"], &no_app);
        assert_eq!(code, 1);
        assert!(err.contains("--exe <path>"), "{err}");

        let (code, _, err) = world.run(&["install", "--exe", "nope"], &world.host(true));
        assert_eq!(code, 1);
        assert!(err.contains("is not a file"), "{err}");

        let no_home = Host {
            home: None,
            ..world.host(true)
        };
        let (code, _, err) = world.run(&["install"], &no_home);
        assert_eq!(code, 1);
        assert!(err.contains("home folder"), "{err}");
    }

    #[test]
    fn wrong_arguments_are_usage_errors() {
        let world = World::new();
        let host = world.host(true);
        let cases: [(&[&str], &str); 3] = [
            (&[], "Give an action: install or uninstall."),
            (
                &["remove"],
                "Unknown action 'remove'. Use install or uninstall.",
            ),
            (
                &["uninstall", "--exe", "x"],
                "--exe only applies to install.",
            ),
        ];
        for (argv, message) in cases {
            let (code, out, err) = world.run(argv, &host);
            assert_eq!(code, 2, "{argv:?}");
            assert_eq!(out, "");
            assert_eq!(
                err,
                format!("morphring: {message}\nRun 'morphring help integrate' for usage.\n")
            );
        }
        // Case does not matter, as with help. A dry run, because on Windows a real
        // install would scribble our verb into the registry of whoever runs the tests.
        assert_eq!(world.run(&["INSTALL", "--dry-run"], &host).0, 0);
    }

    #[test]
    fn find_app_prefers_the_folder_of_this_binary_then_path() {
        let world = World::new();
        let bin = world.dir.path().join("bin");
        let elsewhere = world.dir.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();

        // Beside the current binary.
        assert_eq!(
            find_app(Some(&bin.join("morphring")), None),
            Some(world.app())
        );
        // Not beside it, so PATH; relative PATH entries are skipped.
        let path = std::env::join_paths([PathBuf::from("bin"), bin.clone()]).unwrap();
        assert_eq!(
            find_app(Some(&elsewhere.join("morphring")), Some(&path)),
            Some(world.app())
        );
        assert_eq!(find_app(Some(&elsewhere.join("morphring")), None), None);
        // A folder with the app's name is not the app.
        fs::create_dir_all(elsewhere.join(APP_BINARY)).unwrap();
        assert_eq!(find_app(Some(&elsewhere.join("morphring")), None), None);
    }

    #[test]
    fn lists_read_like_english() {
        assert_eq!(english_list(&[]), "");
        assert_eq!(english_list(&["Finder"]), "Finder");
        assert_eq!(
            english_list(&["Explorer", "Send To"]),
            "Explorer and Send To"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_lists_every_file_manager() {
        assert_eq!(
            english_list(&integration::surfaces()),
            "Dolphin, Nautilus, Nemo and Thunar"
        );
    }
}
