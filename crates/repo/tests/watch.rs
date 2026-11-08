//! External changes reach the Repo service through the watcher, and the Slot coalesces the
//! refreshes they cause: a newer Read Job Supersedes the pending one and stops the running
//! one. The Supersede tests drive refreshes with Opens, whose generations are known.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cairn_repo::{ChangesSnapshot, Generation, Head, Intent, Snapshot};
use common::{Harness, Release, Seen, git, open, slash, small_fixture_clone, touch};

#[test]
fn a_git_switch_run_outside_the_service_is_followed_by_a_refs_snapshot() {
    let (_dir, repo) = small_fixture_clone();
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
    install_slow_clean_filter(
        &repo,
        dir.path(),
        &format!(
            "if [ ! -f '{started}' ]; then\n\
             touch '{started}'\n\
             while [ ! -f '{release}' ]; do sleep 0.05; done\n\
             fi\n",
            started = slash(&started),
            release = slash(&release),
        ),
    );

    let harness = Harness::start();
    let _release = Release(&release);
    harness.service.send(Intent::Open(repo.clone()));
    let deadline = Instant::now() + Duration::from_secs(30);
    while !started.exists() {
        assert!(
            Instant::now() < deadline,
            "the first status read never ran the filter"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // The first read is blocked in its filter. A newer one must not wait for it: without
    // the stop, nothing publishes until `release` exists, and this drain times out.
    harness.service.send(Intent::Open(repo.clone()));
    let mut seen = Seen::default();
    seen.until(&harness, |seen| !unstaged(seen).is_empty());
    let published = unstaged(&seen);
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
    // Every Unstaged read takes a while (git runs the filter many times per read), so most
    // newer reads arrive while one is running and stop it.
    install_slow_clean_filter(&repo, dir.path(), "sleep 0.02\n");
    let harness = Harness::start();
    // A fresh service numbers the Unstaged Slot's reads 1, 2, 3, ... one per Open.
    const OPENS: u64 = 20;
    for _ in 0..OPENS {
        harness.service.send(Intent::Open(repo.clone()));
    }
    let mut seen = Seen::default();
    seen.until(&harness, |seen| {
        unstaged(seen).iter().any(|u| u.generation.get() == OPENS)
    });

    // Marker: one more Open. Its Snapshot can only follow the ones before it, so anything
    // older than OPENS would have arrived by the time it does.
    harness.service.send(Intent::Open(repo.clone()));
    seen.until(&harness, |seen| {
        unstaged(seen)
            .iter()
            .any(|u| u.generation.get() == OPENS + 1)
    });

    let published = unstaged(&seen);
    let generations: Vec<u64> = published.iter().map(|u| u.generation.get()).collect();
    assert!(
        generations.windows(2).all(|w| w[0] < w[1]),
        "generations must only increase: {generations:?}"
    );
    assert_eq!(generations.last(), Some(&(OPENS + 1)));
    assert!(
        (generations.len() as u64) < OPENS,
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
    // Each Unstaged read takes far longer than the gap between two writes, so a new event
    // always arrives while a read runs.
    install_slow_clean_filter(&repo, dir.path(), "sleep 0.02\n");
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

/// Makes README.md stat-dirty and gives it a clean filter that runs `before` and then
/// passes the content through. Each Unstaged read then runs the filter, through
/// `hash-object`, inside the git it can stop.
fn install_slow_clean_filter(repo: &Path, scratch: &Path, before: &str) {
    let script = scratch.join("slow-clean.sh");
    std::fs::write(&script, format!("{before}cat\n")).unwrap();
    git(
        repo,
        &[
            "config",
            "filter.slow.clean",
            &format!("sh '{}'", slash(&script)),
        ],
    );
    std::fs::write(repo.join(".git/info/attributes"), "README.md filter=slow\n").unwrap();
    touch(&repo.join("README.md"));
}

/// The Unstaged Snapshots in `seen`, oldest first.
fn unstaged(seen: &Seen) -> Vec<&Arc<ChangesSnapshot>> {
    seen.all
        .iter()
        .filter_map(|s| match s {
            Snapshot::Unstaged(u) => Some(u),
            _ => None,
        })
        .collect()
}

/// The Unstaged Snapshots in `seen` newer than `after`, oldest first.
fn unstaged_after(seen: &Seen, after: Generation) -> Vec<&Arc<ChangesSnapshot>> {
    let mut all = unstaged(seen);
    all.retain(|u| u.generation > after);
    all
}

#[test]
fn a_burst_shorter_than_a_read_costs_at_most_two_status_refreshes() {
    let (dir, repo) = small_fixture_clone();
    // ADR 0004's bound holds for a burst that ends while the first read runs. A longer one
    // is a stream, which publishes once per read. Slowing every read keeps this burst short.
    install_slow_clean_filter(&repo, dir.path(), "sleep 0.02\n");
    // Regular files only: a symlink or submodule entry can't be appended to.
    let files: Vec<String> = git(&repo, &["ls-files", "-s"])
        .lines()
        .filter(|l| l.starts_with("100644 "))
        .filter_map(|l| l.split_once('\t').map(|(_, path)| path.to_owned()))
        .take(200)
        .collect();
    assert_eq!(files.len(), 200, "the fixture has enough files for a burst");
    let harness = Harness::start();
    let opened = open(&harness, &repo);

    for file in &files {
        let path = repo.join(file);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(b"\nchanged outside Cairn\n");
        std::fs::write(&path, bytes).unwrap();
    }

    let complete = |u: &ChangesSnapshot| u.result.as_ref().is_ok_and(|c| c.len() == files.len());
    let mut seen = Seen::default();
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
        unstaged(seen).iter().any(|u| u.workdir == elsewhere.path())
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
