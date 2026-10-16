//! `--keep-dates`: give each output the creation and modification times of its
//! source. Ported from Filee's `src/Filee.Core/Conversion/JobQueue.cs` (the
//! `OutputRule.KeepDates` step).
//!
//! Access and modification times go through `filetime`, which handles files and
//! folders on every OS. The creation (birth) time is the fussy one: Linux won't
//! let anyone set it, so we only try on Windows and macOS, through std's
//! `FileTimesExt::set_created`. Every failure is a warning, never a failed job,
//! because nobody wants their converted PDF thrown away over a timestamp.

use std::path::{Path, PathBuf};

use filetime::FileTime;

/// Copies the creation and modification times of `source` onto every path in
/// `outputs`. Best effort: a time that can't be copied is skipped.
pub fn copy_dates(source: &Path, outputs: &[PathBuf]) {
    let meta = match std::fs::metadata(source) {
        Ok(meta) => meta,
        Err(err) => {
            tracing::warn!("could not read the dates of {}: {err}", source.display());
            return;
        }
    };
    let atime = FileTime::from_last_access_time(&meta);
    let mtime = FileTime::from_last_modification_time(&meta);

    for out in outputs {
        // Creation time first: on some platforms opening the file to set it
        // could, in theory, nudge the other times, so filetime gets the last word.
        #[cfg(any(windows, target_os = "macos"))]
        set_created(&meta, out);

        if let Err(err) = filetime::set_file_times(out, atime, mtime) {
            tracing::warn!("could not copy the dates onto {}: {err}", out.display());
        }
    }
}

/// Copies the source's creation time onto `out` (Windows and macOS only).
#[cfg(any(windows, target_os = "macos"))]
fn set_created(meta: &std::fs::Metadata, out: &Path) {
    #[cfg(target_os = "macos")]
    use std::os::macos::fs::FileTimesExt;
    #[cfg(windows)]
    use std::os::windows::fs::FileTimesExt;

    let created = match meta.created() {
        Ok(created) => created,
        Err(err) => {
            tracing::warn!("could not read the creation time of the source: {err}");
            return;
        }
    };

    // Windows wants FILE_FLAG_BACKUP_SEMANTICS to open a folder at all, and that
    // is a rabbit hole for another day. Folders keep their own birthday there.
    #[cfg(windows)]
    if out.is_dir() {
        return;
    }

    let file = match open_for_times(out) {
        Ok(file) => file,
        Err(err) => {
            tracing::warn!(
                "could not open {} to set its creation time: {err}",
                out.display()
            );
            return;
        }
    };
    let times = std::fs::FileTimes::new().set_created(created);
    if let Err(err) = file.set_times(times) {
        tracing::warn!(
            "could not set the creation time of {}: {err}",
            out.display()
        );
    }
}

/// Opens `out` with just enough rights to change its times.
#[cfg(windows)]
fn open_for_times(out: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_WRITE_ATTRIBUTES: enough for SetFileTime, and it still works on a
    // read-only file, unlike asking for full write access.
    const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
    std::fs::OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .open(out)
}

/// Opens `out` with just enough rights to change its times.
#[cfg(target_os = "macos")]
fn open_for_times(out: &Path) -> std::io::Result<std::fs::File> {
    // A read-only descriptor is all fsetattrlist needs, and it opens folders too.
    std::fs::File::open(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A date far enough in the past that no freshly made temp file has it by accident.
    fn old_time() -> FileTime {
        FileTime::from_unix_time(1_000_000_000, 0)
    }

    #[test]
    fn copies_mtime_onto_file_and_folder_outputs() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source.txt");
        std::fs::write(&source, b"hello").unwrap();
        filetime::set_file_times(&source, old_time(), old_time()).unwrap();

        let file_out = dir.path().join("out.txt");
        std::fs::write(&file_out, b"converted").unwrap();
        let folder_out = dir.path().join("pages");
        std::fs::create_dir(&folder_out).unwrap();

        copy_dates(&source, &[file_out.clone(), folder_out.clone()]);

        for out in [&file_out, &folder_out] {
            let meta = std::fs::metadata(out).unwrap();
            assert_eq!(
                FileTime::from_last_modification_time(&meta),
                old_time(),
                "mtime of {}",
                out.display()
            );
        }
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn copies_creation_time_onto_file_outputs() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source.txt");
        std::fs::write(&source, b"hello").unwrap();
        // Let the clock move on so the output is born visibly later.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"converted").unwrap();

        copy_dates(&source, std::slice::from_ref(&out));

        let want = std::fs::metadata(&source).unwrap().created().unwrap();
        let got = std::fs::metadata(&out).unwrap().created().unwrap();
        assert_eq!(got, want);
    }

    #[test]
    fn missing_output_does_not_panic() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source.txt");
        std::fs::write(&source, b"hello").unwrap();
        let present = dir.path().join("present.txt");
        std::fs::write(&present, b"x").unwrap();
        filetime::set_file_times(&source, old_time(), old_time()).unwrap();

        // The missing one is skipped with a warning; the next still gets its dates.
        copy_dates(&source, &[dir.path().join("gone.txt"), present.clone()]);

        let meta = std::fs::metadata(&present).unwrap();
        assert_eq!(FileTime::from_last_modification_time(&meta), old_time());
    }

    #[test]
    fn missing_source_does_not_panic() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("out.txt");
        std::fs::write(&out, b"x").unwrap();
        copy_dates(&dir.path().join("nope.txt"), &[out]);
    }
}
