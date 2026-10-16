//! Commands the webviews can invoke, plus the conversion queue behind them.
//!
//! The ring's life: [`open_ring`] builds a [`RingModel`] for some files from
//! the user's preset library and sends it as [`RING_OPEN`]; the user clicks a
//! slice and the ring invokes [`convert`] with the slice's preset id; the job
//! runs that preset (its target, options and output rule, with the settings'
//! default output filled in) on the queue's worker thread while
//! [`RING_PROGRESS`] events move the progress ring; [`RING_DONE`] says how it
//! ended. Ported from Filee's `src/Filee.App/Services/RingController.cs`
//! (convert and progress) and `ViewModels/RadialViewModel.cs` (the model).
//!
//! Everything that can be tested without a window lives in [`Conversions`] and
//! [`ring_model`]; the Tauri commands are thin wrappers around them.
//!
//! # Job events
//!
//! Every queued conversion is a job with a `jobId` (a `u64`, starting at 1 and
//! going up by one per job, never reused while the app runs). Besides the ring
//! events above, which only reach the ring and only for the job it shows, every
//! job sends three app-wide events that any window may listen for (the toast
//! and history windows do):
//!
//! | event          | payload                                  | when                          |
//! |----------------|------------------------------------------|-------------------------------|
//! | `job-started`  | [`JobStarted`] `{ jobId, files, label }` | the worker starts the job     |
//! | `job-progress` | [`JobProgress`] `{ jobId, progress }`    | progress moved by 1 % or more |
//! | `job-finished` | [`JobFinished`] `{ jobId, summary }`     | the job ended, any outcome    |
//!
//! `files` are the source paths, `label` is what the job converts to (the
//! preset's display name, e.g. `JPG`), `progress` is `0.0..=1.0` and only goes
//! up, and `summary` is the [`ConvertSummary`] that [`convert`] answers with.
//! `job-started` comes first and `job-finished` last for each job; jobs run one
//! at a time, so their events never interleave. After `job-finished` the job
//! is also handed to [`crate::history::record`].
//!
//! # Cancelling
//!
//! [`cancel_job`] stops a job by id. A running job stops its engines and
//! deletes the partial outputs; a job still waiting in the queue never starts.
//! Either way the job ends with the [`Outcome::Cancelled`] outcome. The ring
//! learns which job it shows from the `jobId` of [`RING_PROGRESS`], whose
//! first event (progress 0) is sent as soon as the job is queued.

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;

use morphring_core::formats;
use morphring_core::job::{self, JobEvent};
use morphring_core::output::OutputRule;
use morphring_core::planner::Planner;
use morphring_core::preset::{self, Preset};
use morphring_core::profiles::Library;
use morphring_core::settings::AppSettings;
use morphring_core::slices::{self, DisabledReason, Reach, ReachCtx};
use morphring_core::{
    CancelToken, ConvertOptions, Converter, EngineStatus, ErrorKey, FileResult, FileState,
    FormatId, JobState, Target,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::history::JobRecord;

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

/// App-wide event sent with a [`JobStarted`] payload when a job starts running.
pub const JOB_STARTED: &str = "job-started";

/// App-wide event sent with a [`JobProgress`] payload while a job runs.
pub const JOB_PROGRESS: &str = "job-progress";

/// App-wide event sent with a [`JobFinished`] payload when a job has ended.
pub const JOB_FINISHED: &str = "job-finished";

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

/// One slice of the ring: a preset.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RingSlice {
    /// Id of the preset, passed to [`convert`] as `presetId`, e.g. `half-size`.
    pub preset_id: String,
    /// The preset's target: a format id such as `png`, or `*` for "same
    /// format as the source".
    pub target: String,
    /// Text drawn on the slice: the preset's display name, e.g. `Half size`.
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
    /// The job the ring shows, as [`cancel_job`] wants it.
    pub job_id: u64,
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

/// Payload of [`JOB_STARTED`].
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JobStarted {
    pub job_id: u64,
    /// The source files, in input order.
    pub files: Vec<String>,
    /// What the job converts to, e.g. `JPG`.
    pub label: String,
}

/// Payload of [`JOB_PROGRESS`].
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobProgress {
    pub job_id: u64,
    /// Progress of the whole job, `0.0..=1.0`. Only goes up.
    pub progress: f32,
}

/// Payload of [`JOB_FINISHED`].
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JobFinished {
    pub job_id: u64,
    pub summary: ConvertSummary,
}

/// Something the whole app should be told about a job.
#[derive(Debug, Clone, PartialEq)]
pub enum JobNotice {
    Started(JobStarted),
    Progress(JobProgress),
    /// The event payload, plus the full record for the history. Boxed, because
    /// a preset is a hefty thing to haul around in every progress tick.
    Finished(JobFinished, Box<JobRecord>),
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

/// Builds the ring for `files` from the presets and toolbar profiles in
/// `library`: the profile that fits the files picks the slices, in its order.
///
/// A slice is enabled when at least one file can be converted to its target
/// with the installed `engines`; a disabled slice says why, naming the
/// program to install when one would help.
pub fn ring_model(files: &[PathBuf], engines: &[&dyn Converter], library: &Library) -> RingModel {
    // Two planners, as D10 says: what we can do now, and what we could do if
    // every engine were installed. The second one turns "no" into "not yet".
    let installed = Planner::new(engines, false);
    let every = Planner::new(engines, true);
    let ctx = ReachCtx {
        reach: |from, to| reach(&installed, &every, engines, from, to),
        can_merge: engines
            .iter()
            .any(|e| e.status().is_available() && e.merger().is_some()),
    };
    let suggestion = slices::suggest(files, library, &ctx);
    RingModel {
        files: files.iter().map(|p| display(p)).collect(),
        title: suggestion.title,
        slices: suggestion
            .slices
            .into_iter()
            .map(|slice| RingSlice {
                enabled: slice.is_enabled(),
                preset_id: slice.preset_id,
                target: slice.target,
                label: slice.label,
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

/// Where job notices go. The app emits them app-wide and records history;
/// tests collect them.
pub type JobSink = Arc<dyn Fn(JobNotice) + Send + Sync>;

/// Where the queue reads the app settings and the preset library from, fresh
/// for every ring and every job. The app reads its managed settings; tests
/// hand over a fixed pair.
pub type PrefsSource = Arc<dyn Fn() -> (AppSettings, Library) + Send + Sync>;

type Engines = Arc<Vec<Box<dyn Converter>>>;

/// One conversion waiting in the queue, with the preset already turned into
/// everything `run_job` wants.
struct Request {
    id: u64,
    files: Vec<PathBuf>,
    target: Target,
    options: ConvertOptions,
    rule: OutputRule,
    /// The preset as the user saved it, for the history.
    preset: Preset,
    /// The preset's display name: the job label and the `{preset}` token.
    label: String,
    /// Fired by [`Conversions::cancel`]; also kept on the [`Board`] by id.
    cancel: CancelToken,
    reply: mpsc::Sender<ConvertSummary>,
}

/// The jobs that can still be cancelled, queued or running, by id. Shared by
/// [`Conversions::cancel`] and the worker, which wipes a job off the board
/// once it has ended.
#[derive(Default)]
struct Board {
    tokens: HashMap<u64, CancelToken>,
    /// Id of the job the worker is running; 0 between jobs.
    running: u64,
}

type SharedBoard = Arc<Mutex<Board>>;

fn lock(board: &SharedBoard) -> std::sync::MutexGuard<'_, Board> {
    // A panic while holding this lock leaves a map and a number behind, both
    // still perfectly sensible. Poison is for food, not for job boards.
    board.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Sends `event` to the ring, but only when the ring shows job `id`.
fn emit_if_showing(showing: &AtomicU64, sink: &EventSink, id: u64, event: RingEvent) {
    // The ring only ever shows one job. Events from an older one would yank
    // the new progress ring around, so they quietly go nowhere.
    if showing.load(Ordering::SeqCst) == id {
        sink(event);
    }
}

/// The conversion queue: one worker thread that runs jobs one at a time, in
/// the order they were submitted.
pub struct Conversions {
    engines: Engines,
    prefs: PrefsSource,
    queue: Mutex<mpsc::Sender<Request>>,
    /// Id of the job the ring is showing; 0 when it shows a menu or nothing.
    showing: Arc<AtomicU64>,
    next_id: AtomicU64,
    board: SharedBoard,
    sink: EventSink,
}

impl Conversions {
    /// Starts the worker thread with a fixed preset `library` and
    /// `settings`. `sink` receives the events of the job the ring is
    /// showing, from the worker thread.
    pub fn new(
        engines: Vec<Box<dyn Converter>>,
        sink: EventSink,
        library: Library,
        settings: AppSettings,
    ) -> Self {
        // Nobody listening for job notices? Then they fall on the floor, softly.
        Self::with_job_sink(engines, sink, Arc::new(|_| {}), library, settings)
    }

    /// Like [`Conversions::new`], and every job also reports to `jobs`, from
    /// the worker thread, whether the ring shows it or not.
    pub fn with_job_sink(
        engines: Vec<Box<dyn Converter>>,
        sink: EventSink,
        jobs: JobSink,
        library: Library,
        settings: AppSettings,
    ) -> Self {
        let prefs: PrefsSource = Arc::new(move || (settings.clone(), library.clone()));
        Self::with_prefs(engines, sink, jobs, prefs)
    }

    /// Like [`Conversions::with_job_sink`], but the settings and the library
    /// are read from `prefs` each time they are needed, so edits made on the
    /// settings page reach the next ring without a restart.
    pub fn with_prefs(
        engines: Vec<Box<dyn Converter>>,
        sink: EventSink,
        jobs: JobSink,
        prefs: PrefsSource,
    ) -> Self {
        let engines: Engines = Arc::new(engines);
        let showing = Arc::new(AtomicU64::new(0));
        let board = SharedBoard::default();
        let (queue, requests) = mpsc::channel::<Request>();
        let worker = Worker {
            engines: Arc::clone(&engines),
            showing: Arc::clone(&showing),
            board: Arc::clone(&board),
            sink: Arc::clone(&sink),
            jobs,
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
            prefs,
            queue: Mutex::new(queue),
            showing,
            next_id: AtomicU64::new(1),
            board,
            sink,
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
        let (_, library) = (self.prefs)();
        ring_model(files, &self.engines(), &library)
    }

    /// Queues `files` for conversion with the preset `preset_id` and returns
    /// where the summary will arrive. The new job becomes the one the ring
    /// shows.
    ///
    /// The preset is looked up by id, else by display name (both ignoring
    /// case). Its output rule gets the settings' default output filled in,
    /// and its display name stands in for `{preset}` in the file name.
    pub fn submit(
        &self,
        files: Vec<PathBuf>,
        preset_id: &str,
    ) -> Result<mpsc::Receiver<ConvertSummary>, String> {
        if files.is_empty() {
            return Err("There are no files to convert.".into());
        }
        let (settings, library) = (self.prefs)();
        let preset = preset::find(&library.presets, preset_id)
            .cloned()
            .ok_or_else(|| format!("There is no preset \"{preset_id}\"."))?;
        let target = preset
            .job_target()
            .filter(is_writable)
            .ok_or_else(|| format!("MorphRing cannot write the format \"{}\".", preset.target))?;
        let label = preset.display_name();
        let mut rule = settings.with_default_output(&preset.output);
        rule.pattern = with_preset_name(&rule.pattern, &label);
        let options = preset.options();

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (reply, summary) = mpsc::channel();
        let cancel = CancelToken::new();
        self.showing.store(id, Ordering::SeqCst);
        // Introduce the job to the ring before the worker can say a word about
        // it: this is how the ring learns the id that Escape has to cancel,
        // even while the job is still stuck behind someone else's.
        emit_if_showing(
            &self.showing,
            &self.sink,
            id,
            RingEvent::Progress(RingProgress {
                job_id: id,
                progress: 0.0,
            }),
        );
        lock(&self.board).tokens.insert(id, cancel.clone());
        let queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        let sent = queue.send(Request {
            id,
            files,
            target,
            options,
            rule,
            preset,
            label,
            cancel,
            reply,
        });
        if sent.is_err() {
            lock(&self.board).tokens.remove(&id);
            return Err("The conversion queue has stopped. Restart MorphRing.".to_owned());
        }
        Ok(summary)
    }

    /// Cancels job `job_id` and answers whether there was anything to cancel.
    ///
    /// A running job stops as soon as its engines notice, and its partial
    /// outputs are deleted. A queued job will not run at all: the ring hears
    /// [`Outcome::Cancelled`] right away, and its summary and job notices come
    /// when its turn in the queue arrives. A job that already ended, or an id
    /// that never existed, is left alone.
    pub fn cancel(&self, job_id: u64) -> bool {
        let queued = {
            let board = lock(&self.board);
            let Some(token) = board.tokens.get(&job_id) else {
                return false;
            };
            token.cancel();
            board.running != job_id
        };
        tracing::info!(job = job_id, queued, "conversion cancelled");
        if queued {
            // The worker will skip it without a word to the ring, so the ring
            // hears it from us, now, instead of after the job in front ends.
            emit_if_showing(
                &self.showing,
                &self.sink,
                job_id,
                RingEvent::Done(RingDone {
                    outcome: Outcome::Cancelled,
                    message: None,
                }),
            );
        }
        true
    }

    /// Queues the conversion and waits for it without blocking the async
    /// runtime: the wait happens on a blocking thread.
    pub async fn convert(
        &self,
        files: Vec<String>,
        preset_id: String,
    ) -> Result<ConvertSummary, String> {
        let files = files.into_iter().map(PathBuf::from).collect();
        let summary = self.submit(files, &preset_id)?;
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
    board: SharedBoard,
    sink: EventSink,
    jobs: JobSink,
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
            // Checked under the board lock: `cancel` then sees this job either
            // as queued (and we skip it) or as running (and run_job stops),
            // never as some third thing that falls between the chairs.
            let skipped = {
                let mut board = lock(&self.board);
                board.running = id;
                request.cancel.is_cancelled()
            };
            (self.jobs)(JobNotice::Started(JobStarted {
                job_id: id,
                files: request.files.iter().map(|p| display(p)).collect(),
                label: request.label.clone(),
            }));
            let (summary, record) = if skipped {
                cancelled_before_start(&request)
            } else {
                // run_job already catches panics per file; this is the belt to
                // its braces, so one cursed job never takes the queue down.
                panic::catch_unwind(AssertUnwindSafe(|| self.run_job(&request))).unwrap_or_else(
                    |_| {
                        let record = JobRecord {
                            job_id: id,
                            preset: request.preset.clone(),
                            state: JobState::Failed,
                            results: Vec::new(),
                        };
                        (
                            failed_summary("The conversion stopped unexpectedly."),
                            record,
                        )
                    },
                )
            };
            {
                let mut board = lock(&self.board);
                board.tokens.remove(&id);
                board.running = 0;
            }
            if let Some(message) = &summary.message {
                tracing::warn!(job = id, "conversion failed: {message}");
            }
            // A skipped job already told the ring when it was cancelled.
            if !skipped {
                self.emit(
                    id,
                    RingEvent::Done(RingDone {
                        outcome: summary.outcome,
                        message: summary.message.clone(),
                    }),
                );
            }
            (self.jobs)(JobNotice::Finished(
                JobFinished {
                    job_id: id,
                    summary: summary.clone(),
                },
                Box::new(record),
            ));
            // The caller may have stopped listening (the ring was closed); the
            // files are converted either way, so that is not an error.
            let _ = request.reply.send(summary);
        }
    }

    fn run_job(&self, request: &Request) -> (ConvertSummary, JobRecord) {
        let engines: Vec<&dyn Converter> = self.engines.iter().map(|e| e.as_ref()).collect();
        let cancel = &request.cancel;
        let progress = Mutex::new(Progress::new(request.files.len()));
        let on_event = |event: JobEvent| {
            // Keep the lock until the update is out. Files convert in parallel,
            // and two threads that each computed their value and then raced to
            // the emit could send 67% before 50%: a progress arc doing the
            // moonwalk.
            let mut progress = progress.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(value) = progress.apply(&event) {
                // Same coalescing for both audiences: the ring and the toast print
                // whole percents, so they want exactly the same updates.
                (self.jobs)(JobNotice::Progress(JobProgress {
                    job_id: request.id,
                    progress: value,
                }));
                self.emit(
                    request.id,
                    RingEvent::Progress(RingProgress {
                        job_id: request.id,
                        progress: value,
                    }),
                );
            }
        };
        tracing::info!(
            job = request.id,
            files = request.files.len(),
            preset = %request.preset.id,
            "conversion started"
        );
        let results = job::run_job(
            &request.files,
            request.target,
            &request.options,
            &request.rule,
            &engines,
            cancel,
            &on_event,
        );
        // A cancel that lands after the last file finished stopped nothing, so
        // the job gets its real outcome; "cancelled" with outputs on disk
        // would be a fib.
        let cancelled =
            cancel.is_cancelled() && results.iter().any(|r| r.state == FileState::Cancelled);
        let summary = summarize(&results, cancelled);
        let record = JobRecord {
            job_id: request.id,
            preset: request.preset.clone(),
            state: JobState::from_files(&results, cancelled),
            results,
        };
        (summary, record)
    }

    fn emit(&self, id: u64, event: RingEvent) {
        emit_if_showing(&self.showing, &self.sink, id, event);
    }
}

/// What a job cancelled while it was still queued ends with: every file
/// cancelled, nothing written.
fn cancelled_before_start(request: &Request) -> (ConvertSummary, JobRecord) {
    let results: Vec<FileResult> = request
        .files
        .iter()
        .map(|source| FileResult {
            source: source.clone(),
            state: FileState::Cancelled,
            outputs: Vec::new(),
        })
        .collect();
    let summary = summarize(&results, true);
    let record = JobRecord {
        job_id: request.id,
        preset: request.preset.clone(),
        state: JobState::Cancelled,
        results,
    };
    (summary, record)
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
    let reason = DisabledReason::error(*key, (!detail.is_empty()).then(|| (*detail).to_owned()));
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

/// Whether a job can write `target`. "Same as the source" is writable by
/// definition: the file got here by being a format we read, and the planner
/// sorts out whether we can also write it.
fn is_writable(target: &Target) -> bool {
    match target {
        Target::SameAsSource => true,
        Target::Format(id) => formats::find_by_id(id.as_str()).is_some_and(|f| f.writable),
    }
}

/// Puts `name` in place of every `{preset}` token (any case) in `pattern`.
///
/// `run_job` would fill `{preset}` with the target format's name; the ring
/// wants the preset's own name, so it is baked into the pattern up front.
/// Braces in the name become parentheses, so a preset cheekily called
/// `{name}` can't smuggle a second token into the file name.
fn with_preset_name(pattern: &str, name: &str) -> String {
    const TOKEN: &str = "{preset}";
    let safe = name.replace('{', "(").replace('}', ")");
    let mut out = String::with_capacity(pattern.len());
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        if after
            .get(..TOKEN.len())
            .is_some_and(|t| t.eq_ignore_ascii_case(TOKEN))
        {
            out.push_str(&safe);
            rest = &after[TOKEN.len()..];
        } else {
            out.push('{');
            rest = &after[1..];
        }
    }
    out.push_str(rest);
    out
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
        let handle = app.clone();
        let jobs: JobSink = Arc::new(move |notice| on_job_notice(&handle, notice));
        let handle = app.clone();
        // Read on every ring and every job: a preset edited a minute ago
        // should be on the very next ring, not on the one after a restart.
        let prefs: PrefsSource = Arc::new(move || crate::settings::current(&handle));
        app.manage(Conversions::with_prefs(
            morphring_engines::registry::all(),
            sink,
            jobs,
            prefs,
        ));
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

fn on_job_notice<R: Runtime>(app: &AppHandle<R>, notice: JobNotice) {
    let sent = match &notice {
        JobNotice::Started(started) => app.emit(JOB_STARTED, started),
        JobNotice::Progress(progress) => app.emit(JOB_PROGRESS, progress),
        JobNotice::Finished(finished, _) => app.emit(JOB_FINISHED, finished),
    };
    if let Err(e) = sent {
        tracing::warn!("could not send a job event: {e}");
    }
    if let JobNotice::Finished(_, record) = &notice {
        crate::history::record(app, record);
    }
}

/// The ring model for `files`, as [`open_ring`] would show it.
pub fn ring_model_for<R: Runtime>(app: &AppHandle<R>, files: &[PathBuf]) -> RingModel {
    conversions(app).ring_model(files)
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
        crate::ring_window::show(&ring);
    });
}

/// Health check for the frontend: answers `"pong"`.
#[tauri::command]
pub fn ping() -> &'static str {
    "pong"
}

/// Converts `files` with the preset `presetId` and answers when the job has
/// ended. The preset's target, options and output rule apply; a preset that
/// saves to the default location uses the one from the settings.
///
/// Progress and the outcome also reach the ring window as [`RING_PROGRESS`]
/// and [`RING_DONE`] events. Jobs run one at a time, in the order they were
/// asked for. The error is for a request that could not be queued.
#[tauri::command]
pub async fn convert<R: Runtime>(
    app: AppHandle<R>,
    files: Vec<String>,
    preset_id: String,
) -> Result<ConvertSummary, String> {
    conversions(&app).convert(files, preset_id).await
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

/// Cancels job `jobId`, running or queued; it then ends with the
/// `cancelled` outcome. Does nothing for a job that already ended.
#[tauri::command]
pub fn cancel_job<R: Runtime>(app: AppHandle<R>, job_id: u64) -> Result<(), String> {
    // No queue yet means no job was ever submitted, so there is nothing to
    // cancel. Spinning up every engine just to say "no" would be rude.
    if let Some(conversions) = app.try_state::<Conversions>() {
        conversions.cancel(job_id);
    }
    Ok(())
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

    /// A queue with the factory presets and default settings.
    fn queue(engines: Vec<Box<dyn Converter>>, sink: EventSink) -> Conversions {
        Conversions::new(engines, sink, Library::builtin(), AppSettings::default())
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
        let model = ring_model(&files, &[&img, &office], &Library::builtin());

        assert_eq!(model.title, "2 × PNG");
        assert_eq!(model.files, ["/a/one.png", "/a/two.PNG"]);
        assert!(model.slices.len() <= slices::MAX_SLICES);
        // Presets come as the profile lists them, own format included; with
        // no png self edge in these fakes, that slice simply has no route.
        assert!(!slice(&model, "png").enabled);

        let jpg = slice(&model, "jpg");
        assert!(jpg.enabled);
        assert_eq!(jpg.label, "JPG");
        assert_eq!(jpg.preset_id, "to-jpg");
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
        let model = ring_model(
            &[PathBuf::from("notes.unknownext")],
            &[&img],
            &Library::builtin(),
        );
        assert!(!model.slices.is_empty());
        for s in &model.slices {
            assert!(!s.enabled);
        }
        for target in ["png", "txt"] {
            assert_eq!(
                slice(&model, target).disabled_reason.as_deref(),
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
                    preset_id: "to-jpg".into(),
                    target: "jpg".into(),
                    label: "JPG".into(),
                    enabled: true,
                    disabled_reason: None,
                },
                RingSlice {
                    preset_id: "to-pdf".into(),
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
                    { "presetId": "to-jpg", "target": "jpg", "label": "JPG", "enabled": true },
                    { "presetId": "to-pdf", "target": "pdf", "label": "PDF", "enabled": false, "disabledReason": "why" }
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
            serde_json::to_value(RingProgress {
                job_id: 2,
                progress: 0.5
            })
            .unwrap(),
            serde_json::json!({ "jobId": 2, "progress": 0.5 })
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
        let conv = queue(vec![Box::new(images())], sink);

        let summary = tauri::async_runtime::block_on(conv.convert(files, "to-jpg".into())).unwrap();

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
        let conv = queue(vec![Box::new(images())], sink);

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
        let conv = queue(vec![Box::new(img)], sink);
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
        let conv = queue(vec![Box::new(images())], sink);
        let err = conv
            .submit(vec![PathBuf::from("a.png")], "nope")
            .unwrap_err();
        assert_eq!(err, "There is no preset \"nope\".");
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
        let conv = queue(vec![Box::new(Shared(Arc::clone(&img)))], sink);
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
        let conv = queue(vec![Box::new(img)], sink);

        let first = conv
            .submit(vec![touch(dir.path(), "a.png")], "jpg")
            .unwrap();
        // The user reopened the ring for other files before the first job ended.
        let _ = conv.ring_model(&[dir.path().join("b.png")]);
        assert_eq!(first.recv().unwrap().outcome, Outcome::Succeeded);
        // All the ring heard of job 1 is its introduction, before the reopen.
        assert_eq!(
            *events.lock().unwrap(),
            [RingEvent::Progress(RingProgress {
                job_id: 1,
                progress: 0.0
            })]
        );
        events.lock().unwrap().clear();

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
    fn the_ring_learns_its_job_id_before_anything_runs() {
        let dir = tempfile::tempdir().unwrap();
        let (sink, events) = recorder();
        let conv = queue(vec![Box::new(images())], sink);
        let summary = conv
            .submit(vec![touch(dir.path(), "a.png")], "jpg")
            .unwrap();
        assert_eq!(summary.recv().unwrap().outcome, Outcome::Succeeded);
        let events = events.lock().unwrap();
        assert_eq!(
            events.first(),
            Some(&RingEvent::Progress(RingProgress {
                job_id: 1,
                progress: 0.0
            }))
        );
        for event in events.iter() {
            if let RingEvent::Progress(p) = event {
                assert_eq!(p.job_id, 1);
            }
        }
    }

    #[test]
    fn cancelling_nothing_is_a_polite_no() {
        let dir = tempfile::tempdir().unwrap();
        let (sink, events) = recorder();
        let conv = queue(vec![Box::new(images())], sink);
        assert!(!conv.cancel(42), "no such job");
        let summary = conv
            .submit(vec![touch(dir.path(), "a.png")], "jpg")
            .unwrap();
        assert_eq!(summary.recv().unwrap().outcome, Outcome::Succeeded);
        // Give the worker a moment to wipe the job off the board after replying.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !lock(&conv.board).tokens.is_empty() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!conv.cancel(1), "the job already ended");
        assert!(matches!(
            events.lock().unwrap().last(),
            Some(RingEvent::Done(RingDone {
                outcome: Outcome::Succeeded,
                ..
            }))
        ));
    }

    #[test]
    fn a_queued_job_is_cancelled_before_it_runs() {
        let dir = tempfile::tempdir().unwrap();
        let mut img = images();
        img.delay = Duration::from_millis(300);
        let img = Arc::new(img);
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
        let (sink, events) = recorder();
        let (jobs, notices) = job_recorder();
        let conv = Conversions::with_job_sink(
            vec![Box::new(Shared(Arc::clone(&img)))],
            sink,
            jobs,
            Library::builtin(),
            AppSettings::default(),
        );

        let a = touch(dir.path(), "a.png");
        let first = conv.submit(vec![a.clone()], "jpg").unwrap();
        let second = conv
            .submit(vec![touch(dir.path(), "b.png")], "jpg")
            .unwrap();
        assert!(conv.cancel(2));

        // The ring shows job 2 and hears about it at once, while job 1 still
        // has a good 300 ms of napping to do.
        assert_eq!(
            events.lock().unwrap().last(),
            Some(&RingEvent::Done(RingDone {
                outcome: Outcome::Cancelled,
                message: None
            }))
        );

        assert_eq!(first.recv().unwrap().outcome, Outcome::Succeeded);
        let summary = second.recv().unwrap();
        assert_eq!(summary.outcome, Outcome::Cancelled);
        assert!(summary.outputs.is_empty());
        assert_eq!((summary.done, summary.failed), (0, 0));
        assert!(!dir.path().join("b.jpg").exists());
        assert_eq!(*img.order.lock().unwrap(), [a], "job 2 ran anyway");

        // The ring heard "cancelled" exactly once.
        let dones = events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, RingEvent::Done(_)))
            .count();
        assert_eq!(dones, 1);

        // App-wide, job 2 still starts and finishes, in queue order.
        let notices = notices.lock().unwrap();
        let ids: Vec<(char, u64)> = notices
            .iter()
            .filter_map(|n| match n {
                JobNotice::Started(s) => Some(('s', s.job_id)),
                JobNotice::Finished(f, _) => Some(('f', f.job_id)),
                JobNotice::Progress(_) => None,
            })
            .collect();
        assert_eq!(ids, [('s', 1), ('f', 1), ('s', 2), ('f', 2)]);
        let Some(JobNotice::Finished(finished, record)) = notices.last() else {
            panic!("no finish in {notices:?}");
        };
        assert_eq!(finished.summary, summary);
        assert_eq!(record.state, JobState::Cancelled);
        assert_eq!(record.results.len(), 1);
        assert_eq!(record.results[0].state, FileState::Cancelled);
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

    fn job_recorder() -> (JobSink, Arc<Mutex<Vec<JobNotice>>>) {
        let notices = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&notices);
        let sink: JobSink = Arc::new(move |n| seen.lock().unwrap().push(n));
        (sink, notices)
    }

    #[test]
    fn job_ids_count_up_from_one_and_every_job_reports_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let (ring, ring_events) = recorder();
        let (jobs, notices) = job_recorder();
        let conv = Conversions::with_job_sink(
            vec![Box::new(images())],
            ring,
            jobs,
            Library::builtin(),
            AppSettings::default(),
        );

        let pending: Vec<_> = ["a.png", "b.png", "c.png"]
            .iter()
            .map(|name| conv.submit(vec![touch(dir.path(), name)], "jpg").unwrap())
            .collect();
        // A refused request must not burn an id.
        assert!(conv.submit(Vec::new(), "jpg").is_err());
        let last = conv
            .submit(vec![touch(dir.path(), "d.png")], "jpg")
            .unwrap();
        for summary in pending.into_iter().chain([last]) {
            assert_eq!(summary.recv().unwrap().outcome, Outcome::Succeeded);
        }

        let notices = notices.lock().unwrap();
        let mut expected_id = 1;
        let mut current = None;
        for notice in notices.iter() {
            match notice {
                JobNotice::Started(started) => {
                    assert_eq!(current, None, "jobs overlapped: {notices:?}");
                    assert_eq!(started.job_id, expected_id);
                    assert_eq!(started.label, "JPG");
                    assert_eq!(started.files.len(), 1);
                    current = Some(started.job_id);
                }
                JobNotice::Progress(progress) => {
                    assert_eq!(Some(progress.job_id), current);
                }
                JobNotice::Finished(finished, record) => {
                    assert_eq!(Some(finished.job_id), current);
                    assert_eq!(record.job_id, finished.job_id);
                    assert_eq!(record.state, JobState::Completed);
                    assert_eq!(record.preset.target, "jpg");
                    assert_eq!(record.results.len(), 1);
                    assert_eq!(finished.summary.outcome, Outcome::Succeeded);
                    current = None;
                    expected_id += 1;
                }
            }
        }
        assert_eq!(expected_id, 5, "four jobs finished: {notices:?}");
        // Each job reached 100 % exactly once on the app-wide channel.
        let full = notices
            .iter()
            .filter(|n| matches!(n, JobNotice::Progress(p) if p.progress == 1.0))
            .count();
        assert_eq!(full, 4);
        // The ring heard only about the last job it was shown.
        assert!(matches!(
            ring_events.lock().unwrap().last(),
            Some(RingEvent::Done(_))
        ));
    }

    #[test]
    fn a_failed_job_is_finished_and_recorded_too() {
        let dir = tempfile::tempdir().unwrap();
        let (ring, _) = recorder();
        let (jobs, notices) = job_recorder();
        let conv = Conversions::with_job_sink(
            vec![Box::new(images())],
            ring,
            jobs,
            Library::builtin(),
            AppSettings::default(),
        );
        let bad = touch(dir.path(), "bad.txt");
        let summary = conv.submit(vec![bad], "png").unwrap().recv().unwrap();
        assert_eq!(summary.outcome, Outcome::Failed);
        let notices = notices.lock().unwrap();
        let Some(JobNotice::Finished(finished, record)) = notices.last() else {
            panic!("no finish in {notices:?}");
        };
        assert_eq!(finished.summary, summary);
        assert_eq!(record.state, JobState::Failed);
    }

    #[test]
    fn job_events_are_the_camel_case_json_the_ui_reads() {
        let started = JobStarted {
            job_id: 3,
            files: vec!["/a.png".into()],
            label: "JPG".into(),
        };
        assert_eq!(
            serde_json::to_value(&started).unwrap(),
            serde_json::json!({ "jobId": 3, "files": ["/a.png"], "label": "JPG" })
        );
        assert_eq!(
            serde_json::to_value(JobProgress {
                job_id: 3,
                progress: 0.5
            })
            .unwrap(),
            serde_json::json!({ "jobId": 3, "progress": 0.5 })
        );
        let finished = JobFinished {
            job_id: 3,
            summary: failed_summary("nope"),
        };
        assert_eq!(
            serde_json::to_value(&finished).unwrap(),
            serde_json::json!({
                "jobId": 3,
                "summary": { "outcome": "failed", "outputs": [], "done": 0, "failed": 0, "message": "nope" }
            })
        );
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

    #[test]
    fn the_preset_name_fills_the_preset_token() {
        assert_eq!(
            with_preset_name("{name}_{preset}", "JPG 70%"),
            "{name}_JPG 70%"
        );
        assert_eq!(
            with_preset_name("{PRESET}-{Preset}", "Half size"),
            "Half size-Half size"
        );
        assert_eq!(with_preset_name("{name}", "x"), "{name}");
        assert_eq!(with_preset_name("{pre}{preset", "x"), "{pre}{preset");
        // No token smuggling through a creative preset name.
        assert_eq!(with_preset_name("{preset}", "{name}"), "(name)");
        assert_eq!(with_preset_name("", "x"), "");
    }

    #[test]
    fn a_ring_slice_runs_its_preset_with_options_and_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let (sink, _) = recorder();
        let (jobs, notices) = job_recorder();
        let mut library = Library::builtin();
        let mut custom = library.preset("to-jpg").unwrap().clone();
        custom.id = "custom".into();
        custom.name = "Tiny {name}".into();
        custom.output.pattern = "{name}-{preset}".into();
        library.presets.push(custom);
        let conv = Conversions::with_job_sink(
            vec![Box::new(images())],
            sink,
            jobs,
            library,
            AppSettings::default(),
        );

        let summary = conv
            .submit(vec![touch(dir.path(), "a.png")], "custom")
            .unwrap()
            .recv()
            .unwrap();
        assert_eq!(summary.outcome, Outcome::Succeeded, "{summary:?}");
        assert!(dir.path().join("a-Tiny (name).jpg").is_file());

        let notices = notices.lock().unwrap();
        let Some(JobNotice::Started(started)) = notices.first() else {
            panic!("no start in {notices:?}");
        };
        assert_eq!(started.label, "Tiny {name}");
        let Some(JobNotice::Finished(_, record)) = notices.last() else {
            panic!("no finish in {notices:?}");
        };
        assert_eq!(record.preset.id, "custom");
    }

    #[test]
    fn same_as_source_presets_keep_the_format() {
        let dir = tempfile::tempdir().unwrap();
        let (sink, _) = recorder();
        let mut img = images();
        img.edges.push(Edge::new("png", "png"));
        let conv = queue(vec![Box::new(img)], sink);
        let summary = conv
            .submit(vec![touch(dir.path(), "cat.png")], "half-size")
            .unwrap()
            .recv()
            .unwrap();
        assert_eq!(summary.outcome, Outcome::Succeeded, "{summary:?}");
        assert!(dir.path().join("cat_50.png").is_file());
    }

    #[test]
    fn the_default_output_comes_from_the_settings() {
        use morphring_core::settings::DefaultLocation;
        let dir = tempfile::tempdir().unwrap();
        let (sink, _) = recorder();
        let mut settings = AppSettings::default();
        settings.default_output.location = DefaultLocation::Subfolder;
        settings.default_output.subfolder_name = "out".into();
        let conv = Conversions::new(vec![Box::new(images())], sink, Library::builtin(), settings);
        let summary = conv
            .submit(vec![touch(dir.path(), "a.png")], "to-jpg")
            .unwrap()
            .recv()
            .unwrap();
        assert_eq!(summary.outcome, Outcome::Succeeded, "{summary:?}");
        assert!(dir.path().join("out").join("a.jpg").is_file());
    }

    #[test]
    fn presets_are_found_by_name_too_and_bad_targets_are_refused() {
        let (sink, _) = recorder();
        let mut library = Library::builtin();
        let mut odd = Preset {
            id: "odd".into(),
            target: "nonsense".into(),
            ..Preset::default()
        };
        odd.name = "Odd".into();
        library.presets.push(odd);
        let conv = Conversions::new(
            vec![Box::new(images())],
            sink,
            library,
            AppSettings::default(),
        );
        let err = conv
            .submit(vec![PathBuf::from("a.png")], "odd")
            .unwrap_err();
        assert_eq!(err, "MorphRing cannot write the format \"nonsense\".");
        // Display name, any case: "half size" is the Half size preset.
        assert!(
            conv.submit(vec![PathBuf::from("a.png")], "half size")
                .is_ok()
        );
    }
}
