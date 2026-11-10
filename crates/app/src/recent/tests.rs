//! The recent repositories in the real window, with the real Repo service and git behind it:
//! what joins the list, how it survives a restart, and choosing and removing an entry.

use std::path::{Path, PathBuf};

use cairn_repo::Intent;
use gpui::TestAppContext;

use super::KEPT;
use crate::testing::{App, git, hermetic, open_with_recent};

/// Repositories `a` and `b`, each with one commit, and a store file in its own folder that
/// doesn't exist yet. Paths are canonical, as the list keeps them.
struct Fixture {
    a: PathBuf,
    b: PathBuf,
    store: PathBuf,
    dir: tempfile::TempDir,
}

fn fixture() -> Fixture {
    hermetic();
    let dir = tempfile::tempdir().unwrap();
    let root = canonical(dir.path());
    let repo = |name: &str| {
        let repo = root.join(name);
        git(
            &root,
            &[
                "init",
                "--quiet",
                "--initial-branch=main",
                repo.to_str().unwrap(),
            ],
        );
        std::fs::write(repo.join("file.txt"), "one\n").unwrap();
        git(&repo, &["add", "file.txt"]);
        git(&repo, &["commit", "--quiet", "-m", "base"]);
        repo
    };
    Fixture {
        a: repo("a"),
        b: repo("b"),
        store: root.join("app-data").join("Cairn").join("recent-repos.txt"),
        dir,
    }
}

/// `dir` as the list keeps it: canonical, and on Windows without the `\\?\` that git can't
/// use.
fn canonical(dir: &Path) -> PathBuf {
    let path = dir.canonicalize().unwrap();
    match path.to_str().and_then(|p| p.strip_prefix(r"\\?\")) {
        Some(plain) if cfg!(windows) => PathBuf::from(plain),
        _ => path,
    }
}

/// Opens `repo` and waits until the list shows it at the top.
fn open_and_wait(app: &mut App, repo: &Path) {
    app.send(Intent::Open(repo.to_path_buf()));
    app.until("the opened repository tops the list", |app| {
        app.recent().first().map(PathBuf::as_path) == Some(repo)
    });
}

/// Waits until the sidebar shows the Refs Snapshot for `workdir`, and returns whether it is Ok.
fn refs_shown(app: &mut App, workdir: &Path) -> bool {
    let mut ok = false;
    app.until("the Refs Snapshot for the Open is shown", |app| {
        app.workspace.read_with(app.cx, |workspace, cx| {
            match workspace.refs.read(cx).snapshot() {
                Some(snapshot) if snapshot.workdir == workdir => {
                    ok = snapshot.result.is_ok();
                    true
                }
                _ => false,
            }
        })
    });
    ok
}

/// Writes `entries` to `store` by recording them, oldest first, as earlier launches would.
fn earlier_launches(store: &Path, entries: &[PathBuf]) {
    let mut repos = super::RecentRepos::load(Some(store.to_path_buf()));
    for entry in entries.iter().rev() {
        repos.record(entry);
    }
}

#[gpui::test]
fn opening_a_then_b_then_a_lists_a_then_b_and_survives_a_restart(cx: &mut TestAppContext) {
    let f = fixture();
    {
        let mut app = open_with_recent(cx, None, Some(f.store.clone()));
        open_and_wait(&mut app, &f.a);
        open_and_wait(&mut app, &f.b);
        open_and_wait(&mut app, &f.a);
        // Seen red: `record` keeping the old entry instead of moving it lists A, B, A.
        assert_eq!(app.recent(), [f.a.clone(), f.b.clone()]);
    }

    let mut restarted = open_with_recent(cx, None, Some(f.store.clone()));
    // Seen red: `save` never writing the file starts the restarted window empty.
    assert_eq!(restarted.recent(), [f.a.clone(), f.b.clone()]);
}

#[gpui::test]
fn reopening_the_open_repository_after_removing_it_records_it_again(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open_with_recent(cx, None, Some(f.store.clone()));
    open_and_wait(&mut app, &f.a);
    app.draw();
    app.click("open-recent");
    app.click(&format!("recent:menu:remove:{}", f.a.display()));
    assert!(app.recent().is_empty());

    // As the Open button's folder picker does once a folder is chosen.
    app.workspace
        .update(app.cx, |workspace, cx| workspace.open(f.a.clone(), cx));
    // Seen red: `forget_recorded` leaving the open workdir as recorded times out here.
    app.until("the reopened repository is listed again", |app| {
        app.recent() == [f.a.clone()]
    });
}

#[gpui::test]
fn a_failed_open_isnt_added(cx: &mut TestAppContext) {
    let f = fixture();
    let not_a_repo = canonical(f.dir.path()).join("plain");
    std::fs::create_dir(&not_a_repo).unwrap();
    let mut app = open_with_recent(cx, None, Some(f.store.clone()));

    app.send(Intent::Open(not_a_repo.clone()));
    // The marker: the failed Open's own Refs Snapshot is shown, and recording happens as it is.
    assert!(!refs_shown(&mut app, &not_a_repo));
    // Seen red: the Bridge recording an Err Refs Snapshot too lists the folder.
    assert!(app.recent().is_empty());
    assert!(!f.store.exists());

    // Control: a repository that opens is added by the same window.
    open_and_wait(&mut app, &f.a);
}

#[gpui::test]
fn a_corrupt_or_missing_store_file_starts_with_an_empty_list(cx: &mut TestAppContext) {
    let f = fixture();
    let missing = open_with_recent(cx, None, Some(f.store.clone())).recent();
    assert!(missing.is_empty());

    std::fs::create_dir_all(f.store.parent().unwrap()).unwrap();
    for corrupt in [
        &b"\xff\xfe not text"[..],
        format!("{}\n{}\n", f.a.display(), f.b.display()).as_bytes(),
    ] {
        std::fs::write(&f.store, corrupt).unwrap();
        let listed = open_with_recent(cx, None, Some(f.store.clone())).recent();
        // Seen red: `read` taking any first line as the header lists the second path.
        assert!(listed.is_empty(), "{listed:?} listed from {corrupt:?}");
    }

    // A folder where the file should be can't be read either.
    std::fs::remove_file(&f.store).unwrap();
    std::fs::create_dir(&f.store).unwrap();
    let mut app = open_with_recent(cx, None, Some(f.store.clone()));
    assert!(app.recent().is_empty());
    // Control: the same window still opens and records a repository.
    open_and_wait(&mut app, &f.a);
}

#[gpui::test]
fn choosing_an_entry_in_the_empty_state_opens_it(cx: &mut TestAppContext) {
    let f = fixture();
    earlier_launches(&f.store, &[f.a.clone(), f.b.clone()]);
    let mut app = open_with_recent(cx, None, Some(f.store.clone()));
    app.draw();

    app.click(&format!("recent:empty:open:{}", f.b.display()));
    // Seen red: choosing that emits no `OpenRecent` times out here.
    assert!(refs_shown(&mut app, &f.b));
    app.until("the chosen repository tops the list", |app| {
        app.recent() == [f.b.clone(), f.a.clone()]
    });
    // The empty state gives way to the repository.
    assert!(!app.shown(&format!("recent:empty:open:{}", f.a.display())));
}

#[gpui::test]
fn choosing_an_entry_under_open_recent_opens_it(cx: &mut TestAppContext) {
    let f = fixture();
    earlier_launches(&f.store, &[f.a.clone(), f.b.clone()]);
    let mut app = open_with_recent(cx, Some(&f.a), Some(f.store.clone()));
    assert!(refs_shown(&mut app, &f.a));
    app.draw();

    app.click("open-recent");
    app.click(&format!("recent:menu:open:{}", f.b.display()));
    // Seen red: choosing that emits no `OpenRecent` times out here.
    assert!(refs_shown(&mut app, &f.b));
    // Choosing closes the list. Seen red: `choose` leaving `menu_open` set.
    assert!(!app.shown(&format!("recent:menu:open:{}", f.a.display())));
}

#[gpui::test]
fn removing_an_entry_drops_it_from_the_file(cx: &mut TestAppContext) {
    let f = fixture();
    earlier_launches(&f.store, &[f.a.clone(), f.b.clone()]);
    {
        let mut app = open_with_recent(cx, None, Some(f.store.clone()));
        app.draw();
        app.click(&format!("recent:empty:remove:{}", f.a.display()));
        assert_eq!(app.recent(), std::slice::from_ref(&f.b));
    }

    let mut restarted = open_with_recent(cx, None, Some(f.store.clone()));
    // Seen red: `remove` dropping the entry in memory only, without `save`.
    assert_eq!(restarted.recent(), std::slice::from_ref(&f.b));
}

#[gpui::test]
fn a_recent_repository_that_is_gone_shows_gits_error_and_stays_listed(cx: &mut TestAppContext) {
    let f = fixture();
    earlier_launches(&f.store, &[f.b.clone(), f.a.clone()]);
    std::fs::remove_dir_all(&f.a).unwrap();
    let mut app = open_with_recent(cx, None, Some(f.store.clone()));
    app.draw();

    app.click(&format!("recent:empty:open:{}", f.a.display()));
    assert!(!refs_shown(&mut app, &f.a));
    // Seen red: the Bridge recording an Err Refs Snapshot too moves A to the top.
    assert_eq!(app.recent(), [f.b.clone(), f.a.clone()]);
    let mut restarted = open_with_recent(cx, None, Some(f.store.clone()));
    assert_eq!(restarted.recent(), [f.b.clone(), f.a.clone()]);
}

#[gpui::test]
fn a_repository_is_listed_once_by_its_worktree_root_however_it_was_opened(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open_with_recent(cx, None, Some(f.store.clone()));
    app.send(Intent::Open(f.a.join(".git")));
    assert!(refs_shown(&mut app, &f.a.join(".git")));
    // Seen red: `worktree_root` keeping the `.git` folder times out here.
    app.until("the worktree root is listed", |app| {
        app.recent() == [f.a.clone()]
    });

    open_and_wait(&mut app, &f.b);
    #[cfg(unix)]
    {
        let link = canonical(f.dir.path()).join("link-to-a");
        std::os::unix::fs::symlink(&f.a, &link).unwrap();
        app.send(Intent::Open(link));
        // Seen red: `std::path::absolute` in place of `canonicalize` keeps the link.
        app.until("the worktree root moves to the top", |app| {
            app.recent() == [f.a.clone(), f.b.clone()]
        });
    }
}

#[gpui::test]
fn the_list_keeps_the_most_recent_twenty(cx: &mut TestAppContext) {
    let f = fixture();
    let older: Vec<PathBuf> = (0..KEPT)
        .map(|n| canonical(f.dir.path()).join(format!("old-{n}")))
        .collect();
    earlier_launches(&f.store, &older);
    let mut app = open_with_recent(cx, None, Some(f.store.clone()));
    assert_eq!(app.recent(), older);

    open_and_wait(&mut app, &f.a);
    let mut expected = vec![f.a.clone()];
    expected.extend_from_slice(&older[..KEPT - 1]);
    // Seen red: `record` without `truncate` keeps 21.
    assert_eq!(app.recent(), expected);
}
