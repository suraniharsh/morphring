//! Test support: the one `FakeConverter` that every crate's tests share.
//!
//! Enabled for this crate's own tests and, for other crates, through the
//! `test-support` feature.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::contract::{
    CancelToken, ConvertOptions, Converter, Edge, EngineStatus, OutputAllocator, Step,
};
use crate::error::EngineError;

/// What a `FakeConverter` does when asked to convert.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Behaviour {
    /// Writes one small file through the allocator.
    Copy,
    /// Returns `EngineError::Failed` with this message.
    Fail(String),
    /// Panics.
    Panic,
    /// Allocates one output, sleeps (checking cancel every 10 ms), then writes it.
    Sleep(Duration),
    /// Writes this many files with suffixes `_p1`, `_p2`, ...
    Outputs(usize),
}

/// Call counters of a `FakeConverter`, still readable after the fake has been
/// boxed and handed to the code under test.
#[derive(Clone, Debug, Default)]
pub struct FakeCounters(Arc<Counters>);

#[derive(Debug, Default)]
struct Counters {
    calls: AtomicUsize,
    running: AtomicUsize,
    high_water: AtomicUsize,
}

impl FakeCounters {
    /// How many times `convert` was called.
    pub fn calls(&self) -> usize {
        self.0.calls.load(Ordering::SeqCst)
    }

    /// The largest number of `convert` calls that ran at the same time.
    pub fn max_concurrent(&self) -> usize {
        self.0.high_water.load(Ordering::SeqCst)
    }
}

/// Decrements the running count even when the fake panics on purpose.
struct RunningGuard<'a>(&'a Counters);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.running.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A configurable in-memory engine for tests.
#[derive(Clone, Debug)]
pub struct FakeConverter {
    id: &'static str,
    display_name: &'static str,
    edges: Vec<Edge>,
    max_parallelism: usize,
    status: EngineStatus,
    behaviour: Behaviour,
    declines: bool,
    counters: FakeCounters,
}

impl FakeConverter {
    /// An available engine with no edges that copies.
    pub fn new(id: &'static str) -> Self {
        Self {
            id,
            display_name: id,
            edges: Vec::new(),
            max_parallelism: 0,
            status: EngineStatus::Available {
                path: PathBuf::from(format!("/usr/bin/{id}")),
                version: "1.0".into(),
            },
            behaviour: Behaviour::Copy,
            declines: false,
            counters: FakeCounters::default(),
        }
    }

    /// Adds a direct conversion.
    pub fn edge(mut self, from: &'static str, to: &'static str, cost: u32) -> Self {
        self.edges.push(Edge::with_cost(from, to, cost));
        self
    }

    /// Sets the display name used in missing-engine hints.
    pub fn display_name(mut self, name: &'static str) -> Self {
        self.display_name = name;
        self
    }

    /// Sets the per-engine concurrency limit.
    pub fn max_parallelism(mut self, n: usize) -> Self {
        self.max_parallelism = n;
        self
    }

    /// Marks the engine as not installed, with this install hint.
    pub fn missing(mut self, hint: &str) -> Self {
        self.status = EngineStatus::Missing { hint: hint.into() };
        self
    }

    /// Makes `accepts` return false for every option set, so the job runner
    /// must leave this engine out.
    pub fn declining(mut self) -> Self {
        self.declines = true;
        self
    }

    /// Sets what `convert` does.
    pub fn behaviour(mut self, behaviour: Behaviour) -> Self {
        self.behaviour = behaviour;
        self
    }

    /// A handle on the call counters; keep it before boxing the fake.
    pub fn counters(&self) -> FakeCounters {
        self.counters.clone()
    }

    fn write(path: &PathBuf, step: &Step) -> Result<(), EngineError> {
        fs::write(path, format!("fake {} -> {}\n", step.from, step.to))?;
        Ok(())
    }
}

impl Converter for FakeConverter {
    fn id(&self) -> &'static str {
        self.id
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    fn edges(&self) -> &[Edge] {
        &self.edges
    }

    fn max_parallelism(&self) -> usize {
        self.max_parallelism
    }

    fn status(&self) -> EngineStatus {
        self.status.clone()
    }

    fn accepts(&self, _options: &ConvertOptions) -> bool {
        !self.declines
    }

    fn convert(
        &self,
        step: &Step,
        out: &mut dyn OutputAllocator,
        progress: &dyn Fn(f32),
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError> {
        let c = &self.counters.0;
        c.calls.fetch_add(1, Ordering::SeqCst);
        let now = c.running.fetch_add(1, Ordering::SeqCst) + 1;
        c.high_water.fetch_max(now, Ordering::SeqCst);
        let _guard = RunningGuard(c);

        // A real engine spends its life waiting on a child process. The fake naps
        // for 5 ms instead, otherwise every call finishes before the next thread
        // has even put its shoes on and the high-water mark is a boring 1.
        thread::sleep(Duration::from_millis(5));
        progress(0.0);

        let ext = step.to.as_str();
        let written = match &self.behaviour {
            Behaviour::Copy => match out.allocate(ext, "") {
                Some(path) => {
                    Self::write(&path, step)?;
                    vec![path]
                }
                None => Vec::new(),
            },
            Behaviour::Fail(msg) => return Err(EngineError::Failed(msg.clone())),
            Behaviour::Panic => panic!("FakeConverter {} panicked on purpose", self.id),
            Behaviour::Sleep(duration) => {
                let Some(path) = out.allocate(ext, "") else {
                    return Ok(Vec::new());
                };
                let start = Instant::now();
                while start.elapsed() < *duration {
                    if cancel.is_cancelled() {
                        return Err(EngineError::Cancelled);
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Self::write(&path, step)?;
                vec![path]
            }
            Behaviour::Outputs(n) => {
                let mut written = Vec::new();
                for i in 1..=*n {
                    if let Some(path) = out.allocate(ext, &format!("_p{i}")) {
                        Self::write(&path, step)?;
                        written.push(path);
                    }
                }
                written
            }
        };
        progress(1.0);
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;

    use super::*;
    use crate::contract::{ConvertOptions, FormatId};

    /// Allocates `<dir>/out<suffix>.<ext>` and remembers what it handed out.
    struct DirAllocator {
        dir: PathBuf,
        handed_out: Vec<PathBuf>,
        skip: bool,
    }

    impl OutputAllocator for DirAllocator {
        fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
            if self.skip {
                return None;
            }
            let path = self.dir.join(format!("out{suffix}.{ext}"));
            self.handed_out.push(path.clone());
            Some(path)
        }
    }

    fn step(dir: &Path) -> Step {
        Step {
            input: dir.join("in.png"),
            from: FormatId::new("png"),
            to: FormatId::new("jpg"),
            options: ConvertOptions::default(),
            work_dir: dir.to_path_buf(),
        }
    }

    fn allocator(dir: &Path) -> DirAllocator {
        DirAllocator {
            dir: dir.to_path_buf(),
            handed_out: Vec::new(),
            skip: false,
        }
    }

    #[test]
    fn copy_writes_through_the_allocator() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = FakeConverter::new("fake").edge("png", "jpg", 10);
        let mut out = allocator(tmp.path());
        let progress = Mutex::new(Vec::new());
        let written = fake
            .convert(
                &step(tmp.path()),
                &mut out,
                &|p| progress.lock().unwrap().push(p),
                &CancelToken::new(),
            )
            .unwrap();
        assert_eq!(written, out.handed_out);
        assert_eq!(written, vec![tmp.path().join("out.jpg")]);
        assert!(written[0].is_file());
        assert_eq!(*progress.lock().unwrap(), vec![0.0, 1.0]);
        assert_eq!(fake.counters().calls(), 1);
    }

    #[test]
    fn skip_allocation_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = FakeConverter::new("fake");
        let mut out = DirAllocator {
            skip: true,
            ..allocator(tmp.path())
        };
        let written = fake
            .convert(&step(tmp.path()), &mut out, &|_| {}, &CancelToken::new())
            .unwrap();
        assert!(written.is_empty());
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    #[test]
    fn outputs_fan_out_with_page_suffixes() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = FakeConverter::new("fake").behaviour(Behaviour::Outputs(3));
        let mut out = allocator(tmp.path());
        let written = fake
            .convert(&step(tmp.path()), &mut out, &|_| {}, &CancelToken::new())
            .unwrap();
        let names: Vec<_> = written
            .iter()
            .map(|p| p.file_name().unwrap().to_owned())
            .collect();
        assert_eq!(names, ["out_p1.jpg", "out_p2.jpg", "out_p3.jpg"]);
    }

    #[test]
    fn fail_and_missing_are_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = FakeConverter::new("fake")
            .missing("pacman -S fake")
            .behaviour(Behaviour::Fail("nope".into()));
        assert_eq!(
            fake.status(),
            EngineStatus::Missing {
                hint: "pacman -S fake".into()
            }
        );
        let err = fake.convert(
            &step(tmp.path()),
            &mut allocator(tmp.path()),
            &|_| {},
            &CancelToken::new(),
        );
        assert!(matches!(err, Err(EngineError::Failed(msg)) if msg == "nope"));
    }

    #[test]
    fn sleep_stops_when_cancelled() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = FakeConverter::new("fake").behaviour(Behaviour::Sleep(Duration::from_secs(30)));
        let cancel = CancelToken::new();
        cancel.cancel();
        let start = Instant::now();
        let mut out = allocator(tmp.path());
        let err = fake.convert(&step(tmp.path()), &mut out, &|_| {}, &cancel);
        assert!(matches!(err, Err(EngineError::Cancelled)));
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!out.handed_out[0].exists());
    }

    #[test]
    fn records_the_concurrency_high_water_mark() {
        let tmp = tempfile::tempdir().unwrap();
        let fake =
            FakeConverter::new("fake").behaviour(Behaviour::Sleep(Duration::from_millis(100)));
        let counters = fake.counters();
        thread::scope(|s| {
            for i in 0..3 {
                let (fake, dir) = (&fake, tmp.path().join(i.to_string()));
                fs::create_dir(&dir).unwrap();
                s.spawn(move || {
                    fake.convert(
                        &step(&dir),
                        &mut allocator(&dir),
                        &|_| {},
                        &CancelToken::new(),
                    )
                    .unwrap();
                });
            }
        });
        assert_eq!(counters.calls(), 3);
        assert_eq!(counters.max_concurrent(), 3);
    }

    #[test]
    fn panic_still_releases_the_running_slot() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = FakeConverter::new("fake").behaviour(Behaviour::Panic);
        let counters = fake.counters();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = fake.convert(
                &step(tmp.path()),
                &mut allocator(tmp.path()),
                &|_| {},
                &CancelToken::new(),
            );
        }));
        assert!(result.is_err());
        assert_eq!(counters.0.running.load(Ordering::SeqCst), 0);
    }
}
