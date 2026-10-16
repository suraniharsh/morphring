//! The one place production code starts external programs.
//!
//! Every engine runs its tool through [`run`]: an argument list (never a
//! shell), stdout and stderr drained on their own threads, a timeout, a stall
//! watchdog, and cancellation that kills the whole process tree (a process group on Unix, a
//! Job Object on Windows). Ported from Filee's
//! `src/Filee.Engines/Infrastructure/ProcessRunner.cs`.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use morphring_core::CancelToken;

/// How many stderr lines are kept for error messages.
pub const STDERR_TAIL_LINES: usize = 40;

/// How often the runner checks for exit, cancellation and the deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How often the stall watchdog looks at the clock, unless told otherwise
/// (Filee's `FfmpegRunner` checks once a second too).
pub const DEFAULT_STALL_POLL: Duration = Duration::from_secs(1);

/// Options for [`run`].
#[derive(Default)]
pub struct RunOptions<'a> {
    /// Kill the process group and return [`RunError::TimedOut`] after this long.
    /// `None` waits forever (cancellation still works).
    pub timeout: Option<Duration>,
    /// Kill the process group and return [`RunError::Stalled`] when the program
    /// prints nothing on stdout or stderr for this long. `None` disables the
    /// watchdog. Works alongside `timeout`.
    pub stall: Option<Duration>,
    /// How often the stall watchdog checks for silence. `None` means
    /// [`DEFAULT_STALL_POLL`]. Only matters when `stall` is set.
    pub stall_poll: Option<Duration>,
    /// Working directory of the child. `None` inherits ours.
    pub cwd: Option<PathBuf>,
    /// Environment variables removed from the child's environment.
    pub env_remove: Vec<&'static str>,
    /// Called with every stdout line as it arrives, for progress parsing.
    pub on_stdout_line: Option<&'a (dyn Fn(&str) + Sync)>,
}

/// What a successful run produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunOutput {
    /// Everything the program wrote to stdout (lossy UTF-8, `\n` line ends).
    pub stdout: String,
    /// The last [`STDERR_TAIL_LINES`] lines of stderr.
    pub stderr_tail: Vec<String>,
}

/// Why a run did not succeed.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The cancel token fired; the process group was killed.
    #[error("cancelled")]
    Cancelled,
    /// The timeout passed; the process group was killed.
    #[error("timed out")]
    TimedOut,
    /// The program printed nothing for `after`; the process group was killed.
    #[error("stopped making progress for {} s", .after.as_secs())]
    Stalled { after: Duration },
    /// The program could not be started at all.
    #[error("could not start {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: io::Error,
    },
    /// The program exited unsuccessfully. `code` is `None` when a signal killed it.
    #[error("{}", failed_message(*.code, .tail))]
    Failed {
        code: Option<i32>,
        tail: Vec<String>,
    },
}

fn failed_message(code: Option<i32>, tail: &[String]) -> String {
    let head = match code {
        Some(code) => format!("exited with code {code}"),
        None => "was killed by a signal".to_string(),
    };
    let summary = summarize(tail);
    if summary.is_empty() {
        head
    } else {
        format!("{head}: {summary}")
    }
}

/// Runs `program` with `args` and waits for it to finish.
///
/// Returns [`RunError::Failed`] for a non-zero exit. Callers whose tools have
/// unreliable exit codes (soffice) should judge success by the output file.
pub fn run(
    program: &Path,
    args: &[OsString],
    opts: RunOptions<'_>,
    cancel: &CancelToken,
) -> Result<RunOutput, RunError> {
    tracing::debug!(command = %command_line(program, args), "running");

    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = &opts.cwd {
        cmd.current_dir(cwd);
    }
    for key in &opts.env_remove {
        cmd.env_remove(key);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // A fresh process group with the child as leader, so one killpg takes
        // out the child and everything it spawned (looking at you, soffice.bin).
        cmd.process_group(0);
    }

    let mut child = cmd.spawn().map_err(|source| RunError::Spawn {
        program: program.display().to_string(),
        source,
    })?;
    let tree = attach_tree(&child);

    // Both pipes must be drained while we wait, or a chatty tool fills the
    // 64 KiB pipe buffer and we stare at each other forever.
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout = Mutex::new(String::new());
    let tail = Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES));
    let on_line = opts.on_stdout_line;
    // The watchdog's heartbeat: milliseconds since `started` at which the
    // tool last said anything, on either pipe. Both readers bump it, the wait
    // loop reads it. Relaxed is plenty; we're timing seconds, not nanoseconds.
    let started = Instant::now();
    let last_line = AtomicU64::new(0);
    let watchdog = opts.stall.map(|after| Watchdog {
        after,
        poll: opts.stall_poll.unwrap_or(DEFAULT_STALL_POLL),
        started,
        last_line: &last_line,
    });

    let outcome = thread::scope(|scope| {
        if let Some(pipe) = stdout_pipe {
            let stdout = &stdout;
            let last_line = &last_line;
            scope.spawn(move || {
                for_each_line(pipe, |line| {
                    heartbeat(last_line, started);
                    if let Some(cb) = on_line {
                        cb(line);
                    }
                    if let Ok(mut buf) = stdout.lock() {
                        buf.push_str(line);
                        buf.push('\n');
                    }
                });
            });
        }
        if let Some(pipe) = stderr_pipe {
            let tail = &tail;
            let last_line = &last_line;
            scope.spawn(move || {
                for_each_line(pipe, |line| {
                    heartbeat(last_line, started);
                    if let Ok(mut tail) = tail.lock() {
                        push_tail(&mut tail, line.to_string());
                    }
                });
            });
        }

        let outcome = wait(&mut child, &tree, opts.timeout, watchdog, cancel);
        // Whatever ended the wait, sweep the group. A tool that exits while
        // its grandkids keep our pipes open would otherwise block the reader
        // threads (and this scope) until the grandkids get bored.
        kill_group(&mut child, &tree);
        outcome
    });

    let stdout = stdout.into_inner().unwrap_or_else(|e| e.into_inner());
    let stderr_tail: Vec<String> = tail
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .into_iter()
        .collect();

    match outcome? {
        status if status.success() => Ok(RunOutput {
            stdout,
            stderr_tail,
        }),
        status => {
            tracing::debug!(?status, program = %program.display(), "process failed");
            Err(RunError::Failed {
                code: status.code(),
                tail: stderr_tail,
            })
        }
    }
}

/// Records "the tool just said something" for the stall watchdog.
fn heartbeat(last_line: &AtomicU64, started: Instant) {
    let now = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    // fetch_max, because the two reader threads race and time only goes one way.
    last_line.fetch_max(now, Ordering::Relaxed);
}

/// The stall watchdog's settings plus the heartbeat it watches.
#[derive(Clone, Copy)]
struct Watchdog<'a> {
    after: Duration,
    poll: Duration,
    started: Instant,
    last_line: &'a AtomicU64,
}

impl Watchdog<'_> {
    /// True once the tool has been silent for at least `after`. Silence is
    /// counted from the start of the run until the first line arrives, so a
    /// tool that never prints anything is caught too.
    fn stalled(&self) -> bool {
        let last = Duration::from_millis(self.last_line.load(Ordering::Relaxed));
        self.started.elapsed().saturating_sub(last) >= self.after
    }
}

/// Polls the child until it exits, the token fires, the deadline passes or
/// the stall watchdog decides the tool has gone to sleep.
fn wait(
    child: &mut Child,
    tree: &Tree,
    timeout: Option<Duration>,
    watchdog: Option<Watchdog<'_>>,
    cancel: &CancelToken,
) -> Result<ExitStatus, RunError> {
    let deadline = timeout.map(|t| Instant::now() + t);
    // The watchdog runs on its own (slower) beat than the exit poll, so a
    // test can crank it up and production can leave it at a relaxed 1 s.
    let mut next_stall_check = watchdog.map(|w| Instant::now() + w.poll);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(err) => {
                // try_wait only fails if the pid vanished under us, which
                // means something else reaped it. Treat it like a crash.
                tracing::warn!(%err, "could not poll child process");
                return Ok(child.wait().unwrap_or_else(|_| failed_status()));
            }
        }
        if cancel.is_cancelled() {
            kill_group(child, tree);
            let _ = child.wait();
            return Err(RunError::Cancelled);
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            kill_group(child, tree);
            let _ = child.wait();
            return Err(RunError::TimedOut);
        }
        if let (Some(dog), Some(next)) = (watchdog, next_stall_check) {
            let now = Instant::now();
            if now >= next {
                if dog.stalled() {
                    tracing::debug!(after = ?dog.after, "process stalled, killing it");
                    kill_group(child, tree);
                    let _ = child.wait();
                    return Err(RunError::Stalled { after: dog.after });
                }
                next_stall_check = Some(now + dog.poll);
            }
        }
        thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(unix)]
fn failed_status() -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(1 << 8)
}

#[cfg(windows)]
fn failed_status() -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(1)
}

/// What it takes to kill a child together with everything it started. On Unix
/// the process group id is the child's pid, so there is nothing extra to keep.
#[cfg(not(windows))]
struct Tree;

/// What it takes to kill a child together with everything it started: the Job
/// Object it was put in, when that worked.
#[cfg(windows)]
type Tree = Option<crate::win_job::Job>;

#[cfg(not(windows))]
fn attach_tree(_child: &Child) -> Tree {
    Tree
}

#[cfg(windows)]
fn attach_tree(child: &Child) -> Tree {
    // The child gets a head start between spawn and attach, so a grandchild
    // forked in that sliver escapes the job. Tools take longer than that to
    // even find their config files, so the race stays theoretical.
    crate::win_job::attach(child)
}

/// Kills the child's whole process group. Safe to call more than once.
#[cfg(unix)]
fn kill_group(child: &mut Child, _tree: &Tree) {
    let Ok(pgid) = libc::pid_t::try_from(child.id()) else {
        return;
    };
    // soffice forks soffice.bin and then wanders off like a cat. Killing only
    // the child leaks a headless office suite per cancelled job, so the whole
    // group goes. After a normal exit this just sweeps leftovers, or gets
    // ESRCH when there are none, which is the happy path.
    #[allow(
        unsafe_code,
        reason = "killpg takes plain integers and has no memory-safety preconditions"
    )]
    // SAFETY: killpg only sends a signal; pgid came from a child we spawned
    // with process_group(0), so it names that child's group.
    unsafe {
        libc::killpg(pgid, libc::SIGKILL);
    }
}

/// Kills the child's Job Object (the child and everything it started), or just
/// the child when it could not be put in a job. Safe to call more than once.
#[cfg(windows)]
fn kill_group(child: &mut Child, tree: &Tree) {
    match tree {
        Some(job) => job.terminate(),
        // No job, no tree kill: grandchildren are on their own, like in v0.1.
        None => {
            let _ = child.kill();
        }
    }
}

/// Reads `pipe` line by line (lossy UTF-8, `\r\n` trimmed) until EOF.
fn for_each_line(pipe: impl Read, mut f: impl FnMut(&str)) {
    let mut reader = BufReader::new(pipe);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                while buf.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
                    buf.pop();
                }
                f(&String::from_utf8_lossy(&buf));
            }
        }
    }
}

/// Appends `line`, dropping the oldest line once the tail is full.
fn push_tail(tail: &mut VecDeque<String>, line: String) {
    if tail.len() == STDERR_TAIL_LINES {
        tail.pop_front();
    }
    tail.push_back(line);
}

/// A short, readable version of a tool's stderr: its last 3 distinct
/// non-empty lines without `[libx264 @ 0x55d...]` style prefixes, joined
/// with spaces.
pub fn summarize(tail: &[String]) -> String {
    let mut distinct: Vec<&str> = Vec::new();
    for line in tail.iter().map(|l| strip_prefix(l.trim())) {
        if !line.is_empty() && !distinct.contains(&line) {
            distinct.push(line);
        }
    }
    let start = distinct.len().saturating_sub(3);
    distinct[start..].join(" ")
}

/// "[aost#0:1/wmav2 @ 0x12a] too many channels" -> "too many channels".
fn strip_prefix(mut line: &str) -> &str {
    // ffmpeg stacks these sometimes ("[in#0 @ 0x..] [mov @ 0x..] ..."), so
    // peel until there are no more onion layers.
    while line.starts_with('[') {
        match line.find("] ") {
            Some(end) if end > 0 => line = line[end + 2..].trim_start(),
            _ => break,
        }
    }
    line
}

/// Moves `from` (a file or folder) to `to`, falling back to copy and remove
/// when a rename is not possible (for example across filesystems).
pub fn move_into_place(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        // /tmp is tmpfs on Arch, so rename from a temp dir into ~ fails with
        // EXDEV all day long. Any rename failure gets the slow path; if the
        // source is really gone the copy reports that properly.
        Err(err) => {
            tracing::debug!(%err, from = %from.display(), to = %to.display(), "rename failed, copying");
            copy_then_remove(from, to)
        }
    }
}

fn copy_then_remove(from: &Path, to: &Path) -> io::Result<()> {
    if let Err(err) = copy_recursive(from, to) {
        // Never leave half a file under the user's chosen name.
        let _ = if to.is_dir() {
            std::fs::remove_dir_all(to)
        } else {
            std::fs::remove_file(to)
        };
        return Err(err);
    }
    if from.is_dir() {
        std::fs::remove_dir_all(from)
    } else {
        std::fs::remove_file(from)
    }
}

fn copy_recursive(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// The command line for logs, quoted so spaces and odd names stay visible.
fn command_line(program: &Path, args: &[OsString]) -> String {
    std::iter::once(program.as_os_str())
        .chain(args.iter().map(OsString::as_os_str))
        .map(|a: &OsStr| format!("{:?}", a.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn stderr_tail_keeps_only_the_last_40_lines() {
        let mut tail = VecDeque::new();
        for i in 1..=100 {
            push_tail(&mut tail, format!("line{i}"));
        }
        assert_eq!(tail.len(), 40);
        assert_eq!(tail.front().map(String::as_str), Some("line61"));
        assert_eq!(tail.back().map(String::as_str), Some("line100"));
    }

    #[test]
    fn summarize_strips_log_prefixes() {
        let tail = vec![
            "[libx264 @ 0x55d0c0a1b2c0] height not divisible by 2 (101x101)".to_string(),
            "[vost#0:0/libx264 @ 0x55d0c0a1] [enc:libx264 @ 0x55d] Error while opening encoder"
                .to_string(),
        ];
        assert_eq!(
            summarize(&tail),
            "height not divisible by 2 (101x101) Error while opening encoder"
        );
    }

    #[test]
    fn summarize_keeps_last_three_distinct_lines() {
        let tail: Vec<String> = ["a", "b", "", "c", "c", "[x @ 0x1] d", "  "]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(summarize(&tail), "b c d");
        assert_eq!(summarize(&[]), "");
    }

    #[test]
    fn summarize_leaves_brackets_without_a_space_alone() {
        let tail = vec!["[done]".to_string()];
        assert_eq!(summarize(&tail), "[done]");
    }

    #[test]
    fn move_into_place_moves_a_file_between_two_tempdirs() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let from = a.path().join("out.png");
        let to = b.path().join("final.png");
        std::fs::write(&from, b"pixels").unwrap();
        move_into_place(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read(&to).unwrap(), b"pixels");
    }

    #[test]
    fn copy_fallback_moves_files_and_folders() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let file = a.path().join("one.txt");
        std::fs::write(&file, b"1").unwrap();
        copy_then_remove(&file, &b.path().join("one.txt")).unwrap();
        assert!(!file.exists());
        assert_eq!(std::fs::read(b.path().join("one.txt")).unwrap(), b"1");

        let dir = a.path().join("pages");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/p1.png"), b"p").unwrap();
        copy_then_remove(&dir, &b.path().join("pages")).unwrap();
        assert!(!dir.exists());
        assert_eq!(
            std::fs::read(b.path().join("pages/sub/p1.png")).unwrap(),
            b"p"
        );
    }

    #[test]
    fn move_into_place_reports_a_missing_source() {
        let a = tempfile::tempdir().unwrap();
        let err = move_into_place(&a.path().join("nope"), &a.path().join("dest"));
        assert!(err.is_err());
        assert!(!a.path().join("dest").exists());
    }

    #[test]
    fn spawn_error_names_the_program() {
        let err = run(
            Path::new("/definitely/not/a/morphring/tool"),
            &[],
            RunOptions::default(),
            &CancelToken::new(),
        )
        .unwrap_err();
        assert!(matches!(err, RunError::Spawn { .. }));
        assert!(err.to_string().contains("/definitely/not/a/morphring/tool"));
    }

    #[test]
    fn stalled_message_names_the_quiet_period() {
        let err = RunError::Stalled {
            after: Duration::from_secs(120),
        };
        assert_eq!(err.to_string(), "stopped making progress for 120 s");
    }

    #[test]
    fn stall_is_off_by_default() {
        assert_eq!(RunOptions::default().stall, None);
    }

    #[test]
    fn failed_message_includes_the_summary() {
        let err = RunError::Failed {
            code: Some(1),
            tail: vec!["[mov @ 0x1] moov atom not found".to_string()],
        };
        assert_eq!(err.to_string(), "exited with code 1: moov atom not found");
    }

    #[cfg(unix)]
    mod unix {
        use super::*;

        fn sh(script: &str) -> (PathBuf, Vec<OsString>) {
            (PathBuf::from("sh"), os(&["-c", script]))
        }

        /// Polls `kill -0` until the pid is gone (or reaped to a zombie-free state).
        fn gone_within(pid: &str, limit: Duration) -> bool {
            let start = Instant::now();
            while start.elapsed() < limit {
                let alive = Command::new("kill")
                    .args(["-0", pid])
                    .stderr(Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                    .map(|s| {
                        s.rsplit_once(')')
                            .is_some_and(|(_, r)| r.trim_start().starts_with('Z'))
                    })
                    .unwrap_or(false);
                if !alive || zombie {
                    return true;
                }
                thread::sleep(Duration::from_millis(20));
            }
            false
        }

        fn read_pid(path: &Path) -> String {
            let start = Instant::now();
            loop {
                if let Ok(s) = std::fs::read_to_string(path) {
                    if s.ends_with('\n') {
                        return s.trim().to_string();
                    }
                }
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "pid never written"
                );
                thread::sleep(Duration::from_millis(10));
            }
        }

        fn cancel_after(cancel: &CancelToken, pid_file: PathBuf) {
            let cancel = cancel.clone();
            thread::spawn(move || {
                read_pid(&pid_file);
                thread::sleep(Duration::from_millis(100));
                cancel.cancel();
            });
        }

        #[test]
        fn cancelling_sleep_returns_quickly_and_kills_the_child() {
            let dir = tempfile::tempdir().unwrap();
            let pid_file = dir.path().join("pid");
            // exec keeps the pid, so the pid we wrote is the sleep itself.
            let (prog, args) = sh(&format!(
                "echo $$ > '{}'; exec sleep 30",
                pid_file.display()
            ));
            let cancel = CancelToken::new();
            cancel_after(&cancel, pid_file.clone());
            let started = Instant::now();
            let res = run(&prog, &args, RunOptions::default(), &cancel);
            let cancelled_at = read_pid(&pid_file);
            assert!(matches!(res, Err(RunError::Cancelled)), "{res:?}");
            // 100 ms of setup slack on top of the 1 s budget for the kill itself.
            assert!(started.elapsed() < Duration::from_millis(1500));
            assert!(gone_within(&cancelled_at, Duration::from_secs(1)));
        }

        #[test]
        fn cancelling_kills_grandchildren_too() {
            let dir = tempfile::tempdir().unwrap();
            let pid_file = dir.path().join("grandchild");
            let (prog, args) = sh(&format!(
                "sleep 30 & echo $! > '{}'; wait",
                pid_file.display()
            ));
            let cancel = CancelToken::new();
            cancel_after(&cancel, pid_file.clone());
            let res = run(&prog, &args, RunOptions::default(), &cancel);
            assert!(matches!(res, Err(RunError::Cancelled)), "{res:?}");
            let grandchild = read_pid(&pid_file);
            assert!(
                gone_within(&grandchild, Duration::from_secs(2)),
                "grandchild {grandchild} survived"
            );
        }

        #[test]
        fn timeout_kills_a_slow_child() {
            let opts = RunOptions {
                timeout: Some(Duration::from_millis(200)),
                ..RunOptions::default()
            };
            let started = Instant::now();
            let res = run(Path::new("sleep"), &os(&["5"]), opts, &CancelToken::new());
            assert!(matches!(res, Err(RunError::TimedOut)), "{res:?}");
            assert!(started.elapsed() < Duration::from_secs(2));
        }

        #[test]
        fn a_quiet_child_is_killed_as_stalled() {
            let dir = tempfile::tempdir().unwrap();
            let pid_file = dir.path().join("pid");
            // Says hello once, then plays dead. exec keeps the pid honest.
            let (prog, args) = sh(&format!(
                "echo hello; echo $$ > '{}'; exec sleep 30",
                pid_file.display()
            ));
            let stall = Duration::from_millis(500);
            let opts = RunOptions {
                stall: Some(stall),
                ..RunOptions::default()
            };
            let started = Instant::now();
            let res = run(&prog, &args, opts, &CancelToken::new());
            let elapsed = started.elapsed();
            assert!(
                matches!(res, Err(RunError::Stalled { after }) if after == stall),
                "{res:?}"
            );
            assert!(elapsed >= stall, "killed too early: {elapsed:?}");
            assert!(
                elapsed < stall + Duration::from_millis(1500),
                "killed too late: {elapsed:?}"
            );
            let pid = read_pid(&pid_file);
            assert!(gone_within(&pid, Duration::from_secs(1)));
        }

        #[test]
        fn a_silent_child_is_stalled_even_without_a_first_line() {
            let opts = RunOptions {
                stall: Some(Duration::from_millis(200)),
                stall_poll: Some(Duration::from_millis(50)),
                ..RunOptions::default()
            };
            let started = Instant::now();
            let res = run(Path::new("sleep"), &os(&["5"]), opts, &CancelToken::new());
            assert!(matches!(res, Err(RunError::Stalled { .. })), "{res:?}");
            assert!(started.elapsed() < Duration::from_secs(2));
        }

        #[test]
        fn a_chatty_child_outlives_the_stall_period() {
            // Runs for ~1.2 s, four times the stall period, but never goes
            // quiet for long, alternating stdout and stderr to feed both pipes.
            let (prog, args) = sh(
                "for i in 1 2 3 4 5 6 7 8; do echo out$i; sleep 0.075; echo err$i >&2; sleep 0.075; done",
            );
            let opts = RunOptions {
                stall: Some(Duration::from_millis(300)),
                stall_poll: Some(Duration::from_millis(20)),
                ..RunOptions::default()
            };
            let started = Instant::now();
            let out = run(&prog, &args, opts, &CancelToken::new()).unwrap();
            assert!(started.elapsed() > Duration::from_millis(300));
            assert_eq!(out.stdout.lines().count(), 8);
            assert_eq!(out.stderr_tail.len(), 8);
        }

        #[test]
        fn timeout_still_wins_when_the_child_keeps_talking() {
            let (prog, args) = sh("while true; do echo tick; sleep 0.05; done");
            let opts = RunOptions {
                timeout: Some(Duration::from_millis(300)),
                stall: Some(Duration::from_secs(5)),
                ..RunOptions::default()
            };
            let started = Instant::now();
            let res = run(&prog, &args, opts, &CancelToken::new());
            assert!(matches!(res, Err(RunError::TimedOut)), "{res:?}");
            assert!(started.elapsed() < Duration::from_secs(2));
        }

        #[test]
        fn stdout_callback_sees_each_line() {
            let seen = Mutex::new(Vec::new());
            let cb = |line: &str| seen.lock().unwrap().push(line.to_string());
            let opts = RunOptions {
                on_stdout_line: Some(&cb),
                ..RunOptions::default()
            };
            let out = run(
                Path::new("printf"),
                &os(&["a\\nb\\n"]),
                opts,
                &CancelToken::new(),
            )
            .unwrap();
            assert_eq!(*seen.lock().unwrap(), ["a", "b"]);
            assert_eq!(out.stdout, "a\nb\n");
        }

        #[test]
        fn failure_carries_code_and_stderr_tail() {
            let (prog, args) = sh("for i in $(seq 1 100); do echo line$i >&2; done; exit 3");
            let err = run(&prog, &args, RunOptions::default(), &CancelToken::new()).unwrap_err();
            let RunError::Failed { code, tail } = err else {
                panic!("expected Failed, got {err:?}");
            };
            assert_eq!(code, Some(3));
            assert_eq!(tail.len(), STDERR_TAIL_LINES);
            assert_eq!(tail[0], "line61");
            assert_eq!(tail[39], "line100");
        }

        #[test]
        fn cwd_and_env_remove_reach_the_child() {
            let dir = tempfile::tempdir().unwrap();
            let opts = RunOptions {
                cwd: Some(dir.path().to_path_buf()),
                env_remove: vec!["HOME"],
                ..RunOptions::default()
            };
            let (prog, args) = sh("pwd; echo \"home=${HOME-unset}\"");
            let out = run(&prog, &args, opts, &CancelToken::new()).unwrap();
            let mut lines = out.stdout.lines();
            let pwd = PathBuf::from(lines.next().unwrap());
            assert_eq!(
                std::fs::canonicalize(pwd).unwrap(),
                std::fs::canonicalize(dir.path()).unwrap()
            );
            assert_eq!(lines.next(), Some("home=unset"));
        }

        #[test]
        fn a_daemonized_grandchild_does_not_hang_the_runner() {
            // The child exits at once, but its background sleep holds stdout
            // open. Without the post-exit group sweep this takes 30 s.
            let started = Instant::now();
            let (prog, args) = sh("sleep 30 & echo started");
            let out = run(&prog, &args, RunOptions::default(), &CancelToken::new()).unwrap();
            assert_eq!(out.stdout, "started\n");
            assert!(started.elapsed() < Duration::from_secs(5));
        }

        #[test]
        fn non_utf8_and_spaces_in_args_survive() {
            use std::os::unix::ffi::OsStringExt;
            let weird = OsString::from_vec(b"a b\xff'\"".to_vec());
            let out = run(
                Path::new("printf"),
                &[OsString::from("%s"), weird],
                RunOptions::default(),
                &CancelToken::new(),
            )
            .unwrap();
            assert_eq!(out.stdout, "a b\u{fffd}'\"\n");
        }
    }
}
