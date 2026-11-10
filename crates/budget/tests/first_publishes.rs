//! A launch's breakdown learns when the Repo service published each Slot's first Snapshot from
//! the service's own `read_job` and `publish` spans. Its own test binary, because it installs the
//! process's subscriber.

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use cairn_budget::clock;
use cairn_budget::fixture::{SAVED, Sandbox};
use cairn_budget::launch::FirstPublishes;
use cairn_budget::probe::{Line, Mark, Slot};
use cairn_repo::{Intent, RepoService, Snapshot};
use common::small_fixture;
use tracing::span::{Attributes, Id};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt as _};

/// Records the name of every span it sees: a layer beside `FirstPublishes`, such as Cairn's log.
struct Names(Arc<Mutex<Vec<&'static str>>>);

impl<S: tracing::Subscriber> Layer<S> for Names {
    fn on_new_span(&self, attrs: &Attributes<'_>, _: &Id, _: Context<'_, S>) {
        self.0.lock().unwrap().push(attrs.metadata().name());
    }
}

/// Drains `service` until `done` holds for the Slots drained so far, by when each was first
/// drained.
fn drain_until(
    service: &RepoService,
    woken: &mpsc::Receiver<()>,
    drained: &mut BTreeMap<Slot, Vec<(u64, Snapshot)>>,
    done: impl Fn(&BTreeMap<Slot, Vec<(u64, Snapshot)>>) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(drained) {
        assert!(Instant::now() < deadline, "timed out: {:?}", drained.keys());
        let _ = woken.recv_timeout(Duration::from_millis(50));
        let now = clock::now_ns();
        for snapshot in service.drain() {
            let slot = match &snapshot {
                Snapshot::Refs(_) => Slot::Refs,
                Snapshot::Staged(_) => Slot::Staged,
                Snapshot::Unstaged(_) => Slot::Unstaged,
                Snapshot::Untracked(_) => Slot::Untracked,
                _ => continue,
            };
            drained.entry(slot).or_default().push((now, snapshot));
        }
    }
}

#[test]
fn each_slots_first_published_snapshot_is_marked_once_before_it_is_drained() {
    let lines: Arc<Mutex<Vec<Line>>> = Arc::default();
    let sink = lines.clone();
    let names: Arc<Mutex<Vec<&'static str>>> = Arc::default();
    // On top, as Cairn installs it, over the layer that logs.
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(Names(names.clone()))
            .with(FirstPublishes::layer(move |line| {
                sink.lock().unwrap().push(line)
            })),
    )
    .unwrap();
    let sandbox = Sandbox::clone_of(&small_fixture()).unwrap();
    let (wake, woken) = mpsc::channel();
    let service = RepoService::start(move || {
        let _ = wake.send(());
    });

    let opened = clock::now_ns();
    service.send(Intent::Open(sandbox.repo().to_path_buf()));
    let mut drained = BTreeMap::new();
    drain_until(&service, &woken, &mut drained, |d| {
        d.len() == Slot::ALL.len()
    });

    let published = |lines: &[Line]| -> Vec<(Slot, u64)> {
        lines
            .iter()
            .filter_map(|line| match line {
                Line::Mark {
                    mark: Mark::Published(slot),
                    ns,
                } => Some((*slot, *ns)),
                _ => None,
            })
            .collect()
    };
    let first = published(&lines.lock().unwrap());
    // Seen red: with `FirstPublishes` unfiltered and answering no for every other callsite, no
    // layer under it sees the Open's `intent` span.
    assert!(
        names.lock().unwrap().contains(&"intent"),
        "the layer beside it sees only {:?}",
        names.lock().unwrap()
    );
    for slot in Slot::ALL {
        let marks: Vec<u64> = first
            .iter()
            .filter(|(s, _)| *s == slot)
            .map(|(_, ns)| *ns)
            .collect();
        assert_eq!(marks.len(), 1, "{slot:?}: {first:?}");
        let drained_ns = drained[&slot][0].0;
        assert!(
            opened <= marks[0] && marks[0] <= drained_ns,
            "{slot:?} published at {}, opened at {opened}, drained at {drained_ns}",
            marks[0]
        );
    }

    // A later answer from Unstaged, which published again, is no first Snapshot: it marks
    // nothing more. Unstaged only, since the save's answer is what this waits for.
    let before = drained[&Slot::Unstaged].len();
    sandbox.save(true).unwrap();
    drain_until(&service, &woken, &mut drained, |d| {
        d[&Slot::Unstaged][before..].iter().any(|(_, s)| match s {
            Snapshot::Unstaged(u) => u
                .result
                .as_ref()
                .is_ok_and(|changes| changes.iter().any(|c| c.path.to_string() == SAVED)),
            _ => false,
        })
    });
    let unstaged = |marks: &[(Slot, u64)]| -> Vec<(Slot, u64)> {
        marks
            .iter()
            .filter(|(slot, _)| *slot == Slot::Unstaged)
            .copied()
            .collect()
    };
    // Seen red: with every publish marked, the save's Unstaged answer.
    let after = published(&lines.lock().unwrap());
    assert_eq!(unstaged(&after), unstaged(&first));
}
