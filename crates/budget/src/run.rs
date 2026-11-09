//! Measuring: starts the real Cairn, makes a change, and times it to the first presented frame
//! that shows the change.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{JoinHandle, sleep};
use std::time::{Duration, Instant};

use crate::clock;
use crate::fixture::{BRANCH_A, BRANCH_B, KEEP, SAVED, Sandbox};
use crate::presented::{MetalTrace, PresentMonTrace, PresentSource, largest_gap_ms};
use crate::probe::{ANIMATE, ENV, Expect, Frame, Line, Trace};
use crate::report::{Environment, Row};
use crate::spawn;
use crate::stats::{self, HeldKeyRun};

/// Samples each run throws away first, so that the OS file cache is warm for the ones it keeps.
pub(crate) const WARM_UP_SAMPLES: usize = 3;
/// How long the screen has to stay still before the next sample starts, so that one sample's
/// late refreshes can't land in the next.
pub(crate) const QUIET: Duration = Duration::from_millis(250);
pub(crate) const SAMPLE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a launch stays up on Windows, where its first frame is found only afterwards. Well
/// past the Budget's cap, so that a slow first frame reads as a miss rather than as missing.
const LAUNCH_HOLD: Duration = Duration::from_secs(1);
/// #8: zed#61469 was reported after 5 to 30 held keys, so longer holds add nothing.
const HOLD: Duration = Duration::from_secs(10);
/// How often the held-key test changes a file, for the watcher to notify about.
const CHURN: Duration = Duration::from_millis(8);
const PRESENTMON_SESSION: &str = "CairnBudget";
/// Why a run stops when the window loses focus: its numbers would measure the window manager.
const LOST_FOCUS: &str = "Cairn's window lost focus during a sample, so GPUI throttled it and \
                          the numbers can't count: keep it in front and leave the machine alone";

/// How a harness run is set up.
#[derive(Clone, Debug)]
pub struct Config {
    /// The `cairn` binary, built with the `measure` feature.
    pub cairn: PathBuf,
    /// PresentMon's console exe. Windows only.
    pub presentmon: Option<PathBuf>,
    /// Holds `ripgrep` and `linux-graph`.
    pub fixtures: PathBuf,
    pub runs: usize,
    pub samples: usize,
}

/// A running Cairn, and everything it has said.
struct Cairn {
    child: Child,
    stdin: ChildStdin,
    trace: Arc<Mutex<Trace>>,
    reader: JoinHandle<()>,
}

impl Cairn {
    fn start(config: &Config, repo: &Path, animate: bool) -> Result<Self, String> {
        let mut child = spawn::tool(&config.cairn)?
            .arg(repo)
            .env(ENV, if animate { ANIMATE } else { "1" })
            // `tracing` writes to stdout, which is the harness's pipe. It's also work a window
            // run from the Dock has nowhere to send.
            .env("RUST_LOG", "off")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("couldn't start {}: {e}", config.cairn.display()))?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let trace = Arc::new(Mutex::new(Trace::default()));
        let recording = trace.clone();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(line) = Line::parse(&line) {
                    recording.lock().unwrap().record(line);
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            trace,
            reader,
        })
    }

    /// Checks that this is a build whose numbers can count.
    fn check_build(&self) -> Result<(), String> {
        let release = self
            .wait("the probe's first line", SAMPLE_TIMEOUT, |t| t.release)
            .map_err(|e| {
                format!("{e}. Build it with `--features measure`, as scripts/budget.sh does")
            })?;
        if !release {
            return Err("cairn is a debug build, and numbers count only from release".to_owned());
        }
        Ok(())
    }

    fn wait<T>(
        &self,
        what: &str,
        timeout: Duration,
        mut found: impl FnMut(&Trace) -> Option<T>,
    ) -> Result<T, String> {
        let deadline = Instant::now() + timeout;
        loop {
            {
                let trace = self.trace.lock().unwrap();
                if let Some(value) = found(&trace) {
                    return Ok(value);
                }
                if Instant::now() >= deadline {
                    let last = trace.frames().last();
                    return Err(format!(
                        "timed out waiting for {what}. Last frame: {last:?}"
                    ));
                }
            }
            sleep(Duration::from_millis(1));
        }
    }

    /// Waits until no Slot has answered for [`QUIET`]. By what the frames show rather than
    /// whether they keep coming, because a window can be drawing all the time.
    fn settle(&self) -> Result<(), String> {
        let shown = |trace: &Trace| {
            trace
                .frames()
                .last()
                .map(|f| (f.refs, f.staged, f.unstaged, f.untracked))
        };
        let mut seen = shown(&self.trace.lock().unwrap());
        let mut since = Instant::now();
        let deadline = since + SAMPLE_TIMEOUT;
        while since.elapsed() < QUIET {
            sleep(Duration::from_millis(5));
            let now = shown(&self.trace.lock().unwrap());
            if now != seen {
                seen = now;
                since = Instant::now();
            }
            if Instant::now() >= deadline {
                return Err("the Slots never stopped answering".to_owned());
            }
        }
        Ok(())
    }

    fn send(&mut self, command: &str) -> Result<(), String> {
        writeln!(self.stdin, "{command}")
            .and_then(|()| self.stdin.flush())
            .map_err(|e| format!("cairn has gone away: {e}"))
    }

    /// Quits through the app's own Quit, and returns all it said.
    fn quit(mut self) -> Result<Trace, String> {
        let _ = self.send("quit");
        let deadline = Instant::now() + SAMPLE_TIMEOUT;
        loop {
            match self.child.try_wait().map_err(|e| e.to_string())? {
                Some(_) => break,
                None if Instant::now() >= deadline => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return Err("cairn didn't quit".to_owned());
                }
                None => sleep(Duration::from_millis(10)),
            }
        }
        let _ = self.reader.join();
        Arc::try_unwrap(self.trace)
            .map(|trace| trace.into_inner().unwrap())
            .map_err(|_| "the trace is still shared".to_owned())
    }
}

/// Stops PresentMon if a run ends in an error, when nothing else would. Cairn needs no such thing:
/// it quits when the harness closes its stdin.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Where a run's frame times come from.
enum Recorder {
    Metal,
    PresentMon {
        exe: PathBuf,
        child: KillOnDrop,
        csv: PathBuf,
        _dir: tempfile::TempDir,
    },
}

impl Recorder {
    /// Whether Cairn must keep drawing for this source to time its frames. In an idle window, and
    /// for the first few frames after one, Metal reports a presented time of 0 even though the
    /// window is on screen. PresentMon sees every frame from outside.
    fn needs_animation(&self) -> bool {
        matches!(self, Recorder::Metal)
    }

    /// Starts recording. On Windows that's a PresentMon session, before Cairn starts so that its
    /// first frame is in the trace. A launch PresentMon misses is an error, never a skip.
    fn start(config: &Config) -> Result<Self, String> {
        if cfg!(target_os = "macos") {
            return Ok(Recorder::Metal);
        }
        if !cfg!(windows) {
            return Err("numbers count only from macOS and the physical Windows PC".to_owned());
        }
        let exe = config.presentmon.clone().ok_or(
            "on Windows the harness needs PresentMon: pass --presentmon or set CAIRN_PRESENTMON",
        )?;
        let name = config
            .cairn
            .file_name()
            .ok_or("the cairn path has no file name")?;
        let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
        let csv = dir.path().join("presentmon.csv");
        let child = spawn::tool(&exe)?
            .arg("--process_name")
            .arg(name)
            // Its flips say when a DirectComposition frame reached the screen.
            .args(["--process_name", crate::presented::DWM])
            .arg("--output_file")
            .arg(&csv)
            .args(["--session_name", PRESENTMON_SESSION])
            .args([
                "--no_console_stats",
                "--stop_existing_session",
                "--qpc_time",
            ])
            .stdout(Stdio::null())
            .spawn()
            .map_err(|e| format!("couldn't start PresentMon at {}: {e}", exe.display()))?;
        // ETW takes a moment to start tracing, and PresentMon doesn't say when it has.
        sleep(Duration::from_secs(2));
        Ok(Recorder::PresentMon {
            exe,
            child: KillOnDrop(child),
            csv,
            _dir: dir,
        })
    }

    /// Ends recording and returns the trace of the whole session.
    fn finish(self) -> Result<Option<PresentMonTrace>, String> {
        let Recorder::PresentMon {
            exe,
            mut child,
            csv,
            _dir,
        } = self
        else {
            return Ok(None);
        };
        spawn::tool(&exe)?
            .args([
                "--session_name",
                PRESENTMON_SESSION,
                "--terminate_existing_session",
            ])
            .stdout(Stdio::null())
            .status()
            .map_err(|e| format!("couldn't stop PresentMon: {e}"))?;
        let deadline = Instant::now() + SAMPLE_TIMEOUT;
        while child.0.try_wait().map_err(|e| e.to_string())?.is_none() {
            if Instant::now() >= deadline {
                return Err("PresentMon didn't stop".to_owned());
            }
            sleep(Duration::from_millis(50));
        }
        let text = std::fs::read_to_string(&csv)
            .map_err(|e| format!("PresentMon left no CSV at {}: {e}", csv.display()))?;
        PresentMonTrace::parse(&text, clock::ticks_per_second()).map(Some)
    }
}

/// One process's frame times.
fn source_for(
    trace: &Trace,
    presentmon: Option<&PresentMonTrace>,
) -> Result<Box<dyn PresentSource>, String> {
    match presentmon {
        None => Ok(Box::new(MetalTrace::new(trace.drawables()))),
        Some(whole) => {
            let pid = trace.pid.ok_or("the probe never said its process id")?;
            Ok(Box::new(whole.of_process(pid)))
        }
    }
}

/// What a sample does to the repository, and from where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// External: a file saved outside Cairn.
    Save,
    /// External: `git switch` outside Cairn, timed from when git has exited. git's own time on a
    /// large repository is not Cairn's to spend.
    GitSwitch,
    /// A Local write Job: the Intent to switch, as the UI would send it.
    IntentSwitch,
}

impl Change {
    /// Whether this is made outside Cairn.
    pub fn is_external(self) -> bool {
        matches!(self, Change::Save | Change::GitSwitch)
    }

    /// The branch the `index`th switch goes to. Switches alternate, so each undoes the last.
    fn branch(index: usize) -> &'static str {
        if index.is_multiple_of(2) {
            BRANCH_B
        } else {
            BRANCH_A
        }
    }

    /// The `index`th change. Changes alternate, so each undoes the last.
    pub(crate) fn expect(self, index: usize) -> Expect {
        let forward = index.is_multiple_of(2);
        match self {
            Change::Save => Expect::Unstaged {
                path: SAVED.to_owned(),
                listed: forward,
            },
            Change::GitSwitch | Change::IntentSwitch => Expect::Switched {
                branch: Self::branch(index).to_owned(),
                untracked: KEEP.to_owned(),
                // Ignored on BRANCH_B.
                listed: !forward,
            },
        }
    }

    /// Makes an External change, and returns when the clock starts.
    pub(crate) fn perform_external(self, index: usize, sandbox: &Sandbox) -> Result<u64, String> {
        match self {
            Change::Save => {
                let start = clock::now_ns();
                sandbox.save(index.is_multiple_of(2))?;
                Ok(start)
            }
            Change::GitSwitch => {
                sandbox.switch(Self::branch(index))?;
                Ok(clock::now_ns())
            }
            Change::IntentSwitch => Err("a Local write Job isn't an External change".to_owned()),
        }
    }

    /// Makes the change, and returns when the clock starts.
    fn perform(self, index: usize, sandbox: &Sandbox, cairn: &mut Cairn) -> Result<u64, String> {
        match self {
            Change::Save | Change::GitSwitch => self.perform_external(index, sandbox),
            Change::IntentSwitch => {
                let branch = Self::branch(index);
                let sent = cairn.trace.lock().unwrap().intents().len();
                cairn.send(&format!("switch {branch}"))?;
                cairn.wait("Cairn to send the Intent", SAMPLE_TIMEOUT, |trace| {
                    trace.intents().get(sent).map(|(ns, _)| *ns)
                })
            }
        }
    }
}

/// The frame on screen now, once every Slot has answered the Open.
fn wait_opened(cairn: &Cairn) -> Result<(), String> {
    cairn.wait("the repository to open", SAMPLE_TIMEOUT, |trace| {
        trace
            .frames()
            .iter()
            .any(|f| f.refs > 0 && f.staged > 0 && f.unstaged > 0 && f.untracked > 0)
            .then_some(())
    })
}

/// One run: one Cairn process, samples and warm-up, on the sandbox as it started.
fn latency_run(config: &Config, sandbox: &Sandbox, change: Change) -> Result<Vec<f64>, String> {
    sandbox.save(false)?;
    sandbox.switch(BRANCH_A)?;
    let recorder = Recorder::start(config)?;
    let mut cairn = Cairn::start(config, sandbox.repo(), recorder.needs_animation())?;
    cairn.check_build()?;
    wait_opened(&cairn)?;

    let mut timed = Vec::with_capacity(config.samples);
    for index in 0..WARM_UP_SAMPLES + config.samples {
        cairn.settle()?;
        let (baseline, before): (Frame, usize) = {
            let trace = cairn.trace.lock().unwrap();
            let frames = trace.frames();
            let last = frames.last().ok_or("no frame has painted")?;
            (last.clone(), frames.len())
        };
        let expect = change.expect(index);
        let start = change.perform(index, sandbox, &mut cairn)?;
        let paint = cairn.wait(&format!("{expect:?}"), SAMPLE_TIMEOUT, |trace| {
            trace.frames()[before..]
                .iter()
                .find(|frame| expect.met_by(frame, &baseline))
                .map(|frame| frame.paint_ns)
        })?;
        if index >= WARM_UP_SAMPLES {
            // From the frame before the change: a `git switch`'s clock starts after its frame
            // may already have painted.
            let lost_focus = cairn
                .trace
                .lock()
                .unwrap()
                .inactive_during(baseline.paint_ns, paint);
            if lost_focus {
                return Err(LOST_FOCUS.to_owned());
            }
            timed.push((start, paint));
        }
    }
    // Lets Metal report the last frames, which it does after they reach the screen.
    cairn.settle()?;
    let trace = cairn.quit()?;
    let presentmon = recorder.finish()?;
    let source = source_for(&trace, presentmon.as_ref())?;
    timed
        .into_iter()
        .map(|(start, paint)| {
            let shown = source
                .shown_at(paint)
                .ok_or("a frame that showed the change was never presented: the window has to stay in front")?;
            // A switch can reach the screen before the git that made it has exited.
            Ok(shown.saturating_sub(start) as f64 / 1e6)
        })
        .collect()
}

/// One run of launches: process start to first presented frame.
fn launch_run(config: &Config, sandbox: &Sandbox) -> Result<Vec<f64>, String> {
    let recorder = Recorder::start(config)?;
    let mut launches = Vec::with_capacity(config.samples);
    // One discarded launch: warm means the binary has run since boot.
    for index in 0..1 + config.samples {
        let start = clock::now_ns();
        let cairn = Cairn::start(config, sandbox.repo(), recorder.needs_animation())?;
        cairn.check_build()?;
        if matches!(recorder, Recorder::Metal) {
            cairn.wait("the first presented frame", SAMPLE_TIMEOUT, |trace| {
                MetalTrace::new(trace.drawables()).first_presented()
            })?;
        } else {
            cairn.wait("the first frame", SAMPLE_TIMEOUT, |trace| {
                trace.frames().first().map(|_| ())
            })?;
            sleep(LAUNCH_HOLD);
        }
        let trace = cairn.quit()?;
        if index > 0 {
            launches.push((start, trace));
        }
    }
    let presentmon = recorder.finish()?;
    launches
        .into_iter()
        .map(|(start, trace)| {
            let source = source_for(&trace, presentmon.as_ref())?;
            let shown = source
                .first_presented()
                .ok_or("a launch's first frame isn't in the trace")?;
            Ok(shown.saturating_sub(start) as f64 / 1e6)
        })
        .collect()
}

/// One run of the held-key test: a person holds a key, because the OS repeats only a real one.
fn held_key_run(config: &Config, sandbox: &Sandbox) -> Result<HeldKeyRun, String> {
    sandbox.save(false)?;
    sandbox.switch(BRANCH_A)?;
    let recorder = Recorder::start(config)?;
    // Drawing all the time, so that a freeze shows as a gap in the presented frames.
    let cairn = Cairn::start(config, sandbox.repo(), true)?;
    cairn.check_build()?;
    wait_opened(&cairn)?;

    // Background notifies: real changes, through the watcher and the Bridge.
    let stop = AtomicBool::new(false);
    let run = std::thread::scope(|scope| -> Result<_, String> {
        let churn = scope.spawn(|| {
            let mut modified = true;
            while !stop.load(Ordering::Relaxed) {
                let _ = sandbox.save(modified);
                modified = !modified;
                sleep(CHURN);
            }
        });
        eprintln!(
            "Click the Cairn window, then press and hold an arrow key. Keep holding until told."
        );
        let started = cairn.wait("a repeated key", Duration::from_secs(120), |trace| {
            trace
                .keys()
                .iter()
                .find(|(_, held)| *held)
                .map(|(ns, _)| *ns)
        });
        if started.is_ok() {
            sleep(HOLD);
            eprintln!("Release the key.");
        }
        stop.store(true, Ordering::Relaxed);
        let _ = churn.join();
        started
    });
    let start = run?;
    let end = start + HOLD.as_nanos() as u64;

    sleep(QUIET);
    let trace = cairn.quit()?;
    if trace.inactive_during(start, end) {
        return Err(LOST_FOCUS.to_owned());
    }
    let presentmon = recorder.finish()?;
    let source = source_for(&trace, presentmon.as_ref())?;
    let in_hold = |ns: u64| (start..=end).contains(&ns);
    let answers: Vec<u64> = trace
        .frames()
        .iter()
        .filter(|f| in_hold(f.paint_ns))
        .map(|f| f.unstaged)
        .collect();
    let repeats: Vec<u64> = trace
        .keys()
        .iter()
        .filter(|(ns, held)| *held && in_hold(*ns))
        .map(|(ns, _)| *ns)
        .collect();
    Ok(HeldKeyRun {
        held_keys: repeats.len(),
        longest_key_silence_ms: largest_gap_ms(&repeats, start, end),
        notifies: match (answers.first(), answers.last()) {
            (Some(first), Some(last)) => last - first,
            _ => 0,
        },
        largest_gap_ms: largest_gap_ms(&source.displayed(), start, end),
    })
}

fn row(
    name: &str,
    fixture: &str,
    sandbox: &Sandbox,
    summary: stats::Summary,
    runs: Vec<Vec<f64>>,
) -> Row {
    Row {
        name: name.to_owned(),
        fixture: fixture.to_owned(),
        fixture_head: sandbox.fixture_head().to_owned(),
        summary,
        runs,
    }
}

/// A fixture, checked rather than substituted: a Budget names its fixture.
fn fixture(config: &Config, name: &str) -> Result<PathBuf, String> {
    let path = config.fixtures.join(name);
    if !path.join(".git").exists() {
        return Err(format!(
            "no fixture at {}. Budgets name their fixture, and none stands in for another: \
             `scripts/fixtures.sh check` says how to set it up",
            path.display()
        ));
    }
    Ok(path)
}

/// The Linux kernel fixture, which the plan defines as having a commit-graph.
pub(crate) fn kernel_fixture(config: &Config) -> Result<PathBuf, String> {
    let path = fixture(config, "linux-graph")?;
    let info = path.join(".git/objects/info");
    if !info.join("commit-graph").exists()
        && !info.join("commit-graphs/commit-graph-chain").exists()
    {
        return Err(format!(
            "{} has no commit-graph, so it isn't the fixture the Budget names: \
             `scripts/fixtures.sh check` says how to fix it",
            path.display()
        ));
    }
    Ok(path)
}

fn say(row: &Row) {
    eprintln!(
        "{}: statistic {:.1} ms, p95 {:.1} ms, worst {:.1} ms: {:?}",
        row.name,
        row.summary.statistic_ms,
        row.summary.p95_ms,
        row.summary.worst_sample_ms,
        row.summary.verdict,
    );
}

/// Launch, on the small fixture. The row says any fixture will do.
pub fn measure_launch(config: &Config) -> Result<Row, String> {
    let sandbox = Sandbox::clone_of(&fixture(config, "ripgrep")?)?;
    let runs = (0..config.runs)
        .map(|_| launch_run(config, &sandbox))
        .collect::<Result<Vec<_>, _>>()?;
    let summary = stats::judge(&stats::LAUNCH, &runs);
    let row = row("Launch", "ripgrep", &sandbox, summary, runs);
    say(&row);
    Ok(row)
}

/// Intent to UI after a Local write Job, on the small fixture, as its row says.
pub fn measure_local_write(config: &Config) -> Result<Row, String> {
    let sandbox = Sandbox::clone_of(&fixture(config, "ripgrep")?)?;
    let runs = (0..config.runs)
        .map(|_| latency_run(config, &sandbox, Change::IntentSwitch))
        .collect::<Result<Vec<_>, _>>()?;
    let summary = stats::judge(&stats::LOCAL_WRITE, &runs);
    let row = row(
        "Intent to UI, Local write Job (switch)",
        "ripgrep",
        &sandbox,
        summary,
        runs,
    );
    say(&row);
    Ok(row)
}

/// External change, both kinds the row names, on the Linux kernel. Its clone takes a while.
pub fn measure_external_change(config: &Config) -> Result<Vec<Row>, String> {
    eprintln!("Cloning the kernel fixture into a temp directory...");
    let sandbox = Sandbox::clone_of(&kernel_fixture(config)?)?;
    [
        ("External change, file saved", Change::Save),
        ("External change, git switch", Change::GitSwitch),
    ]
    .into_iter()
    .map(|(name, change)| {
        let runs = (0..config.runs)
            .map(|_| latency_run(config, &sandbox, change))
            .collect::<Result<Vec<_>, _>>()?;
        let summary = stats::judge(&stats::EXTERNAL_CHANGE, &runs);
        let row = row(name, "linux-graph", &sandbox, summary, runs);
        say(&row);
        Ok(row)
    })
    .collect()
}

/// The held-key test, under background notifies, on the small fixture.
pub fn measure_held_key(config: &Config) -> Result<Row, String> {
    let sandbox = Sandbox::clone_of(&fixture(config, "ripgrep")?)?;
    let runs = (0..config.runs)
        .map(|_| held_key_run(config, &sandbox))
        .collect::<Result<Vec<_>, _>>()?;
    let summary = stats::judge_held_key(&runs);
    let gaps = runs.iter().map(|r| vec![r.largest_gap_ms]).collect();
    let row = row(
        "Held key under notifies, largest gap",
        "ripgrep",
        &sandbox,
        summary,
        gaps,
    );
    say(&row);
    Ok(row)
}

/// The toolchain's version. rustup finds its toolchain through the home directory, so this has to
/// run before the harness replaces it for git.
pub fn rust_version() -> Result<String, String> {
    line_from("rustc", &["--version"])
}

/// One line of a program's output, or an error saying which program.
fn line_from(program: &str, args: &[&str]) -> Result<String, String> {
    let out = spawn::tool(program)?
        .args(args)
        .output()
        .map_err(|e| format!("couldn't run {program}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{program} {args:?} failed"));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" "))
}

/// What the numbers depend on. Any fact it can't find is an error, because a result that
/// doesn't say where it came from can't be used.
pub fn environment(
    config: &Config,
    repo: &Path,
    rust_version: String,
) -> Result<Environment, String> {
    let (machine, os) = if cfg!(target_os = "macos") {
        (
            line_from("sysctl", &["-n", "hw.model", "machdep.cpu.brand_string"])?,
            format!("macOS {}", line_from("sw_vers", &["-productVersion"])?),
        )
    } else if cfg!(windows) {
        let var = |name: &str| std::env::var(name).map_err(|_| format!("{name} isn't set"));
        (
            format!("{} {}", var("COMPUTERNAME")?, var("PROCESSOR_IDENTIFIER")?),
            line_from("cmd", &["/C", "ver"])?,
        )
    } else {
        return Err("numbers count only from macOS and the physical Windows PC".to_owned());
    };
    let git = cairn_write::Git::shared().map_err(|e| e.to_string())?;
    let status = git
        .run(
            &cairn_write::Invocation::new(cairn_write::JobClass::Read, repo)
                .args(["status", "--porcelain"]),
        )
        .map_err(|e| e.to_string())?;
    let dirty = if status.stdout.is_empty() {
        ""
    } else {
        " with uncommitted changes"
    };
    Ok(Environment {
        machine,
        os,
        git_version: git.version().to_string(),
        git_path: git.path().display().to_string(),
        rust_version,
        cairn_commit: format!("{}{dirty}", crate::fixture::head_of(repo)?),
        frame_source: match &config.presentmon {
            Some(exe) if cfg!(windows) => format!("PresentMon at {}", exe.display()),
            _ => "Metal presentedTime".to_owned(),
        },
    })
}
