//! The fetch Network Job.

use std::path::Path;

use cairn_write::{Git, Invocation, JobClass, RunError, Stop};

use crate::JobState;

/// `git -c fetch.writeCommitGraph=true fetch --all --prune --progress`. `output` sees what
/// git writes as it writes it, and `stop` kills git. The commit-graph keeps repos opened
/// in Cairn fast without a separate Job; git before 2.24 doesn't know the setting and
/// ignores it.
///
/// `--progress` makes git report progress although its stderr isn't a terminal, but only
/// a fetch that talks to the remote itself honours it. `--all` hands each remote to a
/// sub-fetch that git starts without `--progress`, unless there is just one remote and git
/// is 2.37 or newer. So a repo with several remotes, or any repo on older git, shows no
/// progress reports, though it still shows everything else git and the remote write.
pub(crate) fn fetch(workdir: &Path, stop: &Stop, output: &mut dyn FnMut(&[u8])) -> JobState {
    let git = match Git::shared() {
        Ok(git) => git,
        Err(e) => return JobState::Done(Err(e.to_string())),
    };
    let invocation = Invocation::new(JobClass::Network, workdir)
        .args([
            "-c",
            "fetch.writeCommitGraph=true",
            "fetch",
            "--all",
            "--prune",
            "--progress",
        ])
        .stop_on(stop);
    match git
        .run_streaming(&invocation, output)
        .and_then(|out| out.success())
    {
        Ok(_) => JobState::Done(Ok(())),
        Err(RunError::Stopped { .. }) => JobState::Stopped,
        Err(e) => JobState::Done(Err(e.to_string())),
    }
}
