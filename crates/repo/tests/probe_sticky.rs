//! THROWAWAY probe, not for merging. Its own binary because it breaks the hermetic global config.
//! Does an error stay on screen when the thing that fixes it reports no watched change?
#![allow(clippy::all)]

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use cairn_repo::Snapshot;
use common::{Harness, git, open};

#[derive(Default, Debug)]
struct Shown {
    refs: Option<(u64, Result<(), String>)>,
    staged: Option<(u64, Result<(), String>)>,
    unstaged: Option<(u64, Result<(), String>)>,
    untracked: Option<(u64, Result<(), String>)>,
}

fn plain<T>(r: &Result<T, String>) -> Result<(), String> {
    r.as_ref().map(|_| ()).map_err(Clone::clone)
}

impl Shown {
    fn drain(&mut self, h: &Harness) {
        for s in h.service.drain() {
            match s {
                Snapshot::Refs(s) => self.refs = Some((s.generation.get(), plain(&s.result))),
                Snapshot::Staged(s) => self.staged = Some((s.generation.get(), plain(&s.result))),
                Snapshot::Unstaged(s) => {
                    self.unstaged = Some((s.generation.get(), plain(&s.result)))
                }
                Snapshot::Untracked(s) => {
                    self.untracked = Some((s.generation.get(), plain(&s.result)))
                }
                _ => {}
            }
        }
    }
    fn all(&self) -> [(&'static str, &Option<(u64, Result<(), String>)>); 4] {
        [
            ("refs", &self.refs),
            ("staged", &self.staged),
            ("unstaged", &self.unstaged),
            ("untracked", &self.untracked),
        ]
    }
    fn report(&self, when: &str) {
        for (name, slot) in self.all() {
            println!("[{when}] {name}: {slot:?}");
        }
    }
    fn until(&mut self, h: &Harness, what: &str, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.drain(h);
            if done(self) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out: {what}: {self:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn is_err(slot: &Option<(u64, Result<(), String>)>) -> bool {
    matches!(slot, Some((_, Err(_))))
}
fn is_ok(slot: &Option<(u64, Result<(), String>)>) -> bool {
    matches!(slot, Some((_, Ok(()))))
}

fn change_something_watched(repo: &Path, n: u32) {
    std::fs::write(repo.join("tracked.txt"), format!("change {n}\n")).unwrap();
    std::fs::write(repo.join(".git/FETCH_HEAD"), format!("{n}\n")).unwrap();
}

fn run(through_git: bool) {
    println!("=== refs through git from the first read: {through_git}");
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("r");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "--quiet"]);
    std::fs::write(repo.join("tracked.txt"), "x\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "x"]);
    if through_git {
        git(&repo, &["config", "core.safecrlf", "true"]);
    }
    common::assert_filesystem_events_available(&repo);
    let home = std::env::var_os("HOME").unwrap();
    let global = Path::new(&home).join(".gitconfig");

    let h = Harness::start();
    let _ = open(&h, &repo);
    let mut shown = Shown::default();

    // The failure: every git read now dies on the global config.
    std::fs::write(&global, "[broken\n").unwrap();
    change_something_watched(&repo, 1);
    shown.until(&h, "status reads fail", |s| {
        is_err(&s.staged) && is_err(&s.unstaged)
    });
    // Let every late event and pending read finish while the failure still holds.
    std::thread::sleep(Duration::from_secs(3));
    shown.drain(&h);
    shown.report("settled while broken");

    // The fix. Outside the repository, so nothing reports it.
    std::fs::remove_file(&global).unwrap();
    let git_says = git(&repo, &["for-each-ref", "--count=1"]);
    println!("git itself now reads fine: {}", git_says.trim());
    std::thread::sleep(Duration::from_secs(5));
    shown.drain(&h);
    shown.report("5s after the fix, no watched change");

    // The control: the same state clears as soon as a watched change arrives.
    change_something_watched(&repo, 2);
    shown.until(&h, "a watched change clears it", |s| {
        is_ok(&s.refs) && is_ok(&s.staged) && is_ok(&s.unstaged)
    });
    shown.report("after one watched change");
}

#[test]
fn an_error_stays_until_a_watched_change_follows_the_fix() {
    run(false);
    run(true);
}
