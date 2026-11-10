//! The stash Local write Job: the first of Stash and switch's two Jobs.

use std::path::Path;

use cairn_write::{Git, Invocation, JobClass, StartGate};

/// `git stash push -m "WIP on <branch>"`, with `-u` when `untracked`, naming the branch
/// HEAD is on, or HEAD's short sha when it is detached. With nothing to stash git prints
/// `No local changes to save` and exits 0, which is success. `output` sees what git
/// writes to stderr as it writes it. Errors are git's own words, from stdout and stderr,
/// or its exit status when it says nothing. `gate` keeps git from starting once a Force
/// quit has decided to exit.
pub(crate) fn stash(
    workdir: &Path,
    untracked: bool,
    gate: &StartGate,
    output: &mut dyn FnMut(&[u8]),
) -> Result<(), String> {
    let git = Git::shared().map_err(|e| e.to_string())?;
    let message = format!("WIP on {}", head_name(git, workdir, gate)?);
    let mut args = vec!["stash", "push"];
    if untracked {
        args.push("-u");
    }
    args.extend(["-m", &message]);
    let out = git
        .run_streaming(
            &Invocation::new(JobClass::LocalWrite, workdir)
                .args(args)
                .through(gate),
            output,
        )
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    // `git stash` reports some failures on stdout, such as `needs merge`, and some with no
    // word at all, such as a held index lock.
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        out.stderr_text()
    );
    Err(if said.trim().is_empty() {
        format!("git stash push failed with {}", out.status)
    } else {
        said
    })
}

/// The short name of the branch HEAD is on, or HEAD's short sha when it is detached: the
/// name of what the switch leaves. Through `gate` too, so no git starts after a Force quit.
fn head_name(git: &Git, workdir: &Path, gate: &StartGate) -> Result<String, String> {
    let read = |args: &[&str]| {
        git.run(
            &Invocation::new(JobClass::Read, workdir)
                .args(args.iter().copied())
                .through(gate),
        )
    };
    // `--quiet` makes a detached HEAD exit 1 with nothing on stderr.
    let branch =
        read(&["symbolic-ref", "--quiet", "--short", "HEAD"]).map_err(|e| e.to_string())?;
    let out = if branch.status.success() {
        branch
    } else {
        read(&["rev-parse", "--short", "HEAD"])
            .and_then(|out| out.success())
            .map_err(|e| e.to_string())?
    };
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
