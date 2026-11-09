//! THROWAWAY probe, not for merging. Measures, does not assert: run with
//! `--nocapture --test-threads=1`. `PROBE_ITERS` scales the loops.
#![allow(clippy::all)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cairn_repo::{Head, Snapshot};
use cairn_write::{Git, Invocation, JobClass};
use common::{Harness, git, git_output, open};
use notify::{EventKind, RecursiveMode, Watcher as _};

fn iters(default: u32) -> u32 {
    std::env::var("PROBE_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn new_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("r");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "--quiet"]);
    std::fs::write(repo.join("tracked.txt"), "x\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "x"]);
    (dir, repo)
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Keeps every core busy while alive, as the parallel test binaries did on CI.
struct Load(Arc<AtomicBool>);
impl Load {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        for _ in 0..std::thread::available_parallelism().map_or(4, |n| n.get()) {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::hint::black_box(0u64);
                }
            });
        }
        Self(stop)
    }
}
impl Drop for Load {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

// ---- A: is the failure window real, and does it outlast the event that reports the replacement?

#[derive(Default)]
struct Log {
    /// When a git read started, and its stderr if it failed.
    git_failures: Vec<(Instant, String)>,
    git_reads: u64,
    /// A failed plain open of `.git/config`, with the OS error.
    open_failures: Vec<(Instant, Option<i32>)>,
    opens: u64,
}

fn window_vs_event(loaded: bool) {
    let (_dir, repo) = new_repo();
    let _load = loaded.then(Load::start);
    let log = Arc::new(Mutex::new(Log::default()));
    let stop = Arc::new(AtomicBool::new(false));
    let config = repo.join(".git/config");

    let mut workers = Vec::new();
    for _ in 0..2 {
        let (log, stop, repo) = (log.clone(), stop.clone(), repo.clone());
        workers.push(std::thread::spawn(move || {
            let g = Git::shared().unwrap();
            while !stop.load(Ordering::Relaxed) {
                let started = Instant::now();
                let out = g
                    .run(
                        &Invocation::new(JobClass::Read, &repo).args(["for-each-ref", "--count=1"]),
                    )
                    .unwrap();
                let mut log = log.lock().unwrap();
                log.git_reads += 1;
                if !out.status.success() {
                    log.git_failures
                        .push((started, one_line(&out.stderr_text())));
                }
            }
        }));
    }
    {
        let (log, stop, config) = (log.clone(), stop.clone(), config.clone());
        workers.push(std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let at = Instant::now();
                let opened = std::fs::File::open(&config);
                let mut log = log.lock().unwrap();
                log.opens += 1;
                if let Err(e) = opened {
                    log.open_failures.push((at, e.raw_os_error()));
                }
            }
        }));
    }

    // When each report of `.git/config` arrived, as the watcher's handler would see it.
    let events = Arc::new(Mutex::new(Vec::<Instant>::new()));
    let mut watcher = {
        let events = events.clone();
        notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
            if let Ok(e) = e
                && !matches!(e.kind, EventKind::Access(_))
                && e.paths
                    .iter()
                    .any(|p| p.file_name().is_some_and(|n| n == "config"))
            {
                events.lock().unwrap().push(Instant::now());
            }
        })
        .unwrap()
    };
    watcher
        .watch(
            &repo.canonicalize().unwrap().join(".git"),
            RecursiveMode::NonRecursive,
        )
        .unwrap();

    let n = iters(1000);
    let mut writer_failures = BTreeMap::<String, u32>::new();
    // (end of the settle period, last event of that replacement)
    let mut rounds = Vec::new();
    for i in 0..n {
        let before = Instant::now();
        let out = git_output(&repo, &["config", "probe.value", &i.to_string()]);
        if !out.status.success() {
            *writer_failures
                .entry(one_line(&out.stderr_text()))
                .or_default() += 1;
        }
        std::thread::sleep(Duration::from_millis(120));
        let last = events
            .lock()
            .unwrap()
            .iter()
            .copied()
            .filter(|t| *t >= before)
            .max();
        rounds.push((before, Instant::now(), last));
    }
    stop.store(true, Ordering::Relaxed);
    for w in workers {
        w.join().unwrap();
    }
    let log = log.lock().unwrap();
    let after = |t: Instant| {
        rounds.iter().find_map(|(from, to, last)| {
            (t >= *from && t < *to)
                .then(|| last.filter(|l| t > *l).map(|l| t - l))
                .flatten()
        })
    };
    let git_after: Vec<_> = log
        .git_failures
        .iter()
        .filter_map(|(t, _)| after(*t))
        .collect();
    let open_after: Vec<_> = log
        .open_failures
        .iter()
        .filter_map(|(t, _)| after(*t))
        .collect();
    let mut msgs = BTreeMap::<&str, u32>::new();
    for (_, m) in &log.git_failures {
        *msgs.entry(m).or_default() += 1;
    }
    let mut codes = BTreeMap::<Option<i32>, u32>::new();
    for (_, c) in &log.open_failures {
        *codes.entry(*c).or_default() += 1;
    }
    println!(
        "=== A window_vs_event loaded={loaded} replacements={n} rounds_without_event={}",
        rounds.iter().filter(|r| r.2.is_none()).count()
    );
    println!(
        "git reads {} failed {} | failed reads STARTED after the last event: {} (largest gap {:?})",
        log.git_reads,
        log.git_failures.len(),
        git_after.len(),
        git_after.iter().max()
    );
    println!(
        "opens {} failed {} os-errors {codes:?} | failed opens after the last event: {} (largest gap {:?})",
        log.opens,
        log.open_failures.len(),
        open_after.len(),
        open_after.iter().max()
    );
    println!("git failure text: {msgs:#?}");
    println!("`git config` itself failed: {writer_failures:#?}");
}

#[test]
fn a_window_vs_event_idle() {
    window_vs_event(false);
}

#[test]
fn a_window_vs_event_loaded() {
    window_vs_event(true);
}

// ---- B: through the real service: does a replaced file ever leave an error showing?

#[derive(Default)]
struct Slots(BTreeMap<&'static str, Result<(), String>>);

fn drain(h: &Harness, slots: &mut Slots, errors: &mut BTreeMap<String, u32>) -> bool {
    let batch = h.service.drain();
    let any = !batch.is_empty();
    let mut note = |name: &'static str, r: Result<(), String>| {
        if let Err(e) = &r {
            *errors
                .entry(format!("{name}: {}", one_line(e)))
                .or_default() += 1;
        }
        slots.0.insert(name, r);
    };
    fn plain<T>(r: &Result<T, String>) -> Result<(), String> {
        r.as_ref().map(|_| ()).map_err(Clone::clone)
    }
    for s in batch {
        match s {
            Snapshot::Refs(s) => note("refs", plain(&s.result)),
            Snapshot::Staged(s) => note("staged", plain(&s.result)),
            Snapshot::Unstaged(s) => note("unstaged", plain(&s.result)),
            Snapshot::Untracked(s) => note("untracked", plain(&s.result)),
            _ => {}
        }
    }
    any
}

fn settle(h: &Harness, slots: &mut Slots, errors: &mut BTreeMap<String, u32>, quiet: Duration) {
    let mut last = Instant::now();
    while last.elapsed() < quiet {
        if drain(h, slots, errors) {
            last = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn service_stress(workload: &str, change: fn(&Path, u32), loaded: bool) {
    let (_dir, repo) = new_repo();
    common::assert_filesystem_events_available(&repo);
    let _load = loaded.then(Load::start);
    let h = Harness::start();
    let _ = open(&h, &repo);
    let (mut slots, mut errors) = (Slots::default(), BTreeMap::new());
    settle(&h, &mut slots, &mut errors, Duration::from_millis(500));
    let n = iters(150);
    let mut stuck = Vec::new();
    for i in 0..n {
        change(&repo, i);
        settle(&h, &mut slots, &mut errors, Duration::from_millis(350));
        let showing: Vec<_> = slots
            .0
            .iter()
            .filter(|(_, r)| r.is_err())
            .map(|(k, r)| (*k, r.clone()))
            .collect();
        if !showing.is_empty() {
            // Long past any file replacement. Does git read fine while the error shows?
            settle(&h, &mut slots, &mut errors, Duration::from_secs(3));
            let still: Vec<_> = slots
                .0
                .iter()
                .filter(|(_, r)| r.is_err())
                .map(|(k, r)| (*k, one_line(r.as_ref().unwrap_err())))
                .collect();
            let git_ok = git_output(&repo, &["for-each-ref", "--count=1"])
                .status
                .success();
            println!(
                "iteration {i}: showing {showing:?}; after 3s still {still:?}; git reads ok now: {git_ok}"
            );
            if !still.is_empty() && git_ok {
                stuck.push((i, still));
            }
            // Recover, so one stuck error isn't counted again.
            git(&repo, &["config", "probe.recover", &i.to_string()]);
            settle(&h, &mut slots, &mut errors, Duration::from_millis(500));
        }
    }
    println!(
        "=== B service workload={workload} loaded={loaded} changes={n} errors_published={} STUCK={}",
        errors.values().sum::<u32>(),
        stuck.len()
    );
    println!("errors: {errors:#?}");
    println!("stuck: {stuck:#?}");
}

fn change_config(repo: &Path, i: u32) {
    let _ = git_output(repo, &["config", "probe.value", &i.to_string()]);
}
fn change_index(repo: &Path, i: u32) {
    std::fs::write(repo.join("tracked.txt"), format!("{i}\n")).unwrap();
    let _ = git_output(repo, &["add", "tracked.txt"]);
}
fn change_commit(repo: &Path, i: u32) {
    let _ = git_output(
        repo,
        &["commit", "--quiet", "--allow-empty", "-m", &i.to_string()],
    );
}

#[test]
fn b_config_idle() {
    service_stress("config", change_config, false);
}
#[test]
fn b_config_loaded() {
    service_stress("config", change_config, true);
}
#[test]
fn b_index_idle() {
    service_stress("index", change_index, false);
}
#[test]
fn b_index_loaded() {
    service_stress("index", change_index, true);
}
#[test]
fn b_commit_loaded() {
    service_stress("commit", change_commit, true);
}

// ---- C: does a failed git step in `fallback::refs` publish a wrong HEAD with no error?
// Unix only because chmod is how it makes `.git/config` fail on and off.

#[cfg(unix)]
#[test]
fn c_flickering_config_misreports_head_through_git() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, repo) = new_repo();
    // A real reason for Refs to be read through git from the first read.
    git(&repo, &["config", "core.safecrlf", "true"]);
    let branch = git(&repo, &["symbolic-ref", "HEAD"]).trim().to_owned();
    let tip = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    let h = Harness::start();
    let _ = open(&h, &repo);

    let config = repo.join(".git/config");
    let stop = Arc::new(AtomicBool::new(false));
    let toggler = {
        let (stop, config) = (stop.clone(), config.clone());
        std::thread::spawn(move || {
            let mut mode = 0o000;
            while !stop.load(Ordering::Relaxed) {
                let _ = std::fs::set_permissions(&config, std::fs::Permissions::from_mode(mode));
                mode = if mode == 0 { 0o644 } else { 0 };
                std::thread::sleep(Duration::from_millis(3));
            }
            let _ = std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o644));
        })
    };
    let (mut right, mut wrong, mut errors) = (0u32, BTreeMap::<String, u32>::new(), 0u32);
    let until = Instant::now() + Duration::from_secs(u64::from(iters(20)).min(60));
    while Instant::now() < until {
        // A watched change, in case the chmods alone report nothing.
        let _ = std::fs::write(repo.join(".git/FETCH_HEAD"), "x\n");
        for s in h.service.drain() {
            if let Snapshot::Refs(r) = s {
                match &r.result {
                    Err(_) => errors += 1,
                    Ok(refs) => match &refs.head {
                        Head::Branch {
                            name,
                            target: Some(t),
                        } if name.as_str() == branch && t.to_string() == tip => right += 1,
                        other => *wrong.entry(format!("{other:?}")).or_default() += 1,
                    },
                }
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    stop.store(true, Ordering::Relaxed);
    toggler.join().unwrap();
    println!("=== C flicker: truth = {branch} at {tip}");
    println!("Ok+right {right} | Err {errors} | Ok+WRONG HEAD {wrong:#?}");
}

// ---- D: is a failed detection remembered as "nothing special about this repository"?

/// How many git runs in this repository's parent directory named `subcommand`, by trace2.
#[cfg(unix)]
fn git_runs(repo: &Path, subcommand: &str) -> usize {
    let log = Path::new(&std::env::var_os("HOME").unwrap()).join("trace2.jsonl");
    let parent = repo
        .parent()
        .and_then(Path::file_name)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    std::fs::read_to_string(log).map_or(0, |log| {
        log.lines()
            .filter(|l| {
                l.contains(r#""event":"start""#)
                    && l.contains(&parent)
                    && l.contains(&format!(r#""{subcommand}""#))
            })
            .count()
    })
}

#[cfg(unix)]
#[test]
fn d_detection_during_a_failure_is_remembered() {
    use std::os::unix::fs::PermissionsExt;
    for fail_first in [false, true] {
        let (_dir, repo) = new_repo();
        git(&repo, &["config", "core.safecrlf", "true"]);
        let config = repo.join(".git/config");
        let h = Harness::start();
        if fail_first {
            std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        h.service.send(cairn_repo::Intent::Open(repo.clone()));
        let first = h.drain_until(|s| matches!(s, Snapshot::Refs(_)));
        let first_ok = first
            .iter()
            .any(|s| matches!(s, Snapshot::Refs(r) if r.result.is_ok()));
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o644)).unwrap();
        let wait = || {
            let t = Instant::now();
            while t.elapsed() < Duration::from_millis(1500) {
                let _ = h.service.drain();
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        wait();
        let before = git_runs(&repo, "for-each-ref");
        std::fs::write(repo.join(".git/FETCH_HEAD"), "x\n").unwrap();
        wait();
        let after = git_runs(&repo, "for-each-ref");
        println!(
            "=== D core.safecrlf=true, config unreadable at Open: {fail_first}; first Refs ok: {first_ok}; for-each-ref runs for the later refresh: {}",
            after - before
        );
    }
}
