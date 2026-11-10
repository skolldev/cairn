//! The sidebar in the real window, with the real Repo service and git behind it: its
//! sections, folders and remote groups, the filter box, and how HEAD shows.

use std::path::{Path, PathBuf};

use gpui::TestAppContext;

use crate::testing::{App, git, hermetic, open, repo_with_commit};

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
