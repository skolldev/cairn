//! The switch Local write Job.

use std::path::Path;

use cairn_write::{Git, Invocation, JobClass, StartGate};

/// `git switch --no-guess -- <branch>`. `--no-guess` keeps it to local branches: without
/// it git would create a branch from a remote-tracking one of the same name. `output`
/// sees what git and its hooks write as it is written. Errors are git's own words. `gate`
/// keeps git from starting once a Force quit has decided to exit.
pub(crate) fn switch(
    workdir: &Path,
    branch: &str,
    gate: &StartGate,
    output: &mut dyn FnMut(&[u8]),
) -> Result<(), String> {
    let git = Git::shared().map_err(|e| e.to_string())?;
    let out = git
        .run_streaming(
            &Invocation::new(JobClass::LocalWrite, workdir)
                .args(["switch", "--no-guess", "--", branch])
                .through(gate),
            output,
        )
        .map_err(|e| e.to_string())?;
    out.success().map(drop).map_err(|e| e.to_string())
}

/// git's refusal to switch because local changes would be overwritten.
pub(crate) struct Refusal {
    /// The refusal named untracked files that the switch would overwrite.
    pub(crate) untracked: bool,
}

/// git's two refusals for local changes, each the start of an `error:` line, as git writes
/// them under the runner's `LC_ALL=C`. The tests pin them against real git.
const TRACKED: &str =
    "error: Your local changes to the following files would be overwritten by checkout:";
const UNTRACKED: &str =
    "error: The following untracked working tree files would be overwritten by checkout:";

/// Whether a failed switch's error is git refusing for local changes, and which. git
/// writes both refusals when both kinds of change are in the way.
pub(crate) fn refusal(error: &str) -> Option<Refusal> {
    let has = |refusal: &str| error.lines().any(|line| line == refusal);
    let untracked = has(UNTRACKED);
    (untracked || has(TRACKED)).then_some(Refusal { untracked })
}
