//! The read paths: refs, full status and history walks on both engines, on the small fixture
//! and, where a machine has it, the Linux kernel. A CI signal, not a Budget
//! (`CODING_STANDARDS.md`): the kernel never runs in CI, and a Budget comes from a local
//! harness run before a phase gate.

#[path = "../tests/common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use cairn_read::Repo;
use common::{
    Harness, commit_graph_exists, fixture_root, git, open, remove_commit_graph, small_fixture_clone,
};
use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, SamplingMode, criterion_group, criterion_main};

/// One read per bench. Only what a bench's own Slot needs is timed: the Open that sets up a
/// walk isn't. Refs go straight to gix, whose engine isn't in question, because an Open
/// would read status beside them.
fn benches_for(
    group: &mut BenchmarkGroup<WallTime>,
    graph: Option<&Path>,
    no_graph: Option<&Path>,
) {
    if let Some(repo) = graph.or(no_graph) {
        group.bench_function("refs", |b| {
            b.iter(|| Repo::open(repo).unwrap().refs().unwrap())
        });
        // Status has no Intent of its own, so this times what's nearest: an Open, which
        // starts the watcher and reads Staged, Unstaged, Untracked and refs side by side.
        let harness = Harness::start();
        group.bench_function("open_with_full_status", |b| {
            b.iter(|| {
                harness.service.drain();
                open(&harness, repo)
            })
        });
    }
    for (name, repo) in [("walk_gix", graph), ("walk_rev_list", no_graph)] {
        let Some(repo) = repo else { continue };
        let harness = Harness::start();
        open(&harness, repo);
        group.bench_function(name, |b| {
            b.iter(|| harness.service.walk_history().recv().unwrap().unwrap())
        });
    }
}

fn small(c: &mut Criterion) {
    let (_graph_dir, graph) = small_fixture_clone();
    git(&graph, &["commit-graph", "write", "--reachable"]);
    assert!(commit_graph_exists(&graph), "gix walks only with a graph");
    let (_no_graph_dir, no_graph) = small_fixture_clone();
    remove_commit_graph(&no_graph);
    assert!(
        !commit_graph_exists(&no_graph),
        "git walks only without one"
    );
    // After the first git spawn, so that `hermetic` has run, and before any timing: each git
    // writing a trace2 log is a cost only the tests choose to pay.
    // SAFETY: the bench binary is single-threaded here. No Repo service is running yet.
    unsafe { std::env::remove_var("GIT_TRACE2_EVENT") };

    let mut group = c.benchmark_group("small");
    group
        .sample_size(20)
        .measurement_time(Duration::from_secs(3));
    benches_for(&mut group, Some(&graph), Some(&no_graph));
    group.finish();
}

/// One of the kernel fixtures, if this machine has it as it should be. Saying so, because a
/// bench that quietly isn't there looks like one that passed. They're local only: #5 has
/// their setup, and the fixtures are shared with other tools, so a fixture is checked rather
/// than repaired.
fn kernel_fixture(name: &str, wants_graph: bool) -> Option<PathBuf> {
    let path = fixture_root().join(name);
    if !path.join(".git").exists() {
        println!(
            "skipped: kernel benches on {name}: no fixture at {}",
            path.display()
        );
        return None;
    }
    if commit_graph_exists(&path) != wants_graph {
        let has = if wants_graph { "has no" } else { "has a" };
        println!(
            "skipped: kernel benches on {name}: {} {has} commit-graph, so it isn't the case it names",
            path.display()
        );
        return None;
    }
    Some(path)
}

fn kernel(c: &mut Criterion) {
    let graph = kernel_fixture("linux-graph", true);
    let no_graph = kernel_fixture("linux-nograph", false);
    if graph.is_none() && no_graph.is_none() {
        return;
    }
    let mut group = c.benchmark_group("kernel");
    // Each sample takes seconds, so ten flat samples is the fewest criterion allows.
    group
        .sample_size(10)
        .sampling_mode(SamplingMode::Flat)
        .measurement_time(Duration::from_secs(30));
    benches_for(&mut group, graph.as_deref(), no_graph.as_deref());
    group.finish();
}

criterion_group!(benches, small, kernel);
criterion_main!(benches);
