//! Unpacking archives into a folder: ZIP, TAR and TAR.GZ, in plain Rust.
//!
//! A port of Filee's `ArchiveConverter.ExtractToFolderAsync` with the 7-Zip
//! call swapped for our own loop per format. Every entry goes through the
//! checks in [`super::safety`]: encrypted ZIP entries refuse the whole
//! archive, links are skipped, unsafe names refuse the whole archive, sizes
//! are enforced while copying, and the entry count and free disk space are
//! capped. Archive to archive (zip -> tgz) is P2-35's business: it routes
//! through `folder` and the packer's folder edges.
//!
//! The dance, in order: allocate the output folder, unpack into a hidden
//! staging folder next to it (same drive, so the last step is a rename),
//! peel off a single top folder, then rename or merge into place. The staging
//! folder is a `TempDir`, so every early return cleans up after itself.

use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use flate2::read::MultiGzDecoder;
use morphring_core::{CancelToken, Edge, EngineError, OutputAllocator, Step};

use super::safety::{
    MAX_ENTRIES, content_root, copy_limited, damaged_message, encrypted_message, ensure_room,
    remove_links, resolve_entry_path, too_many_entries_message, unsafe_message,
};

/// The formats unpacked here. Everything else Filee reads (7z, rar, iso, ...)
/// needs 7-Zip and waits for v0.3.
const SOURCES: [&str; 3] = ["zip", "tar", "tgz"];

/// The edges this part handles: each source to `folder`, at the default cost.
pub(crate) fn edges() -> Vec<Edge> {
    SOURCES
        .iter()
        .map(|from| Edge::with_cost(from, "folder", Edge::DEFAULT_COST))
        .collect()
}

/// Converts `step`, which is one of [`edges`].
pub(crate) fn convert(
    step: &Step,
    out: &mut dyn OutputAllocator,
    progress: &dyn Fn(f32),
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    extract(step, out, progress, cancel, MAX_ENTRIES)
}

/// [`convert`] with the entry cap as a parameter, so a test can trip it
/// without building a million-entry archive (the CI runners thank us).
fn extract(
    step: &Step,
    out: &mut dyn OutputAllocator,
    progress: &dyn Fn(f32),
    cancel: &CancelToken,
    max_entries: usize,
) -> Result<Vec<PathBuf>, EngineError> {
    let kind = match step.from.as_str() {
        "zip" => Kind::Zip,
        "tar" => Kind::Tar,
        "tgz" => Kind::Tgz,
        other => {
            return Err(EngineError::Failed(format!(
                "Built-in archives cannot unpack {other}"
            )));
        }
    };
    if cancel.is_cancelled() {
        return Err(EngineError::Cancelled);
    }

    // An empty extension asks for a folder; None means the skip policy said no.
    let Some(output) = out.allocate("", "") else {
        return Ok(Vec::new());
    };
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&parent)?;
    if output.symlink_metadata().is_ok_and(|m| !m.is_dir()) {
        return Err(in_the_way(&output));
    }

    let ctx = Context {
        archive: archive_name(&step.input),
        place: parent.clone(),
        progress,
        cancel,
        max_entries,
    };
    progress(0.0);

    // Hidden-ish staging folder next to the output. The dot hides it on Unix;
    // on Windows it shows up in Explorer for a moment, which is a price worth
    // paying for not calling SetFileAttributes on a folder that lives for
    // a few seconds.
    let staging = tempfile::Builder::new()
        .prefix(&format!(".{}.morphring-", file_name(&output)))
        .tempdir_in(&parent)?;

    match kind {
        Kind::Zip => unzip(&step.input, staging.path(), &ctx)?,
        Kind::Tar => untar(open(&step.input)?, staging.path(), &ctx)?,
        Kind::Tgz => untar(
            MultiGzDecoder::new(BufReader::new(open(&step.input)?)),
            staging.path(),
            &ctx,
        )?,
    }

    // We never create links, but this costs one directory walk and turns
    // "we never" into "there are none".
    remove_links(staging.path())?;
    if cancel.is_cancelled() {
        return Err(EngineError::Cancelled);
    }
    let root = content_root(staging.path())?;
    move_folder(&root, &output)?;
    progress(1.0);
    // staging drops here and takes whatever is left (an empty husk, at most).
    Ok(vec![output])
}

#[derive(Clone, Copy)]
enum Kind {
    Zip,
    Tar,
    Tgz,
}

/// What every entry loop needs to know besides the entries.
struct Context<'a> {
    /// The archive's file name, for messages.
    archive: String,
    /// The folder the output lands in, for the disk check and its message.
    place: PathBuf,
    progress: &'a dyn Fn(f32),
    cancel: &'a CancelToken,
    max_entries: usize,
}

impl Context<'_> {
    fn damaged(&self, detail: impl std::fmt::Display) -> EngineError {
        EngineError::Failed(damaged_message(&self.archive, &detail.to_string()))
    }

    fn unsafe_entry(&self, entry: &str) -> EngineError {
        EngineError::Failed(unsafe_message(&self.archive, entry))
    }

    fn too_many(&self) -> EngineError {
        EngineError::Failed(too_many_entries_message(&self.archive))
    }

    fn check_cancel(&self) -> Result<(), EngineError> {
        if self.cancel.is_cancelled() {
            Err(EngineError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn ensure_room(&self, bytes: u64) -> Result<(), EngineError> {
        ensure_room(bytes, &self.place)
            .map_err(|no_room| EngineError::Failed(no_room.message(&self.archive, &self.place)))
    }

    /// Turns a copy failure into a message: a lying size is damage, anything
    /// else (disk full, permissions) is an honest I/O error.
    fn copy_error(&self, err: io::Error) -> EngineError {
        if err.kind() == io::ErrorKind::InvalidData {
            self.damaged(err)
        } else {
            EngineError::Io(err)
        }
    }
}

/// Unpacks every entry of the ZIP at `input` into `dest`.
fn unzip(input: &Path, dest: &Path, ctx: &Context<'_>) -> Result<(), EngineError> {
    let mut zip = zip::ZipArchive::new(BufReader::new(open(input)?)).map_err(|e| ctx.damaged(e))?;
    let count = zip.len();
    if count > ctx.max_entries {
        return Err(ctx.too_many());
    }

    // ZIP has a central directory, so everything can be checked before a
    // single byte is written: encryption, names and the total size. A
    // hostile archive is refused whole rather than half unpacked.
    let mut total: u64 = 0;
    for index in 0..count {
        ctx.check_cancel()?;
        let entry = zip.by_index_raw(index).map_err(|e| ctx.damaged(e))?;
        if entry.encrypted() {
            return Err(EngineError::Failed(encrypted_message(&ctx.archive)));
        }
        if entry.is_symlink() {
            continue;
        }
        let name = entry_name(&entry);
        if resolve_entry_path(dest, &name).is_none() && !is_blank(&name) {
            return Err(ctx.unsafe_entry(&name));
        }
        total = total.saturating_add(entry.size());
    }
    ctx.ensure_room(total)?;

    for index in 0..count {
        ctx.check_cancel()?;
        let mut entry = zip.by_index(index).map_err(|e| ctx.damaged(e))?;
        if entry.is_symlink() {
            // A link would let a later entry write through it to anywhere.
            // Filee deletes links after the fact; we just never make them.
            continue;
        }
        let name = entry_name(&entry);
        let Some(path) = resolve_entry_path(dest, &name) else {
            // Only a blank name gets here (".", "/" is refused above): nothing
            // to write, nothing to worry about.
            continue;
        };
        if entry.is_dir() {
            fs::create_dir_all(&path)?;
        } else {
            let declared = entry.size();
            write_file(&path, &mut entry, declared, ctx)?;
        }
        (ctx.progress)(0.02 + 0.93 * (index + 1) as f32 / count.max(1) as f32);
    }
    Ok(())
}

/// The entry's name as written in the archive, lossy where it is not UTF-8.
fn entry_name<R: Read>(entry: &zip::read::ZipFile<'_, R>) -> String {
    match entry.name() {
        Ok(name) => name.into_owned(),
        Err(_) => String::from_utf8_lossy(entry.name_raw()).into_owned(),
    }
}

/// True when a name has no segments at all ("", ".", "./"), which resolves
/// to nothing without being an attack.
fn is_blank(name: &str) -> bool {
    !super::safety::is_unsafe_path(name)
        && name
            .split(['/', '\\'])
            .all(|part| part.is_empty() || part == ".")
}

/// Unpacks every entry of the TAR stream `reader` into `dest`.
///
/// TAR has no index, so names are checked as they stream past: an unsafe one
/// stops the job and the staging folder (with the half we did unpack) is
/// thrown away, so nothing reaches the output either way. Free space is
/// checked per entry for the same reason, and progress follows the
/// compressed bytes read, since the entry count is unknown until the end.
fn untar<R: Read>(reader: R, dest: &Path, ctx: &Context<'_>) -> Result<(), EngineError> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive.entries().map_err(|e| ctx.damaged(e))?;
    let mut count: usize = 0;
    let mut written: u64 = 0;
    for entry in entries {
        ctx.check_cancel()?;
        let mut entry = entry.map_err(|e| ctx.damaged(e))?;
        count += 1;
        if count > ctx.max_entries {
            return Err(ctx.too_many());
        }

        let kind = entry.header().entry_type();
        let is_dir = kind.is_dir();
        let is_file = kind.is_file() || kind.is_contiguous() || kind.is_gnu_sparse();
        if !is_dir && !is_file {
            // Symlinks, hard links, devices, FIFOs and global PAX headers: none
            // of them is a file we want, and the first two are how archives
            // escape their folder.
            continue;
        }

        let name = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
        let Some(path) = resolve_entry_path(dest, &name) else {
            if is_blank(&name) {
                continue;
            }
            return Err(ctx.unsafe_entry(&name));
        };

        if is_dir {
            fs::create_dir_all(&path)?;
        } else {
            let declared = entry.header().size().map_err(|e| ctx.damaged(e))?;
            written = written.saturating_add(declared);
            ctx.ensure_room(declared)?;
            write_file(&path, &mut entry, declared, ctx)?;
        }
        // Without a total, ease towards 0.95 instead of pretending to know.
        let done = written as f32 / (written as f32 + 64.0 * 1024.0 * 1024.0);
        (ctx.progress)(0.02 + 0.93 * done.max(count as f32 / (count as f32 + 1000.0)));
    }
    Ok(())
}

/// Writes one entry to `path`, at most `declared` bytes of it.
fn write_file(
    path: &Path,
    reader: &mut dyn Read,
    declared: u64,
    ctx: &Context<'_>,
) -> Result<(), EngineError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.is_dir() {
        // "a/" then a file "a": the folder got there first, and a folder is
        // a lot harder to explain away than a skipped duplicate.
        return Ok(());
    }
    let mut file = File::create(path)?;
    copy_limited(reader, &mut file, declared).map_err(|e| ctx.copy_error(e))?;
    Ok(())
}

/// Moves the unpacked `from` to `to`: a rename when `to` is free, a merge
/// when a folder is already there (the overwrite policy let it through).
fn move_folder(from: &Path, to: &Path) -> Result<(), EngineError> {
    match to.symlink_metadata() {
        Err(_) => match fs::rename(from, to) {
            Ok(()) => Ok(()),
            // Staging sits next to the output, so this is rare: a folder
            // that appeared in between, or a filesystem with opinions.
            Err(_) => {
                fs::create_dir_all(to)?;
                merge_into(from, to)
            }
        },
        Ok(meta) if meta.file_type().is_symlink() => Err(EngineError::Failed(format!(
            "{} is a link; MorphRing does not unpack through links.",
            to.display()
        ))),
        Ok(meta) if meta.is_dir() => merge_into(from, to),
        Ok(_) => Err(in_the_way(to)),
    }
}

/// Moves everything inside `from` into the existing folder `to`, replacing
/// files with the same name and merging folders.
fn merge_into(from: &Path, to: &Path) -> Result<(), EngineError> {
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let existing = target.symlink_metadata().ok();
        if entry.file_type()?.is_dir() {
            match existing {
                Some(meta) if meta.file_type().is_symlink() => {
                    return Err(EngineError::Failed(format!(
                        "{} is a link; MorphRing does not unpack through links.",
                        target.display()
                    )));
                }
                Some(meta) if meta.is_dir() => merge_into(&source, &target)?,
                Some(_) => {
                    // A file in the way of a folder loses, as in Filee.
                    fs::remove_file(&target)?;
                    fs::rename(&source, &target)?;
                }
                None => fs::rename(&source, &target)?,
            }
        } else {
            match existing {
                Some(meta) if meta.is_dir() => {
                    return Err(in_the_way(&target));
                }
                Some(_) => {
                    // Windows' rename replaces files but not read-only ones,
                    // and a link should be replaced, not written through.
                    fs::remove_file(&target)?;
                    fs::rename(&source, &target)?;
                }
                None => fs::rename(&source, &target)?,
            }
        }
    }
    Ok(())
}

/// Filee's message for a file sitting where the folder should go.
fn in_the_way(path: &Path) -> EngineError {
    EngineError::Failed(format!(
        "A file named {} is in the way of the extracted folder.",
        file_name(path)
    ))
}

fn open(path: &Path) -> Result<File, EngineError> {
    Ok(File::open(path)?)
}

fn archive_name(path: &Path) -> String {
    file_name(path)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use morphring_core::{ConvertOptions, FormatId};
    use std::io::Write;
    use tempfile::TempDir;

    struct Folder(PathBuf);

    impl OutputAllocator for Folder {
        fn allocate(&mut self, ext: &str, _suffix: &str) -> Option<PathBuf> {
            assert_eq!(ext, "", "a folder is asked for with an empty extension");
            Some(self.0.clone())
        }
    }

    fn zip_with(dir: &Path, names: &[&str]) -> PathBuf {
        let path = dir.join("many.zip");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for name in names {
            zip.start_file(*name, options).unwrap();
            zip.write_all(b"x").unwrap();
        }
        zip.finish().unwrap();
        path
    }

    fn tar_with(dir: &Path, names: &[&str]) -> PathBuf {
        let path = dir.join("many.tar");
        let mut tar = tar::Builder::new(File::create(&path).unwrap());
        for name in names {
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, &b"x"[..]).unwrap();
        }
        tar.finish().unwrap();
        path
    }

    fn run(input: PathBuf, from: &'static str, out: &Path, cap: usize) -> Result<(), EngineError> {
        let step = Step {
            input,
            from: FormatId::new(from),
            to: FormatId::new("folder"),
            options: ConvertOptions::default(),
            work_dir: out.parent().unwrap().join("work"),
        };
        extract(
            &step,
            &mut Folder(out.to_path_buf()),
            &|_| {},
            &CancelToken::new(),
            cap,
        )
        .map(|_| ())
    }

    #[test]
    fn too_many_zip_entries_are_refused_before_anything_is_written() {
        let tmp = TempDir::new().unwrap();
        let input = zip_with(tmp.path(), &["a", "b", "c", "d"]);
        let out = tmp.path().join("out");
        let err = run(input, "zip", &out, 3).unwrap_err();
        assert_eq!(
            err.to_string(),
            "many.zip has more than 1,000,000 entries; MorphRing does not unpack it."
        );
        assert!(!out.exists());
    }

    #[test]
    fn too_many_tar_entries_are_refused_and_staging_is_cleaned() {
        let tmp = TempDir::new().unwrap();
        let input = tar_with(tmp.path(), &["a", "b", "c", "d"]);
        let out = tmp.path().join("out");
        let err = run(input, "tar", &out, 3).unwrap_err();
        assert!(err.to_string().contains("more than 1,000,000 entries"));
        assert!(!out.exists());
        let left: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["many.tar"], "the staging folder must be gone");
    }

    #[test]
    fn exactly_the_cap_is_fine() {
        let tmp = TempDir::new().unwrap();
        let input = zip_with(tmp.path(), &["a", "b", "c"]);
        let out = tmp.path().join("out");
        run(input, "zip", &out, 3).unwrap();
        assert!(out.join("c").is_file());
    }

    #[test]
    fn edges_go_from_each_archive_to_folder() {
        let pairs: Vec<String> = edges()
            .iter()
            .map(|e| format!("{}>{}:{}", e.from, e.to, e.cost))
            .collect();
        assert_eq!(pairs, ["zip>folder:10", "tar>folder:10", "tgz>folder:10"]);
    }

    #[test]
    fn blank_names_are_not_attacks() {
        assert!(is_blank(""));
        assert!(is_blank("./"));
        assert!(!is_blank("/"));
        assert!(!is_blank("a"));
    }
}
