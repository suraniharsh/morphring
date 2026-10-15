//! The one place production code starts external programs.
//!
//! Every engine runs its tool through [`run`]: an argument list (never a
//! shell), stdout and stderr drained on their own threads, a timeout, and
//! cancellation that kills the whole process group. Ported from Filee's
//! `src/Filee.Engines/Infrastructure/ProcessRunner.cs`.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use morphring_core::CancelToken;

/// How many stderr lines are kept for error messages.
pub const STDERR_TAIL_LINES: usize = 40;

/// How often the runner checks for exit, cancellation and the deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Options for [`run`].
#[derive(Default)]
pub struct RunOptions<'a> {
    /// Kill the process group and return [`RunError::TimedOut`] after this long.
    /// `None` waits forever (cancellation still works).
    pub timeout: Option<Duration>,
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

    // Both pipes must be drained while we wait, or a chatty tool fills the
    // 64 KiB pipe buffer and we stare at each other forever.
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout = Mutex::new(String::new());
    let tail = Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES));
    let on_line = opts.on_stdout_line;

    let outcome = thread::scope(|scope| {
        if let Some(pipe) = stdout_pipe {
            let stdout = &stdout;
            scope.spawn(move || {
                for_each_line(pipe, |line| {
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
            scope.spawn(move || {
                for_each_line(pipe, |line| {
                    if let Ok(mut tail) = tail.lock() {
                        push_tail(&mut tail, line.to_string());
                    }
                });
            });
        }

        let outcome = wait(&mut child, opts.timeout, cancel);
        // Whatever ended the wait, sweep the group. A tool that exits while
        // its grandkids keep our pipes open would otherwise block the reader
        // threads (and this scope) until the grandkids get bored.
        kill_group(&mut child);
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

/// Polls the child until it exits, the token fires or the deadline passes.
fn wait(
    child: &mut Child,
    timeout: Option<Duration>,
    cancel: &CancelToken,
) -> Result<ExitStatus, RunError> {
    let deadline = timeout.map(|t| Instant::now() + t);
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
            kill_group(child);
            let _ = child.wait();
            return Err(RunError::Cancelled);
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            kill_group(child);
            let _ = child.wait();
            return Err(RunError::TimedOut);
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

/// Kills the child's whole process group. Safe to call more than once.
#[cfg(unix)]
fn kill_group(child: &mut Child) {
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

/// Kills the child. Grandchildren survive until Job Objects land.
#[cfg(windows)]
fn kill_group(child: &mut Child) {
    // TODO(v0.4): put the child in a Job Object so the whole tree dies with it.
    let _ = child.kill();
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
