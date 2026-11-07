//! PROTOTYPE: git process cost and breakage spike for issue #12. Throwaway.
//!
//! `bench` times spawn round trips. `paths` runs the breakage checklist.
//! std only, so it builds unchanged on macOS and Windows.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// CREATE_NO_WINDOW: what a GUI app must pass so no console flashes up.
    NoWindow,
    /// DETACHED_PROCESS: no console at all.
    Detached,
    /// Inherit the parent console. Reference only: a GUI parent has none, so
    /// a console child would open a visible window.
    Inherit,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::NoWindow => "no-window",
            Mode::Detached => "detached",
            Mode::Inherit => "inherit",
        }
    }
}

#[derive(Clone)]
struct Spawner {
    label: String,
    exe: PathBuf,
    pre: Vec<OsString>,
    mode: Mode,
}

impl Spawner {
    fn new(label: &str, exe: impl Into<PathBuf>, mode: Mode) -> Self {
        Spawner { label: format!("{label} {}", mode.name()), exe: exe.into(), pre: vec![], mode }
    }

    fn command(&self) -> Command {
        let mut c = Command::new(&self.exe);
        c.args(&self.pre);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            match self.mode {
                Mode::NoWindow => {
                    c.creation_flags(CREATE_NO_WINDOW);
                }
                Mode::Detached => {
                    c.creation_flags(DETACHED_PROCESS);
                }
                Mode::Inherit => {}
            }
        }
        c
    }
}

#[derive(Clone)]
enum Loc {
    None,
    /// `git -C <path>`
    DashC(PathBuf),
    /// `Command::current_dir(<path>)`
    Cwd(PathBuf),
}

struct Run {
    status: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    spawn_err: Option<String>,
    elapsed: Duration,
}

impl Run {
    fn ok(&self) -> bool {
        self.spawn_err.is_none() && self.status == Some(0)
    }
    fn out(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
    fn why(&self) -> String {
        if let Some(e) = &self.spawn_err {
            return format!("spawn failed: {e}");
        }
        let err = String::from_utf8_lossy(&self.stderr);
        let line = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        format!("exit {:?}: {line}", self.status)
    }
}

fn git<A: AsRef<OsStr>>(sp: &Spawner, loc: &Loc, args: &[A], stdin: Option<&[u8]>) -> Run {
    let mut c = sp.command();
    match loc {
        Loc::None => {}
        Loc::DashC(p) => {
            c.arg("-C").arg(p);
        }
        Loc::Cwd(p) => {
            c.current_dir(p);
        }
    }
    c.args(args);
    c.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let t = Instant::now();
    let child = c.spawn();
    let mut child = match child {
        Ok(ch) => ch,
        Err(e) => {
            return Run {
                status: None,
                stdout: vec![],
                stderr: vec![],
                spawn_err: Some(e.to_string()),
                elapsed: t.elapsed(),
            };
        }
    };
    if let Some(input) = stdin {
        let mut pipe = child.stdin.take().unwrap();
        let _ = pipe.write_all(input);
        drop(pipe);
    }
    match child.wait_with_output() {
        Ok(o) => Run {
            status: o.status.code(),
            stdout: o.stdout,
            stderr: o.stderr,
            spawn_err: None,
            elapsed: t.elapsed(),
        },
        Err(e) => Run {
            status: None,
            stdout: vec![],
            stderr: vec![],
            spawn_err: Some(e.to_string()),
            elapsed: t.elapsed(),
        },
    }
}

/// Identity and safety settings for scratch repos only. Real config otherwise.
const SCRATCH_CFG: &[&str] = &[
    "-c",
    "user.name=Spike",
    "-c",
    "user.email=spike@example.invalid",
    "-c",
    "commit.gpgsign=false",
];

fn scratch_args(rest: &[&str]) -> Vec<String> {
    SCRATCH_CFG.iter().chain(rest.iter()).map(|s| s.to_string()).collect()
}

// ---------------------------------------------------------------- spawners

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        vec![".exe".into(), "".into()]
    } else {
        vec!["".into()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let p = dir.join(format!("{name}{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// The git exes a GUI app could end up running, each with the modes worth timing.
fn spawners(extra: &[PathBuf]) -> (Vec<Spawner>, Spawner) {
    let mut v = Vec::new();
    #[cfg(windows)]
    {
        // A GUI app started from Explorer gets the registry PATH, which on a
        // default Git for Windows install holds only `Git\cmd`, the launcher shim.
        let root = which("git")
            .and_then(|p| {
                p.ancestors()
                    .find(|a| a.join("cmd").join("git.exe").is_file() && a.join("mingw64").is_dir())
                    .map(Path::to_path_buf)
            })
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\Git"));
        let shim = root.join("cmd").join("git.exe");
        let direct = root.join("mingw64").join("bin").join("git.exe");
        v.push(Spawner::new("direct", &direct, Mode::NoWindow));
        v.push(Spawner::new("shim", &shim, Mode::NoWindow));
        v.push(Spawner::new("direct", &direct, Mode::Detached));
        v.push(Spawner::new("direct", &direct, Mode::Inherit));
        v.push(Spawner::new("shim", &shim, Mode::Inherit));
    }
    #[cfg(not(windows))]
    {
        // /usr/bin/git on macOS is an xcrun shim. Time it against the real exe.
        if let Ok(o) = Command::new("xcrun").args(["--find", "git"]).output() {
            let p = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if o.status.success() && !p.is_empty() {
                v.push(Spawner::new("direct", p, Mode::Inherit));
            }
        }
        if Path::new("/usr/bin/git").exists() {
            v.push(Spawner::new("shim", "/usr/bin/git", Mode::Inherit));
        }
        if v.is_empty() {
            v.push(Spawner::new("path", which("git").expect("no git on PATH"), Mode::Inherit));
        }
    }
    for (i, p) in extra.iter().enumerate() {
        let mode = if cfg!(windows) { Mode::NoWindow } else { Mode::Inherit };
        v.push(Spawner::new(&format!("extra{i}"), p, mode));
    }
    let primary = v[0].clone();
    (v, primary)
}

fn noop_spawners() -> Vec<Spawner> {
    let me = std::env::current_exe().unwrap();
    let modes: &[Mode] = if cfg!(windows) { &[Mode::NoWindow, Mode::Inherit] } else { &[Mode::Inherit] };
    modes
        .iter()
        .map(|&m| {
            let mut s = Spawner::new("self-noop", &me, m);
            s.pre = vec!["noop".into()];
            s
        })
        .collect()
}

// ---------------------------------------------------------------- bench

struct Opts {
    iters: usize,
    runs: usize,
    warmup: usize,
    fixtures: PathBuf,
    only: Option<String>,
    extra: Vec<PathBuf>,
    root: Option<PathBuf>,
}

struct Case {
    name: &'static str,
    loc: Loc,
    args: Vec<String>,
    stdin: Option<Vec<u8>>,
    /// Run for every spawner, not just the primary pair.
    all: bool,
}

fn stats(mut d: Vec<Duration>) -> (f64, f64, f64, f64) {
    d.sort();
    let ms = |x: Duration| x.as_secs_f64() * 1000.0;
    let pick = |q: f64| {
        let i = ((q * d.len() as f64).ceil() as usize).clamp(1, d.len()) - 1;
        ms(d[i])
    };
    (ms(d[0]), pick(0.5), pick(0.95), ms(*d.last().unwrap()))
}

fn bench(o: &Opts) {
    let (spawners, primary) = spawners(&o.extra);
    let noops = noop_spawners();
    let small = o.fixtures.join("ripgrep");
    let kernel = o.fixtures.join("linux-graph");
    let root = o.root.clone().unwrap_or_else(|| std::env::temp_dir().join(format!("cairn-spawn-{}", std::process::id())));
    let scratch = root.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();

    let p = &primary;
    let must = |r: Run, what: &str| {
        if !r.ok() {
            panic!("{what}: {}", r.why());
        }
        r
    };
    must(git(p, &Loc::DashC(scratch.clone()), &["init", "-q"], None), "init scratch");
    must(git(p, &Loc::DashC(scratch.clone()), &scratch_args(&["commit", "-q", "--allow-empty", "-m", "root"]), None), "root commit");
    let head = must(git(p, &Loc::DashC(scratch.clone()), &["rev-parse", "HEAD"], None), "rev-parse").out().trim().to_string();

    print_env(&spawners, o);

    let mut cases = vec![Case { name: "version", loc: Loc::None, args: vec!["--version".into()], stdin: None, all: true }];
    if small.is_dir() {
        cases.push(Case { name: "rev-parse small", loc: Loc::DashC(small.clone()), args: vec!["rev-parse".into(), "HEAD".into()], stdin: None, all: true });
        cases.push(Case { name: "status small", loc: Loc::DashC(small.clone()), args: vec!["status".into(), "--porcelain=v2".into(), "-z".into()], stdin: None, all: false });
    } else {
        println!("! no ripgrep fixture at {}", small.display());
    }
    if kernel.is_dir() {
        cases.push(Case { name: "rev-parse kernel", loc: Loc::DashC(kernel.clone()), args: vec!["rev-parse".into(), "HEAD".into()], stdin: None, all: false });
        // Reads the 97k-entry index and HEAD's tree, the read half of a commit. Read-only.
        cases.push(Case { name: "diff-index kernel", loc: Loc::DashC(kernel.clone()), args: vec!["diff-index".into(), "--cached".into(), "--quiet".into(), "HEAD".into()], stdin: None, all: false });
    } else {
        println!("! no kernel fixture at {}", kernel.display());
    }
    cases.push(Case { name: "hash-object stdin", loc: Loc::DashC(scratch.clone()), args: vec!["hash-object".into(), "-w".into(), "--stdin".into()], stdin: Some(b"spike payload\n".to_vec()), all: false });
    cases.push(Case { name: "update-ref", loc: Loc::DashC(scratch.clone()), args: vec!["update-ref".into(), "refs/heads/spike".into(), head.clone()], stdin: None, all: false });
    cases.push(Case { name: "commit scratch", loc: Loc::DashC(scratch.clone()), args: scratch_args(&["commit", "-q", "--allow-empty", "-m", "bench"]), stdin: None, all: false });

    let primary_pair: Vec<&Spawner> = spawners.iter().filter(|s| s.mode == spawners[0].mode).take(2).collect();

    println!("\n{:<20} {:<20} {:>4} {:>7} {:>7} {:>7} {:>7} {:>7}", "case", "spawner", "run", "first", "min", "p50", "p95", "max");
    let mut summary: Vec<(String, String, f64, f64, f64)> = vec![];
    // Spawn baseline first: OS process creation with no git in it.
    let mut rows: Vec<(&str, Vec<&Spawner>, Loc, Vec<String>, Option<Vec<u8>>)> = vec![];
    rows.push(("noop", noops.iter().collect(), Loc::None, vec![], None));
    for c in &cases {
        let sps: Vec<&Spawner> = if c.all { spawners.iter().collect() } else { primary_pair.clone() };
        rows.push((c.name, sps, c.loc.clone(), c.args.clone(), c.stdin.clone()));
    }
    for (name, sps, loc, args, stdin) in &rows {
        if let Some(f) = &o.only {
            if !name.contains(f.as_str()) {
                continue;
            }
        }
        for sp in sps {
            let mut worst = (0.0f64, 0.0f64, 0.0f64);
            for run in 1..=o.runs {
                let first = git(sp, loc, args, stdin.as_deref());
                if !first.ok() {
                    println!("{:<20} {:<20} FAILED {}", name, sp.label, first.why());
                    break;
                }
                for _ in 0..o.warmup {
                    git(sp, loc, args, stdin.as_deref());
                }
                let mut d = Vec::with_capacity(o.iters);
                for _ in 0..o.iters {
                    let r = git(sp, loc, args, stdin.as_deref());
                    assert!(r.ok(), "{name} {}: {}", sp.label, r.why());
                    d.push(r.elapsed);
                }
                let (mn, p50, p95, mx) = stats(d);
                println!("{:<20} {:<20} {:>4} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>7.2}", name, sp.label, run, first.elapsed.as_secs_f64() * 1000.0, mn, p50, p95, mx);
                worst = (worst.0.max(p50), worst.1.max(p95), worst.2.max(mx));
            }
            summary.push((name.to_string(), sp.label.clone(), worst.0, worst.1, worst.2));
        }
    }

    if o.only.as_deref().is_none_or(|f| "parallel".contains(f)) && small.is_dir() {
        parallel(&primary, &small, o);
    }

    println!("\nSummary, worst of {} runs ({} spawns each), ms", o.runs, o.iters);
    println!("{:<20} {:<20} {:>7} {:>7} {:>7}", "case", "spawner", "p50", "p95", "max");
    for (c, s, a, b, m) in summary {
        println!("{:<20} {:<20} {:>7.2} {:>7.2} {:>7.2}", c, s, a, b, m);
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// Background refresh fires several reads at once. 8 threads, rev-parse each.
fn parallel(sp: &Spawner, repo: &Path, o: &Opts) {
    let threads = 8;
    let per = (o.iters / 4).max(10);
    let loc = Loc::DashC(repo.to_path_buf());
    println!("\nparallel: {threads} threads x {per} rev-parse small, {}", sp.label);
    for run in 1..=o.runs {
        let t = Instant::now();
        let all: Vec<Duration> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..threads)
                .map(|_| {
                    s.spawn(|| {
                        (0..per)
                            .map(|_| {
                                let r = git(sp, &loc, &["rev-parse", "HEAD"], None);
                                assert!(r.ok(), "{}", r.why());
                                r.elapsed
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
        });
        let wall = t.elapsed().as_secs_f64() * 1000.0;
        let n = all.len();
        let (mn, p50, p95, mx) = stats(all);
        println!(
            "  run {run}: wall {wall:.0} ms, {:.0} spawns/s, latency min {mn:.2} p50 {p50:.2} p95 {p95:.2} max {mx:.2}",
            n as f64 / (wall / 1000.0)
        );
    }
}

fn print_env(spawners: &[Spawner], o: &Opts) {
    println!("# git spawn bench");
    println!("os: {} {}", std::env::consts::OS, std::env::consts::ARCH);
    println!("iters {} warmup {} runs {}", o.iters, o.warmup, o.runs);
    for s in spawners {
        let v = git(s, &Loc::None, &["--version"], None);
        println!("spawner {:<20} {}  ({})", s.label, s.exe.display(), v.out().trim());
    }
    #[cfg(windows)]
    {
        let ps = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "$s=Get-MpComputerStatus; \"defender: mode=$($s.AMRunningMode) realtime=$($s.RealTimeProtectionEnabled) onaccess=$($s.OnAccessProtectionEnabled) behavior=$($s.BehaviorMonitorEnabled) tamper=$($s.IsTamperProtected)\"; \"LongPathsEnabled=$((Get-ItemProperty HKLM:\\SYSTEM\\CurrentControlSet\\Control\\FileSystem).LongPathsEnabled)\"",
            ])
            .output();
        if let Ok(o) = ps {
            print!("{}", String::from_utf8_lossy(&o.stdout));
        }
    }
}

// ---------------------------------------------------------------- paths

struct Checks {
    rows: Vec<(String, String, String, String)>,
}

impl Checks {
    fn add(&mut self, scen: &str, step: &str, verdict: &str, detail: impl Into<String>) {
        let detail = detail.into();
        println!("{verdict:<5} {scen:<34} {step:<34} {detail}");
        self.rows.push((verdict.into(), scen.into(), step.into(), detail));
    }
    /// Record a git run. Returns whether it succeeded.
    fn run(&mut self, scen: &str, step: &str, r: &Run) -> bool {
        if r.ok() {
            self.add(scen, step, "PASS", format!("{:.1} ms", r.elapsed.as_secs_f64() * 1000.0));
            true
        } else {
            self.add(scen, step, "FAIL", r.why());
            false
        }
    }
    fn expect(&mut self, scen: &str, step: &str, ok: bool, detail: impl Into<String>) -> bool {
        self.add(scen, step, if ok { "PASS" } else { "FAIL" }, detail);
        ok
    }
}

fn paths(o: &Opts) {
    let (spawners, primary) = spawners(&o.extra);
    let root = o.root.clone().unwrap_or_else(|| std::env::temp_dir().join(format!("cairn-paths-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    print_env(&spawners, &Opts { iters: 0, runs: 0, warmup: 0, ..o.clone_light() });
    println!("root: {} ({} chars)\n", root.display(), root.as_os_str().len());
    let mut ck = Checks { rows: vec![] };

    // Shim and direct both, since the shim rebuilds the command line.
    let quoting: Vec<&Spawner> = spawners.iter().filter(|s| s.mode == primary.mode).take(2).collect();

    for loc_kind in ["-C", "cwd"] {
        spaces(&mut ck, &primary, &root, loc_kind);
    }
    for kind in ["-C", "cwd", "-C longpaths", "cwd longpaths"] {
        long_repo(&mut ck, &primary, &root, kind);
    }
    long_file(&mut ck, &primary, &root);
    for sp in quoting {
        quoting_checks(&mut ck, sp, &root);
    }

    let fails = ck.rows.iter().filter(|r| r.0 == "FAIL").count();
    println!("\n{} checks, {} failed", ck.rows.len(), fails);
    if let Err(e) = std::fs::remove_dir_all(&root) {
        println!("! cleanup of {} failed: {e}", root.display());
    }
}

impl Opts {
    fn clone_light(&self) -> Opts {
        Opts {
            iters: self.iters,
            runs: self.runs,
            warmup: self.warmup,
            fixtures: self.fixtures.clone(),
            only: self.only.clone(),
            extra: self.extra.clone(),
            root: self.root.clone(),
        }
    }
}

fn loc_for(kind: &str, p: &Path) -> Loc {
    if kind.starts_with("cwd") { Loc::Cwd(p.to_path_buf()) } else { Loc::DashC(p.to_path_buf()) }
}

/// Repo under a directory with spaces, umlauts and CJK, file names likewise.
fn spaces(ck: &mut Checks, sp: &Spawner, root: &Path, kind: &str) {
    let scen = format!("spaces+unicode repo, {kind}");
    let repo = root.join("dir with spaces").join("Ümläut ß 日本").join(format!("repo {}", kind.replace('-', "")));
    std::fs::create_dir_all(&repo).unwrap();
    let loc = loc_for(kind, &repo);
    let file = "file with spaces ä 🎉.txt";
    if !ck.run(&scen, "init", &git(sp, &loc, &["init", "-q"], None)) {
        return;
    }
    std::fs::write(repo.join(file), "hello\n").unwrap();
    ck.run(&scen, "add -- <file>", &git(sp, &loc, &["add", "--", file], None));
    ck.run(&scen, "commit", &git(sp, &loc, &scratch_args(&["commit", "-q", "-m", "first"]), None));
    let st = git(sp, &loc, &["status", "--porcelain=v2", "-z"], None);
    ck.expect(&scen, "status -z clean", st.ok() && st.stdout.is_empty(), st.out());
    let log = git(sp, &loc, &["log", "-1", "--name-only", "-z", "--format="], None);
    ck.expect(&scen, "log -z returns UTF-8 name", log.ok() && log.out().contains(file), format!("{:?}", log.out()));
    std::fs::write(repo.join(file), "changed\n").unwrap();
    let st = git(sp, &loc, &["status", "--porcelain=v2"], None);
    let quoted = st.out().contains("\\303");
    ck.add(&scen, "status without -z", "INFO", if quoted { "name is octal-escaped (core.quotepath): parse with -z" } else { "name unescaped" });
    let st = git(sp, &loc, &["status", "--porcelain=v2", "-z"], None);
    ck.expect(&scen, "status -z sees edit", st.out().contains(file), format!("{:?}", st.out()));
    ck.run(&scen, "restore -- <file>", &git(sp, &loc, &["restore", "--", file], None));
    let back = std::fs::read_to_string(repo.join(file)).unwrap_or_default();
    ck.expect(&scen, "restore wrote file", back.replace('\r', "") == "hello\n", format!("{back:?}"));
    ck.run(&scen, "switch -c feature/ümlaut", &git(sp, &loc, &["switch", "-q", "-c", "feature/ümlaut"], None));
    let b = git(sp, &loc, &["branch", "--show-current"], None);
    ck.expect(&scen, "branch name round-trips", b.out().trim() == "feature/ümlaut", b.out().trim().to_string());
}

/// A path that crosses MAX_PATH (260) by a clear margin.
fn deep(base: &Path, min_len: usize, seg: &str) -> PathBuf {
    let mut p = base.to_path_buf();
    let mut i = 0;
    while p.as_os_str().len() < min_len {
        p = p.join(format!("{seg}{i:02}"));
        i += 1;
    }
    p
}

fn long_repo(ck: &mut Checks, sp: &Spawner, root: &Path, kind: &str) {
    let tag = kind.replace([' ', '-'], "");
    let repo = deep(&root.join(format!("long-{tag}")), 290, "segment-of-forty-characters-padding-");
    let scen = format!("repo path {} chars, {kind}", repo.as_os_str().len());
    if let Err(e) = std::fs::create_dir_all(&repo) {
        ck.add(&scen, "create dir (Rust)", "FAIL", e.to_string());
        return;
    }
    // `-c` goes before `-C`, so the setting is in place before git changes directory.
    let mut sp = sp.clone();
    if kind.ends_with("longpaths") {
        sp.pre = vec!["-c".into(), "core.longpaths=true".into()];
    }
    let sp = &sp;
    let a = |rest: &[&str]| -> Vec<String> { rest.iter().map(|s| s.to_string()).collect() };
    let loc = loc_for(kind, &repo);
    if !ck.run(&scen, "init", &git(sp, &loc, &a(&["init", "-q"]), None)) {
        return;
    }
    if kind.ends_with("longpaths") {
        // Persist it so later commands without -c also see it.
        git(sp, &loc, &a(&["config", "core.longpaths", "true"]), None);
    }
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    ck.run(&scen, "add", &git(sp, &loc, &a(&["add", "a.txt"]), None));
    let mut c = vec!["commit", "-q", "-m", "x"];
    c.splice(0..0, SCRATCH_CFG.iter().copied());
    ck.run(&scen, "commit", &git(sp, &loc, &a(&c), None));
    let st = git(sp, &loc, &a(&["status", "--porcelain=v2", "-z"]), None);
    ck.expect(&scen, "status clean", st.ok() && st.stdout.is_empty(), if st.ok() { st.out() } else { st.why() });
}

fn long_file(ck: &mut Checks, sp: &Spawner, root: &Path) {
    let repo = root.join("lf");
    std::fs::create_dir_all(&repo).unwrap();
    let file_dir = deep(&repo, 280, "nested-directory-name-for-depth-");
    let file = file_dir.join("deep.txt");
    let rel = file.strip_prefix(&repo).unwrap().to_string_lossy().replace('\\', "/");
    let scen = format!("file path {} chars, short repo", file.as_os_str().len());
    let loc = Loc::DashC(repo.clone());
    ck.run(&scen, "init", &git(sp, &loc, &["init", "-q"], None));
    std::fs::create_dir_all(&file_dir).unwrap();
    std::fs::write(&file, "deep\n").unwrap();

    let r = git(sp, &loc, &["add", "--", &rel], None);
    ck.add(&scen, "add, longpaths unset", if r.ok() { "PASS" } else { "FAIL" }, if r.ok() { "ok".into() } else { r.why() });
    let r = git(sp, &loc, &["status", "--porcelain=v2", "-z", "-uall"], None);
    ck.add(&scen, "status -uall, longpaths unset", if r.ok() && r.out().contains("deep.txt") { "PASS" } else { "FAIL" }, if r.ok() { format!("{} bytes, mentions file: {}", r.stdout.len(), r.out().contains("deep.txt")) } else { r.why() });

    let lp = |rest: &[&str]| -> Vec<String> { ["-c", "core.longpaths=true"].iter().chain(rest.iter()).map(|s| s.to_string()).collect() };
    ck.run(&scen, "add, -c core.longpaths=true", &git(sp, &loc, &lp(&["add", "--", &rel]), None));
    ck.run(&scen, "commit, longpaths", &git(sp, &loc, &lp(&scratch_args(&["commit", "-q", "-m", "deep"]).iter().map(String::as_str).collect::<Vec<_>>()), None));
    let st = git(sp, &loc, &lp(&["status", "--porcelain=v2", "-z"]), None);
    ck.expect(&scen, "status clean, longpaths", st.ok() && st.stdout.is_empty(), if st.ok() { st.out() } else { st.why() });
    std::fs::remove_file(&file).unwrap();
    ck.run(&scen, "checkout -- . restores, longpaths", &git(sp, &loc, &lp(&["checkout", "--", "."]), None));
    ck.expect(&scen, "file back on disk", file.is_file(), "");

    let st = git(sp, &loc, &["status", "--porcelain=v2", "-z"], None);
    ck.add(&scen, "status, longpaths unset (tracked)", if st.ok() && st.stdout.is_empty() { "PASS" } else { "FAIL" }, if st.ok() { format!("{:?}", st.out()) } else { st.why() });

    for (label, cfg) in [("clone, longpaths unset", false), ("clone, -c core.longpaths=true", true)] {
        let dst = root.join(if cfg { "lf-clone-lp" } else { "lf-clone" });
        let mut args: Vec<String> = vec![];
        if cfg {
            args.extend(["-c".into(), "core.longpaths=true".into()]);
        }
        args.extend(["clone".into(), "-q".into(), repo.to_string_lossy().into_owned(), dst.to_string_lossy().into_owned()]);
        let r = git(sp, &Loc::None, &args, None);
        let there = dst.join(file.strip_prefix(&repo).unwrap()).is_file();
        ck.add(&scen, label, if r.ok() && there { "PASS" } else { "FAIL" }, if r.ok() { format!("file checked out: {there}") } else { r.why() });
    }
}

fn quoting_checks(ck: &mut Checks, sp: &Spawner, root: &Path) {
    let scen = format!("arg quoting, {}", sp.label);
    let repo = root.join(format!("q-{}", sp.label.replace(' ', "-")));
    std::fs::create_dir_all(&repo).unwrap();
    let loc = Loc::DashC(repo.clone());
    ck.run(&scen, "init", &git(sp, &loc, &["init", "-q"], None));
    let msgs = [
        "plain",
        r#"say "hi" there"#,
        r"back\slash and \\double",
        r"trailing backslash\",
        r#"backslash before quote \" end"#,
        r#"ends with quote""#,
        "ümlaut ß 日本 🎉",
        "two\nlines",
        "%PATH% $HOME !bang! ^caret &amp |pipe <lt >gt",
        "-starts with dash",
        "  leading spaces",
        "tab\there",
    ];
    for m in msgs {
        let c = git(sp, &loc, &scratch_args(&["commit", "-q", "--allow-empty", "-m", m]), None);
        if !c.ok() {
            ck.add(&scen, &format!("-m {m:?}"), "FAIL", c.why());
            continue;
        }
        let back = git(sp, &loc, &["log", "-1", "--format=%B"], None).out();
        let back = back.trim_end_matches('\n');
        ck.expect(&scen, &format!("-m {m:?}"), back == m, if back == m { String::new() } else { format!("got {back:?}") });
    }
    let body = "via stdin ümlaut\n\nbody 🎉 \"quoted\"";
    let c = git(sp, &loc, &scratch_args(&["commit", "-q", "--allow-empty", "-F", "-"]), Some(body.as_bytes()));
    let back = git(sp, &loc, &["log", "-1", "--format=%B"], None).out();
    ck.expect(&scen, "-F - (stdin) round-trips", c.ok() && back.trim_end_matches('\n') == body, if c.ok() { format!("{:?}", back.trim_end()) } else { c.why() });

    for name in ["-dash.txt", "semi;colon & amp.txt", "%PATH%.txt", "[brackets].txt", "quote'single.txt"] {
        std::fs::write(repo.join(name), "x\n").unwrap();
        let r = git(sp, &loc, &["add", "--", name], None);
        let ls = git(sp, &loc, &["ls-files", "-z", "--", name], None);
        let listed = ls.out().split('\0').any(|n| n == name);
        ck.expect(&scen, &format!("add -- {name:?}"), r.ok() && listed, if r.ok() { format!("indexed: {listed}") } else { r.why() });
    }
}

// ---------------------------------------------------------------- console

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetConsoleWindow() -> isize;
}
#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn IsWindowVisible(hwnd: isize) -> i32;
}

/// Run by a hook: appends whether this grandchild has a console and whether it shows.
fn probe(out: &str) {
    #[cfg(windows)]
    let line = unsafe {
        let h = GetConsoleWindow();
        format!("console={} visible={}\n", h != 0, h != 0 && IsWindowVisible(h) != 0)
    };
    #[cfg(not(windows))]
    let line = "console=n/a visible=n/a\n".to_string();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(out).unwrap();
    f.write_all(line.as_bytes()).unwrap();
}

/// What git's own children see under each creation flag, and what commit costs
/// with auto-maintenance on and off.
fn console(o: &Opts) {
    let (spawners, primary) = spawners(&o.extra);
    let root = o.root.clone().unwrap_or_else(|| std::env::temp_dir().join(format!("cairn-console-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&root);
    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join(".git").join("hooks")).unwrap();
    let loc = Loc::DashC(repo.clone());
    assert!(git(&primary, &loc, &["init", "-q"], None).ok());
    let me = std::env::current_exe().unwrap().to_string_lossy().replace('\\', "/");
    let out = root.join("probe.txt");
    let out_s = out.to_string_lossy().replace('\\', "/");
    std::fs::write(repo.join(".git/hooks/post-commit"), format!("#!/bin/sh\n\"{me}\" probe \"{out_s}\"\n")).unwrap();

    println!("# what a post-commit hook (sh -> probe exe) sees");
    for sp in &spawners {
        let _ = std::fs::remove_file(&out);
        let r = git(sp, &loc, &scratch_args(&["commit", "-q", "--allow-empty", "-m", "probe"]), None);
        let seen = std::fs::read_to_string(&out).unwrap_or_else(|_| "hook did not run\n".into());
        println!("{:<20} commit {:<6} hook: {}", sp.label, if r.ok() { "ok" } else { "FAILED" }, seen.trim());
    }
    std::fs::remove_file(repo.join(".git/hooks/post-commit")).unwrap();

    println!("\n# commit cost, auto-maintenance on vs off, {} spawns, ms", o.iters);
    println!("{:<20} {:<16} {:>7} {:>7} {:>7}", "spawner", "maintenance", "p50", "p95", "max");
    for sp in spawners.iter().take(3) {
        for (label, extra) in [("auto (default)", vec![]), ("auto=false", vec!["-c", "maintenance.auto=false"])] {
            let mut args: Vec<&str> = extra.clone();
            args.extend(["commit", "-q", "--allow-empty", "-m", "x"]);
            let args = scratch_args(&args);
            for _ in 0..o.warmup {
                git(sp, &loc, &args, None);
            }
            let d: Vec<Duration> = (0..o.iters).map(|_| git(sp, &loc, &args, None).elapsed).collect();
            let (_, p50, p95, mx) = stats(d);
            println!("{:<20} {:<16} {:>7.2} {:>7.2} {:>7.2}", sp.label, label, p50, p95, mx);
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------- main

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = if args.first().is_some_and(|a| !a.starts_with("--")) { args.remove(0) } else { "bench".into() };
    if cmd == "noop" {
        return;
    }
    if cmd == "probe" {
        probe(&args[0]);
        return;
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
    let mut o = Opts { iters: 100, runs: 3, warmup: 5, fixtures: home.join("cairn-fixtures"), only: None, extra: vec![], root: None };
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut v = || it.next().unwrap_or_else(|| panic!("{a} needs a value"));
        match a.as_str() {
            "--iters" => o.iters = v().parse().unwrap(),
            "--runs" => o.runs = v().parse().unwrap(),
            "--warmup" => o.warmup = v().parse().unwrap(),
            "--fixtures" => o.fixtures = v().into(),
            "--only" => o.only = Some(v()),
            "--git" => o.extra.push(v().into()),
            "--root" => o.root = Some(v().into()),
            _ => panic!("unknown flag {a}"),
        }
    }
    match cmd.as_str() {
        "bench" => bench(&o),
        "paths" => paths(&o),
        "console" => console(&o),
        _ => panic!("unknown command {cmd}: bench | paths | console"),
    }
}
