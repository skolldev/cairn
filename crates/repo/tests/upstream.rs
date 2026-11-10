//! Each local branch's upstream in the Refs Snapshot, and the ahead/behind counts the
//! ShowCounts Intent asks for, checked against real git.

mod common;

use std::path::{Path, PathBuf};

use std::sync::Arc;

use cairn_repo::{Count, CountsSnapshot, Intent, RefEntry, RefName, Refs, Snapshot};
use common::{
    Harness, Release, SlowRemote, git, git_read, git_runs, open_refs, refs_of, slow_remote,
    wait_for_wrapper_runs,
};

/// A repository `copy` cloned from `origin`, whose local branches cover each kind of upstream:
///
/// - `main` tracks `origin/main`, 2 commits ahead and 3 behind.
/// - `topic` tracks `origin/topic`, 1 commit ahead.
/// - `lonely` has no upstream.
/// - `orphan` is configured to track `origin/gone`, which doesn't exist: its upstream is gone.
/// - `dot` tracks the local `main` (`branch.dot.remote = .`).
/// - `short` tracks `lonely` by its short name, which git expands for `.`.
/// - `tagged` tracks `refs/held/tag`, an annotated tag outside the listed refs.
/// - `excluded` tracks `origin/excluded`, gone, which a negative refspec rules out of fetches.
///   git still maps it through the positive refspec.
struct Tracking {
    _dir: tempfile::TempDir,
    repo: PathBuf,
}

fn commit(repo: &Path, message: &str) -> String {
    git(repo, &["commit", "--quiet", "--allow-empty", "-m", message]);
    git(repo, &["rev-parse", "HEAD"]).trim().to_owned()
}

fn tracking() -> Tracking {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin");
    let repo = dir.path().join("copy");
    let path = |p: &Path| p.to_str().unwrap().to_owned();
    git(
        dir.path(),
        &["init", "--quiet", "-b", "main", &path(&origin)],
    );
    commit(&origin, "base");
    git(
        dir.path(),
        &["clone", "--quiet", &path(&origin), &path(&repo)],
    );

    for message in ["upstream 1", "upstream 2", "upstream 3"] {
        commit(&origin, message);
    }
    git(&origin, &["switch", "--quiet", "-c", "topic", "main~3"]);
    commit(&origin, "topic upstream");
    git(&origin, &["switch", "--quiet", "main"]);

    for message in ["local 1", "local 2"] {
        commit(&repo, message);
    }
    git(&repo, &["fetch", "--quiet", "origin"]);
    git(
        &repo,
        &["branch", "--quiet", "--track", "topic", "origin/topic"],
    );
    git(&repo, &["switch", "--quiet", "topic"]);
    commit(&repo, "topic local");
    git(&repo, &["switch", "--quiet", "main"]);
    git(&repo, &["branch", "--quiet", "--no-track", "lonely"]);
    git(&repo, &["branch", "--quiet", "--no-track", "orphan"]);
    git(&repo, &["config", "branch.orphan.remote", "origin"]);
    git(&repo, &["config", "branch.orphan.merge", "refs/heads/gone"]);
    git(&repo, &["branch", "--quiet", "--track", "dot", "main"]);
    for (branch, remote, merge) in [
        ("short", ".", "lonely"),
        ("tagged", ".", "refs/held/tag"),
        ("excluded", "origin", "refs/heads/excluded"),
    ] {
        git(&repo, &["branch", "--quiet", "--no-track", branch]);
        git(
            &repo,
            &["config", &format!("branch.{branch}.remote"), remote],
        );
        git(&repo, &["config", &format!("branch.{branch}.merge"), merge]);
    }
    git(&repo, &["tag", "-a", "-m", "held", "held-tag"]);
    let tag = git(&repo, &["rev-parse", "refs/tags/held-tag"]);
    git(&repo, &["update-ref", "refs/held/tag", tag.trim()]);
    git(&repo, &["tag", "-d", "held-tag"]);
    git(
        &repo,
        &[
            "config",
            "--add",
            "remote.origin.fetch",
            "^refs/heads/excluded",
        ],
    );
    Tracking { _dir: dir, repo }
}

/// One line per local branch: its name, its upstream, and `gone` for a gone upstream.
fn render_upstreams(refs: &Refs) -> Vec<String> {
    refs.local
        .iter()
        .map(|RefEntry { name, upstream, .. }| {
            let (upstream, gone) = match upstream {
                Some(upstream) => (upstream.name.as_str(), upstream.target.is_none()),
                None => ("", false),
            };
            format!("{name}\0{upstream}\0{}", if gone { "gone" } else { "" })
        })
        .collect()
}

/// The same lines from git: `%(upstream)`, and `[gone]` from `%(upstream:track)`.
fn git_upstreams(repo: &Path) -> Vec<String> {
    git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(upstream)%00%(upstream:track,nobracket)",
            "refs/heads/",
        ],
    )
    .lines()
    .map(|line| {
        let (head, track) = line.rsplit_once('\0').unwrap();
        format!("{head}\0{}", if track == "gone" { "gone" } else { "" })
    })
    .collect()
}

fn upstream_of<'a>(refs: &'a Refs, branch: &str) -> Option<&'a cairn_repo::Upstream> {
    refs.local
        .iter()
        .find(|entry| entry.name.short() == branch)
        .unwrap_or_else(|| panic!("no branch {branch}"))
        .upstream
        .as_ref()
}

fn assert_upstreams(repo: &Path, refs: &Refs) {
    let main = upstream_of(refs, "main").expect("main has an upstream");
    assert_eq!(main.name.as_str(), "refs/remotes/origin/main");
    assert_eq!(
        main.target.as_ref().map(|t| t.as_str().to_owned()),
        Some(git(repo, &["rev-parse", "origin/main"]).trim().to_owned()),
    );
    assert_eq!(upstream_of(refs, "lonely"), None);
    let orphan = upstream_of(refs, "orphan").expect("a gone upstream is still an upstream");
    assert_eq!(orphan.name.as_str(), "refs/remotes/origin/gone");
    assert_eq!(orphan.target, None, "gone");
    let dot = upstream_of(refs, "dot").expect("dot tracks main");
    assert_eq!(dot.name.as_str(), "refs/heads/main");
    // Seen red: `dwim_local` returning the merge name as it is.
    let short = upstream_of(refs, "short").expect("short tracks lonely");
    assert_eq!(short.name.as_str(), "refs/heads/lonely");
    // A tag stays on its tag object, as `%(objectname)` does.
    // Seen red: `ref_target` peeling with `peel_to_id`.
    let tagged = upstream_of(refs, "tagged").expect("tagged tracks a tag");
    assert_eq!(
        tagged.target.as_ref().map(|t| t.as_str().to_owned()),
        Some(oid(repo, "refs/held/tag"))
    );
    assert_ne!(
        oid(repo, "refs/held/tag"),
        oid(repo, "refs/held/tag^{commit}")
    );
    // Seen red: matching negative refspecs too.
    let excluded = upstream_of(refs, "excluded").expect("git ignores negative refspecs here");
    assert_eq!(excluded.name.as_str(), "refs/remotes/origin/excluded");
    assert_eq!(render_upstreams(refs), git_upstreams(repo));
}

#[test]
fn each_local_branch_carries_its_upstream_as_git_resolves_it() {
    let t = tracking();
    let snapshot = open_refs(&t.repo);
    assert_upstreams(&t.repo, snapshot.result.as_ref().expect("the copy opens"));
}

#[test]
fn the_cli_refs_read_carries_the_same_upstreams() {
    let t = tracking();
    // Picks the CLI read, as `cli_refs_path_matches_git_for_each_ref_head_and_stash` does.
    git(&t.repo, &["config", "core.safecrlf", "true"]);
    let snapshot = open_refs(&t.repo);
    assert_upstreams(&t.repo, snapshot.result.as_ref().expect("the copy opens"));
}

/// The counts Snapshot, if `snapshot` is one.
fn counts_of(snapshot: &Snapshot) -> Option<&Arc<CountsSnapshot>> {
    match snapshot {
        Snapshot::Counts(counts) => Some(counts),
        _ => None,
    }
}

fn branches(names: &[&str]) -> Vec<RefName> {
    names
        .iter()
        .map(|name| RefName::new(format!("refs/heads/{name}")))
        .collect()
}

/// Waits for a counts Snapshot that `done` accepts, and returns it.
fn counts_until(
    harness: &Harness,
    mut done: impl FnMut(&CountsSnapshot) -> bool,
) -> Arc<CountsSnapshot> {
    let seen = harness.drain_until(|s| counts_of(s).is_some_and(|c| done(c)));
    seen.iter()
        .rev()
        .find_map(counts_of)
        .expect("a counts Snapshot")
        .clone()
}

fn count<'a>(counts: &'a CountsSnapshot, branch: &str) -> Option<&'a Count> {
    counts
        .result
        .as_ref()
        .expect("counts read")
        .branches
        .get(&RefName::new(format!("refs/heads/{branch}")))
}

/// `git rev-list --left-right --count <branch>...<upstream>`, as ahead and behind.
fn rev_list_count(repo: &Path, branch: &str) -> (usize, usize) {
    let out = git_read(
        repo,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{branch}...{branch}@{{upstream}}"),
        ],
    );
    let out = String::from_utf8(out).unwrap();
    let (ahead, behind) = out.trim().split_once('\t').unwrap();
    (ahead.parse().unwrap(), behind.parse().unwrap())
}

/// The counts Cairn ran for `repo`, by git's trace2 log: each `rev-list --left-right` run.
fn counts_run(repo: &Path) -> Vec<String> {
    git_runs(repo)
        .into_iter()
        .filter(|run| run.contains("rev-list") && run.contains("--left-right"))
        .collect()
}

#[test]
fn counts_match_git_rev_list_for_a_branch_ahead_2_and_behind_3() {
    let t = tracking();
    let harness = Harness::start();
    harness.service.send(Intent::Open(t.repo.clone()));
    harness
        .service
        .send(Intent::ShowCounts(branches(&["main"])));
    let counts = counts_until(&harness, |c| {
        c.result.as_ref().is_ok_and(|c| !c.branches.is_empty())
    });
    let main = count(&counts, "main").expect("main is counted");
    assert_eq!((main.ahead, main.behind), (2, 3));
    assert_eq!((main.ahead, main.behind), rev_list_count(&t.repo, "main"));
    assert_eq!(
        main.local.as_str(),
        git(&t.repo, &["rev-parse", "main"]).trim()
    );
    assert_eq!(
        main.upstream.as_str(),
        git(&t.repo, &["rev-parse", "origin/main"]).trim()
    );
}

/// Whether the counts Read Job is done: nothing visible is left to count.
fn finished(counts: &CountsSnapshot) -> bool {
    counts.result.as_ref().is_ok_and(|c| c.pending.is_empty())
}

fn oid(repo: &Path, rev: &str) -> String {
    git(repo, &["rev-parse", rev]).trim().to_owned()
}

/// Waits for a Refs Snapshot that `refs_done` accepts, then for the finished counts of that
/// Refs read or a later one, and returns those counts. Their Read Job ran every count that
/// Refs read could start, so they mark that none is left to come.
fn counts_after_refs(
    harness: &Harness,
    mut refs_done: impl FnMut(&Refs) -> bool,
) -> Arc<CountsSnapshot> {
    let mut read = None;
    let mut counted = None;
    harness.drain_until(|s| {
        if let Some(refs) = refs_of(s)
            && read.is_none()
            && refs.result.as_ref().is_ok_and(&mut refs_done)
        {
            read = Some(refs.generation);
        }
        if let (Some(read), Some(counts)) = (read, counts_of(s))
            && finished(counts)
            && counts.result.as_ref().is_ok_and(|c| c.refs >= read)
        {
            counted = Some(counts.clone());
        }
        counted.is_some()
    });
    counted.unwrap()
}

#[test]
fn only_the_branches_named_as_visible_are_counted() {
    let t = tracking();
    let topic = oid(&t.repo, "topic");
    assert_ne!(topic, oid(&t.repo, "main"));
    let harness = Harness::start();
    harness.service.send(Intent::Open(t.repo.clone()));
    harness
        .service
        .send(Intent::ShowCounts(branches(&["main", "lonely"])));
    let counts = counts_until(&harness, |c| finished(c) && count(c, "main").is_some());

    // Seen red: counting every branch with an upstream, not only the visible ones (both
    // `state.visible` filters in `Counter::count` removed).
    assert_eq!(count(&counts, "topic"), None);
    assert!(
        !counts_run(&t.repo).iter().any(|run| run.contains(&topic)),
        "{:?}",
        counts_run(&t.repo)
    );
    assert_eq!(count(&counts, "lonely"), None, "no upstream, no count");

    // Control: once named, topic is counted.
    harness
        .service
        .send(Intent::ShowCounts(branches(&["main", "topic"])));
    let counts = counts_until(&harness, |c| finished(c) && count(c, "topic").is_some());
    assert_eq!(
        count(&counts, "topic").map(|c| (c.ahead, c.behind)),
        Some((1, 0))
    );
    assert!(counts_run(&t.repo).iter().any(|run| run.contains(&topic)));
}

#[test]
fn a_refs_refresh_that_moves_no_oid_counts_nothing_and_a_moved_branch_alone_is_recounted() {
    let t = tracking();
    common::assert_filesystem_events_available(&t.repo);
    let main = oid(&t.repo, "main");
    let harness = Harness::start();
    harness.service.send(Intent::Open(t.repo.clone()));
    harness
        .service
        .send(Intent::ShowCounts(branches(&["main", "topic"])));
    counts_until(&harness, |c| {
        finished(c) && count(c, "main").is_some() && count(c, "topic").is_some()
    });
    assert_eq!(counts_run(&t.repo).len(), 2, "{:?}", counts_run(&t.repo));

    // A new branch refreshes Refs and moves neither pair.
    git(&t.repo, &["branch", "--no-track", "other"]);
    let counts = counts_after_refs(&harness, |refs| {
        refs.local
            .iter()
            .any(|b| b.name.as_str() == "refs/heads/other")
    });
    assert!(count(&counts, "main").is_some() && count(&counts, "topic").is_some());
    // Seen red: never looking in the cache (`state.cache.get` always `None`).
    assert_eq!(counts_run(&t.repo).len(), 2, "{:?}", counts_run(&t.repo));

    // Control: moving topic onto its upstream counts topic again, and only topic.
    let upstream = oid(&t.repo, "origin/topic");
    git(&t.repo, &["update-ref", "refs/heads/topic", &upstream]);
    let counts = counts_after_refs(&harness, |refs| {
        refs.local
            .iter()
            .any(|b| b.name.as_str() == "refs/heads/topic" && b.target.as_str() == upstream)
    });
    assert_eq!(
        count(&counts, "topic").map(|c| (c.ahead, c.behind)),
        Some((0, 0))
    );
    let runs = counts_run(&t.repo);
    assert_eq!(runs.len(), 3, "{runs:?}");
    assert_eq!(
        runs.iter().filter(|run| run.contains(&main)).count(),
        1,
        "{runs:?}"
    );
}

/// A clone of the small fixture whose branch `held` tracks `refs/held/up` (`branch.held.remote
/// = .`), which points at a local tag of the commit only the slow remote has. The clone is a
/// partial clone, so counting `held` makes `git rev-list` fetch that commit to peel the tag,
/// and blocks in the remote's wrapper until `release` exists. A tag and not a missing parent,
/// because newer git no longer fetches a parent during a walk (`docs/git-versions.md`), as
/// `held_walk` in `lanes.rs` found. Outside `refs/heads/`, `refs/remotes/` and `refs/tags/`,
/// because `%(*objectname)` in a Refs read through git peels every tag it lists, and would
/// fetch too. Status never peels it.
fn held_count() -> SlowRemote {
    let f = slow_remote();
    git(
        &f.remote,
        &["config", "uploadpack.allowAnySHA1InWant", "true"],
    );
    git(&f.remote, &["config", "uploadpack.allowFilter", "true"]);
    let tag = format!(
        "object {}\ntype commit\ntag held\ntagger Cairn Test <test@cairn.invalid> 1700000000 +0000\n\nneeds the remote\n",
        f.new
    );
    let file = f.dir.path().join("tag-object");
    std::fs::write(&file, tag).unwrap();
    let tag = git(
        &f.repo,
        &["hash-object", "-t", "tag", "-w", file.to_str().unwrap()],
    );
    git(&f.repo, &["update-ref", "refs/held/up", tag.trim()]);
    git(
        &f.repo,
        &["branch", "--quiet", "--no-track", "held", &f.old],
    );
    git(&f.repo, &["config", "branch.held.remote", "."]);
    git(&f.repo, &["config", "branch.held.merge", "refs/held/up"]);
    git(&f.repo, &["config", "core.repositoryFormatVersion", "1"]);
    git(&f.repo, &["config", "extensions.partialClone", "origin"]);
    git(&f.repo, &["config", "remote.origin.promisor", "true"]);
    f
}

#[test]
fn a_count_held_in_git_never_delays_a_refs_snapshot() {
    let f = held_count();
    common::assert_filesystem_events_available(&f.repo);
    let harness = Harness::start();
    let _release = Release(&f.release);
    harness.service.send(Intent::Open(f.repo.clone()));
    harness
        .service
        .send(Intent::ShowCounts(branches(&["held"])));
    wait_for_wrapper_runs(&f.log, 1);

    // The count is held until `release` exists, so the Refs Snapshot showing this branch
    // arrives while it is. Not `drain_until`: its Force quit on a timeout would wait for
    // whatever holds the count, so a failing run would hang. This one releases the count
    // before it fails.
    git(&f.repo, &["branch", "--no-track", "other"]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut seen = Vec::new();
    while !seen.iter().filter_map(refs_of).any(|r| {
        r.result.as_ref().is_ok_and(|refs| {
            refs.local
                .iter()
                .any(|b| b.name.as_str() == "refs/heads/other")
        })
    }) {
        if std::time::Instant::now() > deadline {
            std::fs::write(&f.release, "").unwrap();
            // Seen red: holding the counter's lock across `git rev-list`, which the Refs read
            // then waits on.
            panic!("no Refs Snapshot while a count was held; drained: {seen:?}");
        }
        seen.extend(harness.service.drain());
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        counts_run(&f.repo)
            .iter()
            .any(|run| run.ends_with("no exit yet")),
        "the count was still held: {:?}",
        counts_run(&f.repo)
    );
    assert!(
        !seen
            .iter()
            .filter_map(counts_of)
            .any(|c| count(c, "held").is_some()),
        "held was counted while its count was held"
    );

    std::fs::write(&f.release, "").unwrap();
    let counts = counts_until(&harness, |c| count(c, "held").is_some());
    assert_eq!(
        count(&counts, "held").map(|c| (c.ahead, c.behind)),
        Some((0, 1))
    );
}

#[test]
fn a_running_count_finishes_while_its_branch_stays_on_screen() {
    let f = held_count();
    git(&f.repo, &["branch", "--quiet", "--no-track", "spare"]);
    let tag = oid(&f.repo, "refs/held/up");
    let held_runs = || {
        counts_run(&f.repo)
            .iter()
            .filter(|run| run.contains(&tag))
            .count()
    };
    let harness = Harness::start();
    let _release = Release(&f.release);
    harness.service.send(Intent::Open(f.repo.clone()));
    harness
        .service
        .send(Intent::ShowCounts(branches(&["held"])));
    wait_for_wrapper_runs(&f.log, 1);

    // Control: once `held` scrolls away, its count is Superseded. The marker is the next Read
    // Job finishing, which waits in the Slot until the held one has ended, and nothing but a
    // stop ends it before `release` exists.
    harness
        .service
        .send(Intent::ShowCounts(branches(&[f.branch.as_str()])));
    counts_until(&harness, |c| finished(c) && count(c, &f.branch).is_some());
    assert!(!f.release.exists());

    harness
        .service
        .send(Intent::ShowCounts(branches(&["held", f.branch.as_str()])));
    wait_for_wrapper_runs(&f.log, 2);
    // `held` stays on screen, so its count runs on.
    harness.service.send(Intent::ShowCounts(branches(&[
        "held",
        f.branch.as_str(),
        "spare",
    ])));
    std::fs::write(&f.release, "").unwrap();
    let counts = counts_until(&harness, |c| finished(c) && count(c, "held").is_some());
    assert_eq!(
        count(&counts, "held").map(|c| (c.ahead, c.behind)),
        Some((0, 1))
    );
    // One count stopped by the control, one that finished. Seen red: `show` always
    // Superseding, which stops the second and counts `held` a third time.
    assert_eq!(held_runs(), 2, "{:?}", counts_run(&f.repo));
}
