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
use common::{git_write, small_fixture};

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

/// Holds one Unstaged read in each `git switch` sample from before git exits until well after
/// the drain: the read queued behind the answer that #58's publish bound leaves out.
///
/// git's `post-checkout` hook waits for the reads the switch started, then leaves a `hold`
/// and makes the top-level README.md stat-dirty, which starts another Unstaged read. A clean
/// filter on README.md, which that read runs through `hash-object`, takes the `hold` and keeps
/// the read for 3 s. The hook waits until the `hold` is taken, so git exits, and the drain
/// comes, only once that read is running. Only a Read Job takes it (`--no-optional-locks` sets
/// `GIT_OPTIONAL_LOCKS=0` for the filter), never the switch's own index refresh. Each hook sets
/// a different old mtime, so README.md is stat-dirty whatever the switch refreshed, and never
/// racily clean.
struct HeldReads(tempfile::TempDir);

impl HeldReads {
    fn install(repo: &Path) -> Self {
        let scratch = tempfile::tempdir().unwrap();
        let s = slash(scratch.path());
        let filter = scratch.path().join("slow-clean.sh");
        std::fs::write(
            &filter,
            format!(
                "if [ \"$GIT_OPTIONAL_LOCKS\" = 0 ] && mv '{s}/hold' '{s}/taken' 2>/dev/null; \
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
        // The first switch, `trace_change`'s own before the Open, has no Repo service to hold a
        // read of, so it leaves no `hold`.
        //
        // The first `sleep 1` is a fixed wait for the switch's own reads to answer, which
        // nothing marks. Its control is the Seen-red break below: had they not answered, the
        // held read would be the answer, the drain would wait for it, and the break would stay
        // green.
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\nsleep 1\n\
                 echo >> '{s}/switches'\n\
                 n=$(wc -l < '{s}/switches')\n\
                 stamp=\"20000101$(printf %02d $((n / 60 % 24)))$(printf %02d $((n % 60)))\"\n\
                 [ \"$n\" -gt 1 ] || exit 0\n\
                 : > '{s}/hold'\n\
                 touch -t \"$stamp\" README.md\n\
                 tries=0\n\
                 while [ -e '{s}/hold' ] && [ $tries -lt 400 ]; do sleep 0.05; tries=$((tries + 1)); done\n\
                 [ -e '{s}/hold' ] || echo >> '{s}/held'\n"
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self(scratch)
    }

    fn lines(&self, name: &str) -> usize {
        std::fs::read_to_string(self.0.path().join(name))
            .unwrap_or_default()
            .lines()
            .count()
    }

    /// How many switches ran the hook, the one before the Open included.
    fn switches(&self) -> usize {
        self.lines("switches")
    }

    /// How many switches a Read Job's filter took the `hold` in before git exited.
    fn held_before_git_exited(&self) -> usize {
        self.lines("held")
    }
}

#[test]
fn a_git_switch_sample_leaves_out_the_read_that_publishes_after_its_drain() {
    trace::install().unwrap();
    let sandbox = Sandbox::clone_of(&small_fixture()).unwrap();
    let held = HeldReads::install(sandbox.repo());

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
    // Control: every switch after the Open, the warm-up's included, had a read held from
    // before git exited, so before the drain, for 3 s, so each sample had a read to leave out.
    let switches = held.switches();
    assert!(switches > samples.len() + 1, "{switches} switches");
    assert_eq!(
        held.held_before_git_exited(),
        switches - 1,
        "a read was held before git exited, after every switch after the Open"
    );
}
