//! `trace` explains where an External change's time goes: each sample's `read_job` spans,
//! from a headless Repo service. A sample holds the reads that ended after its change began
//! and had published or ended by the time its expected state was drained, so one sample's
//! reads never show up in another's timeline.

mod common;

use std::collections::BTreeSet;

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
            // Not seen red here: keeping every read that had started by the drain adds the one
            // queued behind the read that answered, but on this fixture none starts in time.
            // `trace` on linux-graph showed such reads before the publish bound.
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
