//! The sidebar in the real window, with the real Repo service and git behind it: its
//! sections, folders and remote groups, the filter box, and how HEAD shows.

use std::path::{Path, PathBuf};

use gpui::TestAppContext;

use crate::testing::{
    App, Release, block_until, git, hermetic, open, repo_with_commit, slash, slow_remote,
};

/// A repository on `main` with `feature/a` and `feature/b`, remotes `origin` (with its
/// symbolic `HEAD`) and `upstream`, tags `v1.0` and `v2.0`, and one stash.
struct Fixture {
    repo: PathBuf,
    _dir: tempfile::TempDir,
}

fn fixture() -> Fixture {
    hermetic();
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_with_commit(dir.path(), "copy");
    let path = |p: &Path| p.to_str().unwrap().to_owned();
    git(&repo, &["branch", "feature/a"]);
    git(&repo, &["branch", "feature/b"]);
    git(&repo, &["tag", "v1.0"]);
    git(&repo, &["tag", "-a", "-m", "two", "v2.0"]);
    for remote in ["origin", "upstream"] {
        let bare = dir.path().join(format!("{remote}.git"));
        git(dir.path(), &["init", "--quiet", "--bare", &path(&bare)]);
        git(&repo, &["remote", "add", remote, &path(&bare)]);
        git(&repo, &["push", "--quiet", remote, "main"]);
        git(&repo, &["fetch", "--quiet", remote]);
    }
    git(&repo, &["remote", "set-head", "origin", "main"]);
    std::fs::write(repo.join("file.txt"), "two\n").unwrap();
    git(
        &repo,
        &["stash", "push", "--quiet", "-m", "work in progress"],
    );
    Fixture { repo, _dir: dir }
}

/// Opens `repo` and waits until the sidebar shows its refs.
fn open_sidebar<'a>(cx: &'a mut TestAppContext, repo: &Path) -> App<'a> {
    let mut app = open(cx, Some(repo));
    app.until("the sidebar shows the refs", |app| {
        app.draw();
        app.shown("sidebar:stash@{0}")
    });
    app
}

fn top(app: &mut App, selector: &str) -> f32 {
    let bounds = app
        .bounds(selector)
        .unwrap_or_else(|| panic!("{selector} isn't drawn"));
    f32::from(bounds.origin.y)
}

/// Types `text` into the filter box.
fn filter(app: &mut App, text: &str) {
    app.click("sidebar:filter");
    app.cx.simulate_input(text);
    app.draw();
}

#[gpui::test]
fn the_sidebar_shows_four_sections_in_order(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open_sidebar(cx, &f.repo);

    let sections = ["Local", "Remotes", "Tags", "Stashes"].map(|section| {
        let selector = format!("sidebar:section:{section}");
        top(&mut app, &selector)
    });
    // Seen red: Tags built before Remotes in `Tree::new`.
    assert!(sections.is_sorted(), "sections out of order: {sections:?}");
    for shown in [
        "sidebar:refs/heads/main",
        "sidebar:refs/tags/v1.0",
        "sidebar:refs/tags/v2.0",
        "sidebar:stash@{0}",
    ] {
        assert!(app.shown(shown), "{shown} isn't shown");
    }
}

#[gpui::test]
fn a_slash_in_a_branch_name_shows_as_a_folder_holding_its_branches(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open_sidebar(cx, &f.repo);

    // Seen red: `insert` making every segment a leaf leaves no folder to draw.
    let folder = top(&mut app, "sidebar:refs/heads/feature/");
    assert!(folder < top(&mut app, "sidebar:refs/heads/feature/a"));
    assert!(
        top(&mut app, "sidebar:refs/heads/feature/a")
            < top(&mut app, "sidebar:refs/heads/feature/b")
    );

    app.click("sidebar:refs/heads/feature/");
    assert!(!app.shown("sidebar:refs/heads/feature/a"));
    assert!(!app.shown("sidebar:refs/heads/feature/b"));
    assert!(
        app.shown("sidebar:refs/heads/main"),
        "collapsing the folder hid main"
    );

    app.click("sidebar:refs/heads/feature/");
    assert!(app.shown("sidebar:refs/heads/feature/a"));
}

#[gpui::test]
fn remote_branches_are_grouped_by_remote(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open_sidebar(cx, &f.repo);

    let origin = top(&mut app, "sidebar:refs/remotes/origin/");
    let upstream = top(&mut app, "sidebar:refs/remotes/upstream/");
    let origin_main = top(&mut app, "sidebar:refs/remotes/origin/main");
    assert!(origin < origin_main && origin_main < upstream);
    assert!(upstream < top(&mut app, "sidebar:refs/remotes/upstream/main"));

    app.click("sidebar:refs/remotes/origin/");
    // Seen red: inserting remote branches beside their group instead of in it.
    assert!(!app.shown("sidebar:refs/remotes/origin/main"));
    assert!(app.shown("sidebar:refs/remotes/upstream/main"));
}

#[gpui::test]
fn a_remotes_symbolic_head_isnt_shown_as_a_branch(cx: &mut TestAppContext) {
    let f = fixture();
    // Control: the fixture has it.
    assert_eq!(
        git(&f.repo, &["symbolic-ref", "refs/remotes/origin/HEAD"]).trim(),
        "refs/remotes/origin/main"
    );
    let mut app = open_sidebar(cx, &f.repo);

    // The marker: the refs around it are drawn.
    assert!(app.shown("sidebar:refs/remotes/origin/main"));
    // Seen red: without the symref filter in `Tree::new`, it shows under `origin`.
    assert!(!app.shown("sidebar:refs/remotes/origin/HEAD"));
}

#[gpui::test]
fn each_section_collapses(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open_sidebar(cx, &f.repo);

    for (section, row) in [
        ("Local", "sidebar:refs/heads/main"),
        ("Remotes", "sidebar:refs/remotes/upstream/"),
        ("Tags", "sidebar:refs/tags/v1.0"),
        ("Stashes", "sidebar:stash@{0}"),
    ] {
        app.click(&format!("sidebar:section:{section}"));
        // Seen red: a section that is always open.
        assert!(!app.shown(row), "{row} is shown with {section} collapsed");
    }
    assert!(app.shown("sidebar:section:Stashes"));
}

#[gpui::test]
fn the_filter_keeps_matches_in_open_folders_and_clearing_it_restores_the_collapse_state(
    cx: &mut TestAppContext,
) {
    let f = fixture();
    let mut app = open_sidebar(cx, &f.repo);
    app.click("sidebar:refs/heads/feature/");
    assert!(!app.shown("sidebar:refs/heads/feature/b"));

    filter(&mut app, "b");
    assert!(app.shown("sidebar:refs/heads/feature/"));
    // Seen red: rows built as if the filter were empty keep the folder closed.
    assert!(app.shown("sidebar:refs/heads/feature/b"));
    // Seen red: every ref matching.
    assert!(!app.shown("sidebar:refs/heads/feature/a"));
    assert!(!app.shown("sidebar:refs/heads/main"));
    assert!(!app.shown("sidebar:refs/tags/v1.0"));

    app.cx.simulate_keystrokes("backspace");
    app.draw();
    assert!(app.shown("sidebar:refs/heads/main"));
    assert!(app.shown("sidebar:refs/heads/feature/"));
    // Seen red: clearing the collapse state while a filter is set.
    assert!(
        !app.shown("sidebar:refs/heads/feature/b"),
        "the folder was opened by the filter and left open"
    );
}

#[gpui::test]
fn the_filter_ignores_case(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open_sidebar(cx, &f.repo);

    filter(&mut app, "FEATURE/A");
    // Seen red: a filter that keeps its case.
    assert!(app.shown("sidebar:refs/heads/feature/a"));
    assert!(!app.shown("sidebar:refs/heads/feature/b"));
}

#[gpui::test]
fn the_current_branch_is_marked(cx: &mut TestAppContext) {
    let f = fixture();
    git(&f.repo, &["switch", "--quiet", "feature/a"]);
    let mut app = open(cx, Some(&f.repo));
    // Seen red: no row marked current times out here.
    app.until("the current branch is marked", |app| {
        app.draw();
        app.shown("sidebar:current:refs/heads/feature/a")
    });

    assert!(!app.shown("sidebar:current:refs/heads/main"));
    assert!(!app.shown("sidebar:detached"), "a branch shows as detached");
}

#[gpui::test]
fn a_detached_head_shows_its_short_oid_above_the_sections(cx: &mut TestAppContext) {
    let f = fixture();
    git(&f.repo, &["switch", "--quiet", "--detach", "v2.0"]);
    let oid = git(&f.repo, &["rev-parse", "--short=10", "HEAD"]);
    let mut app = open_sidebar(cx, &f.repo);

    // Seen red: a 7-character oid isn't drawn under this selector.
    let detached = format!("sidebar:detached:{}", oid.trim());
    assert!(top(&mut app, &detached) < top(&mut app, "sidebar:section:Local"));
    assert!(!app.shown("sidebar:current:refs/heads/main"));
}

#[gpui::test]
fn a_local_branch_shows_its_counts_and_a_gone_upstream_says_so(cx: &mut TestAppContext) {
    hermetic();
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_with_commit(dir.path(), "copy");
    let bare = dir.path().join("origin.git");
    let bare = bare.to_str().unwrap();
    git(dir.path(), &["init", "--quiet", "--bare", bare]);
    git(&repo, &["remote", "add", "origin", bare]);
    git(&repo, &["push", "--quiet", "-u", "origin", "main"]);
    let base = git(&repo, &["rev-parse", "main"]).trim().to_owned();
    for message in ["local 1", "local 2"] {
        git(
            &repo,
            &["commit", "--quiet", "--allow-empty", "-m", message],
        );
    }
    let mut upstream = base.clone();
    for message in ["upstream 1", "upstream 2", "upstream 3"] {
        let tree = format!("{base}^{{tree}}");
        upstream = git(
            &repo,
            &["commit-tree", &tree, "-p", &upstream, "-m", message],
        )
        .trim()
        .to_owned();
    }
    git(
        &repo,
        &["update-ref", "refs/remotes/origin/main", &upstream],
    );
    git(
        &repo,
        &["branch", "--quiet", "--track", "level", "origin/main"],
    );
    git(&repo, &["branch", "--quiet", "--no-track", "orphan"]);
    git(&repo, &["config", "branch.orphan.remote", "origin"]);
    git(&repo, &["config", "branch.orphan.merge", "refs/heads/gone"]);

    let mut app = open(cx, Some(&repo));
    // Seen red: no `report_visible` call in the list's processor, so nothing is counted.
    app.until("main shows its counts", |app| {
        app.draw();
        app.shown("sidebar:counts:refs/heads/main:↑2 ↓3")
    });
    // Seen red: `divergence` returning nothing for a gone upstream.
    assert!(app.shown("sidebar:gone:refs/heads/orphan"));
    // The marker that `level` was counted: its (0, 0) count has reached the sidebar.
    app.until("level is counted", |app| {
        let workspace = app.workspace.clone();
        app.cx.read(|cx| {
            workspace
                .read(cx)
                .refs
                .read(cx)
                .counts()
                .is_some_and(|counts| {
                    counts.result.as_ref().is_ok_and(|c| {
                        c.branches
                            .get(&cairn_repo::RefName::new("refs/heads/level"))
                            .is_some_and(|c| (c.ahead, c.behind) == (0, 0))
                    })
                })
        })
    });
    app.draw();
    // Seen red: `divergence` keeping zero counts.
    assert!(!app.shown("sidebar:counts:refs/heads/level:↑0 ↓0"));
    assert!(!app.shown("sidebar:gone:refs/heads/level"));
}

#[gpui::test]
fn frames_drawn_while_a_branch_on_screen_is_counted_never_start_its_count_over(
    cx: &mut TestAppContext,
) {
    hermetic();
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_with_commit(dir.path(), "copy");
    let bare = dir.path().join("origin.git");
    let bare = bare.to_str().unwrap();
    git(dir.path(), &["init", "--quiet", "--bare", bare]);
    git(&repo, &["remote", "add", "origin", bare]);
    git(&repo, &["push", "--quiet", "origin", "main"]);
    // `main`'s upstream is a tag of a commit only the remote has, in a partial clone, so the
    // count's `git rev-list` fetches it, and waits in the remote's wrapper until released.
    // The tag sits outside the refs a Refs read lists, so that read never peels it.
    let base = git(&repo, &["rev-parse", "main"]).trim().to_owned();
    let remote_only = git(
        Path::new(bare),
        &[
            "commit-tree",
            &format!("{base}^{{tree}}"),
            "-p",
            &base,
            "-m",
            "on the remote",
        ],
    )
    .trim()
    .to_owned();
    git(
        Path::new(bare),
        &["update-ref", "refs/heads/remote-only", &remote_only],
    );
    git(
        Path::new(bare),
        &["config", "uploadpack.allowAnySHA1InWant", "true"],
    );
    let tag = dir.path().join("tag-object");
    std::fs::write(
        &tag,
        format!(
            "object {remote_only}\ntype commit\ntag held\ntagger Cairn Test <test@cairn.invalid> 1700000000 +0000\n\nneeds the remote\n"
        ),
    )
    .unwrap();
    let tag = git(
        &repo,
        &["hash-object", "-t", "tag", "-w", tag.to_str().unwrap()],
    );
    git(&repo, &["update-ref", "refs/held/up", tag.trim()]);
    git(&repo, &["config", "core.repositoryFormatVersion", "1"]);
    git(&repo, &["config", "extensions.partialClone", "origin"]);
    git(&repo, &["config", "remote.origin.promisor", "true"]);
    git(&repo, &["config", "branch.main.remote", "."]);
    git(&repo, &["config", "branch.main.merge", "refs/held/up"]);
    let log = dir.path().join("remote.log");
    let release = Release(dir.path().join("release"));
    slow_remote(
        &repo,
        dir.path(),
        &format!(
            "echo run >> '{}'\n{}",
            slash(&log),
            block_until(&release.0, dir.path())
        ),
    );
    let runs = || std::fs::read_to_string(&log).map_or(0, |log| log.lines().count());

    let mut app = open(cx, Some(&repo));
    // Seen red: `report_visible` reporting each range the list asks for as it asks. The row
    // it measures alone is no local branch, so every frame Superseded the count, and its git
    // was stopped before it reached the remote.
    app.until("main's count waits in the remote", |app| {
        app.draw();
        runs() >= 1
    });
    // Frames while it waits. The list asks for its first row alone on each one, to measure
    // it, before the rows it draws.
    for _ in 0..20 {
        app.draw();
        app.cx
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    release.release();
    app.until("main shows its count", |app| {
        app.draw();
        app.shown("sidebar:counts:refs/heads/main:↓1")
    });
    // A count started over would reach the remote again.
    assert_eq!(runs(), 1, "the count of main started over");
}

/// A repository whose config includes `extra.config`, a file outside it that the watcher
/// doesn't watch, so breaking or fixing it reports no External change. Refs read through git,
/// as `core.safecrlf` asks, so that a broken config fails them.
fn with_unwatched_config() -> (PathBuf, PathBuf, tempfile::TempDir) {
    hermetic();
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_with_commit(dir.path(), "copy");
    let extra = dir.path().join("extra.config");
    std::fs::write(&extra, "").unwrap();
    git(&repo, &["config", "core.safecrlf", "true"]);
    git(&repo, &["config", "include.path", &slash(&extra)]);
    (repo, extra, dir)
}

#[gpui::test]
fn coming_back_to_the_window_reads_again_refs_that_failed_and_says_couldnt_read(
    cx: &mut TestAppContext,
) {
    let (repo, extra, _dir) = with_unwatched_config();
    let mut app = open(cx, Some(&repo));
    app.until("the sidebar shows main", |app| {
        app.draw();
        app.shown("sidebar:refs/heads/main")
    });
    app.cx.update(|window, _| window.activate_window());

    std::fs::write(&extra, "[broken\n").unwrap();
    // HEAD written again as it was: an External change that reads Refs.
    let head = std::fs::read(repo.join(".git/HEAD")).unwrap();
    std::fs::write(repo.join(".git/HEAD"), &head).unwrap();
    // Seen red: with `read` never set, a failed refresh says "Couldn't open".
    app.until("the sidebar shows git's error", |app| {
        app.draw();
        app.shown("sidebar:error:Couldn't read refs")
    });
    assert!(!app.shown("sidebar:error:Couldn't open"));
    // One write can report several events, and a read a late one starts after the fix would
    // clear the error without a Recheck. Nothing marks the last one, so this waits; the
    // control is this test going red without the Recheck on focus.
    std::thread::sleep(std::time::Duration::from_secs(1));
    app.until("late events are read", |app| {
        app.draw();
        app.shown("sidebar:error:Couldn't read refs")
    });

    std::fs::write(&extra, "").unwrap();
    app.cx.deactivate_window();
    app.cx.update(|window, _| window.activate_window());
    // Seen red: without the Recheck on window activation, the error stays.
    app.until("the sidebar shows main again", |app| {
        app.draw();
        app.shown("sidebar:refs/heads/main")
    });
}

#[gpui::test]
fn a_repository_that_never_opened_says_couldnt_open(cx: &mut TestAppContext) {
    hermetic();
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(cx, Some(dir.path()));
    app.until("the sidebar shows git's error", |app| {
        app.draw();
        app.shown("sidebar:error:Couldn't open")
    });
}
