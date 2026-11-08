//! Push Intent to a Network Job: the Snapshots say a push is running, so `app` can ask
//! before quitting.

mod common;

use cairn_repo::{Intent, JobKind, JobState};
use common::{
    Harness, Release, Seen, git, is_ended, open, slow_remote, wait_until_network_job_is_waiting,
};

#[test]
fn a_push_is_reported_as_running_and_updates_the_remote() {
    let f = slow_remote();
    let tip = f.commit_to_push();
    let harness = Harness::start();
    let _release = Release(&f.release);
    open(&harness, &f.repo);

    harness.service.send(Intent::Push);
    let mut seen = Seen::default();
    wait_until_network_job_is_waiting(&mut seen, &harness, &JobKind::Push);
    // Blocked in the wrapper, git on the remote hasn't run yet.
    assert!(!f.remote_has_pushed());

    std::fs::write(&f.release, "").unwrap();
    seen.until(&harness, |s| {
        s.job(&JobKind::Push).is_some_and(|j| is_ended(j))
    });
    let job = seen.job(&JobKind::Push).unwrap();
    assert_eq!(job.state, JobState::Done(Ok(())), "{job:?}");
    assert_eq!(git(&f.remote, &["rev-parse", "refs/heads/pushed"]), tip);
}
