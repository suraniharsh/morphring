//! The LibreOffice engine: office documents, spreadsheets, presentations and
//! drawings through `soffice --headless --convert-to`.
//!
//! Ported from Filee's `src/Filee.Engines/Office/LibreOfficeConverter.cs`
//! (`ConvertAsync`, `TakeProfileAsync`, `ReturnProfile` and `RunAsync`).
//! The profile warm-up is deferred to a later release.

pub mod filters;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

use morphring_core::{
    CancelToken, Converter, Edge, EngineError, EngineStatus, OutputAllocator, Step,
};

use crate::discovery::{self, Tool};
use crate::process::{self, RunError, RunOptions};

/// How many conversions run at once, each with its own user profile.
pub const WORKERS: usize = 2;

/// How long one soffice run may take before its process group is killed.
pub const TIMEOUT: Duration = Duration::from_secs(180);

// How often a conversion waiting for a free profile looks at the cancel token.
const POOL_POLL: Duration = Duration::from_millis(50);

/// Converts office files with LibreOffice.
pub struct SofficeEngine {
    data_dir: PathBuf,
    edges: Vec<Edge>,
    profiles: ProfilePool,
}

impl SofficeEngine {
    /// An engine that keeps its worker profiles in MorphRing's data directory
    /// (`$XDG_DATA_HOME/morphring` on Linux).
    pub fn new() -> Self {
        Self::new_in(default_data_dir())
    }

    /// An engine that keeps its worker profiles under `data_dir`, in
    /// `libreoffice-profiles/worker-N`.
    pub fn new_in(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            edges: filters::edges(),
            profiles: ProfilePool::new(WORKERS),
        }
    }

    /// The folder that holds the worker profiles' parent (`libreoffice-profiles`).
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

impl Default for SofficeEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn default_data_dir() -> PathBuf {
    // No home folder at all (a bare container, a weird service account) still
    // deserves working conversions; the profiles just won't outlive a reboot.
    directories::ProjectDirs::from("", "", "morphring")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("morphring"))
}

impl Converter for SofficeEngine {
    fn id(&self) -> &'static str {
        "soffice"
    }

    fn display_name(&self) -> &'static str {
        discovery::display_name(Tool::Soffice)
    }

    fn edges(&self) -> &[Edge] {
        &self.edges
    }

    fn max_parallelism(&self) -> usize {
        WORKERS
    }

    fn status(&self) -> EngineStatus {
        discovery::status(Tool::Soffice)
    }

    fn convert(
        &self,
        step: &Step,
        out: &mut dyn OutputAllocator,
        progress: &dyn Fn(f32),
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError> {
        let Some(soffice) = discovery::locate(Tool::Soffice) else {
            return Err(EngineError::NotInstalled {
                tool: self.display_name(),
                hint: discovery::install_hint_here(Tool::Soffice),
            });
        };
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }

        // Ask for the name up front: under the skip policy there is no point
        // booting an entire office suite just to throw its work away.
        let Some(target) = out.allocate(step.to.as_str(), "") else {
            return Ok(Vec::new());
        };

        let Some(slot) = self.profiles.take(cancel) else {
            return Err(EngineError::Cancelled);
        };
        progress(0.1);

        let profile = filters::profile_dir(&self.data_dir, slot.index());
        fs::create_dir_all(&profile)?;
        fs::create_dir_all(&step.work_dir)?;
        // soffice names its output after the input and we can't talk it out of
        // that, so every run gets a private outdir and we take what lands in it.
        let outdir = tempfile::Builder::new()
            .prefix("lo-")
            .tempdir_in(&step.work_dir)?;

        let extension = morphring_core::formats::extension_of(&step.input);
        let extension = (!extension.is_empty()).then_some(extension.as_str());
        let filter = filters::filter_for(step.from, step.to, extension);
        let args = filters::args(&step.input, &filter, outdir.path(), &profile);

        let opts = RunOptions {
            timeout: Some(TIMEOUT),
            ..RunOptions::default()
        };
        let run = process::run(&soffice, &args, opts, cancel);
        drop(slot);

        let failure = match run {
            Ok(_) => None,
            Err(RunError::Cancelled) => return Err(EngineError::Cancelled),
            Err(RunError::TimedOut) => {
                return Err(EngineError::Failed(format!(
                    "LibreOffice did not finish within {} seconds",
                    TIMEOUT.as_secs()
                )));
            }
            Err(RunError::Spawn { program, source }) => {
                return Err(EngineError::Failed(format!(
                    "could not start {program}: {source}"
                )));
            }
            // soffice's exit code is a mood ring, not a status: it has exited 0
            // with nothing written and 1 with a perfectly good PDF. The file in
            // the outdir is the only witness I trust.
            Err(err @ RunError::Failed { .. }) => Some(err.to_string()),
        };
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }

        let Some(produced) = first_non_empty_file(outdir.path())? else {
            let detail = match failure {
                Some(why) => format!("LibreOffice produced no output (soffice {why})"),
                None => "LibreOffice produced no output".to_string(),
            };
            return Err(EngineError::Failed(detail));
        };
        process::move_into_place(&produced, &target)?;
        progress(1.0);
        Ok(vec![target])
    }
}

/// The first non-empty regular file in `dir`, if any.
fn first_non_empty_file(dir: &Path) -> std::io::Result<Option<PathBuf>> {
    let mut names: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() && meta.len() > 0 {
            names.push(entry.path());
        }
    }
    // read_dir order is whatever the filesystem fancies; sorting keeps the
    // choice deterministic if soffice ever drops more than one file.
    names.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(names.into_iter().next())
}

/// Hands out worker profile numbers so no two soffice processes ever share
/// a profile.
struct ProfilePool {
    free: Mutex<Vec<usize>>,
    returned: Condvar,
}

impl ProfilePool {
    fn new(workers: usize) -> Self {
        // Reversed so pop() hands out worker-0 first, like Filee's stack.
        Self {
            free: Mutex::new((0..workers).rev().collect()),
            returned: Condvar::new(),
        }
    }

    /// Waits for a free profile, or returns `None` once `cancel` fires.
    fn take(&self, cancel: &CancelToken) -> Option<ProfileSlot<'_>> {
        let mut free = self.free.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if cancel.is_cancelled() {
                return None;
            }
            if let Some(index) = free.pop() {
                return Some(ProfileSlot { pool: self, index });
            }
            // A timed wait, because a cancel doesn't ring the condvar's bell.
            free = self
                .returned
                .wait_timeout(free, POOL_POLL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// A borrowed worker profile; dropping it gives the profile back.
struct ProfileSlot<'a> {
    pool: &'a ProfilePool,
    index: usize,
}

impl ProfileSlot<'_> {
    fn index(&self) -> usize {
        self.index
    }
}

impl Drop for ProfileSlot<'_> {
    fn drop(&mut self) {
        // Runs on panics and early returns too, so a crashed conversion can't
        // quietly shrink the pool to one worker (and then zero).
        self.pool
            .free
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(self.index);
        self.pool.returned.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Instant;

    #[test]
    fn pool_hands_out_distinct_profiles_starting_at_zero() {
        let pool = ProfilePool::new(2);
        let cancel = CancelToken::new();
        let a = pool.take(&cancel).unwrap();
        let b = pool.take(&cancel).unwrap();
        assert_eq!(a.index(), 0);
        assert_eq!(b.index(), 1);
    }

    #[test]
    fn dropping_a_slot_returns_it() {
        let pool = ProfilePool::new(1);
        let cancel = CancelToken::new();
        let first = pool.take(&cancel).unwrap().index();
        let again = pool.take(&cancel).unwrap().index();
        assert_eq!(first, again);
    }

    #[test]
    fn third_taker_waits_until_a_slot_is_returned() {
        let pool = ProfilePool::new(2);
        let cancel = CancelToken::new();
        let a = pool.take(&cancel).unwrap();
        let _b = pool.take(&cancel).unwrap();
        let got = AtomicBool::new(false);
        thread::scope(|s| {
            let waiter = s.spawn(|| {
                let slot = pool.take(&cancel).unwrap();
                got.store(true, Ordering::SeqCst);
                slot.index()
            });
            thread::sleep(Duration::from_millis(150));
            assert!(
                !got.load(Ordering::SeqCst),
                "took a profile that was in use"
            );
            drop(a);
            assert_eq!(waiter.join().unwrap(), 0);
        });
    }

    #[test]
    fn cancel_releases_a_waiting_taker() {
        let pool = Arc::new(ProfilePool::new(1));
        let cancel = CancelToken::new();
        let _held = pool.take(&cancel).unwrap();
        let started = Instant::now();
        thread::scope(|s| {
            let waiter = s.spawn(|| pool.take(&cancel).map(|slot| slot.index()));
            thread::sleep(Duration::from_millis(100));
            cancel.cancel();
            assert_eq!(waiter.join().unwrap(), None);
        });
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn engine_basics() {
        let dir = tempfile::tempdir().unwrap();
        let engine = SofficeEngine::new_in(dir.path());
        assert_eq!(engine.id(), "soffice");
        assert_eq!(engine.display_name(), "LibreOffice");
        assert_eq!(engine.max_parallelism(), 2);
        assert_eq!(engine.data_dir(), dir.path());
        assert!(
            engine
                .edges()
                .iter()
                .any(|e| e.from.as_str() == "pptx" && e.to.as_str() == "pdf" && e.cost == 0)
        );
    }

    #[test]
    fn first_non_empty_file_skips_empty_ones() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(first_non_empty_file(dir.path()).unwrap(), None);
        fs::write(dir.path().join("a.pdf"), b"").unwrap();
        assert_eq!(first_non_empty_file(dir.path()).unwrap(), None);
        fs::write(dir.path().join("b.pdf"), b"%PDF").unwrap();
        assert_eq!(
            first_non_empty_file(dir.path()).unwrap(),
            Some(dir.path().join("b.pdf"))
        );
    }
}
