//! Path checks for archive entries: no absolute paths, no `..`, no links that
//! point outside the destination folder (zip slip and its cousins), plus the
//! entry cap, the disk check and the size-limited copy.
//!
//! A port of Filee's `ArchiveSafety.cs` (and the two helpers from
//! `ArchiveFiles.cs` that belong with it). Everything here is a plain function
//! so the zip and tar loops can share it, and so the tests can poke at it
//! without building a single hostile archive.

// The extract loop (P2-34) is the first real caller; until then the compiler
// would like us to know that nobody loves these functions yet. We know.
#![cfg_attr(
    not(test),
    allow(dead_code, reason = "the archive extract loop (P2-34) calls these")
)]

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// More entries than this are refused: real archives stay far below it, entry
/// bombs do not.
pub(crate) const MAX_ENTRIES: usize = 1_000_000;

/// Space that must stay free on the drive after unpacking (64 MiB).
pub(crate) const RESERVE: u64 = 64 * 1024 * 1024;

/// Characters no segment may contain. These are the Windows rules, applied on
/// every OS so an archive unpacks to the same names everywhere (and a folder
/// unpacked on Linux can still be copied to a Windows share later).
const INVALID_NAME_CHARS: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Windows device names. A file called `CON.txt` is not a file on Windows, it
/// is the console wearing a fake moustache.
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// True if an entry path is absolute, starts with a drive letter, or climbs
/// out of the target with a `..` segment. An empty path is safe (and useless).
pub(crate) fn is_unsafe_path(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    if bytes[0] == b'/' || bytes[0] == b'\\' {
        return true;
    }
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return true;
    }
    raw.split(['/', '\\']).any(|part| part == "..")
}

/// Turns an entry name into a path inside `root`: separators are normalised,
/// empty and `.` segments dropped, characters Windows does not allow replaced,
/// device names prefixed with `_`. `None` when the name is unsafe, has no
/// segments left, or would somehow land outside `root`.
pub(crate) fn resolve_entry_path(root: &Path, raw: &str) -> Option<PathBuf> {
    if is_unsafe_path(raw) {
        return None;
    }
    let parts: Vec<String> = raw
        .split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .map(sanitize_segment)
        .collect();
    if parts.is_empty() {
        return None;
    }

    let mut full = root.to_path_buf();
    full.extend(&parts);
    // Belt and braces: after sanitising there is no `..` or separator left, so
    // this cannot fail. Filee checks anyway, and so do we, because "cannot
    // happen" is how zip slip CVEs start their careers.
    let inside = full.starts_with(root)
        && full.components().count() > root.components().count()
        && full
            .components()
            .skip(root.components().count())
            .all(|c| matches!(c, std::path::Component::Normal(_)));
    inside.then_some(full)
}

/// One path segment made valid on Windows: invalid and control characters
/// become `_`, trailing dots and spaces go, an empty result becomes `_`, and a
/// device name (the part before the first dot) gets a `_` prefix.
fn sanitize_segment(segment: &str) -> String {
    let replaced: String = segment
        .chars()
        .map(|c| {
            if INVALID_NAME_CHARS.contains(&c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = replaced.trim_end_matches(['.', ' ']);
    if name.is_empty() {
        return "_".to_owned();
    }
    let stem = name.split('.').next().unwrap_or(name);
    // Filee compares the stem as is; Windows also ignores trailing spaces, so
    // `CON .txt` is the console too. One extra trim costs nothing.
    let stem = stem.trim_end_matches(' ');
    if RESERVED_NAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(stem))
    {
        format!("_{name}")
    } else {
        name.to_owned()
    }
}

/// The archive would not fit: `needed` bytes declared, `free` on the drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NoRoom {
    pub needed: u64,
    pub free: u64,
}

impl NoRoom {
    /// Filee's message, for an archive called `archive` unpacking onto `place`.
    pub(crate) fn message(&self, archive: &str, place: &Path) -> String {
        format!(
            "{archive} unpacks to {}, but only {} is free on {}.",
            format_size(self.needed),
            format_size(self.free),
            place.display()
        )
    }
}

/// Refuses `total_declared` bytes when they would leave less than [`RESERVE`]
/// free. Unknown free space (`None`) and an empty archive pass, as in Filee:
/// if the disk does fill up, the write itself fails anyway.
pub(crate) fn check_disk(total_declared: u64, free: Option<u64>) -> Result<(), NoRoom> {
    let Some(free) = free else {
        return Ok(());
    };
    if total_declared == 0 {
        return Ok(());
    }
    if total_declared > free.saturating_sub(RESERVE) {
        return Err(NoRoom {
            needed: total_declared,
            free,
        });
    }
    Ok(())
}

/// [`check_disk`] against the drive holding `dir`.
pub(crate) fn ensure_room(total_declared: u64, dir: &Path) -> Result<(), NoRoom> {
    check_disk(total_declared, free_space(dir))
}

/// Bytes free for us on the drive holding `dir`, or `None` when the OS will
/// not say. A folder that does not exist yet is measured at its nearest
/// existing ancestor, since that is where it will be created.
pub(crate) fn free_space(dir: &Path) -> Option<u64> {
    let mut probe = dir;
    loop {
        if probe.exists() {
            return free_space_at(probe);
        }
        probe = probe.parent()?;
    }
}

#[cfg(unix)]
#[allow(
    unsafe_code,
    reason = "statvfs is the only way to ask Unix for free space; the pointers are a live CString and a local struct"
)]
fn free_space_at(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats = MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: c_path is a valid NUL-terminated string that outlives the call,
    // and stats points at writable memory big enough for one statvfs.
    let stats = unsafe {
        if libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) != 0 {
            return None;
        }
        // SAFETY: statvfs returned 0, so it filled the whole struct.
        stats.assume_init()
    };
    // The field types differ per Unix (u64 on Linux, u32 here and there on
    // macOS), so the casts are only "unnecessary" on the machine you are on.
    #[allow(clippy::unnecessary_cast, reason = "field widths vary between Unixes")]
    let (avail, frsize, bsize) = (
        stats.f_bavail as u64,
        stats.f_frsize as u64,
        stats.f_bsize as u64,
    );
    let block = if frsize != 0 { frsize } else { bsize };
    avail.checked_mul(block)
}

#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "GetDiskFreeSpaceExW is the only way to ask Windows for free space; the pointers are a live wide string and a local u64"
)]
fn free_space_at(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut available: u64 = 0;
    // SAFETY: wide is NUL-terminated and outlives the call; available is a
    // writable u64; the two totals we do not want may be null per the docs.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(available)
}

#[cfg(not(any(unix, windows)))]
fn free_space_at(_path: &Path) -> Option<u64> {
    // Some exotic target: no idea, so no check, same as a network share.
    None
}

/// Copies `reader` into `writer` and fails with `InvalidData` once more than
/// `declared` bytes arrive (lying headers, decompression bombs). Returns the
/// number of bytes copied. Nothing past the limit is written.
pub(crate) fn copy_limited<R: Read + ?Sized, W: Write + ?Sized>(
    reader: &mut R,
    writer: &mut W,
    declared: u64,
) -> io::Result<u64> {
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        total = total.saturating_add(read as u64);
        if total > declared {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the data unpacks to more than the archive declares",
            ));
        }
        writer.write_all(&buffer[..read])?;
    }
}

/// Deletes every symbolic link and junction below `root` without following
/// it, so nothing packed or moved later can reach outside the unpacked folder.
/// The links go; whatever they pointed at stays exactly where it was.
pub(crate) fn remove_links(root: &Path) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        // DirEntry::file_type does not follow links, which is the whole point.
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            // A file link goes with remove_file; a directory link or junction
            // on Windows wants remove_dir, which removes the link, not the
            // folder behind it.
            fs::remove_file(&path).or_else(|_| fs::remove_dir(&path))?;
        } else if kind.is_dir() {
            remove_links(&path)?;
        }
    }
    Ok(())
}

/// The folder whose contents are the result: when `staging` holds exactly one
/// entry and it is a folder, that folder (so `photos.zip` holding `photos/`
/// does not unpack to `photos/photos/`), else `staging` itself.
pub(crate) fn content_root(staging: &Path) -> io::Result<PathBuf> {
    let mut entries = fs::read_dir(staging)?;
    let Some(first) = entries.next().transpose()? else {
        return Ok(staging.to_path_buf());
    };
    if entries.next().is_some() {
        return Ok(staging.to_path_buf());
    }
    if first.file_type()?.is_dir() {
        Ok(first.path())
    } else {
        Ok(staging.to_path_buf())
    }
}

/// "is password-protected" message for `archive`.
pub(crate) fn encrypted_message(archive: &str) -> String {
    format!("{archive} is password-protected. MorphRing cannot unpack encrypted archives.")
}

/// "points outside the target folder" message for `entry` in `archive`.
pub(crate) fn unsafe_message(archive: &str, entry: &str) -> String {
    format!("{archive} was not unpacked: the entry \"{entry}\" points outside the target folder.")
}

/// "damaged or incomplete" message for `archive`, with a short `detail`.
pub(crate) fn damaged_message(archive: &str, detail: &str) -> String {
    format!("{archive} is damaged or incomplete ({detail}).")
}

/// "too many entries" message for `archive`.
pub(crate) fn too_many_entries_message(archive: &str) -> String {
    format!("{archive} has more than 1,000,000 entries; MorphRing does not unpack it.")
}

/// A "1.4 GB" style size for messages: 1024-based, one decimal below 10.
pub(crate) fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else if value < 10.0 {
        // Round half away from zero like .NET's "0.0", not half to even.
        format!("{:.1} {}", (value * 10.0).round() / 10.0, UNITS[unit])
    } else {
        format!("{} {}", value.round(), UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- is_unsafe_path: Filee's theory, case for case ----

    #[test]
    fn unsafe_entry_paths_are_recognised() {
        let cases = [
            ("../evil.txt", true),
            ("a/../../evil.txt", true),
            ("..\\evil.txt", true),
            ("/etc/passwd", true),
            ("\\abs.txt", true),
            ("C:\\drive.txt", true),
            ("a/b/c.txt", false),
            ("한글/파일..txt", false),
        ];
        for (path, expected) in cases {
            assert_eq!(is_unsafe_path(path), expected, "{path}");
        }
    }

    #[test]
    fn a_dotdot_in_the_middle_is_rejected_even_if_it_stays_inside() {
        // zip's enclosed_name lets this through; Filee does not, nor do we.
        assert!(is_unsafe_path("foo/../bar"));
        assert!(resolve_entry_path(Path::new("root"), "foo/../bar").is_none());
    }

    #[test]
    fn drive_letters_are_rejected_in_either_case_and_slash() {
        assert!(is_unsafe_path("C:\\x"));
        assert!(is_unsafe_path("c:/x"));
        assert!(is_unsafe_path("z:"));
        assert!(resolve_entry_path(Path::new("root"), "C:\\x").is_none());
        // A colon later on is just an ugly name, not a drive.
        assert!(!is_unsafe_path("1:x"));
        assert!(!is_unsafe_path("ab:c"));
    }

    #[test]
    fn an_empty_path_is_safe_but_resolves_to_nothing() {
        assert!(!is_unsafe_path(""));
        assert!(resolve_entry_path(Path::new("root"), "").is_none());
        assert!(resolve_entry_path(Path::new("root"), "././/").is_none());
    }

    #[test]
    fn dots_that_are_not_dotdot_are_fine() {
        assert!(!is_unsafe_path("...txt"));
        assert!(!is_unsafe_path("a/.../b"));
        assert!(!is_unsafe_path(".hidden"));
    }

    // ---- resolve_entry_path: Filee's fact, plus a few more ----

    #[test]
    fn entry_names_are_made_valid_windows_names_inside_the_root() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        assert_eq!(
            resolve_entry_path(root, "폴더\\파일.txt"),
            Some(root.join("폴더").join("파일.txt"))
        );
        assert_eq!(
            resolve_entry_path(root, "a|b/CON.txt"),
            Some(root.join("a_b").join("_CON.txt"))
        );
        // No NTFS alternate data streams sneaking in through a colon.
        assert_eq!(
            resolve_entry_path(root, "file.txt:stream"),
            Some(root.join("file.txt_stream"))
        );
        assert_eq!(
            resolve_entry_path(root, "./dir//file?"),
            Some(root.join("dir").join("file_"))
        );
    }

    #[test]
    fn device_names_are_renamed_whatever_their_case_or_extension() {
        let root = Path::new("root");
        assert_eq!(resolve_entry_path(root, "CON"), Some(root.join("_CON")));
        assert_eq!(resolve_entry_path(root, "con"), Some(root.join("_con")));
        assert_eq!(
            resolve_entry_path(root, "Lpt9.tar.gz"),
            Some(root.join("_Lpt9.tar.gz"))
        );
        assert_eq!(
            resolve_entry_path(root, "nul/x"),
            Some(root.join("_nul").join("x"))
        );
        assert_eq!(
            resolve_entry_path(root, "CON .txt"),
            Some(root.join("_CON .txt"))
        );
        // Lookalikes stay as they are.
        assert_eq!(
            resolve_entry_path(root, "CONSOLE"),
            Some(root.join("CONSOLE"))
        );
        assert_eq!(resolve_entry_path(root, "COM10"), Some(root.join("COM10")));
    }

    #[test]
    fn trailing_dots_spaces_and_control_characters_are_cleaned() {
        let root = Path::new("root");
        assert_eq!(
            resolve_entry_path(root, "name. . "),
            Some(root.join("name"))
        );
        assert_eq!(resolve_entry_path(root, "..."), Some(root.join("_")));
        assert_eq!(
            resolve_entry_path(root, "a\u{1}b\tc"),
            Some(root.join("a_b_c"))
        );
        assert_eq!(resolve_entry_path(root, "<>\"*"), Some(root.join("____")));
    }

    #[test]
    fn every_resolved_path_stays_under_the_root() {
        let root = Path::new("some").join("root");
        for raw in [
            "a", "a/b/c", "x\\y", "./a/./b", "CON/AUX", "ab:c/d|e", "....",
        ] {
            let full = resolve_entry_path(&root, raw).unwrap();
            assert!(full.starts_with(&root), "{raw} -> {}", full.display());
            assert_ne!(full, root);
        }
    }

    // ---- disk check ----

    #[test]
    fn the_disk_check_keeps_a_64_mib_reserve() {
        let free = 1024 * 1024 * 1024;
        assert_eq!(check_disk(free - RESERVE, Some(free)), Ok(()));
        assert_eq!(
            check_disk(free - RESERVE + 1, Some(free)),
            Err(NoRoom {
                needed: free - RESERVE + 1,
                free
            })
        );
    }

    #[test]
    fn a_nearly_full_disk_refuses_even_a_small_archive() {
        assert!(check_disk(1, Some(RESERVE)).is_err());
        assert!(check_disk(1, Some(0)).is_err());
    }

    #[test]
    fn unknown_free_space_or_an_empty_archive_skips_the_check() {
        assert_eq!(check_disk(u64::MAX, None), Ok(()));
        assert_eq!(check_disk(0, Some(0)), Ok(()));
    }

    #[test]
    fn the_no_room_message_reads_like_filee() {
        let err = NoRoom {
            needed: 3 * 1024 * 1024 * 1024 / 2,
            free: 200 * 1024 * 1024,
        };
        assert_eq!(
            err.message("big.zip", Path::new("/data")),
            format!(
                "big.zip unpacks to 1.5 GB, but only 200 MB is free on {}.",
                Path::new("/data").display()
            )
        );
    }

    #[test]
    fn the_os_reports_free_space_for_a_real_folder() {
        // The real FFI call: statvfs on Unix, GetDiskFreeSpaceExW on Windows.
        let dir = tempfile::tempdir().unwrap();
        let free = free_space(dir.path());
        assert!(free.is_some_and(|f| f > 0), "free space: {free:?}");
        assert_eq!(ensure_room(1, dir.path()), Ok(()));
        assert!(ensure_room(u64::MAX, dir.path()).is_err());
    }

    #[test]
    fn a_folder_that_does_not_exist_yet_is_measured_at_its_parent() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not").join("yet");
        assert!(free_space(&missing).is_some());
    }

    // ---- copy_limited ----

    #[test]
    fn copy_limited_copies_up_to_the_declared_size() {
        let data = vec![7u8; 200_000];
        let mut out = Vec::new();
        let copied = copy_limited(&mut data.as_slice(), &mut out, 200_000).unwrap();
        assert_eq!(copied, 200_000);
        assert_eq!(out, data);
    }

    #[test]
    fn a_lying_size_is_caught() {
        let data = vec![1u8; 100_001];
        let mut out = Vec::new();
        let err = copy_limited(&mut data.as_slice(), &mut out, 100_000).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            err.to_string(),
            "the data unpacks to more than the archive declares"
        );
        assert!(out.len() as u64 <= 100_000, "wrote {} bytes", out.len());
    }

    #[test]
    fn a_short_stream_is_not_this_functions_problem() {
        // Fewer bytes than declared is for the CRC/length check to judge.
        let mut out = Vec::new();
        assert_eq!(copy_limited(&mut &b"abc"[..], &mut out, 10).unwrap(), 3);
        assert_eq!(copy_limited(&mut &b""[..], &mut out, 0).unwrap(), 0);
        assert!(copy_limited(&mut &b"x"[..], &mut out, 0).is_err());
    }

    // ---- remove_links and content_root ----

    /// Makes a link, or says it could not (Windows without the privilege).
    fn try_symlink(target: &Path, link: &Path, dir: bool) -> bool {
        #[cfg(unix)]
        {
            let _ = dir;
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            if dir {
                std::os::windows::fs::symlink_dir(target, link).is_ok()
            } else {
                std::os::windows::fs::symlink_file(target, link).is_ok()
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, link, dir);
            false
        }
    }

    #[test]
    fn links_are_removed_but_their_targets_survive() {
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        fs::write(&secret, "keep me").unwrap();
        let secret_dir = outside.path().join("dir");
        fs::create_dir(&secret_dir).unwrap();
        fs::write(secret_dir.join("inner.txt"), "me too").unwrap();

        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("real.txt"), "real").unwrap();
        let file_link = nested.join("link.txt");
        let dir_link = root.path().join("a").join("dirlink");
        if !try_symlink(&secret, &file_link, false) || !try_symlink(&secret_dir, &dir_link, true) {
            eprintln!("cannot create symlinks here; skipping");
            return;
        }

        remove_links(root.path()).unwrap();

        assert!(fs::symlink_metadata(&file_link).is_err());
        assert!(fs::symlink_metadata(&dir_link).is_err());
        assert!(nested.join("real.txt").is_file());
        assert_eq!(fs::read_to_string(&secret).unwrap(), "keep me");
        assert!(secret_dir.join("inner.txt").is_file());
    }

    #[test]
    fn remove_links_on_a_plain_tree_changes_nothing() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("x/y")).unwrap();
        fs::write(root.path().join("x/y/z.txt"), "z").unwrap();
        remove_links(root.path()).unwrap();
        assert!(root.path().join("x/y/z.txt").is_file());
    }

    #[test]
    fn a_single_top_folder_is_unwrapped() {
        let staging = tempfile::tempdir().unwrap();
        let top = staging.path().join("photos");
        fs::create_dir(&top).unwrap();
        fs::write(top.join("a.jpg"), "a").unwrap();
        assert_eq!(content_root(staging.path()).unwrap(), top);
    }

    #[test]
    fn several_entries_or_a_single_file_stay_as_they_are() {
        let staging = tempfile::tempdir().unwrap();
        fs::write(staging.path().join("only.txt"), "x").unwrap();
        assert_eq!(content_root(staging.path()).unwrap(), staging.path());

        fs::create_dir(staging.path().join("dir")).unwrap();
        assert_eq!(content_root(staging.path()).unwrap(), staging.path());

        let empty = tempfile::tempdir().unwrap();
        assert_eq!(content_root(empty.path()).unwrap(), empty.path());
    }

    // ---- messages ----

    #[test]
    fn messages_follow_filee_word_for_word_apart_from_the_name() {
        assert_eq!(
            encrypted_message("x.zip"),
            "x.zip is password-protected. MorphRing cannot unpack encrypted archives."
        );
        assert_eq!(
            unsafe_message("x.zip", "../evil"),
            "x.zip was not unpacked: the entry \"../evil\" points outside the target folder."
        );
        assert_eq!(
            damaged_message("x.zip", "an entry has no name"),
            "x.zip is damaged or incomplete (an entry has no name)."
        );
        assert_eq!(
            too_many_entries_message("x.zip"),
            "x.zip has more than 1,000,000 entries; MorphRing does not unpack it."
        );
        assert_eq!(MAX_ENTRIES, 1_000_000);
    }

    #[test]
    fn sizes_are_formatted_like_filee() {
        assert_eq!(format_size(0), "0 bytes");
        assert_eq!(format_size(1023), "1023 bytes");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(10 * 1024), "10 KB");
        assert_eq!(format_size(1_503_238_554), "1.4 GB");
        assert_eq!(format_size(64 * 1024 * 1024), "64 MB");
        assert_eq!(format_size(5 * 1024u64.pow(5)), "5120 TB");
    }
}
