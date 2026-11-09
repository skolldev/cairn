//! External changes reach the Repo service through the watcher, and the Slot coalesces the
//! refreshes they cause: a newer Read Job Supersedes the pending one and stops the running
//! one. The Supersede tests drive refreshes with Opens, whose generations are known.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cairn_repo::{ChangesSnapshot, Generation, Head, Intent, RepoPath, Snapshot};
use common::{Harness, Seen, git, open, slash, small_fixture_clone, unstaged, untracked};

#[test]
fn a_git_switch_run_outside_the_service_is_followed_by_a_refs_snapshot() {
    let (_dir, repo) = small_fixture_clone();
    common::assert_filesystem_events_available(&repo);
    git(&repo, &["branch", "elsewhere"]);
    let harness = Harness::start();
    let opened = open(&harness, &repo);

    git(&repo, &["switch", "--quiet", "elsewhere"]);

    let mut seen = Seen::default();
    seen.until(&harness, |seen| {
        seen.all.iter().any(|s| match s {
            Snapshot::Refs(r) if r.generation > opened.refs => r.result.as_ref().is_ok_and(|refs| {
                matches!(&refs.head, Head::Branch { name, .. } if name.as_str() == "refs/heads/elsewhere")
            }),
            _ => false,
        })
    });
}

#[test]
fn a_newer_status_read_stops_the_running_one_instead_of_waiting_for_it() {
    let (dir, repo) = small_fixture_clone();
    let started = dir.path().join("started");
    let release = dir.path().join("release");
    // A clean filter that blocks the first time it runs, until `release` exists. The
    // Unstaged read runs it through `hash-object` for a stat-dirty file.
    common::install_slow_clean_filter(
        &repo,
        dir.path(),
        &common::block_first_run(&started, &release),
    );

    let harness = Harness::start();
    let _release = common::Release(&release);
    harness.service.send(Intent::Open(repo.clone()));
    common::wait_for_file(&started);

    // The first read is blocked in its filter. A newer one must not wait for it: without
    // the stop, nothing publishes until `release` exists, and this drain times out.
    harness.service.send(Intent::Open(repo.clone()));
    let mut seen = Seen::default();
    seen.until(&harness, |seen| !unstaged(&seen.all).is_empty());
    let published = unstaged(&seen.all);
    assert_eq!(published.len(), 1, "the stopped read published nothing");
    let unstaged = published[0];
    // 2, or later if a watcher event asked for another read meanwhile.
    assert!(unstaged.generation.get() >= 2, "the newer read answered");
    assert_eq!(
        unstaged.result,
        Ok(Vec::new()),
        "README.md is only stat-dirty"
    );
    assert!(
        !release.exists(),
        "the first read was stopped, not released"
    );
}

#[test]
fn quick_refreshes_of_one_slot_publish_only_newer_generations_and_never_a_stopped_read() {
    let (dir, repo) = small_fixture_clone();
    // Every Unstaged read takes a while (one filter run of 0.3s), so most newer reads
    // arrive while one is running and stop it. With the `is_current` check in
    // `Reads::submit` removed, this goes red at 0.3s but stays green at 0.02s.
    common::install_slow_clean_filter(&repo, dir.path(), "sleep 0.3\n");
    let harness = Harness::start();
    // A fresh service numbers the Unstaged Slot's reads 1, 2, 3, ... one per Open. The
    // watcher may add refreshes of its own, so later generations can be larger.
    const OPENS: u64 = 20;
    for _ in 0..OPENS {
        harness.service.send(Intent::Open(repo.clone()));
    }
    let mut seen = Seen::default();
    seen.until(&harness, |seen| {
        unstaged(&seen.all)
            .iter()
            .any(|u| u.generation.get() >= OPENS)
    });

    // Marker: open another path. Its Snapshot takes a later generation than every read of
    // `repo`, and the watcher no longer refreshes `repo`, so anything older would have
    // arrived by the time it does.
    let elsewhere = tempfile::tempdir().unwrap();
    harness
        .service
        .send(Intent::Open(elsewhere.path().to_path_buf()));
    seen.until(&harness, |seen| {
        unstaged(&seen.all)
            .iter()
            .any(|u| u.workdir == elsewhere.path())
    });

    let mut published = unstaged(&seen.all);
    published.retain(|u| u.workdir == repo);
    let generations: Vec<u64> = published.iter().map(|u| u.generation.get()).collect();
    assert!(
        generations.windows(2).all(|w| w[0] < w[1]),
        "generations must only increase: {generations:?}"
    );
    let newest = *generations.last().unwrap();
    assert!(
        (generations.len() as u64) < newest,
        "superseded reads must not all publish: {generations:?}"
    );
    // Seen red: with the `is_current` check in `Reads::submit` removed, a stopped read
    // publishes its "stopped" error here.
    for u in published {
        assert_eq!(
            u.result,
            Ok(Vec::new()),
            "generation {:?}: a stopped read's error must never be published",
            u.generation
        );
    }
}

/// Stops its writer thread when dropped, so a failing test never leaves it running.
struct Writer {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Appends a line to `path` every 10 ms until dropped, as a build or a log writer does.
fn keep_writing(path: PathBuf) -> Writer {
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let mut bytes = std::fs::read(&path).unwrap();
                bytes.extend_from_slice(b"more\n");
                std::fs::write(&path, bytes).unwrap();
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    Writer {
        stop,
        thread: Some(thread),
    }
}

#[test]
fn status_keeps_publishing_while_external_changes_keep_arriving() {
    let (dir, repo) = small_fixture_clone();
    common::assert_filesystem_events_available(&repo);
    // Each Unstaged read takes far longer than the gap between two writes, so a new event
    // always arrives while a read runs.
    common::install_slow_clean_filter(&repo, dir.path(), "sleep 0.3\n");
    let harness = Harness::start();
    let opened = open(&harness, &repo);

    let _writer = keep_writing(repo.join("FAQ.md"));
    // If every event dropped the running read's result, nothing would publish until the
    // writes stop, and this drain would time out with the writer still running.
    let mut seen = Seen::default();
    seen.until(&harness, |seen| {
        unstaged_after(seen, opened.unstaged).iter().any(|u| {
            u.result
                .as_ref()
                .is_ok_and(|c| c.iter().any(|f| f.path.as_bytes() == b"FAQ.md"))
        })
    });
}

/// Appends to `path` `times` times, 10 ms apart, as a build writing its output does.
fn append_repeatedly(path: &Path, times: usize) {
    for _ in 0..times {
        let mut bytes = std::fs::read(path).unwrap_or_default();
        bytes.extend_from_slice(b"more\n");
        std::fs::write(path, bytes).unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Creates the untracked file `name` and waits until an Untracked Snapshot lists it. Returns
/// that Snapshot's generation. The file is a marker: every event the watcher saw before it
/// has been classified, and any refresh one asked for took an older generation.
fn untracked_marker(seen: &mut Seen, harness: &Harness, repo: &Path, name: &str) -> u64 {
    std::fs::write(repo.join(name), "marker\n").unwrap();
    let listing = |seen: &Seen| {
        untracked(&seen.all)
            .into_iter()
            .find(|u| u.workdir == repo && u.result.as_deref().is_ok_and(|p| has(p, name)))
            .map(|u| u.generation.get())
    };
    seen.until(harness, |seen| listing(seen).is_some());
    listing(seen).unwrap()
}

/// How many status refreshes the watcher asked for while `change` ran: the generation gap
/// between an Untracked marker before it and one after. Late events from a test's setup
/// land before the first marker, and the second marker's own events add a few. A single
/// refresh can show a change for another event's sake, so a test that needs to know the
/// change itself refreshed repeats it and counts.
fn status_refreshes_during(
    seen: &mut Seen,
    harness: &Harness,
    repo: &Path,
    label: &str,
    change: impl FnOnce(),
) -> u64 {
    let before = untracked_marker(seen, harness, repo, &format!("{label}-before.txt"));
    change();
    untracked_marker(seen, harness, repo, &format!("{label}-after.txt")) - before
}

/// A [`status_refreshes_during`] gap this large means the change itself asked for refreshes.
/// Twenty writes measured about 80 generations.
const REFRESHED: u64 = 10;

/// The most a [`status_refreshes_during`] gap reaches from the second marker's own events,
/// when the change itself refreshed nothing.
const MARKER_ONLY: u64 = 3;

#[test]
fn writes_to_an_ignored_untracked_file_refresh_nothing_and_writes_to_an_unignored_one_do() {
    let (_dir, repo) = small_fixture_clone();
    common::assert_filesystem_events_available(&repo);
    std::fs::write(repo.join(".git/info/exclude"), "build.log\n").unwrap();
    std::fs::write(repo.join("build.log"), "").unwrap();
    let harness = Harness::start();
    open(&harness, &repo);
    let mut seen = Seen::default();

    let ignored = status_refreshes_during(&mut seen, &harness, &repo, "ignored", || {
        append_repeatedly(&repo.join("build.log"), 20)
    });
    // Control: the same writes to a file git doesn't ignore each ask for a refresh, and each
    // refresh takes a new generation.
    let control = status_refreshes_during(&mut seen, &harness, &repo, "control", || {
        append_repeatedly(&repo.join("control.txt"), 20)
    });
    assert!(
        control > REFRESHED,
        "unignored writes should each refresh: {control} generations"
    );
    // Only the second marker's own events refresh. Seen red on Linux (`scripts/linux-tests.sh`):
    // with no `Access` check before the rebuild trigger, every read's open rebuilds the matcher.
    assert!(
        ignored <= MARKER_ONLY,
        "ignored writes refreshed status: {ignored} generations (control: {control})"
    );
}

/// Whether a Snapshot of `repo`'s changes lists `path`.
fn lists_change(u: &ChangesSnapshot, repo: &Path, path: &str) -> bool {
    u.workdir == repo
        && u.result
            .as_ref()
            .is_ok_and(|c| c.iter().any(|f| f.path.as_bytes() == path.as_bytes()))
}

/// Whether an Untracked Snapshot of `repo` newer than `after` passes `check`.
fn untracked_after_shows(
    seen: &Seen,
    repo: &Path,
    after: u64,
    check: impl Fn(&[RepoPath]) -> bool,
) -> bool {
    untracked(&seen.all).iter().any(|u| {
        u.workdir == repo && u.generation.get() > after && u.result.as_deref().is_ok_and(&check)
    })
}

fn has(paths: &[RepoPath], path: &str) -> bool {
    paths.iter().any(|p| p.as_bytes() == path.as_bytes())
}

#[test]
fn a_change_to_a_tracked_file_matching_an_ignore_pattern_shows_in_unstaged() {
    let (_dir, repo) = small_fixture_clone();
    common::assert_filesystem_events_available(&repo);
    std::fs::write(repo.join(".git/info/exclude"), "*.log\n").unwrap();
    std::fs::write(repo.join("tracked.log"), "first\n").unwrap();
    git(&repo, &["add", "-f", "tracked.log"]);
    let harness = Harness::start();
    let opened = open(&harness, &repo);

    let mut seen = Seen::default();
    let refreshes = status_refreshes_during(&mut seen, &harness, &repo, "tracked", || {
        append_repeatedly(&repo.join("tracked.log"), 20)
    });
    // Seen red: with `Tracked::covers` not looking up the entry itself, the writes are skipped.
    assert!(
        refreshes > REFRESHED,
        "each write to the tracked file refreshes: {refreshes} generations"
    );
    seen.until(&harness, |seen| {
        unstaged_after(seen, opened.unstaged)
            .iter()
            .any(|u| lists_change(u, &repo, "tracked.log"))
    });
}

#[test]
fn a_pattern_added_to_an_ignored_gitignore_drops_the_files_it_covers_from_untracked() {
    let (_dir, repo) = small_fixture_clone();
    common::assert_filesystem_events_available(&repo);
    // The `.gitignore` is ignored itself, so only its role as an exclude source refreshes.
    std::fs::write(repo.join(".git/info/exclude"), ".gitignore\n").unwrap();
    std::fs::create_dir(repo.join("notes")).unwrap();
    std::fs::write(repo.join("notes/draft.tmp"), "draft\n").unwrap();
    let harness = Harness::start();
    // Untracked lists `notes/` until the pattern arrives: nothing in it is ignored yet.
    let opened = open(&harness, &repo);

    std::fs::write(repo.join("notes/.gitignore"), "*.tmp\n").unwrap();
    let mut seen = Seen::default();
    // Seen red: with `changes_filter` not matching `.gitignore`, the old matcher skips the
    // ignored `.gitignore`'s own event, and Untracked keeps `notes/`.
    seen.until(&harness, |seen| {
        untracked_after_shows(seen, &repo, opened.untracked.get(), |p| !has(p, "notes/"))
    });
}

#[test]
fn a_file_a_removed_pattern_covered_shows_in_untracked_once_written() {
    let (_dir, repo) = small_fixture_clone();
    common::assert_filesystem_events_available(&repo);
    let gitignore = repo.join(".gitignore");
    let original = std::fs::read(&gitignore).unwrap();
    std::fs::write(&gitignore, [original.as_slice(), b"*.tmp\n"].concat()).unwrap();
    std::fs::write(repo.join("old.tmp"), "old\n").unwrap();
    let harness = Harness::start();
    let opened = open(&harness, &repo);
    let mut seen = Seen::default();
    // The matcher reads a `.gitignore` when a lookup first needs it, so this also has it
    // read the pattern before the pattern goes.
    let ignored = status_refreshes_during(&mut seen, &harness, &repo, "old", || {
        append_repeatedly(&repo.join("old.tmp"), 20)
    });
    // Its control is the `new.tmp` writes below, which refresh once the pattern is gone.
    assert!(
        ignored <= MARKER_ONLY,
        "the pattern applies: {ignored} generations"
    );

    std::fs::write(&gitignore, &original).unwrap();
    seen.until(&harness, |seen| {
        untracked_after_shows(seen, &repo, opened.untracked.get(), |p| has(p, "old.tmp"))
    });
    // Written after the pattern's removal was seen. A matcher that still held the pattern
    // would skip each write.
    let refreshes = status_refreshes_during(&mut seen, &harness, &repo, "new", || {
        append_repeatedly(&repo.join("new.tmp"), 20)
    });
    // Seen red: with `Filter::rebuild` returning at once, the old matcher stays.
    assert!(
        refreshes > REFRESHED,
        "each write to the no longer ignored file refreshes: {refreshes} generations"
    );
    seen.until(&harness, |seen| {
        untracked_after_shows(seen, &repo, opened.untracked.get(), |p| has(p, "new.tmp"))
    });
}

#[test]
fn moving_an_ignored_directory_with_a_tracked_file_out_of_the_worktree_shows_the_deletion() {
    let (dir, repo) = small_fixture_clone();
    common::assert_filesystem_events_available(&repo);
    // No trailing slash: the directory itself matches, so its own event is excluded.
    std::fs::write(repo.join(".git/info/exclude"), "build\n").unwrap();
    std::fs::create_dir(repo.join("build")).unwrap();
    std::fs::write(repo.join("build/keep.txt"), "keep\n").unwrap();
    std::fs::write(repo.join("build/output.o"), "output\n").unwrap();
    git(&repo, &["add", "-f", "build/keep.txt"]);
    let harness = Harness::start();
    let opened = open(&harness, &repo);

    // On macOS each move reports one event, for `build`, and none for the files inside.
    let (inside, outside) = (repo.join("build"), dir.path().join("moved"));
    let mut seen = Seen::default();
    let refreshes = status_refreshes_during(&mut seen, &harness, &repo, "moves", || {
        for _ in 0..10 {
            std::fs::rename(&inside, &outside).unwrap();
            std::thread::sleep(Duration::from_millis(10));
            std::fs::rename(&outside, &inside).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::rename(&inside, &outside).unwrap();
    });
    // Seen red: with `Tracked::covers` not asking `path_is_directory`, the moves are skipped.
    assert!(
        refreshes > REFRESHED,
        "each move of the directory refreshes: {refreshes} generations"
    );
    seen.until(&harness, |seen| {
        unstaged_after(seen, opened.unstaged)
            .iter()
            .any(|u| lists_change(u, &repo, "build/keep.txt"))
    });
}

#[test]
fn a_directory_replaced_without_its_gitignore_stops_skipping_what_that_gitignore_covered() {
    let (dir, repo) = small_fixture_clone();
    common::assert_filesystem_events_available(&repo);
    let sub = repo.join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join(".gitignore"), "*.o\n").unwrap();
    std::fs::write(sub.join("keep.txt"), "keep\n").unwrap();
    git(&repo, &["add", "sub/keep.txt"]);
    let harness = Harness::start();
    open(&harness, &repo);
    let mut seen = Seen::default();
    let ignored = status_refreshes_during(&mut seen, &harness, &repo, "before", || {
        append_repeatedly(&sub.join("first.o"), 20)
    });
    assert!(
        ignored <= MARKER_ONLY,
        "`sub/.gitignore` applies: {ignored} generations"
    );

    let refreshes = status_refreshes_during(&mut seen, &harness, &repo, "after", || {
        // The stack keeps what it read for the directories of the last lookup, so the
        // lookup before the move is one inside `sub`, after the marker's at the root.
        append_repeatedly(&sub.join("first.o"), 1);
        // On macOS the move reports `sub` alone, nothing for `sub/.gitignore`.
        std::fs::rename(&sub, dir.path().join("moved")).unwrap();
        std::fs::create_dir(&sub).unwrap();
        append_repeatedly(&sub.join("second.o"), 20);
    });
    // Seen red: with `Excludes::skips` returning early for a tracked path such as `sub`, the
    // stack keeps the old `sub/.gitignore` and skips these writes.
    assert!(
        refreshes > REFRESHED,
        "nothing ignores `sub/*.o` any more: {refreshes} generations"
    );
}

#[test]
fn in_a_linked_worktree_a_newly_tracked_ignored_file_shows_in_unstaged_when_changed() {
    let (_dir, repo, worktree) = linked_worktree();
    common::assert_filesystem_events_available(&worktree);
    // The common dir's excludes apply to every worktree.
    std::fs::write(repo.join(".git/info/exclude"), "*.log\n").unwrap();
    std::fs::write(worktree.join("now-tracked.log"), "first\n").unwrap();
    let harness = Harness::start();
    let opened = open(&harness, &worktree);

    // The linked worktree's index lies in its own git dir, under the common dir's
    // `worktrees/`.
    git(&worktree, &["add", "-f", "now-tracked.log"]);
    let mut seen = Seen::default();
    seen.until(&harness, |seen| {
        seen.all.iter().any(|s| match s {
            Snapshot::Staged(u) => lists_change(u, &worktree, "now-tracked.log"),
            _ => false,
        })
    });
    let refreshes = status_refreshes_during(&mut seen, &harness, &worktree, "tracked", || {
        append_repeatedly(&worktree.join("now-tracked.log"), 20)
    });
    // Seen red: with `changes_filter` not matching the linked worktree's index, the matcher
    // keeps the file untracked and ignored, and skips each write.
    assert!(
        refreshes > REFRESHED,
        "each write to the newly tracked file refreshes: {refreshes} generations"
    );
    seen.until(&harness, |seen| {
        unstaged_after(seen, opened.unstaged)
            .iter()
            .any(|u| lists_change(u, &worktree, "now-tracked.log"))
    });
}

#[test]
fn renaming_a_file_from_an_ignored_path_to_an_unignored_one_refreshes() {
    let (_dir, repo) = small_fixture_clone();
    common::assert_filesystem_events_available(&repo);
    std::fs::write(repo.join(".git/info/exclude"), "*.log\n").unwrap();
    std::fs::write(repo.join("renamed.log"), "renamed\n").unwrap();
    let harness = Harness::start();
    open(&harness, &repo);
    let mut seen = Seen::default();
    // Late events from the setup could refresh after the rename and hide a skipped one.
    let settled = untracked_marker(&mut seen, &harness, &repo, "settled.txt");

    std::fs::rename(repo.join("renamed.log"), repo.join("renamed.txt")).unwrap();
    // Seen red: with `Excludes::skips` skipping every untracked path, excluded or not.
    seen.until(&harness, |seen| {
        untracked_after_shows(seen, &repo, settled, |p| has(p, "renamed.txt"))
    });
}

/// The Unstaged Snapshots in `seen` newer than `after`, oldest first.
fn unstaged_after(seen: &Seen, after: Generation) -> Vec<&Arc<ChangesSnapshot>> {
    let mut all = unstaged(&seen.all);
    all.retain(|u| u.generation > after);
    all
}

#[test]
fn a_burst_shorter_than_a_read_costs_at_most_two_status_refreshes() {
    let (dir, repo) = small_fixture_clone();
    common::assert_filesystem_events_available(&repo);
    // ADR 0004's bound holds for a burst that ends while the first read runs. A longer one
    // is a stream, which publishes once per read.
    // Regular files only: a symlink or submodule entry can't be appended to.
    let files: Vec<String> = git(&repo, &["ls-files", "-s"])
        .lines()
        .filter(|l| l.starts_with("100644 "))
        .filter_map(|l| l.split_once('\t').map(|(_, path)| path.to_owned()))
        .take(50)
        .collect();
    assert_eq!(files.len(), 50, "the fixture has enough files for a burst");
    let harness = Harness::start();
    let opened = open(&harness, &repo);
    let started = dir.path().join("burst-read-started");
    let release = dir.path().join("burst-read-release");
    let _release = common::Release(&release);
    common::install_slow_clean_filter(
        &repo,
        dir.path(),
        &format!(
            "touch '{started}'\n{wait}",
            started = slash(&started),
            wait = common::wait_for_release(&release),
        ),
    );
    common::wait_for_file(&started);
    // The filter setup can publish a Snapshot before the blocked read starts. The burst
    // begins here, with that read held until its last filesystem event has been seen.
    harness.service.drain();

    // A short gap between writes, so the OS reports each one: Windows collapses a burst
    // that overflows its change buffer into one rescan, and the control below then fails.
    for file in &files {
        let path = repo.join(file);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(b"\nchanged outside Cairn\n");
        std::fs::write(&path, bytes).unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }

    // A ref written after all the files is the marker that the watcher has processed the
    // burst. Refs can publish while the Unstaged read is held in the clean filter.
    git(&repo, &["branch", "burst-finished"]);
    let mut seen = Seen::default();
    seen.until(&harness, |seen| {
        seen.all.iter().any(|s| match s {
            Snapshot::Refs(r) => r.result.as_ref().is_ok_and(|refs| {
                refs.local
                    .iter()
                    .any(|entry| entry.name.as_str() == "refs/heads/burst-finished")
            }),
            _ => false,
        })
    });
    std::fs::write(&release, "").unwrap();

    let complete = |u: &ChangesSnapshot| u.result.as_ref().is_ok_and(|c| c.len() == files.len());
    seen.until(&harness, |seen| {
        unstaged_after(seen, opened.unstaged)
            .iter()
            .any(|u| complete(u))
    });
    let last = unstaged_after(&seen, opened.unstaged)
        .last()
        .unwrap()
        .generation;

    // Marker: open another path. Its Snapshot takes a later generation than every refresh
    // the burst asked for, and the watcher's late events no longer refresh `repo`, so
    // every burst refresh that will ever publish has done so when the marker arrives.
    let elsewhere = tempfile::tempdir().unwrap();
    harness
        .service
        .send(Intent::Open(elsewhere.path().to_path_buf()));
    seen.until(&harness, |seen| {
        unstaged(&seen.all)
            .iter()
            .any(|u| u.workdir == elsewhere.path())
    });
    let mut published = unstaged_after(&seen, opened.unstaged);
    published.retain(|u| u.workdir == repo);
    let last = published
        .iter()
        .find(|u| u.generation == last)
        .expect("the complete Snapshot");
    assert!(complete(last), "the newest Snapshot shows the whole burst");
    // Control: the burst asked for many refreshes. Each one takes a new generation, so
    // without the Slot coalescing them, each would have published.
    assert!(
        last.generation.get() - opened.unstaged.get() > 2,
        "the burst should have asked for more than two refreshes: {:?}",
        last.generation
    );
    assert!(
        published.len() <= 2,
        "at most two status refreshes back to back, saw {}: {:?}",
        published.len(),
        published.iter().map(|u| u.generation).collect::<Vec<_>>()
    );
}

/// A linked worktree of a fresh small fixture clone, detached at its HEAD.
fn linked_worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (dir, repo) = small_fixture_clone();
    let worktree = dir.path().join("linked");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            worktree.to_str().unwrap(),
        ],
    );
    (dir, repo, worktree)
}

/// Opens `workdir`, runs `git switch` there to a new branch at the same commit, and waits
/// for a Refs Snapshot on that branch and a newer Snapshot from each status Slot. The
/// switch changes no worktree file, so only git's own files can ask for those refreshes.
fn assert_an_external_switch_refreshes_refs_and_status(workdir: &Path) {
    common::assert_filesystem_events_available(workdir);
    git(workdir, &["branch", "elsewhere"]);
    let harness = Harness::start();
    let opened = open(&harness, workdir);

    git(workdir, &["switch", "--quiet", "elsewhere"]);

    // The status Slots wait for any newer Snapshot, not a state: a switch to the same
    // commit leaves status as it was. Nothing else in the worktree changes meanwhile.
    let mut seen = Seen::default();
    seen.until(&harness, |seen| {
        let on_elsewhere = seen.all.iter().any(|s| match s {
            Snapshot::Refs(r) if r.workdir == workdir && r.generation > opened.refs => {
                r.result.as_ref().is_ok_and(|refs| {
                    matches!(&refs.head, Head::Branch { name, .. } if name.as_str() == "refs/heads/elsewhere")
                })
            }
            _ => false,
        });
        let refreshed = |slot: StatusSlot, opened: Generation| {
            seen.all.iter().any(|s| {
                status_generation(s, slot).is_some_and(|(w, g)| w == workdir && g > opened)
            })
        };
        on_elsewhere
            && refreshed(StatusSlot::Staged, opened.staged)
            && refreshed(StatusSlot::Unstaged, opened.unstaged)
            && refreshed(StatusSlot::Untracked, opened.untracked)
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatusSlot {
    Staged,
    Unstaged,
    Untracked,
}

/// The workdir and generation of `snapshot`, if it is from `slot`.
fn status_generation(snapshot: &Snapshot, slot: StatusSlot) -> Option<(&Path, Generation)> {
    match (snapshot, slot) {
        (Snapshot::Staged(s), StatusSlot::Staged)
        | (Snapshot::Unstaged(s), StatusSlot::Unstaged) => Some((&s.workdir, s.generation)),
        (Snapshot::Untracked(s), StatusSlot::Untracked) => Some((&s.workdir, s.generation)),
        _ => None,
    }
}

#[test]
fn a_git_switch_in_a_linked_worktree_refreshes_refs_and_status() {
    let (_dir, _repo, worktree) = linked_worktree();
    assert_an_external_switch_refreshes_refs_and_status(&worktree);
}

#[test]
fn a_git_switch_in_a_submodule_refreshes_refs_and_status() {
    let (_dir, repo) = small_fixture_clone();
    // Since git 2.38.1 a submodule can't clone from a local path unless this allows it.
    git(
        &repo,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "--quiet",
            common::small_fixture().to_str().unwrap(),
            "sub",
        ],
    );
    let submodule = repo.join("sub");
    assert!(
        submodule.join(".git").is_file(),
        "a submodule's `.git` is a file pointing into the superproject's `.git/modules`"
    );
    // Seen red: with only the worktree watched, nothing refreshes and this times out.
    assert_an_external_switch_refreshes_refs_and_status(&submodule);
}

#[test]
fn a_branch_created_in_the_main_worktree_shows_in_a_linked_worktrees_refs() {
    let (_dir, repo, worktree) = linked_worktree();
    common::assert_filesystem_events_available(&worktree);
    let harness = Harness::start();
    let opened = open(&harness, &worktree);

    // Every worktree shares the branch, which git writes to the common dir.
    git(&repo, &["branch", "made-in-main"]);

    // Seen red: with the linked worktree's own git dir watched but not the common dir, no
    // Refs Snapshot lists the branch.
    let mut seen = Seen::default();
    seen.until(&harness, |seen| {
        seen.all.iter().any(|s| match s {
            Snapshot::Refs(r) if r.workdir == worktree && r.generation > opened.refs => {
                r.result.as_ref().is_ok_and(|refs| {
                    refs.local
                        .iter()
                        .any(|b| b.name.as_str() == "refs/heads/made-in-main")
                })
            }
            _ => false,
        })
    });
}

#[test]
fn switches_in_the_main_worktree_refresh_no_status_in_a_linked_one_and_its_own_switches_do() {
    let (_dir, repo, worktree) = linked_worktree();
    common::assert_filesystem_events_available(&worktree);
    for (workdir, prefix) in [(&repo, "main"), (&worktree, "linked")] {
        git(workdir, &["branch", &format!("{prefix}-there")]);
        git(workdir, &["branch", &format!("{prefix}-back")]);
    }
    // Twenty switches between two branches at the same commit, so no worktree file changes.
    let switches = |workdir: &Path, prefix: &str| {
        for i in 0..20 {
            let branch = if i % 2 == 0 { "there" } else { "back" };
            git(
                workdir,
                &["switch", "--quiet", &format!("{prefix}-{branch}")],
            );
        }
    };
    let harness = Harness::start();
    open(&harness, &worktree);
    let mut seen = Seen::default();

    // The main worktree's HEAD and index lie in the common dir, which is watched for the
    // linked worktree's refs and config.
    let main = status_refreshes_during(&mut seen, &harness, &worktree, "main", || {
        switches(&repo, "main")
    });
    // Control: the same switches in the linked worktree move its own HEAD, each a refresh.
    let control = status_refreshes_during(&mut seen, &harness, &worktree, "control", || {
        switches(&worktree, "linked")
    });
    assert!(
        control > REFRESHED,
        "the linked worktree's own switches should each refresh: {control} generations"
    );
    // Seen red: with the common dir's HEAD and index classified as the linked worktree's own,
    // the main worktree's switches refresh as many generations as the control's.
    assert!(
        main <= MARKER_ONLY,
        "the main worktree's switches refreshed the linked one: {main} generations (control: {control})"
    );
}
