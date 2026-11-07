//! PROTOTYPE: throwaway spike for issue #11. Not part of Cairn.
//!
//! Times gix history walks and status on the Linux kernel fixture against the gix
//! Floors in #8, times `git` on the same cases, and checks status and hunk parity
//! against `git` on the kernel and on a small autocrlf and filters fixture.

mod filters;
mod hunks;
mod status;
mod walk;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

pub struct Args {
    pub repo: PathBuf,
    pub git: PathBuf,
    pub runs: usize,
    pub warmup: usize,
    pub only: Option<String>,
    pub fixture_dir: PathBuf,
}

fn main() -> Result<()> {
    let mut argv = std::env::args().skip(1);
    let cmd = argv.next().unwrap_or_else(|| "help".into());
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut args = Args {
        repo: home.join("cairn-fixtures").join("linux-graph"),
        git: "git".into(),
        runs: 3,
        warmup: 1,
        only: None,
        fixture_dir: PathBuf::from("results").join("filters-fixture"),
    };
    while let Some(flag) = argv.next() {
        let mut value = || argv.next().with_context(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--repo" => args.repo = value()?.into(),
            "--git" => args.git = value()?.into(),
            "--runs" => args.runs = value()?.parse()?,
            "--only" => args.only = Some(value()?),
            "--fixture-dir" => args.fixture_dir = value()?.into(),
            "--object-cache-mb" => {
                let mb: usize = value()?.parse()?;
                OBJECT_CACHE_BYTES.store(mb * 1024 * 1024, std::sync::atomic::Ordering::Relaxed);
            }
            "--cold" => {
                args.runs = 1;
                args.warmup = 0;
            }
            other => bail!("unknown flag {other}"),
        }
    }

    header(&args)?;
    match cmd.as_str() {
        "walk" => walk::run(&args),
        "status" => status::run(&args),
        "filters" => filters::run(&args),
        "all" => {
            walk::run(&args)?;
            status::run(&args)?;
            filters::run(&args)
        }
        "restore" => status::restore_from_manifest(&args),
        _ => {
            println!(
                "usage: gix-timings-prototype <walk|status|filters|all|restore> \
                 [--repo PATH] [--git PATH] [--runs N] [--cold] [--only CASE] [--fixture-dir PATH] [--object-cache-mb N]"
            );
            Ok(())
        }
    }
}

fn header(args: &Args) -> Result<()> {
    let git_version = git_stdout(&args.git, &args.repo, &["--version"]).unwrap_or_default();
    println!("# gix timings spike (#11)");
    println!("os: {} {}", std::env::consts::OS, std::env::consts::ARCH);
    println!("repo: {}", args.repo.display());
    println!("gix: main at bc4d813");
    println!("git: {}", String::from_utf8_lossy(&git_version).trim());
    println!(
        "runs: {} timed after {} warm-up, worst reported{}",
        args.runs,
        args.warmup,
        if args.warmup == 0 { " (COLD, recorded only)" } else { "" }
    );
    let cache = OBJECT_CACHE_BYTES.load(std::sync::atomic::Ordering::Relaxed);
    if cache > 0 {
        println!("object cache: {} MB (DIAGNOSTIC, not a Floor run)", cache / 1024 / 1024);
    }
    println!(
        "threads: {}",
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0)
    );
    println!();
    Ok(())
}

pub fn wants(args: &Args, case: &str) -> bool {
    args.only.as_deref().is_none_or(|only| case.contains(only))
}

/// Timing of one case: every timed run, in order.
pub struct Sample(pub Vec<Duration>);

impl Sample {
    pub fn worst(&self) -> Duration {
        self.0.iter().copied().max().unwrap_or_default()
    }
    pub fn runs(&self) -> String {
        self.0.iter().map(|d| ms(*d)).collect::<Vec<_>>().join(", ")
    }
}

/// Runs `f` for the warm-up count, then `runs` times timed. Returns the last result.
pub fn measure<T>(args: &Args, mut f: impl FnMut() -> Result<(Duration, T)>) -> Result<(Sample, T)> {
    for _ in 0..args.warmup {
        f()?;
    }
    let mut times = Vec::new();
    let mut last = None;
    for _ in 0..args.runs {
        let (t, v) = f()?;
        times.push(t);
        last = Some(v);
    }
    Ok((Sample(times), last.expect("runs >= 1")))
}

/// Times a `git` command in `repo`, reading all of its stdout as a consumer would.
pub fn time_git(args: &Args, repo: &Path, git_args: &[&str]) -> Result<(Sample, Vec<u8>)> {
    measure(args, || {
        let start = Instant::now();
        let out = git_output(&args.git, repo, git_args)?;
        Ok((start.elapsed(), out))
    })
}

pub fn git_output(git: &Path, repo: &Path, git_args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new(git)
        .arg("-C")
        .arg(repo)
        .args(git_args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("spawning git {git_args:?}"))?;
    if !out.status.success() {
        bail!(
            "git {git_args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

pub fn git_stdout(git: &Path, repo: &Path, git_args: &[&str]) -> Result<Vec<u8>> {
    git_output(git, repo, git_args)
}

pub fn git_run(git: &Path, repo: &Path, git_args: &[&str]) -> Result<()> {
    git_output(git, repo, git_args).map(drop)
}

/// Opens a repository the way Cairn's plan says to: with the git binary's own config.
pub fn open(path: &Path) -> Result<gix::Repository> {
    let mut opts = gix::open::Options::default();
    opts.permissions.config.git_binary = true;
    let mut repo = gix::open_opts(path, opts)?;
    let cache = OBJECT_CACHE_BYTES.load(std::sync::atomic::Ordering::Relaxed);
    if cache > 0 {
        repo.object_cache_size(cache);
    }
    Ok(repo)
}

/// Diagnostic only: an object cache size set by `--object-cache-mb`. Floors use gix's default.
static OBJECT_CACHE_BYTES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn ms(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else {
        format!("{ms:.1} ms")
    }
}

pub fn verdict(worst: Duration, floor: Option<Duration>) -> &'static str {
    match floor {
        None => "recorded",
        Some(f) if worst <= f => "PASS",
        Some(_) => "FAIL",
    }
}

pub fn table_header() {
    println!(
        "| {:<34} | {:>10} | {:>10} | {:>10} | {:<8} | {:<26} |",
        "Case", "Floor", "gix worst", "git worst", "Result", "gix runs"
    );
    println!("|{:-<36}|{:->12}|{:->12}|{:->12}|{:-<10}|{:-<28}|", "", "", "", "", "", "");
}

pub fn table_row(case: &str, floor: Option<Duration>, gix: &Sample, git: Option<&Sample>) {
    println!(
        "| {:<34} | {:>10} | {:>10} | {:>10} | {:<8} | {:<26} |",
        case,
        floor.map(ms).unwrap_or_else(|| "-".into()),
        ms(gix.worst()),
        git.map(|g| ms(g.worst())).unwrap_or_else(|| "-".into()),
        verdict(gix.worst(), floor),
        gix.runs()
    );
}
