//! The Network Jobs: fetch and push.

use std::path::Path;

use cairn_write::{Git, Invocation, JobClass, RunError, Stop};

use crate::JobState;

/// `git -c fetch.writeCommitGraph=true fetch --all --prune --progress`. `output` sees what
/// git writes as it writes it, and `stop` kills git. The commit-graph keeps repos opened
/// in Cairn fast without a separate Job.
///
/// `--progress` makes git report progress although its stderr isn't a terminal, but only
/// a fetch that talks to the remote itself honours it. `--all` hands each remote to a
/// sub-fetch that git starts without `--progress`, unless there is just one remote. So a
/// repo with several remotes shows no progress reports, though it still shows everything
/// else git and the remote write.
pub(crate) fn fetch(workdir: &Path, stop: &Stop, output: &mut dyn FnMut(&[u8])) -> JobState {
    run(
        workdir,
        &[
            "-c",
            "fetch.writeCommitGraph=true",
            "fetch",
            "--all",
            "--prune",
            "--progress",
        ],
        stop,
        output,
    )
}

/// `git push --progress`, which pushes as the user's own `git push` would. `output` sees
/// what git and the remote write as they write it, and `stop` kills git.
pub(crate) fn push(workdir: &Path, stop: &Stop, output: &mut dyn FnMut(&[u8])) -> JobState {
    run(workdir, &["push", "--progress"], stop, output)
}

fn run(workdir: &Path, args: &[&str], stop: &Stop, output: &mut dyn FnMut(&[u8])) -> JobState {
    let git = match Git::shared() {
        Ok(git) => git,
        Err(e) => return JobState::Done(Err(e.to_string())),
    };
    let invocation = Invocation::new(JobClass::Network, workdir)
        .args(args.iter().copied())
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
