//! The Background lane: a long history walk never takes Interactive capacity, and it stops
//! as soon as nobody wants its answer.
//!
//! A walk is held the way git itself would be slow: its `git rev-list` needs a commit that
//! only a slowed promisor remote has. See [`held_walk`].

mod common;

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use cairn_repo::{Head, Intent, Oid, RefsSnapshot, Snapshot};
use common::{
    Harness, Release, Seen, SlowRemote, git, install_slow_clean_filter, open, rev_list_order,
    slow_remote, wait_for_file,
};

/// A repository whose next walk blocks in the slow remote until `release` exists. The ref
/// `refs/held/walk` points at a local tag of a commit only the remote has, and the repository
/// is a partial clone, so `git rev-list --all` fetches the commit to peel the tag. Refs reads
/// list only branches and tags, so they never touch it. A tag and not a commit's missing parent,
/// because newer git no longer fetches a parent during a walk (`docs/git-versions.md`).
fn held_walk() -> SlowRemote {
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
    git(&f.repo, &["update-ref", "refs/held/walk", tag.trim()]);
    git(&f.repo, &["config", "core.repositoryFormatVersion", "1"]);
    git(&f.repo, &["config", "extensions.partialClone", "origin"]);
    git(&f.repo, &["config", "remote.origin.promisor", "true"]);
    f
}

fn wrapper_runs(log: &Path) -> usize {
    std::fs::read_to_string(log).map_or(0, |log| log.lines().count())
}

/// Waits until `log` shows `runs` runs of the slow remote's wrapper, each a walk's git that
/// is now blocked in it.
fn wait_for_wrapper_runs(log: &Path, runs: usize) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while wrapper_runs(log) < runs {
        assert!(
            std::time::Instant::now() < deadline,
            "the walk never got to the remote"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn answer_arrives(walk: &mpsc::Receiver<Result<Vec<Oid>, String>>) -> Result<Vec<Oid>, String> {
    walk.recv_timeout(Duration::from_secs(30))
        .expect("the walk answers")
}

/// The channel closes with no answer: the Superseded walk ended and said nothing.
fn closes_unanswered(walk: &mpsc::Receiver<Result<Vec<Oid>, String>>) {
    assert_eq!(
        walk.recv_timeout(Duration::from_secs(30)),
        Err(mpsc::RecvTimeoutError::Disconnected),
    );
}

fn on_branch(refs: &RefsSnapshot, branch: &str) -> bool {
    refs.result.as_ref().is_ok_and(|refs| {
        let name = format!("refs/heads/{branch}");
        matches!(&refs.head, Head::Branch { name: head, .. } if head.as_str() == name)
            || refs.local.iter().any(|r| r.name.as_str() == name)
    })
}

/// Both Interactive workers would be needed to fail this: one Read Job held in a clean filter
/// takes the first, and a Read Job that strayed from the Background lane would take the second.
/// An Interactive read must still answer, with the walk still blocked.
#[test]
fn a_long_walk_doesnt_delay_an_interactive_read_even_with_a_worker_already_busy() {
    let f = held_walk();
    common::assert_filesystem_events_available(&f.repo);
    let filter_started = f.dir.path().join("filter-started");
    let filter_release = f.dir.path().join("filter-release");
    let harness = Harness::start();
    let _release_walk = Release(&f.release);
    let _release_filter = Release(&filter_release);
    open(&harness, &f.repo);

    let walk = harness.service.walk_history();
    wait_for_wrapper_runs(&f.log, 1);

    // Only the first read to run the filter blocks. Status and Unstaged can both run it at once.
    install_slow_clean_filter(
        &f.repo,
        f.dir.path(),
        &common::block_first_run(&filter_started, &filter_release),
    );
    std::fs::write(f.repo.join("wake-the-watcher"), "").unwrap();
    wait_for_file(&filter_started);

    git(&f.repo, &["branch", "made-while-walking"]);
    let mut seen = Seen::default();
    // Seen red twice: the refs Slot on the Background lane, which queues this read behind the
    // walk, and the history Slot on the Interactive lane, which leaves it no worker.
    seen.until(&harness, |seen| {
        seen.all.iter().any(|s| match s {
            Snapshot::Refs(r) => on_branch(r, "made-while-walking"),
            _ => false,
        })
    });

    // The control: that read beat a walk that was truly still running.
    assert!(
        matches!(walk.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "the walk was still blocked when the Interactive read answered"
    );

    std::fs::write(&f.release, "").unwrap();
    let walked = answer_arrives(&walk).expect("the walk succeeds once released");
    assert!(
        walked.contains(&Oid::from_hex(f.new.as_str())),
        "the fetched parent"
    );
    assert_eq!(walked, rev_list_order(&f.repo));
}

#[test]
fn a_newer_walk_stops_the_running_one_instead_of_waiting_for_it() {
    let f = held_walk();
    let harness = Harness::start();
    let _release = Release(&f.release);
    open(&harness, &f.repo);
    let first = harness.service.walk_history();
    wait_for_wrapper_runs(&f.log, 1);

    let second = harness.service.walk_history();

    // The Background lane has one worker, so the second walk reaching the remote is the
    // marker that the first ended, with nothing released. Seen red: `Running::Finish` for
    // walks leaves the first blocked, and this never arrives.
    wait_for_wrapper_runs(&f.log, 2);
    closes_unanswered(&first);
    std::fs::write(&f.release, "").unwrap();
    answer_arrives(&second).expect("the newer walk answers");
}

#[test]
fn opening_another_repository_stops_the_walk() {
    let f = held_walk();
    let (_other_dir, other) = common::small_fixture_clone();
    let harness = Harness::start();
    let _release = Release(&f.release);
    open(&harness, &f.repo);
    let walk = harness.service.walk_history();
    wait_for_wrapper_runs(&f.log, 1);

    open(&harness, &other);

    // Seen red: without the `drop_jobs` in Open, this waits out its timeout.
    closes_unanswered(&walk);
}

#[test]
fn a_quit_stops_the_walk_and_no_walk_starts_after_it() {
    let f = held_walk();
    let harness = Harness::start();
    let _release = Release(&f.release);
    open(&harness, &f.repo);
    let walk = harness.service.walk_history();
    wait_for_wrapper_runs(&f.log, 1);

    harness.service.send(Intent::Quit);

    // Seen red: leaving the history Slot out of `Reads::close`.
    closes_unanswered(&walk);
    harness.drain_until(|s| matches!(s, Snapshot::Quit));
    assert_eq!(
        answer_arrives(&harness.service.walk_history()),
        Err("no repository is open".to_owned()),
    );
}

/// Closing a window mustn't wait out a walk of a large repository.
#[test]
fn dropping_the_service_stops_the_walk_rather_than_waiting_for_it() {
    let f = held_walk();
    let harness = Harness::start();
    let _release = Release(&f.release);
    open(&harness, &f.repo);
    let walk = harness.service.walk_history();
    wait_for_wrapper_runs(&f.log, 1);

    let (dropped, has_dropped) = mpsc::channel();
    std::thread::spawn(move || {
        drop(harness);
        let _ = dropped.send(());
    });

    // Seen red: without the `drop_jobs` in `RepoService`'s Drop, this times out.
    has_dropped
        .recv_timeout(Duration::from_secs(30))
        .expect("dropping the service returned with the walk's git still blocked");
    closes_unanswered(&walk);
}

/// The operating system's own view of the lanes: what a profiler or `spindump` would show.
#[cfg(target_os = "macos")]
mod threads {
    use std::collections::BTreeMap;
    use std::ffi::CStr;

    use super::Harness;

    /// Every thread in this process with a name starting `cairn-`, with its QoS class. Tests in
    /// this binary run side by side, so each name maps to the QoS classes of all such threads.
    // libc deprecates `mach_task_self` in favour of the mach2 crate, for this one call.
    #[allow(deprecated)]
    fn lane_threads() -> BTreeMap<String, Vec<libc::qos_class_t>> {
        let mut threads = BTreeMap::<String, Vec<libc::qos_class_t>>::new();
        // SAFETY: these calls read the thread list the kernel hands back, which stays valid
        // until the `vm_deallocate`. Each pthread lookup is checked for null: a thread can
        // exit between the list and the lookup.
        unsafe {
            let task = libc::mach_task_self();
            let mut list: libc::thread_act_array_t = std::ptr::null_mut();
            let mut count: libc::mach_msg_type_number_t = 0;
            assert_eq!(libc::task_threads(task, &mut list, &mut count), 0);
            for i in 0..count as usize {
                let thread = libc::pthread_from_mach_thread_np(*list.add(i));
                if thread == 0 as libc::pthread_t {
                    continue;
                }
                let mut name = [0 as libc::c_char; 64];
                if libc::pthread_getname_np(thread, name.as_mut_ptr(), name.len()) != 0 {
                    continue;
                }
                let name = CStr::from_ptr(name.as_ptr()).to_string_lossy().into_owned();
                if !name.starts_with("cairn-") {
                    continue;
                }
                let mut class = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
                let mut priority = 0;
                assert_eq!(
                    libc::pthread_get_qos_class_np(thread, &mut class, &mut priority),
                    0
                );
                threads.entry(name).or_default().push(class);
            }
            libc::vm_deallocate(
                task,
                list as libc::vm_address_t,
                count as usize * std::mem::size_of::<libc::thread_act_t>(),
            );
        }
        threads
    }

    /// Whether every thread of this name has this class, once its first lines have run.
    fn all_are(
        threads: &BTreeMap<String, Vec<libc::qos_class_t>>,
        name: &str,
        class: libc::qos_class_t,
    ) -> bool {
        threads
            .get(name)
            .is_some_and(|classes| classes.iter().all(|c| *c as u32 == class as u32))
    }

    #[test]
    fn each_lane_has_its_own_named_capped_threads_at_its_own_qos() {
        let _harness = Harness::start();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        // A worker sets its QoS as it starts, so give it the moment that takes.
        let threads = loop {
            let threads = lane_threads();
            let settled = all_are(
                &threads,
                "cairn-interactive-0",
                libc::qos_class_t::QOS_CLASS_USER_INITIATED,
            ) && all_are(
                &threads,
                "cairn-interactive-1",
                libc::qos_class_t::QOS_CLASS_USER_INITIATED,
            ) && all_are(
                &threads,
                "cairn-background-0",
                libc::qos_class_t::QOS_CLASS_UTILITY,
            );
            if settled {
                break threads;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the lanes never reached their QoS: {:?}",
                threads.keys().collect::<Vec<_>>()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };

        // Seen red: `Qos::apply` doing nothing, and the Background lane at the Interactive
        // lane's class, both in the wait above. And a second Background worker, here.
        let lanes: Vec<_> = threads
            .keys()
            .filter(|name| {
                name.starts_with("cairn-interactive") || name.starts_with("cairn-background")
            })
            .map(String::as_str)
            .collect();
        // Named by index from zero, so a worker over the cap would add a name here.
        assert_eq!(
            lanes,
            [
                "cairn-background-0",
                "cairn-interactive-0",
                "cairn-interactive-1"
            ]
        );
    }
}
