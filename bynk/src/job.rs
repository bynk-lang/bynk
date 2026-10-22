//! A Windows job object around each process tree `bynk dev` serves (#1762).
//!
//! On Windows, `bynk dev` cannot stop a `wrangler dev` by stopping the child it
//! spawned. Every way it resolves wrangler there (`npx.cmd`, or a `wrangler.cmd`
//! on `PATH` or in the project) is a `.cmd` shim, which runs under `cmd.exe /c`.
//! So the child is the `cmd.exe` wrapper, and the server is four processes
//! further down (`npx` → wrangler's `node` launcher → its CLI → two `workerd
//! serve`s). `TerminateProcess` on the wrapper does not touch its descendants,
//! and Windows has neither the process group nor the readable working directory
//! that the Unix `sweep` (#1742) finds stragglers by.
//!
//! A job object does not need to find them. A process assigned to a job stays in
//! it, and every process it creates afterwards joins it too, whether or not its
//! parent is still running. Terminating the job stops the whole tree, including
//! the `workerd`s a crashed wrangler has already orphaned, which is the shape
//! #1742 found on Unix. The job is created with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so closing the last handle to it also
//! stops the tree. That handle is `bynk dev`'s, so the tree is stopped when a
//! [`Job`] is dropped and when `bynk dev` itself exits, however it exits.
//!
//! The stop is a hard one. On Unix, `bynk dev` sends SIGTERM so that wrangler
//! can run its own teardown, because a SIGKILLed wrangler strands its `workerd`s.
//! Inside a job nothing is stranded, so there is no grace period to wait out.
//!
//! **The assignment race.** A job can only be assigned once the child exists, so
//! there is a moment after `spawn` in which the child runs outside the job. A
//! process it creates in that moment is not in the job. std gives no way to spawn
//! the child suspended and resume it afterwards, because it closes the child's
//! main-thread handle. `cmd.exe` takes milliseconds to start `npx`, and the
//! assignment follows `spawn` directly, so in practice the whole tree is caught.
//!
//! Windows only: the module is not compiled anywhere else.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::process::Child;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};

/// A job object holding one spawned child and everything it starts. Dropping it
/// stops them all.
pub struct Job(HANDLE);

impl Job {
    /// Put `child`, and every process it creates from now on, in a new job.
    ///
    /// Fails if the job cannot be created or configured, or if `child` cannot
    /// be assigned to it. Since Windows 8 a process can be in several nested
    /// jobs, so a `bynk dev` that is itself running in a job (as under some CI
    /// runners and terminals) can still assign its children. The caller decides
    /// what a failure means. `bynk dev` keeps serving and stops the child the old
    /// way.
    pub fn assign(child: &Child) -> io::Result<Job> {
        // SAFETY: plain Win32 calls. Every pointer passed is either null (no
        // security attributes, no name) or to a live, correctly sized local, and
        // the child's handle stays valid for as long as `child` is borrowed.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            // Owned from here, so an early return closes it.
            let job = Job(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if set == 0 {
                return Err(io::Error::last_os_error());
            }
            if AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }
    }

    /// Stop every process in the job. Returns at once: the processes exit
    /// asynchronously, so a caller that needs them gone waits on the child, or
    /// polls [`Job::active`].
    pub fn terminate(&self) {
        // SAFETY: `self.0` is a job handle this value owns and has not closed.
        // A failure (the job is already empty) leaves nothing to do.
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }

    /// How many processes in the job are still running, or `None` if the job
    /// cannot be queried.
    pub fn active(&self) -> Option<u32> {
        // SAFETY: `self.0` is a live job handle, and `info` is a local of
        // exactly the size passed.
        unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
            let ok = QueryInformationJobObject(
                self.0,
                JobObjectBasicAccountingInformation,
                (&raw mut info).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            );
            (ok != 0).then_some(info.ActiveProcesses)
        }
    }
}

impl Drop for Job {
    /// Stop the tree, then close the handle. `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
    /// would stop it on the close alone; terminating first does not depend on
    /// this being the job's last open handle.
    fn drop(&mut self) {
        self.terminate();
        // SAFETY: `self.0` is owned by this value and is closed exactly once,
        // here.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// Wait up to ten seconds for the number of running processes in `job` to
    /// satisfy `ok`, and return it. Panics on the timeout, naming `what` and
    /// the last count, so a condition that never held fails the test rather
    /// than handing a stale count to a looser assertion.
    fn await_active(job: &Job, what: &str, ok: impl Fn(u32) -> bool) -> u32 {
        let started = Instant::now();
        loop {
            let n = job.active().expect("the job can be queried");
            if ok(n) {
                return n;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "timed out waiting for {what}: {n} process(es) running"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A temp dir removed on drop, so a failing assertion does not leave it
    /// behind.
    struct TempDir(std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The #1762 regression, with a `.cmd` in place of wrangler: the script
    /// starts two long-running background processes, as wrangler starts its
    /// `workerd`s. Killing the `cmd.exe` wrapper outright, as `Child::kill` did,
    /// orphans them, the way a crashed wrangler orphans its `workerd`s. They are
    /// still in the job, and stopping the job must stop them.
    ///
    /// `ping -n 300` is the long-running process, not `timeout`, which exits at
    /// once when its stdin is not a console.
    #[test]
    fn a_killed_wrappers_orphans_are_stopped_with_the_job() {
        let dir = TempDir(std::env::temp_dir().join(format!("bynk-job-{}", std::process::id())));
        let _ = std::fs::remove_dir_all(&dir.0);
        std::fs::create_dir_all(&dir.0).expect("the temp dir is created");
        let script = dir.0.join("fake-wrangler.cmd");
        std::fs::write(
            &script,
            "@echo off\r\n\
             start /b ping -n 300 127.0.0.1 >nul\r\n\
             start /b ping -n 300 127.0.0.1 >nul\r\n\
             ping -n 300 127.0.0.1 >nul\r\n",
        )
        .expect("the script is written");
        let mut child = Command::new(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the .cmd spawns");
        let job = Job::assign(&child).expect("the child is assigned to a job");
        // `cmd.exe` and its three `ping`s.
        let running = await_active(&job, "the script to start its pings", |n| n >= 4);

        let _ = child.kill();
        let _ = child.wait();
        let orphans = await_active(&job, "the wrapper to leave the job", |n| n < running);
        assert!(
            orphans >= 3,
            "the pings should outlive their parent, as the workerds do: {orphans} of \
             {running} process(es) left after the wrapper was killed"
        );

        job.terminate();
        await_active(&job, "the job's termination to stop every process", |n| {
            n == 0
        });
    }

    /// Dropping the job stops the tree too: a context that exits on its own is
    /// dropped from `bynk dev`'s list, and its orphans go with it.
    #[test]
    fn dropping_the_job_stops_the_tree() {
        let mut child = Command::new("cmd")
            .args(["/c", "ping -n 300 127.0.0.1 >nul"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("cmd spawns");
        let job = Job::assign(&child).expect("the child is assigned to a job");
        await_active(&job, "cmd to start ping", |n| n >= 2);
        drop(job);
        let started = Instant::now();
        while child.try_wait().unwrap().is_none() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the child survived its job being dropped"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
