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
//! - **Windows:** `taskkill /T /F` ends git's process tree. Windows has no signal git
//!   cleans up on, so a lock git held at that moment can remain, exactly as it would
//!   after ending git from Task Manager.

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
    /// The running git's process id, from spawn until its pipes close. Cleared before the
    /// runner waits on git, so a reaped process id, which the OS may reuse, is never
    /// signalled.
    running: Mutex<Option<u32>>,
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
        if let Some(pid) = *running {
            self.0.signalled.store(true, Ordering::SeqCst);
            terminate(&self.0, pid);
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
    pub(crate) fn spawn_unless_stopped<T>(
        &self,
        spawn: impl FnOnce() -> std::io::Result<(T, u32)>,
    ) -> Option<std::io::Result<T>> {
        let mut running = self.0.running.lock().unwrap();
        if self.is_stopped() {
            return None;
        }
        Some(spawn().map(|(child, pid)| {
            *running = Some(pid);
            child
        }))
    }

    /// Forgets the process, before the runner waits on it.
    pub(crate) fn finished(&self) {
        self.0.running.lock().unwrap().take();
    }
}

/// Asks git's process group to exit, and kills it if it hasn't after [`GRACE`]. The kill
/// is sent only while git's pipes are open: a child that closed them and outlives git is
/// left alone, because once git is reaped its group id may name someone else's processes.
#[cfg(unix)]
fn terminate(shared: &Arc<Shared>, pid: u32) {
    let group = -(pid as libc::pid_t);
    // SAFETY: `kill` has no memory-safety preconditions. The group is git's own: the
    // runner put git in a new group whose id is its pid, and the pid is still unreaped.
    unsafe { libc::kill(group, libc::SIGTERM) };
    let shared = shared.clone();
    std::thread::spawn(move || {
        std::thread::sleep(GRACE);
        if *shared.running.lock().unwrap() == Some(pid) {
            // SAFETY: as above; the lock is held, so the runner can't reap git meanwhile.
            unsafe { libc::kill(group, libc::SIGKILL) };
        }
    });
}

/// Ends git's process tree.
#[cfg(windows)]
fn terminate(_shared: &Arc<Shared>, pid: u32) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // taskkill walks the tree from git, so it must run while git is alive: the pid stays
    // git's until the runner drops its handle, which happens only after `finished`.
    let _ = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}
