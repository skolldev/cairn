//! `trace` explains where an External change's time goes: each sample's `read_job` spans,
//! from a headless Repo service. A sample holds the reads that ended after its change began
//! and had published or ended by the time its expected state was drained, so one sample's
//! reads never show up in another's timeline.

mod common;

use std::collections::BTreeSet;
use std::path::Path;

use cairn_budget::fixture::Sandbox;
use cairn_budget::run::Change;
use cairn_budget::trace;
use common::small_fixture;

#[test]
fn each_saved_file_sample_holds_the_reads_its_own_save_started_and_no_others() {
    trace::install().unwrap();
    let sandbox = Sandbox::clone_of(&small_fixture()).unwrap();
    let ours = sandbox.repo().display().to_string();

    let samples = trace::trace_change(&sandbox, Change::Save, 3).unwrap();

    assert_eq!(samples.len(), 3, "the warm-up samples aren't kept");
    let mut seen = BTreeSet::new();
    let mut last_unstaged = 0;
    for sample in &samples {
        assert!(
            sample.changed_ns <= sample.start_ns && sample.start_ns <= sample.drained_ns,
            "{sample:?}"
        );
        // The Open's reads, and the warm-up's, came before this sample's change.
        assert!(
            sample
                .spans
                .iter()
                .all(|s| s.field("refresh") != Some("open")),
            "{sample:?}"
        );
        for span in &sample.spans {
            assert_eq!(span.repository, ours, "{span:?}");
            // Seen red: attributing every span that had started by the drain, without the
            // window starting at the change, puts the Open's and earlier samples' reads here.
            assert!(span.closed_ns > sample.changed_ns, "{span:?} in {sample:?}");
            // Seen red in `a_git_switch_sample_leaves_out_the_read_that_publishes_after_its_drain`,
            // not here: on this fixture no save's read publishes after its drain.
            assert!(
                span.published_ns.unwrap_or(span.closed_ns) <= sample.drained_ns,
                "{span:?} in {sample:?}"
            );
            let key = (
                span.field("slot").map(str::to_owned),
                span.field("generation").map(str::to_owned),
            );
            assert!(seen.insert(key), "{span:?} is in two samples");
        }
        // The read that answered: Unstaged, asked again by the watcher, newer than the last
        // sample's.
        let unstaged = sample
            .spans
            .iter()
            .filter(|s| {
                s.field("slot") == Some("unstaged") && s.field("refresh") == Some("worktree")
            })
            .filter_map(|s| s.field("generation")?.parse::<u64>().ok())
            .max()
            .unwrap_or_else(|| panic!("no Unstaged read: {sample:?}"));
        assert!(unstaged > last_unstaged, "{sample:?}");
        last_unstaged = unstaged;
    }
}

/// `path` as git's own sh reads it: forward slashes, on Windows too.
fn slash(path: &Path) -> String {
    path.to_str().unwrap().replace('\\', "/")
}

/// Holds one Unstaged read in each `git switch` sample, after the switch's own reads have
/// answered, until well after the drain: the read queued behind the answer that #58's publish
/// bound leaves out. Returns the scratch directory, where `count` is how many switches ran the
/// hook and each `taken.*` file is a read the filter held.
///
/// git's `post-checkout` hook waits for the reads the switch started, then leaves a `hold`
/// and makes the top-level README.md stat-dirty, which starts another Unstaged read before git
/// exits. A clean filter on README.md, which that read runs through `hash-object`, takes the
/// `hold` and keeps the read for longer than the switch has left. Only a Read Job takes it
/// (`--no-optional-locks` sets `GIT_OPTIONAL_LOCKS=0` for the filter), never the switch's own
/// index refresh. Each hook sets a different old mtime, so README.md is stat-dirty whatever the
/// switch refreshed, and never racily clean.
fn hold_a_read_after_each_switch(repo: &Path) -> tempfile::TempDir {
    let scratch = tempfile::tempdir().unwrap();
    let s = slash(scratch.path());
    let filter = scratch.path().join("slow-clean.sh");
    std::fs::write(
        &filter,
        format!(
            "if [ \"$GIT_OPTIONAL_LOCKS\" = 0 ] && mv '{s}/hold' \"{s}/taken.$$\" 2>/dev/null; \
             then sleep 3; fi\ncat\n"
        ),
    )
    .unwrap();
    git_write(
        repo,
        &[
            "config",
            "filter.slow.clean",
            &format!("sh '{}'", slash(&filter)),
        ],
    );
    std::fs::write(
        repo.join(".git/info/attributes"),
        "/README.md filter=slow\n",
    )
    .unwrap();
    let hook = repo.join(".git/hooks/post-checkout");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\nsleep 1\nn=$(cat '{s}/count' 2>/dev/null || echo 0)\nn=$((n + 1))\n\
             echo \"$n\" > '{s}/count'\n: > '{s}/hold'\n\
             touch -t \"20000101$(printf %02d $((n / 60 % 24)))$(printf %02d $((n % 60)))\" README.md\n\
             sleep 1\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    scratch
}

/// `git` in `repo`, as a Local write Job runs it.
fn git_write(repo: &Path, args: &[&str]) {
    let out = cairn_write::Git::shared()
        .expect("a usable git is installed")
        .run(
            &cairn_write::Invocation::new(cairn_write::JobClass::LocalWrite, repo)
                .args(args.iter().copied()),
        )
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", out.stderr_text());
}

#[test]
fn a_git_switch_sample_leaves_out_the_read_that_publishes_after_its_drain() {
    trace::install().unwrap();
    let sandbox = Sandbox::clone_of(&small_fixture()).unwrap();
    let scratch = hold_a_read_after_each_switch(sandbox.repo());

    let samples = trace::trace_change(&sandbox, Change::GitSwitch, 2).unwrap();

    assert_eq!(samples.len(), 2, "the warm-up samples aren't kept");
    for sample in &samples {
        for span in &sample.spans {
            // Seen red: without the publish bound (`published <= drained` in the headless
            // trace's span filter), the read the filter held, which started before the drain
            // and published after it, is in its sample.
            assert!(
                span.published_ns.unwrap_or(span.closed_ns) <= sample.drained_ns,
                "{span:?} in {sample:?}"
            );
        }
    }
    // Control: every switch, the warm-up's and the one before the Open included, had a read
    // held, so each sample had a read that published after its drain to leave out.
    let switches: usize = std::fs::read_to_string(scratch.path().join("count"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let held = std::fs::read_dir(scratch.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("taken.")
        })
        .count();
    assert!(switches > samples.len(), "{switches} switches");
    assert_eq!(held, switches, "a read was held after every switch");
}
