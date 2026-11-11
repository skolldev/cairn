//! The switch Local write Job.

use std::path::Path;

use cairn_write::{Git, Invocation, JobClass, StartGate};

use crate::RefName;

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

/// Switches to the remote-tracking branch `remote`, such as `refs/remotes/origin/feature/x`,
/// as [`Intent::SwitchToRemote`](crate::Intent::SwitchToRemote) describes: to the local
/// branch that tracks it, read just before, or else to a new one from `git switch --track`.
pub(crate) fn switch_to_remote(
    workdir: &Path,
    remote: &RefName,
    gate: &StartGate,
    output: &mut dyn FnMut(&[u8]),
) -> Result<(), String> {
    let git = Git::shared().map_err(|e| e.to_string())?;
    let tracking = local_upstreams(git, workdir, gate)?;
    if let Some(local) = tracking_branch(remote, &tracking) {
        return switch(workdir, local.short(), gate, output);
    }
    // The full ref name, so a local branch named `origin/feature/x` can't be taken for it.
    // git names the new branch after what follows the remote's name.
    let out = git
        .run_streaming(
            &Invocation::new(JobClass::LocalWrite, workdir)
                .args(["switch", "--track", "--", remote.as_str()])
                .through(gate),
            output,
        )
        .map_err(|e| e.to_string())?;
    out.success().map(drop).map_err(|e| e.to_string())
}

/// Each local branch with its upstream, if it has one, sorted by name as git sorts them.
/// Through `gate` too, so no git starts after a Force quit.
fn local_upstreams(
    git: &Git,
    workdir: &Path,
    gate: &StartGate,
) -> Result<Vec<(RefName, Option<RefName>)>, String> {
    let out = git
        .run(
            &Invocation::new(JobClass::Read, workdir)
                .args([
                    "for-each-ref",
                    "--format=%(refname)%00%(upstream)",
                    "refs/heads/",
                ])
                .through(gate),
        )
        .and_then(|out| out.success())
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let (name, upstream) = line.split_once('\0')?;
            let upstream = (!upstream.is_empty()).then(|| RefName::new(upstream));
            Some((RefName::new(name), upstream))
        })
        .collect())
}

/// The local branch to switch to for `remote`: of those tracking it, the one named as git
/// would name a new one (`feature/x` for `refs/remotes/origin/feature/x`), or else the first.
fn tracking_branch<'a>(
    remote: &RefName,
    locals: &'a [(RefName, Option<RefName>)],
) -> Option<&'a RefName> {
    // What `git switch --track` names the new branch: what follows `refs/remotes/<remote>/`.
    let named = remote.short().split_once('/').map(|(_, name)| name);
    let mut tracking = locals
        .iter()
        .filter(|(_, upstream)| upstream.as_ref() == Some(remote))
        .map(|(name, _)| name)
        .peekable();
    let first = *tracking.peek()?;
    Some(
        tracking
            .find(|name| Some(name.short()) == named)
            .unwrap_or(first),
    )
}
