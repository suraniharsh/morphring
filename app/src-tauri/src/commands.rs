//! Commands the webviews can invoke, plus the conversion queue behind them.
//!
//! The ring's life: [`open_ring`] builds a [`RingModel`] for some files and
//! sends it as [`RING_OPEN`]; the user clicks a slice and the ring invokes
//! [`convert`]; the job runs on the queue's worker thread while
//! [`RING_PROGRESS`] events move the progress ring; [`RING_DONE`] says how it
//! ended. Ported from Filee's `src/Filee.App/Services/RingController.cs`
//! (convert and progress) and `ViewModels/RadialViewModel.cs` (the model).
//!
//! Everything that can be tested without a window lives in [`Conversions`] and
//! [`ring_model`]; the Tauri commands are thin wrappers around them.

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;

use morphring_core::formats;
use morphring_core::job::{self, JobEvent};
use morphring_core::output::OutputRule;
use morphring_core::planner::Planner;
use morphring_core::slices::{self, DisabledReason, Reach};
use morphring_core::{
    CancelToken, ConvertOptions, Converter, EngineStatus, ErrorKey, FileResult, FileState,
    FormatId, JobState,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Label of the hidden window that shows the conversion ring.
pub const RING_WINDOW: &str = "ring";

/// Event sent to the ring window with a [`RingModel`] payload, right before
/// the ring is shown.
pub const RING_OPEN: &str = "ring-open";

/// Event sent to the ring window with a [`RingProgress`] payload while the
/// job it shows is running.
pub const RING_PROGRESS: &str = "ring-progress";

/// Event sent to the ring window with a [`RingDone`] payload when the job it
/// shows has ended.
pub const RING_DONE: &str = "ring-done";

/// What the ring shows for a set of files: the payload of [`RING_OPEN`].
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RingModel {
    /// The files to convert, as given back to [`convert`].
    pub files: Vec<String>,
    /// Centre text, e.g. `3 × JPG` or `2 files`.
    pub title: String,
    /// The slices, clockwise from 12 o'clock.
    pub slices: Vec<RingSlice>,
}

/// One slice of the ring: a target format.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RingSlice {
    /// Format id passed to [`convert`] as `to`, e.g. `png`.
    pub target: String,
    /// Text drawn on the slice, e.g. `PNG`.
    pub label: String,
    /// Whether the slice can be clicked.
    pub enabled: bool,
    /// Why the slice is disabled, shown when it is hovered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
}

/// Payload of [`RING_PROGRESS`].
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RingProgress {
    /// Progress of the whole job, `0.0..=1.0`. Only goes up.
    pub progress: f32,
}

/// How a job ended, as the ring shows it.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// Every file was converted (or skipped because its output existed).
    Succeeded,
    /// At least one file failed.
    Failed,
    /// The job was cancelled.
    Cancelled,
}

/// Payload of [`RING_DONE`].
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RingDone {
    pub outcome: Outcome,
    /// What went wrong, for a failed job.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Something the ring window should be told about.
#[derive(Debug, Clone, PartialEq)]
pub enum RingEvent {
    Progress(RingProgress),
    Done(RingDone),
}

/// What [`convert`] answers once its job has ended.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConvertSummary {
    pub outcome: Outcome,
    /// Files written, in input order.
    pub outputs: Vec<String>,
    /// Number of files that were converted.
    pub done: usize,
    /// Number of files that failed.
    pub failed: usize,
    /// What went wrong, for a failed job.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Builds the ring for `files` from the built-in favourites.
///
/// A slice is enabled when at least one file can be converted to its target
/// with the installed `engines`; a disabled slice says why, naming the
/// program to install when one would help.
pub fn ring_model(files: &[PathBuf], engines: &[&dyn Converter]) -> RingModel {
    // Two planners, as D10 says: what we can do now, and what we could do if
    // every engine were installed. The second one turns "no" into "not yet".
    let installed = Planner::new(engines, false);
    let every = Planner::new(engines, true);
    let suggestion = slices::suggest(files, &slices::default_favourites(), |from, to| {
        reach(&installed, &every, engines, from, to)
    });
    RingModel {
        files: files.iter().map(|p| display(p)).collect(),
        title: suggestion.title,
        slices: suggestion
            .slices
            .into_iter()
            .map(|slice| RingSlice {
                target: slice.target.as_str().to_owned(),
                label: slice.label.to_owned(),
                enabled: slice.is_enabled(),
                disabled_reason: slice.disabled.map(|reason| reason.to_string()),
            })
            .collect(),
    }
}

fn reach(
    installed: &Planner,
    every: &Planner,
    engines: &[&dyn Converter],
    from: FormatId,
    to: FormatId,
) -> Reach {
    if installed.can_convert(from, to) {
        return Reach::Ready;
    }
    let Some(route) = every.plan(from, to) else {
        return Reach::NoRoute;
    };
    route
        .engines()
        .filter_map(|i| engines.get(i))
        .find_map(|engine| match engine.status() {
            EngineStatus::Missing { hint } => Some(Reach::NeedsEngine {
                engine: engine.display_name().to_owned(),
                hint,
            }),
            EngineStatus::Available { .. } => None,
        })
        // A route through engines that are all installed, yet the installed
        // planner found nothing? Only if a status flipped between the two
        // planners. Calling it "no route" is the honest answer for this instant.
        .unwrap_or(Reach::NoRoute)
}

/// Where ring events go. The app sends them to the ring window; tests collect them.
pub type EventSink = Arc<dyn Fn(RingEvent) + Send + Sync>;

type Engines = Arc<Vec<Box<dyn Converter>>>;

/// One conversion waiting in the queue.
struct Request {
    id: u64,
    files: Vec<PathBuf>,
    target: FormatId,
    reply: mpsc::Sender<ConvertSummary>,
}

/// The conversion queue: one worker thread that runs jobs one at a time, in
/// the order they were submitted.
pub struct Conversions {
    engines: Engines,
    queue: Mutex<mpsc::Sender<Request>>,
    /// Id of the job the ring is showing; 0 when it shows a menu or nothing.
    showing: Arc<AtomicU64>,
    next_id: AtomicU64,
}

impl Conversions {
    /// Starts the worker thread. `sink` receives the events of the job the
    /// ring is showing, from the worker thread.
    pub fn new(engines: Vec<Box<dyn Converter>>, sink: EventSink) -> Self {
        let engines: Engines = Arc::new(engines);
        let showing = Arc::new(AtomicU64::new(0));
        let (queue, requests) = mpsc::channel::<Request>();
        let worker = Worker {
            engines: Arc::clone(&engines),
            showing: Arc::clone(&showing),
            sink,
        };
        // One thread, one channel, FIFO for free. Jobs already run their files in
        // parallel, so a second job running alongside would only fight the first
        // one for the same cores and make both look slow.
        let spawned = thread::Builder::new()
            .name("morphring-jobs".into())
            .spawn(move || worker.run(requests));
        if let Err(e) = spawned {
            // Without a worker every submit gets a clean "could not start" error
            // instead of a hang, because the receiver is gone with the closure.
            tracing::error!("could not start the conversion worker: {e}");
        }
        Self {
            engines,
            queue: Mutex::new(queue),
            showing,
            next_id: AtomicU64::new(1),
        }
    }

    /// The engines conversions run with, in priority order.
    pub fn engines(&self) -> Vec<&dyn Converter> {
        self.engines.iter().map(|e| e.as_ref()).collect()
    }

    /// Builds the ring for `files`. The ring then shows a menu, so events of
    /// jobs started earlier are no longer sent.
    pub fn ring_model(&self, files: &[PathBuf]) -> RingModel {
        self.showing.store(0, Ordering::SeqCst);
        ring_model(files, &self.engines())
    }

    /// Queues `files` for conversion to `to` and returns where the summary
    /// will arrive. The new job becomes the one the ring shows.
    pub fn submit(
        &self,
        files: Vec<PathBuf>,
        to: &str,
    ) -> Result<mpsc::Receiver<ConvertSummary>, String> {
        if files.is_empty() {
            return Err("There are no files to convert.".into());
        }
        let target = formats::lookup_target(to)
            .filter(|f| f.writable)
            .ok_or_else(|| format!("MorphRing cannot write the format \"{to}\"."))?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (reply, summary) = mpsc::channel();
        self.showing.store(id, Ordering::SeqCst);
        let queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        queue
            .send(Request {
                id,
                files,
                target: target.id,
                reply,
            })
            .map_err(|_| "The conversion queue has stopped. Restart MorphRing.".to_owned())?;
        Ok(summary)
    }

    /// Queues the conversion and waits for it without blocking the async
    /// runtime: the wait happens on a blocking thread.
    pub async fn convert(&self, files: Vec<String>, to: String) -> Result<ConvertSummary, String> {
        let files = files.into_iter().map(PathBuf::from).collect();
        let summary = self.submit(files, &to)?;
        tauri::async_runtime::spawn_blocking(move || summary.recv())
            .await
            .map_err(|e| format!("The conversion could not be awaited: {e}"))?
            .map_err(|_| "The conversion stopped unexpectedly.".to_owned())
    }
}

/// The worker thread's half of [`Conversions`].
struct Worker {
    engines: Engines,
    showing: Arc<AtomicU64>,
    sink: EventSink,
}

impl Worker {
    fn run(self, requests: mpsc::Receiver<Request>) {
        // Ask every engine for its status while nobody is waiting yet. The first
        // ring then doesn't stall on three `--version` probes.
        for engine in self.engines.iter() {
            let _ = engine.status();
        }
        for request in requests {
            let id = request.id;
            // run_job already catches panics per file; this is the belt to its
            // braces, so one cursed job never takes the whole queue down with it.
            let summary = panic::catch_unwind(AssertUnwindSafe(|| self.run_job(&request)))
                .unwrap_or_else(|_| failed_summary("The conversion stopped unexpectedly."));
            if let Some(message) = &summary.message {
                tracing::warn!(job = id, "conversion failed: {message}");
            }
            self.emit(
                id,
                RingEvent::Done(RingDone {
                    outcome: summary.outcome,
                    message: summary.message.clone(),
                }),
            );
            // The caller may have stopped listening (the ring was closed); the
            // files are converted either way, so that is not an error.
            let _ = request.reply.send(summary);
        }
    }

    fn run_job(&self, request: &Request) -> ConvertSummary {
        let engines: Vec<&dyn Converter> = self.engines.iter().map(|e| e.as_ref()).collect();
        let cancel = CancelToken::new();
        let progress = Mutex::new(Progress::new(request.files.len()));
        let on_event = |event: JobEvent| {
            let update = progress
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .apply(&event);
            if let Some(value) = update {
                self.emit(
                    request.id,
                    RingEvent::Progress(RingProgress { progress: value }),
                );
            }
        };
        tracing::info!(
            job = request.id,
            files = request.files.len(),
            to = %request.target,
            "conversion started"
        );
        let results = job::run_job(
            &request.files,
            request.target,
            &ConvertOptions::default(),
            &OutputRule::default(),
            &engines,
            &cancel,
            &on_event,
        );
        summarize(&results, cancel.is_cancelled())
    }

    fn emit(&self, id: u64, event: RingEvent) {
        // The ring only ever shows one job. Events from an older one would yank
        // the new progress ring around, so they quietly go nowhere.
        if self.showing.load(Ordering::SeqCst) == id {
            (self.sink)(event);
        }
    }
}

/// The job's overall progress: the mean of its files' progress (Filee's
/// `ConversionJob.Progress`), sent only when it moved by a visible amount.
struct Progress {
    files: Vec<f32>,
    sent: f32,
}

impl Progress {
    /// Smallest step worth an event: the ring prints whole percents, and
    /// a webview IPC message per engine tick would be a lot of chatter for nothing.
    const STEP: f32 = 0.01;

    fn new(count: usize) -> Self {
        Self {
            files: vec![0.0; count],
            sent: 0.0,
        }
    }

    /// Records `event` and returns the new job progress if it should be sent.
    fn apply(&mut self, event: &JobEvent) -> Option<f32> {
        let (index, value) = match event {
            JobEvent::FileStarted { .. } => return None,
            JobEvent::FileProgress { index, progress } => (*index, *progress),
            // A failed file is finished too; the ring fills up either way and the
            // outcome colours it afterwards.
            JobEvent::FileFinished { index, .. } => (*index, 1.0),
        };
        let slot = self.files.get_mut(index)?;
        *slot = slot.max(value.clamp(0.0, 1.0));
        let total = self.files.iter().sum::<f32>() / self.files.len().max(1) as f32;
        let total = total.clamp(0.0, 1.0);
        let finished = total >= 1.0 && self.sent < 1.0;
        if finished || total - self.sent >= Self::STEP {
            self.sent = total;
            Some(total)
        } else {
            None
        }
    }
}

/// Turns the file results into what the ring and the caller see, with
/// Filee's outcome mapping: only a fully completed job counts as a success.
fn summarize(results: &[FileResult], cancelled: bool) -> ConvertSummary {
    let outcome = match JobState::from_files(results, cancelled) {
        JobState::Completed => Outcome::Succeeded,
        JobState::Cancelled => Outcome::Cancelled,
        JobState::CompletedWithErrors | JobState::Failed => Outcome::Failed,
    };
    let failures: Vec<(&Path, ErrorKey, &str)> = results
        .iter()
        .filter_map(|r| match &r.state {
            FileState::Failed { key, detail } => Some((r.source.as_path(), *key, detail.as_str())),
            _ => None,
        })
        .collect();
    let message = (outcome == Outcome::Failed).then(|| failure_message(results.len(), &failures));
    ConvertSummary {
        outcome,
        outputs: results
            .iter()
            .flat_map(|r| r.outputs.iter().map(|p| display(p)))
            .collect(),
        done: results
            .iter()
            .filter(|r| r.state == FileState::Done)
            .count(),
        failed: failures.len(),
        message,
    }
}

fn failure_message(total: usize, failures: &[(&Path, ErrorKey, &str)]) -> String {
    let Some((source, key, detail)) = failures.first() else {
        return "The conversion failed.".into();
    };
    let reason = DisabledReason {
        key: *key,
        detail: (!detail.is_empty()).then(|| (*detail).to_owned()),
    };
    let name = source
        .file_name()
        .map_or_else(|| display(source), |n| n.to_string_lossy().into_owned());
    if total == 1 {
        format!("{name}: {reason}")
    } else {
        format!(
            "{} of {total} files failed. {name}: {reason}",
            failures.len()
        )
    }
}

fn failed_summary(message: &str) -> ConvertSummary {
    ConvertSummary {
        outcome: Outcome::Failed,
        outputs: Vec::new(),
        done: 0,
        failed: 0,
        message: Some(message.to_owned()),
    }
}

fn display(path: &Path) -> String {
    // The webview speaks UTF-8 only. A lossy name is fine for showing; the
    // PathBufs that matter never leave Rust except through `convert`, which
    // gets back exactly what the ring was sent.
    path.to_string_lossy().into_owned()
}

/// The app's conversion queue, created on first use with every engine.
fn conversions<R: Runtime>(app: &AppHandle<R>) -> tauri::State<'_, Conversions> {
    // Same trick as instance.rs: lib.rs is shared, so the state is created here
    // on first use. If two threads race, `manage` keeps the first one and the
    // loser's worker exits as soon as its queue is dropped.
    if app.try_state::<Conversions>().is_none() {
        let handle = app.clone();
        let sink: EventSink = Arc::new(move |event| send_to_ring(&handle, event));
        app.manage(Conversions::new(morphring_engines::registry::all(), sink));
    }
    app.state::<Conversions>()
}

fn send_to_ring<R: Runtime>(app: &AppHandle<R>, event: RingEvent) {
    let sent = match &event {
        RingEvent::Progress(p) => app.emit_to(RING_WINDOW, RING_PROGRESS, p),
        RingEvent::Done(d) => app.emit_to(RING_WINDOW, RING_DONE, d),
    };
    if let Err(e) = sent {
        tracing::warn!("could not send progress to the ring: {e}");
    }
}

/// Builds the ring for `files`, sends it to the ring window and shows it.
///
/// Returns at once; the work happens on a blocking thread, because the first
/// call may wait for engine status probes.
pub fn open_ring<R: Runtime>(app: &AppHandle<R>, files: Vec<PathBuf>) {
    if files.is_empty() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(ring) = app.get_webview_window(RING_WINDOW) else {
            tracing::warn!("the ring window is missing; cannot open the ring");
            return;
        };
        let model = conversions(&app).ring_model(&files);
        tracing::debug!(
            files = model.files.len(),
            slices = model.slices.len(),
            "opening the ring"
        );
        // The ring wants its model before it becomes visible, or it would flash
        // last time's slices for a frame.
        if let Err(e) = ring.emit_to(RING_WINDOW, RING_OPEN, model) {
            tracing::warn!("could not send the files to the ring: {e}");
            return;
        }
        // Filee opens the ring under the cursor. On Wayland nobody gets to know
        // where the cursor is, so the ring sits in the middle of the screen.
        if let Err(e) = ring.center() {
            tracing::debug!("could not center the ring: {e}");
        }
        if let Err(e) = ring.show() {
            tracing::warn!("could not show the ring: {e}");
            return;
        }
        if let Err(e) = ring.set_focus() {
            tracing::debug!("could not focus the ring: {e}");
        }
    });
}

/// Health check for the frontend: answers `"pong"`.
#[tauri::command]
pub fn ping() -> &'static str {
    "pong"
}

/// Converts `files` to the format `to` and answers when the job has ended.
///
/// Progress and the outcome also reach the ring window as [`RING_PROGRESS`]
/// and [`RING_DONE`] events. Jobs run one at a time, in the order they were
/// asked for. The error is for a request that could not be queued.
#[tauri::command]
pub async fn convert<R: Runtime>(
    app: AppHandle<R>,
    files: Vec<String>,
    to: String,
) -> Result<ConvertSummary, String> {
    conversions(&app).convert(files, to).await
}

/// Hides the ring window. Does nothing when the ring is not open.
///
/// A running conversion carries on; only the ring goes away.
#[tauri::command]
pub fn ring_close<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    // The ring is created once and reused, so "close" means hide; tearing the
    // webview down every time would make the next open visibly slower.
    match app.get_webview_window(RING_WINDOW) {
        Some(ring) => ring
            .hide()
            .map_err(|e| format!("Could not hide the ring: {e}")),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use morphring_core::{Edge, EngineError, OutputAllocator, Step};
    use std::fs;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    /// A tiny engine: copies the input to the allocated output, optionally
    /// failing or dawdling, and remembers how many conversions ran at once.
    struct Fake {
        id: &'static str,
        name: &'static str,
        edges: Vec<Edge>,
        missing: Option<&'static str>,
        fail: bool,
        delay: Duration,
        running: AtomicUsize,
        peak: AtomicUsize,
        order: Mutex<Vec<PathBuf>>,
    }

    impl Fake {
        fn new(
            id: &'static str,
            name: &'static str,
            edges: &[(&'static str, &'static str)],
        ) -> Self {
            Self {
                id,
                name,
                edges: edges.iter().map(|(f, t)| Edge::new(f, t)).collect(),
                missing: None,
                fail: false,
                delay: Duration::ZERO,
                running: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                order: Mutex::new(Vec::new()),
            }
        }
    }

    impl Converter for Fake {
        fn id(&self) -> &'static str {
            self.id
        }
        fn display_name(&self) -> &'static str {
            self.name
        }
        fn edges(&self) -> &[Edge] {
            &self.edges
        }
        fn status(&self) -> EngineStatus {
            match self.missing {
                Some(hint) => EngineStatus::Missing { hint: hint.into() },
                None => EngineStatus::Available {
                    path: PathBuf::from("/usr/bin/fake"),
                    version: "1".into(),
                },
            }
        }
        fn convert(
            &self,
            step: &Step,
            out: &mut dyn OutputAllocator,
            progress: &dyn Fn(f32),
            _cancel: &CancelToken,
        ) -> Result<Vec<PathBuf>, EngineError> {
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            self.order.lock().unwrap().push(step.input.clone());
            thread::sleep(self.delay);
            progress(0.5);
            let result = if self.fail {
                Err(EngineError::Failed("fake exploded".into()))
            } else {
                let ext = formats::find_by_id(step.to.as_str())
                    .unwrap()
                    .primary_extension();
                let path = out.allocate(ext, "").unwrap();
                fs::copy(&step.input, &path).unwrap();
                progress(1.0);
                Ok(vec![path])
            };
            self.running.fetch_sub(1, Ordering::SeqCst);
            result
        }
    }

    fn touch(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"pixels").unwrap();
        path
    }

    fn recorder() -> (EventSink, Arc<Mutex<Vec<RingEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);
        let sink: EventSink = Arc::new(move |e| seen.lock().unwrap().push(e));
        (sink, events)
    }

    fn images() -> Fake {
        Fake::new(
            "img",
            "Imagey",
            &[("png", "jpg"), ("png", "webp"), ("jpg", "png")],
        )
    }

    fn office_missing() -> Fake {
        let mut office = Fake::new("office", "LibreOffice", &[("png", "pdf")]);
        office.missing = Some("pacman -S libreoffice-fresh");
        office
    }

    fn slice<'a>(model: &'a RingModel, target: &str) -> &'a RingSlice {
        model
            .slices
            .iter()
            .find(|s| s.target == target)
            .unwrap_or_else(|| panic!("no {target} slice in {model:?}"))
    }

    #[test]
    fn ping_answers_pong() {
        assert_eq!(ping(), "pong");
    }

    #[test]
    fn ring_model_offers_targets_with_reasons() {
        let (img, office) = (images(), office_missing());
        let files = vec![PathBuf::from("/a/one.png"), PathBuf::from("/a/two.PNG")];
        let model = ring_model(&files, &[&img, &office]);

        assert_eq!(model.title, "2 × PNG");
        assert_eq!(model.files, ["/a/one.png", "/a/two.PNG"]);
        assert!(model.slices.len() <= slices::MAX_SLICES);
        assert!(
            model.slices.iter().all(|s| s.target != "png"),
            "no png→png slice"
        );

        let jpg = slice(&model, "jpg");
        assert!(jpg.enabled);
        assert_eq!(jpg.label, "JPG");
        assert_eq!(jpg.disabled_reason, None);

        let pdf = slice(&model, "pdf");
        assert!(!pdf.enabled);
        assert_eq!(
            pdf.disabled_reason.as_deref(),
            Some(
                "A required program is not installed (needs LibreOffice: pacman -S libreoffice-fresh)"
            )
        );

        let tiff = slice(&model, "tiff");
        assert!(!tiff.enabled);
        assert_eq!(
            tiff.disabled_reason.as_deref(),
            Some("No engine can do this conversion")
        );
    }

    #[test]
    fn ring_model_for_unknown_files_disables_everything() {
        let img = images();
        let model = ring_model(&[PathBuf::from("notes.unknownext")], &[&img]);
        assert!(!model.slices.is_empty());
        for s in &model.slices {
            assert!(!s.enabled);
            assert_eq!(
                s.disabled_reason.as_deref(),
                Some("This file type isn't supported")
            );
        }
    }

    #[test]
    fn ring_model_is_the_camel_case_json_the_ring_reads() {
        let model = RingModel {
            files: vec!["/a.png".into()],
            title: "1 × PNG".into(),
            slices: vec![
                RingSlice {
                    target: "jpg".into(),
                    label: "JPG".into(),
                    enabled: true,
                    disabled_reason: None,
                },
                RingSlice {
                    target: "pdf".into(),
                    label: "PDF".into(),
                    enabled: false,
                    disabled_reason: Some("why".into()),
                },
            ],
        };
        assert_eq!(
            serde_json::to_value(&model).unwrap(),
            serde_json::json!({
                "files": ["/a.png"],
                "title": "1 × PNG",
                "slices": [
                    { "target": "jpg", "label": "JPG", "enabled": true },
                    { "target": "pdf", "label": "PDF", "enabled": false, "disabledReason": "why" }
                ]
            })
        );
        let done = RingDone {
            outcome: Outcome::Succeeded,
            message: None,
        };
        assert_eq!(
            serde_json::to_value(done).unwrap(),
            serde_json::json!({ "outcome": "succeeded" })
        );
        assert_eq!(
            serde_json::to_value(RingProgress { progress: 0.5 }).unwrap(),
            serde_json::json!({ "progress": 0.5 })
        );
    }

    #[test]
    fn convert_runs_the_job_and_reports_progress_then_done() {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<String> = ["a.png", "b.png", "c.png"]
            .iter()
            .map(|n| display(&touch(dir.path(), n)))
            .collect();
        let (sink, events) = recorder();
        let conv = Conversions::new(vec![Box::new(images())], sink);

        let summary = tauri::async_runtime::block_on(conv.convert(files, "JPEG".into())).unwrap();

        assert_eq!(summary.outcome, Outcome::Succeeded);
        assert_eq!(summary.done, 3);
        assert_eq!(summary.failed, 0);
        assert_eq!(summary.message, None);
        assert_eq!(summary.outputs.len(), 3);
        for name in ["a.jpg", "b.jpg", "c.jpg"] {
            assert!(dir.path().join(name).is_file(), "{name} missing");
        }

        let events = events.lock().unwrap();
        let (last, progress) = events.split_last().unwrap();
        assert_eq!(
            *last,
            RingEvent::Done(RingDone {
                outcome: Outcome::Succeeded,
                message: None
            })
        );
        let values: Vec<f32> = progress
            .iter()
            .map(|e| match e {
                RingEvent::Progress(p) => p.progress,
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert!(!values.is_empty());
        assert!(values.windows(2).all(|w| w[0] < w[1]), "{values:?}");
        assert_eq!(values.last(), Some(&1.0));
    }

    #[test]
    fn a_failing_file_fails_the_ring_with_a_message() {
        let dir = tempfile::tempdir().unwrap();
        let good = touch(dir.path(), "good.jpg");
        let bad = touch(dir.path(), "bad.txt");
        let (sink, events) = recorder();
        let conv = Conversions::new(vec![Box::new(images())], sink);

        let summary = conv.submit(vec![good, bad], "png").unwrap().recv().unwrap();

        assert_eq!(summary.outcome, Outcome::Failed);
        assert_eq!((summary.done, summary.failed), (1, 1));
        let message = summary.message.clone().unwrap();
        assert_eq!(
            message,
            "1 of 2 files failed. bad.txt: No engine can do this conversion (txt → png)"
        );
        assert_eq!(
            events.lock().unwrap().last(),
            Some(&RingEvent::Done(RingDone {
                outcome: Outcome::Failed,
                message: Some(message)
            }))
        );
    }

    #[test]
    fn an_engine_error_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut img = images();
        img.fail = true;
        let (sink, _) = recorder();
        let conv = Conversions::new(vec![Box::new(img)], sink);
        let summary = conv
            .submit(vec![touch(dir.path(), "cat.png")], "jpg")
            .unwrap()
            .recv()
            .unwrap();
        assert_eq!(summary.outcome, Outcome::Failed);
        let message = summary.message.unwrap();
        assert!(
            message.starts_with("cat.png: The engine reported an error ("),
            "{message}"
        );
        assert!(message.contains("fake exploded"), "{message}");
    }

    #[test]
    fn bad_requests_are_refused_before_queueing() {
        let (sink, events) = recorder();
        let conv = Conversions::new(vec![Box::new(images())], sink);
        let err = conv
            .submit(vec![PathBuf::from("a.png")], "nope")
            .unwrap_err();
        assert_eq!(err, "MorphRing cannot write the format \"nope\".");
        let err = conv.submit(Vec::new(), "jpg").unwrap_err();
        assert_eq!(err, "There are no files to convert.");
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn jobs_run_one_at_a_time_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut img = images();
        img.delay = Duration::from_millis(30);
        let img = Arc::new(img);

        // Box<dyn Converter> around an Arc, so the test can peek afterwards.
        struct Shared(Arc<Fake>);
        impl Converter for Shared {
            fn id(&self) -> &'static str {
                self.0.id()
            }
            fn display_name(&self) -> &'static str {
                self.0.display_name()
            }
            fn edges(&self) -> &[Edge] {
                self.0.edges()
            }
            fn status(&self) -> EngineStatus {
                self.0.status()
            }
            fn convert(
                &self,
                step: &Step,
                out: &mut dyn OutputAllocator,
                progress: &dyn Fn(f32),
                cancel: &CancelToken,
            ) -> Result<Vec<PathBuf>, EngineError> {
                self.0.convert(step, out, progress, cancel)
            }
        }

        let (sink, _) = recorder();
        let conv = Conversions::new(vec![Box::new(Shared(Arc::clone(&img)))], sink);
        let sources: Vec<PathBuf> = (0..4)
            .map(|i| touch(dir.path(), &format!("f{i}.png")))
            .collect();
        let pending: Vec<_> = sources
            .iter()
            .map(|s| conv.submit(vec![s.clone()], "jpg").unwrap())
            .collect();
        for summary in pending {
            assert_eq!(summary.recv().unwrap().outcome, Outcome::Succeeded);
        }

        assert_eq!(img.peak.load(Ordering::SeqCst), 1, "jobs overlapped");
        assert_eq!(*img.order.lock().unwrap(), sources);
    }

    #[test]
    fn only_the_job_on_the_ring_sends_events() {
        let dir = tempfile::tempdir().unwrap();
        let mut img = images();
        img.delay = Duration::from_millis(20);
        let (sink, events) = recorder();
        let conv = Conversions::new(vec![Box::new(img)], sink);

        let first = conv
            .submit(vec![touch(dir.path(), "a.png")], "jpg")
            .unwrap();
        // The user reopened the ring for other files before the first job ended.
        let _ = conv.ring_model(&[dir.path().join("b.png")]);
        assert_eq!(first.recv().unwrap().outcome, Outcome::Succeeded);
        assert!(events.lock().unwrap().is_empty());

        let second = conv
            .submit(vec![touch(dir.path(), "b.png")], "jpg")
            .unwrap();
        assert_eq!(second.recv().unwrap().outcome, Outcome::Succeeded);
        assert!(matches!(
            events.lock().unwrap().last(),
            Some(RingEvent::Done(RingDone {
                outcome: Outcome::Succeeded,
                ..
            }))
        ));
    }

    #[test]
    fn progress_is_the_mean_and_skips_tiny_steps() {
        let mut p = Progress::new(2);
        let at = |index, progress| JobEvent::FileProgress { index, progress };
        assert_eq!(p.apply(&at(0, 0.5)), Some(0.25));
        assert_eq!(p.apply(&at(0, 0.51)), None, "0.5 % is not worth an event");
        assert_eq!(p.apply(&at(0, 0.4)), None, "progress never goes back");
        let both = p.apply(&at(1, 1.0)).unwrap();
        assert!((both - 0.755).abs() < 1e-6, "{both}");
        let finished = JobEvent::FileFinished {
            index: 0,
            result: FileResult {
                source: PathBuf::from("a"),
                state: FileState::Done,
                outputs: Vec::new(),
            },
        };
        assert_eq!(p.apply(&finished), Some(1.0));
        assert_eq!(p.apply(&finished), None);
        assert_eq!(p.apply(&at(7, 1.0)), None, "unknown index is ignored");
    }

    #[test]
    fn cancelled_jobs_say_so() {
        let results = vec![FileResult {
            source: PathBuf::from("/x/a.png"),
            state: FileState::Cancelled,
            outputs: Vec::new(),
        }];
        let summary = summarize(&results, true);
        assert_eq!(summary.outcome, Outcome::Cancelled);
        assert_eq!(summary.message, None);
    }
}
