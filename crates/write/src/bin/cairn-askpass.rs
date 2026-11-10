//! `cairn-askpass`: the program the runner names in `SSH_ASKPASS` (ADR 0008).
//!
//! `ssh` and `ssh-keygen` run it with the Prompt's text as its one argument, and read the
//! answer from its stdout. Cairn can't show a Prompt yet, so it refuses every one: it exits
//! non-zero with no answer, and the program that asked stops with its own error. Showing the
//! Prompt in Cairn's window (#63) starts here, by sending that text to the app instead.

use std::process::ExitCode;

fn main() -> ExitCode {
    ExitCode::FAILURE
}
