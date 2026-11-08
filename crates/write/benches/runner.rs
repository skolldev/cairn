//! Spawn round trips through the runner, one bench per shape of command: which class it runs
//! as, and what it sends and gets back. A CI signal, not a Budget (`CODING_STANDARDS.md`).

#[path = "../tests/common/mod.rs"]
mod common;

use std::time::Duration;

use cairn_write::{Invocation, JobClass, Stop};
use common::{Repo, git, local_write, ok};
use criterion::{Criterion, criterion_group, criterion_main};

fn round_trips(c: &mut Criterion) {
    let repo = Repo::new();
    std::fs::write(repo.path.join("file"), "content\n").unwrap();
    ok(local_write(&repo.path, &["add", "file"], None));
    ok(local_write(&repo.path, &["commit", "-qm", "first"], None));
    let git = git();
    let read = || Invocation::new(JobClass::Read, &repo.path);

    let mut group = c.benchmark_group("spawn");
    group
        .sample_size(30)
        .measurement_time(Duration::from_secs(3));

    group.bench_function("read", |b| {
        b.iter(|| ok(git.run(&read().args(["rev-parse", "HEAD"])).unwrap()))
    });
    // Attaching a Stop puts git in its own process group or Job Object.
    let stop = Stop::new();
    group.bench_function("read_stoppable", |b| {
        b.iter(|| {
            let invocation = read().args(["rev-parse", "HEAD"]).stop_on(&stop);
            ok(git.run(&invocation).unwrap())
        })
    });
    group.bench_function("read_stdin", |b| {
        b.iter(|| {
            let invocation = read()
                .args(["hash-object", "--stdin"])
                .stdin(b"content\n".to_vec());
            ok(git.run(&invocation).unwrap())
        })
    });
    group.bench_function("read_records", |b| {
        b.iter(|| ok(git.run(&read().args(["ls-files", "-z"])).unwrap()))
    });
    group.bench_function("read_failing", |b| {
        b.iter(|| {
            let output = git.run(&read().args(["rev-parse", "--verify", "-q", "nothing"]));
            assert!(!output.unwrap().status.success());
        })
    });
    group.bench_function("local_write", |b| {
        let write = Invocation::new(JobClass::LocalWrite, &repo.path).args([
            "update-ref",
            "refs/bench/ref",
            "HEAD",
        ]);
        b.iter(|| ok(git.run(&write).unwrap()))
    });
    // Starts upload-pack too, and carries a Stop like every Network Job.
    group.bench_function("network", |b| {
        let path = repo.path.to_str().unwrap();
        let network = Invocation::new(JobClass::Network, &repo.path)
            .args(["ls-remote", path])
            .stop_on(&stop);
        b.iter(|| ok(git.run(&network).unwrap()))
    });
    group.finish();
}

criterion_group!(benches, round_trips);
criterion_main!(benches);
