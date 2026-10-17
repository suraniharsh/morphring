//! Packing a file or a folder into ZIP, TAR or TAR.GZ, in plain Rust.
//!
//! A port of Filee's `ArchivePacker` for the three formats it writes without
//! 7-Zip. ZIP gets UTF-8 names (the zip crate sets the language-encoding
//! flag, bit 11, whenever a name is not plain ASCII) and DOS dates clamped to
//! 1980..2107 in local time; TAR is ustar with PAX extensions for the names
//! and sizes ustar cannot hold, mode 0644 for files and 0755 for folders;
//! TAR.GZ is that TAR through a gzip encoder at the preset's level.
//!
//! The archive is built under a hidden temporary name next to the output and
//! renamed into place when complete, so a failed or cancelled run never
//! leaves half an archive behind and never eats an existing one.
//!
//! Archive to archive (zip -> tgz) is not special-cased: the planner routes it
//! through `folder` (the extract part) and then here. A folder goes in under
//! its own name, which is exactly what the extract part peels off again, so a
//! `photos.zip` holding `photos/...` comes out as a `photos.tar.gz` holding
//! `photos/...` instead of growing a matryoshka of folders. "Compress into
//! one archive" (zip-all) is v0.3's problem.

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Local, Timelike};
use flate2::Compression;
use flate2::write::GzEncoder;
use morphring_core::{
    ArchiveLevel, CancelToken, Edge, EngineError, OutputAllocator, Step, formats,
};

/// The formats written here; 7z, tbz2, txz and the single-file ones need
/// 7-Zip and wait for v0.3.
const TARGETS: [&str; 3] = ["zip", "tar", "tgz"];

/// The edges this part handles: every format that is not an archive, plus
/// `folder`, to each target, at Filee's default cost.
///
/// Archives themselves get no direct edge on purpose: zip -> tgz goes
/// zip -> folder -> tgz, so the archive is unpacked (safely) and repacked,
/// rather than a ZIP sitting inside a TAR.GZ like a gift wrapped twice.
pub(crate) fn edges() -> Vec<Edge> {
    let sources = formats::all()
        .iter()
        .filter(|f| f.category != formats::Category::Archive)
        .map(|f| f.id.as_str())
        .chain([formats::FOLDER.as_str()]);
    sources
        .flat_map(|from| {
            TARGETS
                .iter()
                .map(move |to| Edge::with_cost(from, to, Edge::DEFAULT_COST))
        })
        .collect()
}

/// Converts `step`, which is one of [`edges`].
pub(crate) fn convert(
    step: &Step,
    out: &mut dyn OutputAllocator,
    progress: &dyn Fn(f32),
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    let kind = match step.to.as_str() {
        "zip" => Kind::Zip,
        "tar" => Kind::Tar,
        "tgz" => Kind::Tgz,
        other => {
            return Err(EngineError::Failed(format!(
                "Built-in archives cannot write {other} archives"
            )));
        }
    };
    if cancel.is_cancelled() {
        return Err(EngineError::Cancelled);
    }

    // Walk first: a missing input should fail before an output is allocated,
    // and the byte total drives the progress bar.
    let files = expand(&step.input)?;
    let total: u64 = files.iter().map(|f| f.len).sum();

    let ext =
        formats::find_by_id(step.to.as_str()).map_or(step.to.as_str(), |f| f.primary_extension());
    let Some(output) = out.allocate(ext, "") else {
        return Ok(Vec::new());
    };
    if output.is_dir() {
        return Err(EngineError::Failed(format!(
            "A folder named {} is in the way of the archive.",
            file_name(&output)
        )));
    }
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&parent)?;

    progress(0.0);
    // Same volume as the output, so the final step is a rename. The temp file
    // deletes itself on every early return, which is the whole point.
    let temp = tempfile::Builder::new()
        .prefix(&format!(".{}.morphring-", file_name(&output)))
        .suffix(".tmp")
        .tempfile_in(&parent)?;

    let mut ctx = Context {
        counter: Counter::new(total, progress),
        cancel,
    };
    let level = step.options.archive.level;
    let written = match kind {
        Kind::Zip => write_zip(&files, temp.as_file(), level, &mut ctx),
        Kind::Tar => write_tar(&files, BufWriter::new(temp.as_file()), &mut ctx)
            .and_then(|mut w| Ok(w.flush()?)),
        Kind::Tgz => {
            let gz = GzEncoder::new(BufWriter::new(temp.as_file()), gzip_level(level));
            write_tar(&files, gz, &mut ctx).and_then(|gz| Ok(gz.finish()?.flush()?))
        }
    };
    if let Err(err) = written {
        // A cancelled read surfaces as some I/O error deep inside a writer;
        // the token knows the real story.
        return Err(if cancel.is_cancelled() {
            EngineError::Cancelled
        } else {
            err
        });
    }
    if cancel.is_cancelled() {
        return Err(EngineError::Cancelled);
    }
    // tempfile hands out 0600 files, sensible for a secret, rude for a zip
    // somebody wants to share. Give it the mode File::create would have.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o666 & !current_umask()))?;
    }
    temp.persist(&output)
        .map_err(|e| EngineError::Io(e.error))?;
    progress(1.0);
    Ok(vec![output])
}

/// The process umask, read without changing it. umask(2) can only be
/// queried by setting it, which races every other thread creating files,
/// so on Linux we ask /proc politely and elsewhere assume the usual 022.
#[cfg(unix)]
fn current_umask() -> u32 {
    const USUAL: u32 = 0o022;
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = fs::read_to_string("/proc/self/status") {
            if let Some(mask) = status
                .lines()
                .find_map(|l| l.strip_prefix("Umask:"))
                .and_then(|v| u32::from_str_radix(v.trim(), 8).ok())
            {
                return mask & 0o777;
            }
        }
    }
    USUAL
}

#[derive(Clone, Copy)]
enum Kind {
    Zip,
    Tar,
    Tgz,
}

/// A file or folder in the order it is written, with its name inside the
/// archive (`/`-separated, no leading or trailing slash).
#[derive(Debug)]
struct PackFile {
    path: PathBuf,
    name: String,
    is_dir: bool,
    len: u64,
    modified: SystemTime,
}

/// Every file and folder of `input`, folders before their content, children
/// sorted by name. The input itself is taken as given (the user picked it, so
/// a link to it is followed); links inside a folder are skipped, never
/// followed, so nothing from outside the folder sneaks into the archive.
fn expand(input: &Path) -> Result<Vec<PackFile>, EngineError> {
    let meta = fs::metadata(input).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            EngineError::Failed(format!("{} was not found.", input.display()))
        } else {
            EngineError::Io(e)
        }
    })?;
    let name = file_name(input);
    let mut files = Vec::new();
    if meta.is_dir() {
        expand_folder(input, name, &meta, &mut files)?;
    } else {
        files.push(PackFile {
            path: input.to_path_buf(),
            name,
            is_dir: false,
            len: meta.len(),
            modified: meta.modified().unwrap_or(UNIX_EPOCH),
        });
    }
    Ok(files)
}

fn expand_folder(
    folder: &Path,
    name: String,
    meta: &fs::Metadata,
    files: &mut Vec<PackFile>,
) -> Result<(), EngineError> {
    files.push(PackFile {
        path: folder.to_path_buf(),
        name: name.clone(),
        is_dir: true,
        len: 0,
        modified: meta.modified().unwrap_or(UNIX_EPOCH),
    });
    let mut children: Vec<fs::DirEntry> = fs::read_dir(folder)?.collect::<Result<_, _>>()?;
    // Ordinal, like Filee: byte order of the names, no locale opinions.
    children.sort_by_key(fs::DirEntry::file_name);
    for child in children {
        // symlink_metadata: a link reports itself instead of its target. On
        // Windows, junctions count as links here too, which is what we want.
        let meta = fs::symlink_metadata(child.path())?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let child_name = format!("{name}/{}", child.file_name().to_string_lossy());
        if meta.is_dir() {
            expand_folder(&child.path(), child_name, &meta, files)?;
        } else if meta.is_file() {
            files.push(PackFile {
                path: child.path(),
                name: child_name,
                is_dir: false,
                len: meta.len(),
                modified: meta.modified().unwrap_or(UNIX_EPOCH),
            });
        }
        // Sockets, FIFOs and devices: not files anyone wants zipped.
    }
    Ok(())
}

/// What every writer needs besides the files.
struct Context<'a> {
    counter: Counter<'a>,
    cancel: &'a CancelToken,
}

impl Context<'_> {
    /// Copies exactly `len` bytes of `path` into `to`, counting and honouring
    /// cancellation as it goes.
    fn copy_file(&mut self, file: &PackFile, to: &mut dyn Write) -> Result<(), EngineError> {
        let mut from = File::open(&file.path)?;
        let copied = self.copy(&mut from, to, file.len)?;
        if copied != file.len {
            return Err(changed(&file.path));
        }
        Ok(())
    }

    /// Copies up to `limit` bytes from `from` to `to` and returns how many.
    fn copy(
        &mut self,
        from: &mut dyn Read,
        to: &mut dyn Write,
        limit: u64,
    ) -> Result<u64, EngineError> {
        let mut buffer = vec![0u8; 64 * 1024];
        let mut done: u64 = 0;
        while done < limit {
            if self.cancel.is_cancelled() {
                return Err(EngineError::Cancelled);
            }
            let want = usize::try_from(limit - done).map_or(buffer.len(), |n| n.min(buffer.len()));
            let read = match from.read(&mut buffer[..want]) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            };
            to.write_all(&buffer[..read])?;
            done += read as u64;
            self.counter.add(read as u64);
        }
        Ok(done)
    }
}

/// A file that shrank between the walk and the copy. The archive already
/// promised a size, so the honest thing is to stop.
fn changed(path: &Path) -> EngineError {
    EngineError::Failed(format!(
        "{} changed while it was being packed.",
        path.display()
    ))
}

/// Turns bytes written into progress, reporting every 1 % or so.
struct Counter<'a> {
    total: u64,
    done: u64,
    reported: f32,
    progress: &'a dyn Fn(f32),
}

impl<'a> Counter<'a> {
    fn new(total: u64, progress: &'a dyn Fn(f32)) -> Self {
        Self {
            total,
            done: 0,
            reported: 0.0,
            progress,
        }
    }

    fn add(&mut self, bytes: u64) {
        self.done = self.done.saturating_add(bytes);
        let value = if self.total == 0 {
            1.0
        } else {
            (self.done as f64 / self.total as f64).min(1.0) as f32
        };
        if value - self.reported >= 0.01 {
            self.reported = value;
            // Leave the last sliver for the rename, so 1.0 means "it's there".
            (self.progress)(0.99 * value);
        }
    }
}

// ---------------------------------------------------------------- ZIP

/// Writes `files` as a ZIP into `file`.
fn write_zip(
    files: &[PackFile],
    file: &File,
    level: ArchiveLevel,
    ctx: &mut Context<'_>,
) -> Result<(), EngineError> {
    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;

    let (method, deflate) = match level {
        // Store is Store: no deflate stream wrapped around the raw bytes.
        ArchiveLevel::Store => (CompressionMethod::Stored, None),
        ArchiveLevel::Fast => (CompressionMethod::Deflated, Some(1)),
        ArchiveLevel::Normal => (CompressionMethod::Deflated, Some(6)),
        ArchiveLevel::Maximum => (CompressionMethod::Deflated, Some(9)),
    };
    let zip_err = |e: zip::result::ZipError| match e {
        zip::result::ZipError::Io(io) => EngineError::Io(io),
        other => EngineError::Failed(format!("Could not write the ZIP archive: {other}")),
    };

    let mut zip = zip::ZipWriter::new(BufWriter::new(file));
    for entry in files {
        if ctx.cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        let base = SimpleFileOptions::default().last_modified_time(dos_time(entry.modified));
        if entry.is_dir {
            zip.add_directory(
                format!("{}/", entry.name),
                base.compression_method(CompressionMethod::Stored)
                    .unix_permissions(0o755),
            )
            .map_err(zip_err)?;
            continue;
        }
        let options = base
            .compression_method(method)
            .compression_level(deflate)
            .unix_permissions(0o644)
            // Zip64 only when needed: 4 GiB is where the classic fields give up.
            .large_file(entry.len >= u64::from(u32::MAX));
        zip.start_file(entry.name.as_str(), options)
            .map_err(zip_err)?;
        ctx.copy_file(entry, &mut zip)?;
    }
    let mut inner = zip.finish().map_err(zip_err)?;
    inner.flush()?;
    Ok(())
}

/// The modification time as a DOS date in local time, clamped to the range
/// DOS can hold (1980-01-01 to 2107-12-31), like Filee's `ZipTime`.
fn dos_time(time: SystemTime) -> zip::DateTime {
    let local: DateTime<Local> = time.into();
    let clamped = match local.year() {
        y if y < 1980 => (1980, 1, 1, 0, 0, 0),
        y if y > 2107 => (2107, 12, 31, 0, 0, 0),
        y => (
            y as u16,
            local.month() as u8,
            local.day() as u8,
            local.hour() as u8,
            local.minute() as u8,
            local.second() as u8,
        ),
    };
    let (y, mo, d, h, mi, s) = clamped;
    zip::DateTime::from_date_and_time(y, mo, d, h, mi, s).unwrap_or_default()
}

// ---------------------------------------------------------------- TAR

/// Largest number an 11-digit octal ustar field holds; past it, PAX takes over.
const USTAR_MAX: u64 = 0o77_777_777_777;

/// Writes `files` as a TAR into `writer` and hands the writer back.
fn write_tar<W: Write>(
    files: &[PackFile],
    writer: W,
    ctx: &mut Context<'_>,
) -> Result<W, EngineError> {
    let mut tar = tar::Builder::new(writer);
    for entry in files {
        if ctx.cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        let name = if entry.is_dir {
            format!("{}/", entry.name)
        } else {
            entry.name.clone()
        };
        let mut header = tar::Header::new_ustar();
        let mut pax: Vec<(&str, Vec<u8>)> = Vec::new();
        if !fits_ustar(&mut header, &name) {
            pax.push(("path", name.as_bytes().to_vec()));
            set_fallback_name(&mut header, &name);
        }
        if entry.len > USTAR_MAX {
            pax.push(("size", entry.len.to_string().into_bytes()));
        }
        if !pax.is_empty() {
            tar.append_pax_extensions(pax.iter().map(|(k, v)| (*k, v.as_slice())))?;
        }

        let mtime = entry
            .modified
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        header.set_mtime(mtime.min(USTAR_MAX));
        if entry.is_dir {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_mode(0o755);
            header.set_size(0);
            header.set_cksum();
            tar.append(&header, io::empty())?;
            continue;
        }
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o644);
        header.set_size(entry.len.min(USTAR_MAX));
        header.set_cksum();
        // The builder pads to the size it copied, not the size in the header,
        // so the reader must deliver exactly `len` bytes or fail loudly.
        let mut reader = Exact {
            file: File::open(&entry.path)?,
            left: entry.len,
            ctx,
        };
        tar.append(&header, &mut reader).map_err(|e| {
            if reader.ctx.cancel.is_cancelled() {
                EngineError::Cancelled
            } else if e.kind() == io::ErrorKind::UnexpectedEof {
                changed(&entry.path)
            } else {
                EngineError::Io(e)
            }
        })?;
    }
    Ok(tar.into_inner()?)
}

/// Puts `name` in the ustar name fields when it fits as plain ASCII; false
/// means a PAX `path` record has to carry it.
fn fits_ustar(header: &mut tar::Header, name: &str) -> bool {
    // ustar has no notion of encoding, so anything beyond ASCII rides in PAX,
    // where UTF-8 is the law.
    name.is_ascii() && header.set_path(name).is_ok() && header.path_bytes() == name.as_bytes()
}

/// A best-effort ustar name for readers that ignore PAX: ASCII, at most 100
/// bytes. PAX-aware readers (all of them since about 2001) never see it.
fn set_fallback_name(header: &mut tar::Header, name: &str) {
    let mut fallback: Vec<u8> = name
        .chars()
        .map(|c| {
            if c.is_ascii() && c != '\0' {
                c as u8
            } else {
                b'_'
            }
        })
        .collect();
    let keep_slash = fallback.last() == Some(&b'/');
    fallback.truncate(if keep_slash { 99 } else { 100 });
    if keep_slash && fallback.last() != Some(&b'/') {
        fallback.push(b'/');
    }
    if let Some(ustar) = header.as_ustar_mut() {
        // A failed set_path may have left half a name in the prefix.
        ustar.prefix.fill(0);
    }
    let field = &mut header.as_old_mut().name;
    field.fill(0);
    field[..fallback.len()].copy_from_slice(&fallback);
}

/// Hands out exactly `left` bytes of `file`: fewer is an `UnexpectedEof`,
/// more is ignored (a file that grew gets cut at the size the header claims).
struct Exact<'c, 'a> {
    file: File,
    left: u64,
    ctx: &'c mut Context<'a>,
}

impl Read for Exact<'_, '_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 || buf.is_empty() {
            return Ok(0);
        }
        if self.ctx.cancel.is_cancelled() {
            return Err(io::Error::other("cancelled"));
        }
        let want = usize::try_from(self.left).map_or(buf.len(), |n| n.min(buf.len()));
        let read = self.file.read(&mut buf[..want])?;
        if read == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        self.left -= read as u64;
        self.ctx.counter.add(read as u64);
        Ok(read)
    }
}

/// The gzip level for `level`, the same mapping Filee gives .NET's
/// `CompressionLevel`.
fn gzip_level(level: ArchiveLevel) -> Compression {
    match level {
        ArchiveLevel::Store => Compression::none(),
        ArchiveLevel::Fast => Compression::fast(),
        ArchiveLevel::Normal => Compression::default(),
        ArchiveLevel::Maximum => Compression::best(),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_cover_non_archives_and_folder_but_not_archives() {
        let edges = edges();
        let has = |from: &str, to: &str| {
            edges
                .iter()
                .any(|e| e.from.as_str() == from && e.to.as_str() == to)
        };
        for to in TARGETS {
            assert!(has("txt", to));
            assert!(has("png", to));
            assert!(has("folder", to));
            assert!(!has("zip", to), "zip -> {to} must route through folder");
            assert!(!has("tgz", to));
        }
        assert!(edges.iter().all(|e| e.cost == Edge::DEFAULT_COST));
        assert!(edges.iter().all(|e| e.to.as_str() != "folder"));
    }

    #[test]
    fn dos_times_are_clamped() {
        let old = UNIX_EPOCH; // 1970, before DOS was a twinkle in IBM's eye
        let t = dos_time(old);
        assert_eq!((t.year(), t.month(), t.day()), (1980, 1, 1));
        let far = UNIX_EPOCH + std::time::Duration::from_secs(5_000_000_000); // 2128
        let t = dos_time(far);
        assert_eq!((t.year(), t.month(), t.day()), (2107, 12, 31));
    }

    #[test]
    fn long_and_non_ascii_names_need_pax() {
        let mut header = tar::Header::new_ustar();
        assert!(fits_ustar(&mut header, "a/b.txt"));
        assert!(!fits_ustar(&mut tar::Header::new_ustar(), "caf\u{e9}.txt"));
        let long = "x".repeat(300);
        assert!(!fits_ustar(&mut tar::Header::new_ustar(), &long));
        let mut header = tar::Header::new_ustar();
        set_fallback_name(&mut header, &format!("{long}/"));
        let name = header.path_bytes();
        assert_eq!(name.len(), 100);
        assert_eq!(name.last(), Some(&b'/'));
    }
}
