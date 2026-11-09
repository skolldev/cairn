//! Open Intent to status Snapshots, checked against `git status --porcelain=v2 -z`.

mod common;

use std::path::Path;
use std::sync::Arc;

use cairn_repo::{ChangeKind, ChangesSnapshot, FileChange, Intent, RepoPath, Snapshot};
use common::{Harness, git, git_read, small_fixture_clone, touch};

/// The three status lists, rendered as comparable lines.
#[derive(Debug, PartialEq, Eq)]
struct Status {
    staged: Vec<String>,
    unstaged: Vec<String>,
    untracked: Vec<String>,
}

fn path(p: &RepoPath) -> String {
    String::from_utf8_lossy(p.as_bytes()).into_owned()
}

/// `<letter>[score] <path>[ <- <from>]`, the shape both sides render to.
fn render(change: &FileChange) -> String {
    let (letter, from) = match &change.kind {
        ChangeKind::Added => ("A".to_owned(), None),
        ChangeKind::Deleted => ("D".to_owned(), None),
        ChangeKind::Modified => ("M".to_owned(), None),
        ChangeKind::TypeChanged => ("T".to_owned(), None),
        ChangeKind::Unmerged => ("U".to_owned(), None),
        ChangeKind::Renamed { from, score } => (format!("R{score}"), Some(from)),
    };
    match from {
        Some(from) => format!("{letter} {} <- {}", path(&change.path), path(from)),
        None => format!("{letter} {}", path(&change.path)),
    }
}

/// What git itself reports, split into the three lists.
fn git_status(repo: &Path) -> Status {
    let out = git_read(repo, &["status", "--porcelain=v2", "-z"]);
    let mut records = out
        .strip_suffix(b"\0")
        .unwrap_or(&out)
        .split(|&b| b == 0)
        .filter(|r| !r.is_empty())
        .map(|r| String::from_utf8_lossy(r).into_owned());
    let mut status = Status {
        staged: Vec::new(),
        unstaged: Vec::new(),
        untracked: Vec::new(),
    };
    while let Some(record) = records.next() {
        let fields: Vec<&str> = record.splitn(2, ' ').collect();
        match fields[0] {
            "?" => status.untracked.push(fields[1].to_owned()),
            "1" => {
                // 1 XY sub mH mI mW hH hI path
                let f: Vec<&str> = record.splitn(9, ' ').collect();
                let (x, y) = (&f[1][..1], &f[1][1..]);
                if x != "." {
                    status.staged.push(format!("{x} {}", f[8]));
                }
                if y != "." {
                    status.unstaged.push(format!("{y} {}", f[8]));
                }
            }
            "2" => {
                // 2 XY sub mH mI mW hH hI Xscore path, then origPath as its own record
                let f: Vec<&str> = record.splitn(10, ' ').collect();
                let from = records.next().expect("a rename names its source");
                let (x, y) = (&f[1][..1], &f[1][1..]);
                if x != "." {
                    status.staged.push(format!("{} {} <- {from}", f[8], f[9]));
                }
                if y != "." {
                    status.unstaged.push(format!("{y} {}", f[9]));
                }
            }
            "u" => {
                let f: Vec<&str> = record.splitn(11, ' ').collect();
                status.staged.push(format!("U {}", f[10]));
                status.unstaged.push(format!("U {}", f[10]));
            }
            other => panic!("unexpected porcelain record kind {other:?} in {record:?}"),
        }
    }
    status
}

fn changes(snapshot: &ChangesSnapshot) -> Vec<String> {
    let list = snapshot.result.as_ref().expect("the list reads");
    list.iter().map(render).collect()
}

/// Opens `repo` and waits until every status Slot has answered it.
fn snapshot_status(repo: &Path) -> Status {
    let harness = Harness::start();
    harness.service.send(Intent::Open(repo.to_path_buf()));
    let mut staged: Option<Arc<ChangesSnapshot>> = None;
    let mut unstaged: Option<Arc<ChangesSnapshot>> = None;
    let mut untracked: Option<Vec<String>> = None;
    harness.drain_until(|s| {
        match s {
            Snapshot::Staged(s) => staged = Some(s.clone()),
            Snapshot::Unstaged(s) => unstaged = Some(s.clone()),
            Snapshot::Untracked(s) => {
                let list = s.result.as_ref().expect("untracked files read");
                untracked = Some(list.iter().map(path).collect());
            }
            Snapshot::Refs(_) | Snapshot::Job(_) | Snapshot::Quit => {}
        }
        staged.is_some() && unstaged.is_some() && untracked.is_some()
    });
    Status {
        staged: changes(&staged.unwrap()),
        unstaged: changes(&unstaged.unwrap()),
        untracked: untracked.unwrap(),
    }
}

#[test]
fn cli_fallback_configurations_match_git_porcelain_status() {
    for (key, value) in [
        ("index.sparse", "true"),
        ("remote.origin.promisor", "true"),
        ("core.safecrlf", "true"),
    ] {
        let (_dir, repo) = small_fixture_clone();
        if key == "index.sparse" {
            git(&repo, &["sparse-checkout", "init", "--cone"]);
            git(&repo, &["sparse-checkout", "set", "--sparse-index", "src"]);
            // Seen red: omitting --sparse-index leaves no sparse directory entries.
            assert_eq!(
                git(&repo, &["config", "--get", "index.sparse"]).trim(),
                "true"
            );
            assert!(
                git(&repo, &["ls-files", "--sparse", "--stage"])
                    .lines()
                    .any(|line| line.starts_with("040000 ")),
                "the fixture must contain sparse directory entries"
            );
        } else {
            git(&repo, &["config", key, value]);
        }
        std::fs::write(repo.join("new untracked"), b"new\n").unwrap();
        std::fs::write(repo.join("README.md"), b"staged change\n").unwrap();
        git(&repo, &["add", "README.md"]);
        std::fs::write(repo.join("FAQ.md"), b"unstaged change\n").unwrap();
        std::fs::create_dir(repo.join("untracked dir")).unwrap();
        std::fs::write(repo.join("untracked dir/file"), b"new\n").unwrap();
        std::fs::write(repo.join(".git/info/exclude"), b"ignored-file\n").unwrap();
        std::fs::write(repo.join("ignored-file"), b"ignored\n").unwrap();
        let expected = git_status(&repo);
        assert_eq!(snapshot_status(&repo), expected, "{key}");

        git(&repo, &["config", "status.showUntrackedFiles", "all"]);
        let expected = git_status(&repo);
        assert_eq!(
            snapshot_status(&repo),
            expected,
            "{key} with untracked files expanded"
        );

        git(&repo, &["config", "status.showUntrackedFiles", "no"]);
        assert_eq!(
            snapshot_status(&repo),
            git_status(&repo),
            "{key} with untracked files hidden"
        );
    }
}

#[test]
fn partial_clone_status_matches_git_porcelain() {
    let (_source_dir, source) = small_fixture_clone();
    git(&source, &["config", "uploadpack.allowFilter", "true"]);
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("partial");
    let url = format!("file://{}", source.display());
    git(
        dir.path(),
        &[
            "clone",
            "--quiet",
            "--filter=blob:none",
            &url,
            repo.to_str().unwrap(),
        ],
    );
    assert_eq!(
        git(&repo, &["config", "--get", "remote.origin.promisor"]).trim(),
        "true"
    );
    // Seen red: a filter that downloads every blob leaves no promised objects.
    assert!(
        git(
            &repo,
            &["rev-list", "--objects", "--all", "--missing=print"]
        )
        .lines()
        .any(|line| line.starts_with('?')),
        "the partial clone must actually have missing promised objects"
    );
    std::fs::write(repo.join("partial-untracked"), b"new\n").unwrap();
    std::fs::write(repo.join("README.md"), b"staged change\n").unwrap();
    git(&repo, &["add", "README.md"]);
    std::fs::write(repo.join("FAQ.md"), b"unstaged change\n").unwrap();
    assert_eq!(snapshot_status(&repo), git_status(&repo));
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn cli_untracked_keeps_non_utf8_path_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let (_dir, repo) = small_fixture_clone();
    git(&repo, &["config", "core.safecrlf", "true"]);
    let name = std::ffi::OsStr::from_bytes(b"nonutf8-\xff");
    std::fs::write(repo.join(name), b"new\n").unwrap();
    let harness = Harness::start();
    harness.service.send(Intent::Open(repo));
    let seen = harness.drain_until(|s| match s {
        Snapshot::Untracked(s) => s
            .result
            .as_ref()
            .is_ok_and(|paths| paths.iter().any(|p| p.as_bytes() == b"nonutf8-\xff")),
        _ => false,
    });
    let paths = common::untracked(&seen)[0].result.as_ref().unwrap();
    assert!(paths.iter().any(|p| p.as_bytes() == b"nonutf8-\xff"));
}

#[test]
fn staged_changes_match_git_status() {
    let (_dir, repo) = small_fixture_clone();
    std::fs::write(repo.join("new-file.txt"), "new\n").unwrap();
    std::fs::write(repo.join("README.md"), "rewritten\n").unwrap();
    git(&repo, &["rm", "--quiet", "--", "COPYING"]);
    git(&repo, &["mv", "LICENSE-MIT", "LICENSE-MIT.moved"]);
    git(&repo, &["add", "--", "new-file.txt", "README.md"]);

    let status = snapshot_status(&repo);
    let expected = git_status(&repo);
    assert_eq!(
        expected.staged.len(),
        4,
        "the setup stages four changes: {expected:?}"
    );
    assert_eq!(status.staged, expected.staged);
}

/// The paths `git diff-files` lists: stat-dirty entries included, because it never refreshes.
fn diff_files_paths(repo: &Path) -> Vec<String> {
    let out = git_read(repo, &["diff-files", "--name-only", "-z"]);
    out.split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect()
}

/// Asserts the status Snapshots match `git status`, first with stat-dirty entries in the
/// index and then after `git update-index --refresh` has cleared them.
fn assert_matches_git_before_and_after_refresh(repo: &Path, stat_dirty: &[&str]) {
    // Control: these entries really are stat-dirty, so `diff-files` alone lists them.
    let listed = diff_files_paths(repo);
    for path in stat_dirty {
        assert!(
            listed.iter().any(|p| p == path),
            "{path} is not stat-dirty: {listed:?}"
        );
    }
    let before = snapshot_status(repo);
    assert_eq!(before, git_status(repo), "before an index refresh");
    for path in stat_dirty {
        assert!(
            !before
                .unstaged
                .iter()
                .any(|line| line.ends_with(&format!(" {path}"))),
            "{path} only changed its stat info: {before:?}"
        );
    }

    git(repo, &["update-index", "-q", "--refresh"]);
    let after = snapshot_status(repo);
    assert_eq!(after, git_status(repo), "after an index refresh");
    assert_eq!(before, after, "a refresh changes no content");
}

#[test]
fn status_of_the_small_fixture_matches_git_status_before_and_after_a_refresh() {
    let (_dir, repo) = small_fixture_clone();
    // Staged.
    std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
    git(&repo, &["add", "--", "staged.txt"]);
    git(&repo, &["mv", "LICENSE-MIT", "LICENSE-MIT.moved"]);
    // Staged and then changed again, so it is on both lists.
    std::fs::write(repo.join("README.md"), "staged version\n").unwrap();
    git(&repo, &["add", "--", "README.md"]);
    std::fs::write(repo.join("README.md"), "worktree version\n").unwrap();
    // Unstaged.
    std::fs::write(repo.join("FAQ.md"), "changed\n").unwrap();
    std::fs::remove_file(repo.join("COPYING")).unwrap();
    // Untracked: a file, an all-untracked directory, and a file in a tracked directory.
    std::fs::write(repo.join("untracked.txt"), "u\n").unwrap();
    std::fs::create_dir_all(repo.join("new-dir/nested")).unwrap();
    std::fs::write(repo.join("new-dir/nested/file.txt"), "u\n").unwrap();
    std::fs::write(repo.join("crates/untracked.rs"), "u\n").unwrap();
    // Ignored, so not listed: ripgrep ignores `target`.
    std::fs::create_dir_all(repo.join("target")).unwrap();
    std::fs::write(repo.join("target/ignored.txt"), "i\n").unwrap();
    // Stat-dirty only.
    touch(&repo.join("GUIDE.md"));
    touch(&repo.join("crates/core/main.rs"));

    let expected = git_status(&repo);
    assert!(!expected.staged.is_empty() && !expected.unstaged.is_empty());
    assert_eq!(expected.untracked.len(), 3, "{expected:?}");
    assert_matches_git_before_and_after_refresh(&repo, &["GUIDE.md", "crates/core/main.rs"]);
}

#[test]
fn a_clean_clone_has_empty_status_lists() {
    let (_dir, repo) = small_fixture_clone();
    let status = snapshot_status(&repo);
    assert_eq!(
        status,
        Status {
            staged: vec![],
            unstaged: vec![],
            untracked: vec![]
        }
    );
}

#[test]
fn status_of_the_autocrlf_and_filters_fixture_matches_git_status_before_and_after_a_refresh() {
    let (_dir, repo) = common::filters_fixture_copy();
    let converted = ["crlf.txt", "forced.crlf", "shout.up", "docs/notes.txt"];
    // Control: each of these hashes to its blob only through conversion or the filter.
    for path in converted {
        let raw = git_read(&repo, &["hash-object", "--no-filters", "--", path]);
        let blob = git_read(&repo, &["rev-parse", &format!(":{path}")]);
        assert_ne!(
            raw, blob,
            "{path} needs no conversion, so it proves nothing"
        );
    }
    // Real changes alongside the converted files.
    std::fs::write(repo.join("lf.txt"), "one\r\nchanged\r\n").unwrap();
    std::fs::write(repo.join("data.bin"), "raw\r\nchanged\r\n").unwrap();
    std::fs::write(repo.join("forced.crlf"), "staged\r\n").unwrap();
    git(&repo, &["add", "--", "forced.crlf"]);
    std::fs::write(repo.join("docs/untracked.txt"), "u\r\n").unwrap();

    let expected = git_status(&repo);
    assert_eq!(
        expected.unstaged,
        ["M data.bin", "M lf.txt"],
        "{expected:?}"
    );
    let stat_dirty = ["crlf.txt", "shout.up", "docs/notes.txt"];
    assert_matches_git_before_and_after_refresh(&repo, &stat_dirty);
}

#[test]
fn on_an_unborn_branch_everything_staged_is_added() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "--quiet"]);
    std::fs::write(repo.join("first.txt"), "1\n").unwrap();
    std::fs::write(repo.join("later.txt"), "2\n").unwrap();
    git(repo, &["add", "--", "first.txt"]);

    let status = snapshot_status(repo);
    assert_eq!(status.staged, ["A first.txt"]);
    assert_eq!(status, git_status(repo));
}

#[test]
fn show_untracked_files_all_lists_each_file() {
    let (_dir, repo) = small_fixture_clone();
    std::fs::create_dir_all(repo.join("new-dir/nested")).unwrap();
    std::fs::write(repo.join("new-dir/a.txt"), "u\n").unwrap();
    std::fs::write(repo.join("new-dir/nested/b.txt"), "u\n").unwrap();

    // Control: by default the directory collapses.
    assert_eq!(snapshot_status(&repo).untracked, ["new-dir/"]);
    git(&repo, &["config", "status.showUntrackedFiles", "all"]);
    let status = snapshot_status(&repo);
    assert_eq!(status.untracked, ["new-dir/a.txt", "new-dir/nested/b.txt"]);
    assert_eq!(status, git_status(&repo));
}

/// Names `hash-object --stdin-paths` would misread unquoted: a leading quote, a backslash,
/// a newline, and non-ASCII. Unix only, because Windows forbids most of them.
#[cfg(unix)]
#[test]
fn stat_dirty_files_with_awkward_names_are_still_recognised_as_unchanged() {
    let (_dir, repo) = small_fixture_clone();
    let names = [
        "\"quoted\".txt",
        "back\\slash.txt",
        "new\nline.txt",
        "grüße 文字.txt",
    ];
    for name in names {
        std::fs::write(repo.join(name), "same\n").unwrap();
    }
    git(&repo, &["add", "--all"]);
    git(&repo, &["commit", "--quiet", "-m", "awkward names"]);
    for name in names {
        touch(&repo.join(name));
    }
    std::fs::write(repo.join("new\nline.txt"), "different\n").unwrap();

    let stat_dirty: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| *n != "new\nline.txt")
        .collect();
    assert_matches_git_before_and_after_refresh(&repo, &stat_dirty);
    assert_eq!(snapshot_status(&repo).unstaged, ["M new\nline.txt"]);
}

#[test]
fn opening_a_path_that_is_not_a_repo_publishes_each_error() {
    let dir = tempfile::tempdir().unwrap();
    let harness = Harness::start();
    harness.service.send(Intent::Open(dir.path().to_path_buf()));
    let (mut staged, mut unstaged, mut untracked) = (None, None, None);
    harness.drain_until(|s| {
        match s {
            Snapshot::Staged(s) => staged = Some(s.result.clone()),
            Snapshot::Unstaged(s) => unstaged = Some(s.result.clone()),
            Snapshot::Untracked(s) => untracked = Some(s.result.as_ref().err().cloned()),
            Snapshot::Refs(_) | Snapshot::Job(_) | Snapshot::Quit => {}
        }
        staged.is_some() && unstaged.is_some() && untracked.is_some()
    });
    for (list, error) in [
        ("staged", staged.unwrap().err()),
        ("unstaged", unstaged.unwrap().err()),
    ] {
        let error = error.unwrap_or_else(|| panic!("{list} read a non-repo"));
        assert!(
            error.contains("not a git repository"),
            "{list}: git's own error, got {error:?}"
        );
    }
    assert!(
        untracked.unwrap().is_some_and(|e| !e.is_empty()),
        "untracked read a non-repo"
    );
}
