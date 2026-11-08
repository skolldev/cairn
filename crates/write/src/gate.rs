//! Keeping Local write Jobs from starting once a Force quit has decided to exit.
//!
//! A Local write Job is never stopped (ADR 0002), but one that hasn't started git has nothing
//! to protect. Exiting while git starts could kill it mid-step with nobody warned about that
//! Job, so the decision to exit and each spawn take turns under one lock.

use std::process::Child;
use std::sync::{Arc, Mutex};

/// Local write invocations pass through this with
/// [`Invocation::through`](crate::Invocation::through). Once [`close`](Self::close) returns,
/// git is either already running or never starts, and a run that comes later returns
/// [`RunError::GateClosed`](crate::RunError::GateClosed). A closed gate stays closed.
#[derive(Clone, Debug, Default)]
pub struct StartGate(Arc<Mutex<bool>>);

impl StartGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Closes the gate, waiting for a spawn already under way. Git that is running is left
    /// alone.
    pub fn close(&self) {
        *self.0.lock().unwrap() = true;
    }

    /// Runs `spawn` unless the gate is closed. The check and the spawn happen under one lock,
    /// so a close either prevents the spawn or waits for it.
    pub(crate) fn spawn_unless_closed(
        &self,
        spawn: impl FnOnce() -> std::io::Result<Child>,
    ) -> Option<std::io::Result<Child>> {
        let closed = self.0.lock().unwrap();
        (!*closed).then(spawn)
    }
}
