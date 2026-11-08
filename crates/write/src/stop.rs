//! Stopping a Network Job or a Read Job, made of a process kill.
//!
//! A Network Job stops when the user asks. A Read Job stops when a newer one Supersedes it,
//! and since it takes no lock, killing it leaves the repository as it was (a clean filter
//! the user configured may have side effects of its own). A Local write Job is never killed
//! (ADR 0002), because killing git skips its cleanup.
//!
//! Stopping kills git together with what it started, such as `upload-pack` for a local
//! remote or a remote helper. Those share git's stderr, so killing git alone would leave
//! the runner waiting on a pipe a child still holds open.
//!
//! - **Unix:** git runs in its own process group. Stopping sends the group `SIGTERM`, which
//!   git answers by removing its lock files, and `SIGKILL` if anything is still there
//!   after [`GRACE`].
//! - **Windows:** git runs in its own Job Object, which every process it starts joins too.
//!   Stopping terminates the job: one call, with no process tree to walk. (`taskkill /T`,
//!   used before, walked parent ids and could hang for minutes on CI.) Windows has no signal
//!   git cleans up on, so a lock git held at that moment can remain, exactly as it would
//!   after ending git from Task Manager.

use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(unix)]
use std::time::Duration;

/// How long git gets to exit on `SIGTERM` before it and its children are killed outright.
#[cfg(unix)]
const GRACE: Duration = Duration::from_secs(2);

/// A stop button for one Network or Read invocation: hand it to
/// [`Invocation::stop_on`](crate::Invocation::stop_on), keep a clone, and call
/// [`stop`](Self::stop) from any thread. Once stopped it stays stopped, and any run that
/// uses it afterwards returns [`RunError::Stopped`](crate::RunError::Stopped) without
/// starting git.
#[derive(Clone, Debug, Default)]
pub struct Stop(Arc<Shared>);

#[derive(Debug, Default)]
struct Shared {
    requested: AtomicBool,
    /// Whether the stop reached a running git. A git that failed on its own before that
    /// reports its own error, not a stop.
    signalled: AtomicBool,
    /// How to reach the running git, from spawn until its pipes close. Cleared before the
    /// runner waits on git, so a reaped process id, which the OS may reuse, is never
    /// signalled.
    running: Mutex<Option<Target>>,
}

impl Stop {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stops git if it is running, and keeps it from starting otherwise. Returns at once:
    /// the run reports [`Stopped`](crate::RunError::Stopped) when git has exited.
    pub fn stop(&self) {
        self.0.requested.store(true, Ordering::SeqCst);
        let running = self.0.running.lock().unwrap();
        if let Some(target) = running.as_ref() {
            self.0.signalled.store(true, Ordering::SeqCst);
            terminate(&self.0, target);
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.0.requested.load(Ordering::SeqCst)
    }

    /// Whether the stop reached git while it ran, as opposed to before it started or after
    /// its pipes closed.
    pub(crate) fn reached_git(&self) -> bool {
        self.0.signalled.load(Ordering::SeqCst)
    }

    /// Runs `spawn` unless a stop came first, and records the process it started so that a
    /// later stop can reach it. The check and the record happen under one lock, so a stop
    /// either prevents the spawn or sees the process.
    pub(crate) fn spawn_unless_stopped(
        &self,
        spawn: impl FnOnce() -> std::io::Result<Child>,
    ) -> Option<std::io::Result<Child>> {
        let mut running = self.0.running.lock().unwrap();
        if self.is_stopped() {
            return None;
        }
        Some(spawn().inspect(|child| *running = Some(Target::of(child))))
    }

    /// Forgets the process, before the runner waits on it.
    pub(crate) fn finished(&self) {
        self.0.running.lock().unwrap().take();
    }
}

/// What a stop acts on: git's process group on Unix, git's Job Object on Windows.
#[derive(Debug)]
struct Target {
    pid: u32,
    /// `None` if git couldn't be put in a job, in which case `taskkill` is the fallback.
    #[cfg(windows)]
    job: Option<job::Job>,
}

impl Target {
    #[cfg(unix)]
    fn of(child: &Child) -> Self {
        Self { pid: child.id() }
    }

    /// Puts git in a new Job Object straight after it spawns, before git has started
    /// anything of its own: it reads its config and arguments first, which takes far longer
    /// than this.
    #[cfg(windows)]
    fn of(child: &Child) -> Self {
        use std::os::windows::io::AsRawHandle;
        Self {
            pid: child.id(),
            job: job::Job::containing(child.as_raw_handle()),
        }
    }
}

/// Asks git's process group to exit, and kills it if it hasn't after [`GRACE`]. The kill
/// is sent only while git's pipes are open: a child that closed them and outlives git is
/// left alone, because once git is reaped its group id may name someone else's processes.
#[cfg(unix)]
fn terminate(shared: &Arc<Shared>, target: &Target) {
    let pid = target.pid;
    let group = -(pid as libc::pid_t);
    // SAFETY: `kill` has no memory-safety preconditions. The group is git's own: the
    // runner put git in a new group whose id is its pid, and the pid is still unreaped.
    unsafe { libc::kill(group, libc::SIGTERM) };
    let shared = shared.clone();
    std::thread::spawn(move || {
        std::thread::sleep(GRACE);
        if shared.running.lock().unwrap().as_ref().map(|t| t.pid) == Some(pid) {
            // SAFETY: as above; the lock is held, so the runner can't reap git meanwhile.
            unsafe { libc::kill(group, libc::SIGKILL) };
        }
    });
}

/// Ends git and everything it started: its Job Object, or its process tree if git
/// couldn't be put in one.
#[cfg(windows)]
fn terminate(_shared: &Arc<Shared>, target: &Target) {
    if target.job.as_ref().is_some_and(job::Job::terminate) {
        return;
    }
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // taskkill walks the tree from git, so it must run while git is alive: the pid stays
    // git's until the runner drops its handle, which happens only after `finished`.
    let _ = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &target.pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::RawHandle;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject,
    };

    /// An owned Job Object handle, closed on drop. Closing it doesn't end the processes in
    /// it: only [`terminate`](Self::terminate) does.
    #[derive(Debug)]
    pub(super) struct Job(HANDLE);

    // SAFETY: a Job Object handle may be used and closed from any thread.
    unsafe impl Send for Job {}
    // SAFETY: as above; `terminate` takes `&self` and the OS synchronises it.
    unsafe impl Sync for Job {}

    impl Job {
        /// A new, unnamed Job Object holding `process`, or `None` if Windows refused either.
        pub(super) fn containing(process: RawHandle) -> Option<Self> {
            // SAFETY: null attributes and a null name ask for a default, unnamed job.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return None;
            }
            let job = Self(handle);
            // SAFETY: both handles are valid: the job was just created, and `process` is the
            // runner's own handle to a child it hasn't waited on yet.
            let assigned = unsafe { AssignProcessToJobObject(job.0, process as HANDLE) };
            (assigned != 0).then_some(job)
        }

        /// Ends every process in the job. False if Windows refused.
        pub(super) fn terminate(&self) -> bool {
            // SAFETY: the handle is valid until drop. Exit code 1, as a killed process has.
            unsafe { TerminateJobObject(self.0, 1) != 0 }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is owned and closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}
