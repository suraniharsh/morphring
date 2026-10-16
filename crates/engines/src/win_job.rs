//! Windows Job Objects, so killing a tool also kills everything it started.
//!
//! Windows has no process groups worth the name, so `child.kill()` takes out
//! the tool and leaves its children (hello again, `soffice.bin`) running. A
//! Job Object fixes that: every process the child starts lands in the same
//! job, and one `TerminateJobObject` ends them all. See
//! `docs/research/windows-support.md`, section 2, option A; cargo does the
//! same thing in `src/cargo/util/job.rs`.
//!
//! The race, written down so nobody has to rediscover it: the child is
//! assigned to the job right after `spawn`, not created suspended. Anything it
//! starts in that sliver of time is born outside the job and escapes the tree
//! kill. Real tools spend far longer than that reading their config before
//! they fork (soffice needs well over 100 ms), so we skip the
//! `CREATE_SUSPENDED` + resume-the-main-thread dance until CI proves we need
//! it. `PROC_THREAD_ATTRIBUTE_JOB_LIST` would close the gap for good, but
//! `std::process::Command` cannot pass attribute lists on stable yet.

// The whole module is a thin wrapper over five kernel32 calls, and FFI is
// unsafe by definition. Keeping all of it in here means the rest of the crate
// stays under the workspace-wide `unsafe_code = "deny"`.
#![allow(
    unsafe_code,
    reason = "Job Objects are only reachable through raw Win32 FFI"
)]

use std::io;
use std::os::windows::io::AsRawHandle;
use std::process::Child;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};

/// A Job Object holding one child process and its descendants.
///
/// The job is created with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so dropping
/// it also kills whatever is still inside.
pub(crate) struct Job {
    handle: HANDLE,
}

// SAFETY: a job handle is a plain kernel handle. The kernel serialises access
// to the object, and the Win32 calls we make on it are fine from any thread.
unsafe impl Send for Job {}
// SAFETY: see above; `terminate` only reads the handle value.
unsafe impl Sync for Job {}

impl Job {
    /// Kills every process in the job. Safe to call more than once.
    pub(crate) fn terminate(&self) {
        // SAFETY: `handle` is a valid job handle until Drop closes it.
        let ok = unsafe { TerminateJobObject(self.handle, 1) };
        if ok == 0 {
            // After everyone already exited this can fail harmlessly, so it
            // stays a debug line rather than a warning.
            tracing::debug!(err = %io::Error::last_os_error(), "TerminateJobObject failed");
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: we own `handle` and close it exactly once, here. Thanks to
        // KILL_ON_JOB_CLOSE this also sweeps leftovers after a normal exit,
        // the Windows twin of the Unix killpg sweep.
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

/// Puts `child` in a new Job Object that kills its processes when closed.
/// Returns `None` (and logs at debug level) when that is not possible; the
/// caller then falls back to killing just the child.
pub(crate) fn attach(child: &Child) -> Option<Job> {
    // SAFETY: null security attributes (so the handle is NOT inheritable,
    // which matters: an inherited handle would keep the job alive in the
    // child) and a null name (an anonymous job). Both are documented as valid.
    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if handle.is_null() {
        tracing::debug!(err = %io::Error::last_os_error(), "CreateJobObjectW failed");
        return None;
    }
    // From here on Drop closes the handle on every early return.
    let job = Job { handle };

    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).ok()?;
    // SAFETY: `info` is a live, correctly sized struct of the class we name,
    // and the call only reads it.
    let ok = unsafe {
        SetInformationJobObject(
            job.handle,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            size,
        )
    };
    if ok == 0 {
        tracing::debug!(err = %io::Error::last_os_error(), "SetInformationJobObject failed");
        return None;
    }

    // SAFETY: the child's raw handle stays valid while we borrow `child`.
    let ok = unsafe { AssignProcessToJobObject(job.handle, child.as_raw_handle()) };
    if ok == 0 {
        // Possible if the child already exited, or on ancient Windows where
        // the parent's job forbids nesting. Either way: no tree kill today.
        tracing::debug!(err = %io::Error::last_os_error(), "AssignProcessToJobObject failed");
        return None;
    }
    Some(job)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::ffi::OsString;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::Mutex;
    use std::thread;
    use std::time::{Duration, Instant};

    use morphring_core::CancelToken;
    use windows_sys::Win32::Foundation::STILL_ACTIVE;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use crate::process::{self, RunError, RunOptions};

    /// True while process `pid` exists and has not exited.
    fn is_running(pid: u32) -> bool {
        // SAFETY: plain FFI with no pointers besides the out-param below.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            // No such process any more. (It is our own grandchild, so "access
            // denied" is not a realistic reason to land here.)
            return false;
        }
        let mut code = 0u32;
        // SAFETY: `handle` is valid until the CloseHandle right after, and
        // `code` outlives the call.
        let ok = unsafe { GetExitCodeProcess(handle, &raw mut code) };
        // SAFETY: we opened it, we close it, once.
        unsafe { CloseHandle(handle) };
        ok != 0 && code == STILL_ACTIVE as u32
    }

    /// Polls until `pid` is gone, for at most `limit`.
    fn exits_within(pid: u32, limit: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            if !is_running(pid) {
                return true;
            }
            thread::sleep(Duration::from_millis(100));
        }
        !is_running(pid)
    }

    #[test]
    fn cancel_kills_the_grandchild_too() {
        // powershell starts a ping in its own hidden window (so the ping does
        // not hold our stdout pipe and stall the runner), prints its PID and
        // naps. Killing only powershell would leave the ping orphaned for 30 s.
        let script = "$p = Start-Process ping -ArgumentList '-n','30','127.0.0.1' \
                      -WindowStyle Hidden -PassThru; \
                      [Console]::Out.WriteLine($p.Id); [Console]::Out.Flush(); \
                      Start-Sleep 30";
        let args: Vec<OsString> = ["-NoProfile", "-NonInteractive", "-Command", script]
            .into_iter()
            .map(OsString::from)
            .collect();

        let cancel = CancelToken::new();
        let pid = Mutex::new(None::<u32>);
        let on_line = |line: &str| {
            if let Ok(p) = line.trim().parse::<u32>() {
                *pid.lock().unwrap() = Some(p);
            }
        };

        let result = thread::scope(|scope| {
            scope.spawn(|| {
                // powershell takes its sweet time to boot on a cold runner.
                let start = Instant::now();
                while pid.lock().unwrap().is_none() && start.elapsed() < Duration::from_secs(30) {
                    thread::sleep(Duration::from_millis(50));
                }
                thread::sleep(Duration::from_secs(1));
                cancel.cancel();
            });
            process::run(
                Path::new("powershell"),
                &args,
                RunOptions {
                    timeout: Some(Duration::from_secs(90)),
                    on_stdout_line: Some(&on_line),
                    ..Default::default()
                },
                &cancel,
            )
        });

        assert!(
            matches!(result, Err(RunError::Cancelled)),
            "expected Cancelled, got {result:?}"
        );
        let pid = pid
            .into_inner()
            .unwrap()
            .expect("powershell never printed the ping PID");
        assert!(
            exits_within(pid, Duration::from_secs(3)),
            "grandchild ping (pid {pid}) survived the cancel"
        );
    }

    #[test]
    fn dropping_the_job_kills_the_child() {
        let mut child = Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let job = attach(&child).expect("could not put the child in a job");
        drop(job);

        let start = Instant::now();
        let mut exited = false;
        while start.elapsed() < Duration::from_secs(3) {
            if child.try_wait().unwrap().is_some() {
                exited = true;
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        if !exited {
            // Clean up our own mess before failing the test.
            let _ = child.kill();
        }
        // Always reap, so neither the child nor clippy is left haunted.
        let _ = child.wait();
        assert!(exited, "KILL_ON_JOB_CLOSE did not take the child down");
    }
}
