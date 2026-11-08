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
