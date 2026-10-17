//! The LibreOffice engine: office documents, spreadsheets, presentations and
//! drawings through `soffice --headless --convert-to`.
//!
//! Ported from Filee's `src/Filee.Engines/Office/LibreOfficeConverter.cs`
//! (`ConvertAsync`, `TakeProfileAsync`, `ReturnProfile` and `RunAsync`).
//! The profile warm-up is deferred to a later release.

pub mod filters;

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

use morphring_core::{
    CancelToken, Converter, Edge, EngineError, EngineStatus, OutputAllocator, Step,
};

use crate::discovery::{self, Tool};
use crate::process::{self, RunError, RunOptions};
use crate::progress::Estimate;

/// How many conversions run at once, each with its own user profile.
pub const WORKERS: usize = 2;

/// How long one soffice run may take before its process group is killed.
pub const TIMEOUT: Duration = Duration::from_secs(180);

// How long a conversion usually takes, for the progress estimate. A warm
// soffice does a small document in a second or two; a cold one spends most of
// that just remembering who it is.
const TYPICAL: Duration = Duration::from_secs(4);

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
        let data_dir = data_dir.into();
        Self {
            profiles: ProfilePool::new(data_dir.clone(), WORKERS),
            data_dir,
            edges: filters::edges(),
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

        let Some(slot) = self.profiles.take(cancel)? else {
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
        // soffice says nothing until it's done, so the ring gets an estimate.
        // The whole run happens while we still hold the slot, so nobody who
        // asked our pool can be in this profile and any .lock we see belongs
        // to an outsider (an older MorphRing, or a hand-pointed LibreOffice).
        let (run, busy_profile) = watching_profile(&profile, || {
            Estimate::during(progress, TYPICAL, || {
                process::run(&soffice, &args, opts, cancel)
            })
        });
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
            Err(RunError::Stalled { after }) => {
                return Err(EngineError::Failed(format!(
                    "LibreOffice stopped making progress for {} seconds",
                    after.as_secs()
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
            return Err(EngineError::Failed(no_output_detail(
                failure,
                busy_profile.as_deref(),
            )));
        };
        process::move_into_place(&produced, &target)?;
        progress(1.0);
        Ok(vec![target])
    }
}

/// Runs `run` (one soffice invocation in `profile`) and reports `profile` back
/// if a foreign LibreOffice seemed to hold it.
///
/// We look for squatters ([`profile_in_use`]) both before and after the run,
/// and either sighting counts. The "after" look alone is far too late: a
/// second soffice that finds the profile taken hands its job to the owner,
/// waits for it to shut down and only then exits with code 1, by which time
/// the owner has usually tidied its lock away. Like a burglar who also does
/// the dishes. The owner process itself tends to linger a few milliseconds
/// longer, so the process check still catches the photo finishes.
///
/// Call this only while holding the profile's slot, or our own threads'
/// soffice processes start looking like intruders.
fn watching_profile<T>(profile: &Path, run: impl FnOnce() -> T) -> (T, Option<PathBuf>) {
    let before = profile_in_use(profile);
    let result = run();
    let busy = before || profile_in_use(profile);
    (result, busy.then(|| profile.to_path_buf()))
}

/// Whether some LibreOffice already lives in `profile`: its `.lock` is there,
/// or (on Linux) a running process was started with this profile.
///
/// The process check matters because a booting soffice grabs its IPC pipe
/// well before it writes `.lock`, so for the first second or so the lock
/// file says "vacant" while the pipe says "occupied, go away".
fn profile_in_use(profile: &Path) -> bool {
    filters::office_lock(profile).exists() || office_running_in(profile)
}

#[cfg(target_os = "linux")]
fn office_running_in(profile: &Path) -> bool {
    let arg = filters::user_installation_arg(profile);
    any_process_has_arg(Path::new("/proc"), arg.as_bytes())
}

// No /proc to rummage through here, so the `.lock` checks have to do.
#[cfg(not(target_os = "linux"))]
fn office_running_in(_profile: &Path) -> bool {
    false
}

/// Whether any `<proc_root>/<pid>/cmdline` has `arg` as one whole argument.
/// Whole, because `worker-1` should not be spooked by `worker-10`.
#[cfg(target_os = "linux")]
fn any_process_has_arg(proc_root: &Path, arg: &[u8]) -> bool {
    let Ok(entries) = fs::read_dir(proc_root) else {
        return false;
    };
    entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
        })
        // Processes come and go while we look; a vanished one is not a squatter.
        .filter_map(|e| fs::read(e.path().join("cmdline")).ok())
        .any(|cmdline| cmdline.split(|&b| b == 0).any(|a| a == arg))
}

/// Why a run that left nothing behind failed, as far as we can tell.
///
/// `busy_profile` is the profile whose LibreOffice lock we saw around our run
/// (see [`watching_profile`]): the usual reason a perfectly good document comes back
/// empty-handed is that another LibreOffice owned the profile first.
fn no_output_detail(failure: Option<String>, busy_profile: Option<&Path>) -> String {
    let mut detail = match failure {
        Some(why) => format!("LibreOffice produced no output (soffice {why})"),
        None => "LibreOffice produced no output".to_string(),
    };
    if let Some(profile) = busy_profile {
        detail.push_str(&format!(
            "; its profile {} appears to be in use by another LibreOffice process",
            profile.display()
        ));
    }
    detail
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

/// How many profile slots one data directory offers across all processes.
/// Every MorphRing process (the app, a CLI run, `watch`) gets up to
/// [`WORKERS`] of them, so this leaves room for a small crowd.
const MAX_PROFILES: usize = WORKERS * 8;

/// Hands out worker profile numbers so no two soffice processes ever share
/// a profile, not even ones started by different MorphRing processes.
///
/// Inside a process a slot is counted against `workers`; across processes it
/// is an OS file lock on `worker-N.lock`, which the kernel drops for us even
/// if the holder dies mid-conversion.
struct ProfilePool {
    data_dir: PathBuf,
    workers: usize,
    held: Mutex<Vec<usize>>,
    returned: Condvar,
}

impl ProfilePool {
    fn new(data_dir: PathBuf, workers: usize) -> Self {
        Self {
            data_dir,
            workers,
            held: Mutex::new(Vec::new()),
            returned: Condvar::new(),
        }
    }

    /// Waits for a free profile, or returns `None` once `cancel` fires.
    ///
    /// A slot is free when this process has a worker to spare and no other
    /// process holds the slot's lock. The lowest free number wins, so a lone
    /// process keeps reusing (and keeps warm) worker-0.
    fn take(&self, cancel: &CancelToken) -> std::io::Result<Option<ProfileSlot<'_>>> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if cancel.is_cancelled() {
                return Ok(None);
            }
            if held.len() < self.workers {
                if let Some((index, lock)) = self.try_lock_any(&held)? {
                    held.push(index);
                    return Ok(Some(ProfileSlot {
                        pool: self,
                        index,
                        lock,
                    }));
                }
            }
            // A timed wait, because neither a cancel nor another process
            // letting go of its lock rings the condvar's bell.
            held = self
                .returned
                .wait_timeout(held, POOL_POLL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Locks the lowest slot that nobody holds, in this process or any other.
    fn try_lock_any(&self, held: &[usize]) -> std::io::Result<Option<(usize, Option<File>)>> {
        if let Some(parent) = filters::profile_lock(&self.data_dir, 0).parent() {
            fs::create_dir_all(parent)?;
        }
        for index in (0..MAX_PROFILES).filter(|i| !held.contains(i)) {
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(filters::profile_lock(&self.data_dir, index))?;
            match file.try_lock() {
                Ok(()) => return Ok(Some((index, Some(file)))),
                Err(TryLockError::WouldBlock) => continue,
                // Some filesystems (looking at you, older NFS setups) can't lock
                // at all. Then the in-process bookkeeping is all we have, which
                // is exactly what we had before locks existed.
                Err(TryLockError::Error(err)) if err.kind() == ErrorKind::Unsupported => {
                    return Ok(Some((index, None)));
                }
                Err(TryLockError::Error(err)) => return Err(err),
            }
        }
        Ok(None)
    }
}

/// A borrowed worker profile; dropping it gives the profile back, to this
/// process and (by closing the lock file) to every other one.
struct ProfileSlot<'a> {
    pool: &'a ProfilePool,
    index: usize,
    // Only here to be closed: closing the handle releases the OS lock, no
    // explicit unlock() needed.
    lock: Option<File>,
}

impl ProfileSlot<'_> {
    fn index(&self) -> usize {
        self.index
    }
}

impl Drop for ProfileSlot<'_> {
    fn drop(&mut self) {
        // Runs on panics and early returns too, so a crashed conversion can't
        // quietly shrink the pool to one worker (and then zero). The lock goes
        // first, so whoever we wake up finds the file already unlocked.
        drop(self.lock.take());
        self.pool
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|&i| i != self.index);
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
        let dir = tempfile::tempdir().unwrap();
        let pool = ProfilePool::new(dir.path().into(), 2);
        let cancel = CancelToken::new();
        let a = pool.take(&cancel).unwrap().unwrap();
        let b = pool.take(&cancel).unwrap().unwrap();
        assert_eq!(a.index(), 0);
        assert_eq!(b.index(), 1);
    }

    #[test]
    fn dropping_a_slot_returns_it() {
        let dir = tempfile::tempdir().unwrap();
        let pool = ProfilePool::new(dir.path().into(), 1);
        let cancel = CancelToken::new();
        let first = pool.take(&cancel).unwrap().unwrap().index();
        let again = pool.take(&cancel).unwrap().unwrap().index();
        assert_eq!(first, again);
    }

    #[test]
    fn third_taker_waits_until_a_slot_is_returned() {
        let dir = tempfile::tempdir().unwrap();
        let pool = ProfilePool::new(dir.path().into(), 2);
        let cancel = CancelToken::new();
        let a = pool.take(&cancel).unwrap().unwrap();
        let _b = pool.take(&cancel).unwrap().unwrap();
        let got = AtomicBool::new(false);
        thread::scope(|s| {
            let waiter = s.spawn(|| {
                let slot = pool.take(&cancel).unwrap().unwrap();
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
        let dir = tempfile::tempdir().unwrap();
        let pool = Arc::new(ProfilePool::new(dir.path().into(), 1));
        let cancel = CancelToken::new();
        let _held = pool.take(&cancel).unwrap().unwrap();
        let started = Instant::now();
        thread::scope(|s| {
            let waiter = s.spawn(|| pool.take(&cancel).unwrap().map(|slot| slot.index()));
            thread::sleep(Duration::from_millis(100));
            cancel.cancel();
            assert_eq!(waiter.join().unwrap(), None);
        });
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn pools_in_different_processes_never_share_a_profile() {
        // Two pools on one data dir stand in for two MorphRing processes (the
        // app and a CLI run, say). Each opens its own lock handles, which is
        // all the kernel looks at.
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancelToken::new();
        let app = ProfilePool::new(dir.path().into(), 2);
        let cli = ProfilePool::new(dir.path().into(), 2);
        let a0 = app.take(&cancel).unwrap().unwrap();
        let c0 = cli.take(&cancel).unwrap().unwrap();
        let a1 = app.take(&cancel).unwrap().unwrap();
        let c1 = cli.take(&cancel).unwrap().unwrap();
        let seen = [a0.index(), c0.index(), a1.index(), c1.index()];
        assert_eq!(seen, [0, 1, 2, 3]);
        assert!(filters::profile_lock(dir.path(), 0).is_file());
    }

    #[test]
    fn dropping_a_slot_releases_its_lock_for_other_processes() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancelToken::new();
        let first = ProfilePool::new(dir.path().into(), 1);
        let second = ProfilePool::new(dir.path().into(), 1);
        let held = first.take(&cancel).unwrap().unwrap();
        assert_eq!(held.index(), 0);
        assert_eq!(second.take(&cancel).unwrap().unwrap().index(), 1);
        drop(held);
        let third = ProfilePool::new(dir.path().into(), 1);
        assert_eq!(third.take(&cancel).unwrap().unwrap().index(), 0);
    }

    #[test]
    fn a_full_house_waits_instead_of_failing() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancelToken::new();
        // Somebody else grabbed every slot there is.
        let hog = ProfilePool::new(dir.path().into(), MAX_PROFILES);
        let mut slots: Vec<_> = (0..MAX_PROFILES)
            .map(|_| hog.take(&cancel).unwrap().unwrap())
            .collect();
        let pool = ProfilePool::new(dir.path().into(), 1);
        thread::scope(|s| {
            let waiter = s.spawn(|| pool.take(&cancel).unwrap().map(|slot| slot.index()));
            thread::sleep(Duration::from_millis(150));
            assert!(!waiter.is_finished(), "took a profile someone else holds");
            slots.remove(5);
            assert_eq!(waiter.join().unwrap(), Some(5));
        });
    }

    #[test]
    fn no_output_names_a_busy_profile() {
        let profile = Path::new("/data/libreoffice-profiles/worker-0");
        let why = Some("exited with code 1".to_string());
        assert_eq!(
            no_output_detail(why.clone(), None),
            "LibreOffice produced no output (soffice exited with code 1)"
        );
        assert_eq!(
            no_output_detail(why, Some(profile)),
            "LibreOffice produced no output (soffice exited with code 1); its profile \
             /data/libreoffice-profiles/worker-0 appears to be in use by another \
             LibreOffice process"
        );
        assert_eq!(
            no_output_detail(None, None),
            "LibreOffice produced no output"
        );
    }

    #[test]
    fn a_lock_that_vanishes_during_the_run_still_counts() {
        // The real race: the foreign soffice owns the profile when ours starts,
        // then shuts down (taking its .lock with it) just before ours gives up.
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("worker-0");
        fs::create_dir_all(&profile).unwrap();
        let lock = filters::office_lock(&profile);
        fs::write(&lock, b"someone else").unwrap();
        let (value, busy) = watching_profile(&profile, || {
            fs::remove_file(&lock).unwrap();
            7
        });
        assert_eq!(value, 7);
        assert_eq!(busy.as_deref(), Some(profile.as_path()));
    }

    #[test]
    fn a_lock_that_appears_during_the_run_counts() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("worker-0");
        fs::create_dir_all(&profile).unwrap();
        let (_, busy) = watching_profile(&profile, || {
            fs::write(filters::office_lock(&profile), b"late arrival").unwrap();
        });
        assert_eq!(busy.as_deref(), Some(profile.as_path()));
    }

    #[test]
    fn a_profile_with_a_lock_is_in_use() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("worker-0");
        fs::create_dir_all(&profile).unwrap();
        assert!(!profile_in_use(&profile));
        fs::write(filters::office_lock(&profile), b"x").unwrap();
        assert!(profile_in_use(&profile));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_running_process_in_the_profile_counts_before_any_lock_exists() {
        // A soffice that has only just booted: no .lock yet, but it's in there.
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("worker-0");
        fs::create_dir_all(&profile).unwrap();
        let mut squatter = std::process::Command::new("sh")
            .arg("-c")
            // The trailing `:` stops the shell from exec'ing sleep and taking
            // our telltale argument down with it.
            .arg("sleep 30; :")
            .arg(filters::user_installation_arg(&profile))
            .spawn()
            .unwrap();
        // spawn returns before exec does, so give the shell a moment to show up.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = profile_in_use(&profile);
        while !seen && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
            seen = profile_in_use(&profile);
        }
        let (_, busy) = watching_profile(&profile, || ());
        let neighbour = profile_in_use(&dir.path().join("worker-00"));
        squatter.kill().unwrap();
        squatter.wait().unwrap();
        assert!(seen, "missed a live process in the profile");
        assert_eq!(busy.as_deref(), Some(profile.as_path()));
        assert!(!neighbour, "worker-00 got blamed for worker-0's squatter");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn process_scan_matches_whole_arguments_only() {
        let root = tempfile::tempdir().unwrap();
        let add = |pid: &str, cmdline: &[u8]| {
            fs::create_dir_all(root.path().join(pid)).unwrap();
            fs::write(root.path().join(pid).join("cmdline"), cmdline).unwrap();
        };
        add(
            "12",
            b"soffice\0-env:UserInstallation=file:///p/worker-10/\0",
        );
        add(
            "self",
            b"soffice\0-env:UserInstallation=file:///p/worker-1/\0",
        );
        let arg = b"-env:UserInstallation=file:///p/worker-1/";
        assert!(!any_process_has_arg(root.path(), arg));
        add(
            "13",
            b"oosplash\0--headless\0-env:UserInstallation=file:///p/worker-1/\0",
        );
        assert!(any_process_has_arg(root.path(), arg));
    }

    #[test]
    fn a_quiet_profile_is_not_reported_busy() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("worker-0");
        fs::create_dir_all(&profile).unwrap();
        let (_, busy) = watching_profile(&profile, || ());
        assert_eq!(busy, None);
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
