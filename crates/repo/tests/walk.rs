//! History walks: from all refs and HEAD, in topological order with commit date as the
//! tie-break, through gix when the repository has a commit-graph and `git rev-list` when it
//! doesn't. Which engine ran is read from git's own trace: a walk through gix starts no
//! `git rev-list`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use cairn_repo::{Intent, Oid};
use common::{
    Harness, commit_graph_exists, git, git_ran, open, remove_commit_graph, rev_list_order,
    small_fixture_clone, write_commit, write_commit_graph,
};

/// Walks the open repository, `repo`. A walk takes about a second, so after 30 the wait
/// fails with the git runs the trace2 log shows for `repo`, the way to tell a walk that never
/// started git from a `git rev-list` that never exited.
fn walk(harness: &Harness, repo: &Path) -> Vec<Oid> {
    match harness
        .service
        .walk_history()
        .recv_timeout(Duration::from_secs(30))
    {
        Ok(walked) => walked.expect("the walk succeeds"),
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("something Superseded this walk"),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            common::fail_hung(harness, "no walk answered within 30 s", repo)
        }
    }
}

#[test]
fn with_a_commit_graph_the_walk_matches_git_and_starts_no_rev_list() {
    let (_dir, repo) = small_fixture_clone();
    write_commit_graph(&repo);
    let harness = Harness::start();
    open(&harness, &repo);

    let walked = walk(&harness, &repo);

    // The oracle runs rev-list too, so this is read before it.
    assert!(
        !git_ran(&repo, "rev-list"),
        "a repository with a commit-graph is walked through gix"
    );
    assert!(walked.len() > 2000, "the small fixture has its history");
    assert_eq!(walked, rev_list_order(&repo));
}

#[test]
fn without_a_commit_graph_the_walk_matches_git_through_rev_list() {
    let (_dir, repo) = small_fixture_clone();
    remove_commit_graph(&repo);
    assert!(
        !commit_graph_exists(&repo),
        "the fixture has no commit-graph"
    );
    let harness = Harness::start();
    open(&harness, &repo);

    let walked = walk(&harness, &repo);

    assert!(
        git_ran(&repo, "rev-list"),
        "a repository without a commit-graph is walked through git rev-list"
    );
    assert!(walked.len() > 2000, "the small fixture has its history");
    assert_eq!(walked, rev_list_order(&repo));
}

/// A repository with one empty-tree commit object per call to [`History::commit`], each with
/// exactly the commit date asked for, and nothing else: no refs until a test adds them.
struct History {
    dir: tempfile::TempDir,
    repo: PathBuf,
    tree: String,
}

impl History {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("history");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "--quiet"]);
        let tree = git(&repo, &["mktree"]).trim().to_owned();
        Self { dir, repo, tree }
    }

    fn commit(&self, message: &str, date: u32, parents: &[&str]) -> String {
        write_commit(
            &self.repo,
            self.dir.path(),
            &self.tree,
            parents,
            date,
            message,
        )
    }

    /// Makes `tip` the commit of the branch `main`, and `main` HEAD's branch.
    fn check_out(&self, tip: &str) {
        self.git(&["update-ref", "refs/heads/main", tip]);
        self.git(&["symbolic-ref", "HEAD", "refs/heads/main"]);
    }

    fn git(&self, args: &[&str]) -> String {
        git(&self.repo, args)
    }
}

/// Both engines: the walk on `history` as it stands through `git rev-list`, then on a copy
/// with a commit-graph through gix. A copy, because the first walk's rev-list stays in the
/// trace. Proves which ran each time.
fn walk_on_both_engines(history: &History) -> [Vec<Oid>; 2] {
    let repo = &history.repo;
    assert!(
        !commit_graph_exists(repo),
        "the fixture starts with no graph"
    );
    let harness = Harness::start();
    open(&harness, repo);
    let through_git = walk(&harness, repo);
    assert!(
        git_ran(repo, "rev-list"),
        "no graph: walked by git rev-list"
    );

    let scratch = tempfile::tempdir().unwrap();
    let copy = scratch.path().join("copy");
    copy_dir(repo, &copy);
    write_commit_graph(&copy);
    open(&harness, &copy);
    let through_gix = walk(&harness, &copy);
    assert!(
        !git_ran(&copy, "rev-list"),
        "with a graph: walked by gix, not git rev-list"
    );
    [through_git, through_gix]
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// `git log`'s manual draws this history to explain the orders, numbered by commit date:
///
/// ```text
/// ---1----2----4----7
///     \              \
///      3----5----6----8---
/// ```
///
/// Date order is 8 7 6 5 4 3 2 1, where `--topo-order` alone gives 8 6 5 3 7 4 2 1. Commit 9
/// is 8's child but has the oldest date of all, and still comes first: no parent before its
/// children, whatever the clock says.
#[test]
fn parents_follow_their_children_and_commit_date_breaks_the_ties_on_both_engines() {
    let history = History::new();
    let c1 = history.commit("1", 2000, &[]);
    let c2 = history.commit("2", 2001, &[&c1]);
    let c3 = history.commit("3", 2002, &[&c1]);
    let c4 = history.commit("4", 2003, &[&c2]);
    let c5 = history.commit("5", 2004, &[&c3]);
    let c6 = history.commit("6", 2005, &[&c5]);
    let c7 = history.commit("7", 2006, &[&c4]);
    let c8 = history.commit("8", 2007, &[&c6, &c7]);
    let c9 = history.commit("9", 1000, &[&c8]);
    history.check_out(&c9);
    let expected: Vec<Oid> = [&c9, &c8, &c7, &c6, &c5, &c4, &c3, &c2, &c1]
        .into_iter()
        .map(|id| Oid::from_hex(id.as_str()))
        .collect();

    let [through_git, through_gix] = walk_on_both_engines(&history);

    // Seen red, each engine in turn: `Sorting::TopoOrder` in place of `DateOrder` in gix, and
    // dropping `--date-order` from the rev-list.
    assert_eq!(through_git, expected);
    assert_eq!(through_gix, expected);
}

/// Every kind of start `--all` has: a branch, a remote-tracking branch, a lightweight tag, an
/// annotated tag, a tag of a tag, a ref outside the usual namespaces, a detached HEAD and a
/// linked worktree's HEAD, each the only way to reach its commit. A tag of a tree starts
/// nothing. Every commit has its own date: gix and git break a tie of equal dates differently.
#[test]
fn the_walk_starts_from_every_ref_and_head_on_both_engines() {
    let history = History::new();
    let root = history.commit("root", 3000, &[]);
    let tip = |name: &str, date: u32| history.commit(name, date, &[&root]);
    let branch = tip("branch", 3001);
    let remote = tip("remote", 3002);
    let light = tip("lightweight tag", 3003);
    let annotated = tip("annotated tag", 3004);
    let nested = tip("tag of a tag", 3005);
    let custom = tip("custom ref", 3006);
    let linked = tip("linked worktree's HEAD", 3007);
    let head = tip("detached HEAD", 3008);
    for (name, id) in [
        ("refs/heads/branch", &branch),
        ("refs/remotes/origin/branch", &remote),
        ("refs/tags/light", &light),
        ("refs/custom/ref", &custom),
    ] {
        history.git(&["update-ref", name, id]);
    }
    let tagger = ["-c", "user.name=Cairn", "-c", "user.email=t@cairn.invalid"];
    let tag = |name: &str, target: &str| {
        let args = [&tagger[..], &["tag", "-a", "-m", name, name, target]].concat();
        history.git(&args);
    };
    tag("annotated", &annotated);
    tag("inner", &nested);
    tag("outer", "inner");
    history.git(&["update-ref", "-d", "refs/tags/inner"]);
    history.git(&["update-ref", "refs/tags/a-tree", &history.tree]);
    let worktree = history.dir.path().join("linked");
    history.git(&[
        "worktree",
        "add",
        "--quiet",
        "--detach",
        worktree.to_str().unwrap(),
        &linked,
    ]);
    history.git(&["update-ref", "--no-deref", "HEAD", &head]);

    let expected = rev_list_order(&history.repo);
    let everything: Vec<Oid> = [
        &root, &branch, &remote, &light, &annotated, &nested, &custom, &linked, &head,
    ]
    .into_iter()
    .map(|id| Oid::from_hex(id.as_str()))
    .collect();
    for commit in &everything {
        assert!(
            expected.contains(commit),
            "git's --all reaches {commit}: the fixture is what this test says it is"
        );
    }
    assert_eq!(expected.len(), everything.len());

    let [through_git, through_gix] = walk_on_both_engines(&history);

    assert_eq!(through_git, expected);
    // Seen red: dropping the worktree loop from gix's tips, then HEAD, then every ref but the
    // local branches.
    assert_eq!(through_gix, expected);
}

/// A commit-graph that stops short of the newest commits still speeds up the walk, so gix
/// keeps walking, reading the rest from the object database.
#[test]
fn a_commit_graph_missing_the_newest_commits_is_walked_through_gix_and_misses_nothing() {
    let (_dir, repo) = small_fixture_clone();
    write_commit_graph(&repo);
    git(
        &repo,
        &[
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "after the graph",
        ],
    );
    let newest = common::git_read(&repo, &["rev-parse", "HEAD"]);
    let newest = Oid::from_hex(String::from_utf8(newest).unwrap().trim());
    let harness = Harness::start();
    open(&harness, &repo);

    let walked = walk(&harness, &repo);

    // Seen red: `has_commit_graph` answering false.
    assert!(!git_ran(&repo, "rev-list"), "walked through gix");
    assert_eq!(walked.first(), Some(&newest), "the commit the graph lacks");
    assert_eq!(walked, rev_list_order(&repo));
}

/// Three commits and a commit-graph of them, then `setup`, which makes git ignore the graph.
/// `git rev-list` must walk such a repository, because what the graph says isn't the history
/// git shows: `setup` gets the tip and an unrelated commit.
fn assert_git_walks_a_graph_it_would_ignore(setup: impl FnOnce(&History, &str, &str)) {
    let history = History::new();
    let root = history.commit("root", 4000, &[]);
    let middle = history.commit("middle", 4001, &[&root]);
    let tip = history.commit("tip", 4002, &[&middle]);
    let other = history.commit("other", 4003, &[]);
    history.check_out(&tip);
    write_commit_graph(&history.repo);
    setup(&history, &tip, &other);
    let harness = Harness::start();
    open(&harness, &history.repo);

    let walked = walk(&harness, &history.repo);

    // Seen red, for each caller: `commit_graph()` in place of `commit_graph_if_enabled()` for
    // config and replace refs, and without their check in `has_commit_graph` for grafts and a
    // shallow boundary.
    assert!(git_ran(&history.repo, "rev-list"), "walked by git rev-list");
    assert_eq!(walked, rev_list_order(&history.repo));
}

#[test]
fn git_walks_a_graph_turned_off_by_config() {
    assert_git_walks_a_graph_it_would_ignore(|history, _, _| {
        history.git(&["config", "core.commitGraph", "false"]);
    });
}

#[test]
fn git_walks_a_graph_alongside_a_replace_ref() {
    assert_git_walks_a_graph_it_would_ignore(|history, tip, other| {
        history.git(&["replace", tip, other]);
    });
}

#[test]
fn git_walks_a_graph_alongside_grafts() {
    assert_git_walks_a_graph_it_would_ignore(|history, tip, _| {
        let grafts = history.repo.join(".git/info/grafts");
        std::fs::create_dir_all(grafts.parent().unwrap()).unwrap();
        std::fs::write(grafts, format!("{tip}\n")).unwrap();
    });
}

#[test]
fn git_walks_a_graph_at_a_shallow_boundary() {
    assert_git_walks_a_graph_it_would_ignore(|history, tip, _| {
        let parent = history.git(&["rev-parse", &format!("{tip}^")]);
        std::fs::write(history.repo.join(".git/shallow"), parent).unwrap();
    });
}

#[test]
fn a_commit_graph_gix_cant_read_is_walked_by_git_instead() {
    let (_dir, repo) = small_fixture_clone();
    git(&repo, &["commit-graph", "write", "--reachable"]);
    let chain = repo.join(".git/objects/info/commit-graph");
    let chained = repo.join(".git/objects/info/commit-graphs/commit-graph-chain");
    assert!(
        chain.exists() || chained.exists(),
        "the fixture has a graph"
    );
    // Readable by nothing: git warns and ignores it, and gix reports an error.
    let path = if chain.exists() { chain } else { chained };
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&path, permissions).unwrap();
    std::fs::write(&path, b"this is not a commit-graph").unwrap();
    let harness = Harness::start();
    open(&harness, &repo);

    let walked = walk(&harness, &repo);

    // Seen red: calling gix directly instead of through `ReadMode`, which reports gix's error.
    assert!(
        git_ran(&repo, "rev-list"),
        "gix's error retried through git"
    );
    assert_eq!(walked, rev_list_order(&repo));
}

#[test]
fn a_repository_with_no_commits_walks_to_nothing() {
    let history = History::new();
    let harness = Harness::start();
    harness.service.send(Intent::Open(history.repo.clone()));
    // Seen red: `HEAD` in place of `--all`, which fails on an unborn branch.
    assert_eq!(walk(&harness, &history.repo), Vec::<Oid>::new());
}

/// HEAD names a branch with no commits yet, while other refs have history in the graph.
#[test]
fn an_unborn_head_doesnt_stop_gix_walking_the_refs() {
    let history = History::new();
    let tip = history.commit("tip", 5000, &[]);
    history.git(&["update-ref", "refs/heads/main", &tip]);
    history.git(&["symbolic-ref", "HEAD", "refs/heads/unborn"]);
    write_commit_graph(&history.repo);
    let harness = Harness::start();
    open(&harness, &history.repo);

    let walked = walk(&harness, &history.repo);

    // Seen red: `repo.head_id()?` in place of treating an unborn HEAD as no tip, which fails
    // and falls back to git, so this first assertion is the one that catches it.
    assert!(!git_ran(&history.repo, "rev-list"), "walked through gix");
    assert_eq!(walked, [Oid::from_hex(tip)]);
}

/// A partial clone is one of the repositories the Repo service never reads through gix.
#[test]
fn a_partial_clone_with_a_commit_graph_is_walked_by_git() {
    let history = History::new();
    let tip = history.commit("tip", 6000, &[]);
    history.check_out(&tip);
    write_commit_graph(&history.repo);
    history.git(&["config", "core.repositoryFormatVersion", "1"]);
    history.git(&["config", "extensions.partialClone", "origin"]);
    let harness = Harness::start();
    open(&harness, &history.repo);

    let walked = walk(&harness, &history.repo);

    // Seen red: calling gix directly instead of through `ReadMode`.
    assert!(git_ran(&history.repo, "rev-list"), "walked by git");
    assert_eq!(walked, [Oid::from_hex(tip)]);
}

#[test]
fn with_no_repository_open_the_walk_says_so() {
    let harness = Harness::start();
    // Seen red: closing the channel without saying why.
    let answer = harness.service.walk_history().recv().unwrap();
    assert_eq!(answer, Err("no repository is open".to_owned()));
}
