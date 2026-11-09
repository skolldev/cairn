//! What git's prompts do behind each Windows process-creation flag (#26): asserted under the
//! shipped `CREATE_NO_WINDOW`, and only recorded under `DETACHED_PROCESS`. The result is
//! `docs/detached-process.md`. Run with `--nocapture` to see the records, and with `--ignored`
//! for the ones that need GitHub or open dialogs.

#![cfg(windows)]

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cairn_write::{Invocation, JobClass, RunError, Stop, WindowsProcess};
use common::{Repo, git, ok, read, slashes};
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::System::Console::{GetConsoleCP, GetConsoleWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, IsWindowVisible};

/// Far longer than any of these cases takes when nothing is waiting on a prompt.
const HANG: Duration = Duration::from_secs(20);

/// Run as a child of the prompt under test, this reports the console it found to the file
/// `CAIRN_CONSOLE_PROBE` names.
#[test]
#[ignore = "started by the programs git runs in these tests, not by a test run"]
fn console_probe() {
    let Some(out) = std::env::var_os("CAIRN_CONSOLE_PROBE") else {
        return;
    };
    // SAFETY: neither call has preconditions. They only ask about this process's console.
    let (attached, window) = unsafe { (GetConsoleCP() != 0, GetConsoleWindow()) };
    // SAFETY: `IsWindowVisible` accepts any handle, and this one is checked non-null first.
    let visible = !window.is_null() && unsafe { IsWindowVisible(window) } != 0;
    let line = format!(
        "attached={attached} window={} visible={visible}\n",
        !window.is_null()
    );
    let mut file = std::fs::File::options()
        .create(true)
        .append(true)
        .open(out)
        .unwrap();
    file.write_all(line.as_bytes()).unwrap();
}

/// A shell line that runs [`console_probe`], reporting to `out`.
fn probe_line(out: &Path) -> String {
    let exe = slashes(&std::env::current_exe().unwrap());
    format!(
        "CAIRN_CONSOLE_PROBE='{}' '{exe}' --ignored --exact console_probe >/dev/null 2>&1",
        slashes(out)
    )
}

/// `path` as gpg reads it. gpg is an MSYS program, which takes `/c/Users` and not `C:/Users`.
fn msys_path(path: &Path) -> String {
    let path = slashes(path);
    let (drive, rest) = path.split_once(":/").expect("an absolute path");
    format!("/{}/{rest}", drive.to_lowercase())
}

/// One at a time: [`Watcher`] sees the whole desktop, so a window one case opened would be
/// blamed on another running beside it.
fn one_case_at_a_time() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether a window of this class holds a console: conhost's, or Windows Terminal's when it
/// is the default terminal.
fn is_console(class: &str) -> bool {
    class == "ConsoleWindowClass" || class == "CASCADIA_HOSTING_WINDOW_CLASS"
}

/// Watches the desktop for windows that weren't there when it started, so it also
/// catches one that only flashes, which a probe asking afterwards would miss.
struct Watcher {
    done: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<Vec<String>>,
}

impl Watcher {
    fn start() -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let done = done.clone();
            move || {
                let before: Vec<isize> = visible_windows().into_iter().map(|w| w.0).collect();
                let mut opened = Vec::new();
                while !done.load(Ordering::SeqCst) {
                    for (handle, class) in visible_windows() {
                        if !before.contains(&handle) && !opened.contains(&class) {
                            opened.push(class);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                opened
            }
        });
        Self { done, thread }
    }

    fn opened(self) -> Vec<String> {
        self.done.store(true, Ordering::SeqCst);
        self.thread.join().unwrap()
    }
}

/// Every visible top-level window, with its class.
fn visible_windows() -> Vec<(isize, String)> {
    unsafe extern "system" fn each(window: HWND, found: LPARAM) -> i32 {
        // SAFETY: `found` is the `Vec` passed to `EnumWindows` below, alive for that call,
        // and `EnumWindows` calls back on this thread.
        let found = unsafe { &mut *(found as *mut Vec<(isize, String)>) };
        let mut class = [0u16; 64];
        // SAFETY: `window` came from `EnumWindows`, and the length is the buffer's.
        let len = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
        let class = String::from_utf16_lossy(&class[..len.max(0) as usize]);
        // SAFETY: `IsWindowVisible` accepts any handle.
        let visible = unsafe { IsWindowVisible(window) } != 0;
        if visible {
            found.push((window as isize, class));
        }
        1
    }
    let mut found: Vec<(isize, String)> = Vec::new();
    // SAFETY: the callback only touches `found`, which outlives this call.
    unsafe {
        EnumWindows(
            Some(each),
            &mut found as *mut Vec<(isize, String)> as LPARAM,
        )
    };
    found
}

/// What one case did.
#[derive(Debug)]
struct Seen {
    /// git's exit code, or `None` if it was still running after [`HANG`] and was killed.
    exit: Option<i32>,
    stderr: String,
    took: Duration,
    /// What the probe reported, one line per run. Empty if the prompt never reached it.
    console: Vec<String>,
    /// The classes of the windows that appeared while git ran.
    windows: Vec<String>,
}

impl Seen {
    fn record(&self, process: WindowsProcess, case: &str) {
        println!(
            "RESULT {process:?} {case}: exit={:?} took={}ms windows={:?} console={:?} stderr={:?}",
            self.exit,
            self.took.as_millis(),
            self.windows,
            self.console,
            self.stderr,
        );
    }
}

/// Runs `invocation`, killing it if it outlasts [`HANG`], and reports what it did. Every case
/// runs as a Network Job, whatever git is doing: a hung Local write Job can't be stopped and
/// would outlive the test, and a Read Job adds `--no-optional-locks`. A Job's class doesn't
/// change how git is started.
fn run_watched(invocation: Invocation, probes: &Path) -> Seen {
    let _one = one_case_at_a_time();
    let stop = Stop::new();
    let invocation = invocation.stop_on(&stop);
    let finished = Arc::new(AtomicBool::new(false));
    let watchdog = std::thread::spawn({
        let (stop, finished) = (stop.clone(), finished.clone());
        move || {
            let started = Instant::now();
            while !finished.load(Ordering::SeqCst) {
                if started.elapsed() > HANG {
                    stop.stop();
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    });
    let watcher = Watcher::start();
    let started = Instant::now();
    let result = git().run(&invocation);
    let took = started.elapsed();
    let windows = watcher.opened();
    finished.store(true, Ordering::SeqCst);
    watchdog.join().unwrap();
    let console = std::fs::read_to_string(probes)
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default();
    let (exit, stderr) = match result {
        Ok(out) => (out.status.code(), out.stderr_text().into_owned()),
        Err(RunError::Stopped { stderr }) => (None, String::from_utf8_lossy(&stderr).into_owned()),
        Err(other) => panic!("git didn't run: {other}"),
    };
    Seen {
        exit,
        stderr,
        took,
        console,
        windows,
    }
}

fn script(path: &Path, body: &str) -> String {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    slashes(path)
}

/// A case's repository and the file its probe reports to.
struct Case {
    repo: Repo,
    dir: tempfile::TempDir,
    probes: PathBuf,
}

impl Case {
    fn new() -> Self {
        let repo = Repo::new();
        let dir = tempfile::tempdir().unwrap();
        let probes = dir.path().join("probes");
        Self { repo, dir, probes }
    }

    /// A case whose pre-commit hook runs `body` after the probe, or just the probe.
    fn with_hook(body: &str) -> Self {
        let case = Self::new();
        script(
            &case.repo.path.join(".git/hooks/pre-commit"),
            &format!("{}\n{body}", probe_line(&case.probes)),
        );
        case
    }

    fn invocation(&self, args: &[&str]) -> Invocation {
        Invocation::new(JobClass::Network, &self.repo.path).args(args)
    }

    fn run(&self, process: WindowsProcess, args: &[&str]) -> Seen {
        run_watched(self.invocation(args).windows_process(process), &self.probes)
    }
}

// ---- The cases. Each runs under whichever flag it is given.

/// A commit, which runs the case's hook.
const COMMIT: [&str; 5] = ["commit", "--allow-empty", "-q", "-m", "hooked"];

fn hook(process: WindowsProcess) -> Seen {
    Case::with_hook("").run(process, &COMMIT)
}

/// A hook that asks the terminal directly, the way a hand-written "are you sure?" does.
fn hook_reading_the_terminal(process: WindowsProcess) -> Seen {
    Case::with_hook(
        "if (: </dev/tty) 2>/dev/null; then echo opened-tty >&2; read -r answer </dev/tty; \
         else echo no-tty >&2; fi",
    )
    .run(process, &COMMIT)
}

/// A server that refuses every request and asks for Basic auth, as a private remote does.
/// Returns its URL and every `Authorization` header it was sent.
fn refusing_http_remote() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://127.0.0.1:{}/repo.git",
        listener.local_addr().unwrap().port()
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    // Ends with the test binary.
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&chunk[..n]),
                }
            }
            let request = String::from_utf8_lossy(&request);
            log.lock().unwrap().extend(
                request
                    .lines()
                    .filter_map(|l| l.strip_prefix("Authorization: "))
                    .map(str::to_owned),
            );
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"cairn\"\r\n\
                  Content-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    (url, seen)
}

/// What git sent the remote, beside what it did.
struct Sent {
    seen: Seen,
    authorization: Vec<String>,
}

impl Sent {
    fn record(&self, process: WindowsProcess, case: &str) {
        self.seen.record(process, case);
        println!("  and sent {:?}", self.authorization);
    }
}

fn credential_helper(process: WindowsProcess) -> Sent {
    let case = Case::new();
    let helper = script(
        &case.dir.path().join("helper.sh"),
        &format!(
            "cat >/dev/null\n{}\nif [ \"$1\" = get ]; then echo username=cairn-user; \
             echo password=cairn-secret; fi",
            probe_line(&case.probes)
        ),
    );
    fetch_with(process, &case, &format!("credential.helper=!sh '{helper}'"))
}

fn askpass(process: WindowsProcess) -> Sent {
    let case = Case::new();
    let program = script(
        &case.dir.path().join("askpass.sh"),
        &format!(
            "{}\ncase \"$1\" in Username*) echo cairn-user;; *) echo cairn-secret;; esac",
            probe_line(&case.probes)
        ),
    );
    fetch_with(process, &case, &format!("core.askPass={program}"))
}

/// No helper and no askpass: only the terminal is left, and `GIT_TERMINAL_PROMPT=0` shuts it.
fn no_way_to_ask(process: WindowsProcess) -> Sent {
    let case = Case::new();
    fetch_with(process, &case, "core.askPass=")
}

fn fetch_with(process: WindowsProcess, case: &Case, config: &str) -> Sent {
    let (url, authorization) = refusing_http_remote();
    let args = ["-c", "http.proxy=", "-c", config, "ls-remote", &url];
    let seen = case.run(process, &args);
    let authorization = authorization.lock().unwrap().clone();
    Sent {
        seen,
        authorization,
    }
}

/// The real `ssh`, as git starts it. It gets as far as its ProxyCommand, which then fails,
/// so no server is needed, and what the ProxyCommand reports is the console `ssh` passed on.
fn ssh(process: WindowsProcess) -> Seen {
    let case = Case::new();
    let proxy = script(
        &case.dir.path().join("proxy.sh"),
        &format!("{}\nexit 1", probe_line(&case.probes)),
    );
    let known_hosts = slashes(&case.dir.path().join("known_hosts"));
    let command = format!(
        "core.sshCommand=ssh -F /dev/null -o UserKnownHostsFile='{known_hosts}' \
         -o ProxyCommand='sh {proxy}'"
    );
    let args = [
        "-c",
        &command,
        "ls-remote",
        "ssh://git@cairn.invalid/repo.git",
    ];
    case.run(process, &args)
}

/// A real `ssh` meeting a host it doesn't know, which it asks about on the terminal. Needs
/// GitHub's port 22, so it runs only when asked for. No key is offered and nothing is read
/// from the remote: `ssh` stops at the question, or at refusing to ask it. `options` is
/// what the case adds to that.
fn ssh_asking_about_a_host(
    process: WindowsProcess,
    program: &str,
    null: &str,
    options: &str,
) -> Seen {
    let case = Case::new();
    let known_hosts = slashes(&case.dir.path().join("known_hosts"));
    let command = format!(
        "core.sshCommand='{program}' -F {null} -o UserKnownHostsFile='{known_hosts}' \
         -o GlobalKnownHostsFile={null} -o IdentitiesOnly=yes -o IdentityAgent=none \
         -o ConnectTimeout=10 {options}"
    );
    let args = ["-c", &command, "ls-remote", "git@github.com:git/git.git"];
    case.run(process, &args)
}

/// A throwaway gpg home holding one key, and a `gpg.program` that uses it. Sets up through
/// git's own alias, because git is what the tests may start.
struct Gpg {
    case: Case,
    program: String,
    home: PathBuf,
}

impl Gpg {
    fn new(passphrase: &str) -> Self {
        let case = Case::new();
        let home = case.dir.path().join("gnupg");
        std::fs::create_dir(&home).unwrap();
        let msys_home = msys_path(&home);
        let setup = format!(
            "alias.setup=!GNUPGHOME='{msys_home}' gpg --batch --quiet --pinentry-mode loopback \
             --passphrase '{passphrase}' --quick-generate-key \
             'Cairn Test <test@cairn.invalid>' ed25519 sign never"
        );
        ok(git()
            .run(&case.invocation(&["-c", &setup, "setup"]))
            .unwrap());
        // gpg started the agent for that key. Stop it, so the case's own run starts one under
        // the flag being tested.
        Self::stop_agent(&case, &home);
        let program = script(
            &case.dir.path().join("gpg.sh"),
            &format!(
                "{}\nGNUPGHOME='{msys_home}' exec gpg \"$@\"",
                probe_line(&case.probes)
            ),
        );
        Self {
            case,
            program,
            home,
        }
    }

    fn stop_agent(case: &Case, home: &Path) {
        let stop = format!(
            "alias.stop=!GNUPGHOME='{}' gpgconf --kill gpg-agent",
            msys_path(home)
        );
        let _ = git().run(&case.invocation(&["-c", &stop, "stop"]));
    }

    fn sign(&self, process: WindowsProcess) -> Signed {
        let program = format!("gpg.program={}", self.program);
        let args = [
            "-c",
            &program,
            "-c",
            "user.signingkey=test@cairn.invalid",
            "commit",
            "--allow-empty",
            "-S",
            "-q",
            "-m",
            "signed",
        ];
        let seen = self.case.run(process, &args);
        Self::stop_agent(&self.case, &self.home);
        let commit = read(&self.case.repo.path, &["cat-file", "commit", "HEAD"]);
        Signed {
            seen,
            commit: String::from_utf8_lossy(&commit.stdout).into_owned(),
        }
    }
}

/// What a signing run did, beside the commit it left.
struct Signed {
    seen: Seen,
    commit: String,
}

impl Signed {
    fn is_signed(&self) -> bool {
        self.commit.contains("gpgsig ")
    }

    fn record(&self, process: WindowsProcess, case: &str) {
        self.seen.record(process, case);
        println!("  and left a signed commit: {}", self.is_signed());
    }
}

fn gpg_signing(process: WindowsProcess) -> Signed {
    Gpg::new("").sign(process)
}

/// A key with a passphrase, answered by a pinentry written for the test, so no dialog waits
/// on a person. What it reports is the console gpg-agent passes on.
fn gpg_asking_for_a_passphrase(process: WindowsProcess) -> Signed {
    let gpg = Gpg::new("cairn-secret");
    let pinentry = script(
        &gpg.case.dir.path().join("pinentry.sh"),
        &format!(
            "{}\necho 'OK ready'\nwhile read -r line; do case \"$line\" in \
             GETPIN*) echo 'D cairn-secret'; echo OK;; BYE*) echo OK; exit 0;; \
             *) echo OK;; esac; done",
            probe_line(&gpg.case.probes)
        ),
    );
    std::fs::write(
        gpg.home.join("gpg-agent.conf"),
        format!("pinentry-program {pinentry}\n"),
    )
    .unwrap();
    gpg.sign(process)
}

/// Git ran, and the prompt ran with no console window for anyone to see. Seen red: with
/// `WindowsProcess::NoWindow` set to `CREATE_NEW_CONSOLE` (0x10), every test using this fails
/// at the watcher's check, and at the probe's with that check taken out. With `HANG` at a
/// millisecond, `a_hook_runs_out_of_sight` fails on the first line.
#[track_caller]
fn assert_out_of_sight(seen: &Seen) {
    assert!(seen.exit.is_some(), "git hung: {seen:?}");
    assert!(!seen.console.is_empty(), "the prompt never ran: {seen:?}");
    assert!(!seen.windows.iter().any(|c| is_console(c)), "{seen:?}");
    assert!(
        seen.console.iter().all(|c| c.ends_with("visible=false")),
        "{seen:?}"
    );
}

/// `cairn-user:cairn-secret` in Base64, as curl sends it.
const ANSWERED: &str = "Basic Y2Fpcm4tdXNlcjpjYWlybi1zZWNyZXQ=";

#[test]
fn the_shipped_default_is_a_console_without_a_window() {
    let case = Case::with_hook("");
    let seen = run_watched(case.invocation(&COMMIT), &case.probes);
    // Seen red: with `Invocation::new` starting at `Detached`, this reads `window=true`.
    assert_eq!(
        seen.console,
        ["attached=true window=false visible=false"],
        "{seen:?}"
    );
}

#[test]
fn a_hook_runs_out_of_sight() {
    let seen = hook(WindowsProcess::NoWindow);
    assert_out_of_sight(&seen);
    // Seen red: with the hook ending `exit 1`.
    assert_eq!(seen.exit, Some(0));
}

#[test]
fn a_hook_reading_the_terminal_finds_none_and_does_not_hang() {
    let seen = hook_reading_the_terminal(WindowsProcess::NoWindow);
    assert_out_of_sight(&seen);
    // Seen red: with the hook exiting 1 when it finds no terminal.
    assert_eq!(seen.exit, Some(0));
    // Seen red: with the hook reporting something else when it finds no terminal.
    assert!(seen.stderr.contains("no-tty"), "{seen:?}");
}

#[test]
fn a_credential_helper_answers_out_of_sight() {
    let sent = credential_helper(WindowsProcess::NoWindow);
    assert_out_of_sight(&sent.seen);
    // Seen red: with the helper answering another password. The server refuses every answer,
    // so git failing is the expected end.
    assert_eq!(sent.authorization, [ANSWERED], "{:?}", sent.seen);
}

#[test]
fn an_askpass_program_answers_out_of_sight() {
    let sent = askpass(WindowsProcess::NoWindow);
    assert_out_of_sight(&sent.seen);
    // Seen red: with the program answering another password.
    assert_eq!(sent.authorization, [ANSWERED], "{:?}", sent.seen);
}

#[test]
fn with_no_way_to_ask_git_fails_at_once_with_its_own_words() {
    let sent = no_way_to_ask(WindowsProcess::NoWindow);
    assert_eq!(sent.seen.exit, Some(128), "{:?}", sent.seen);
    // Seen red: with `GIT_TERMINAL_PROMPT=1` in `git_command`.
    assert!(
        sent.seen.stderr.contains("terminal prompts disabled"),
        "{:?}",
        sent.seen
    );
    assert!(sent.authorization.is_empty());
}

#[test]
fn ssh_runs_out_of_sight() {
    let seen = ssh(WindowsProcess::NoWindow);
    assert_out_of_sight(&seen);
}

#[test]
fn gpg_signs_out_of_sight() {
    let signed = gpg_signing(WindowsProcess::NoWindow);
    // Seen red: with `--no-gpg-sign` in place of `-S`.
    assert!(signed.is_signed(), "{}", signed.commit);
    assert_out_of_sight(&signed.seen);
}

#[test]
fn gpg_asks_its_pinentry_out_of_sight() {
    let signed = gpg_asking_for_a_passphrase(WindowsProcess::NoWindow);
    assert!(signed.is_signed(), "{}", signed.commit);
    assert_out_of_sight(&signed.seen);
    // Seen red: with the key's passphrase empty, which leaves only the first of these.
    assert_eq!(
        signed.seen.console.len(),
        2,
        "the gpg wrapper and the pinentry: {:?}",
        signed.seen
    );
}

/// Recorded, not asserted (#14): the results inform a later decision, and a change here is
/// news for `docs/detached-process.md` rather than a failure.
#[test]
fn detached_process_results_are_recorded() {
    let process = WindowsProcess::Detached;
    hook(process).record(process, "hook");
    hook_reading_the_terminal(process).record(process, "hook reading the terminal");
    credential_helper(process).record(process, "credential helper");
    askpass(process).record(process, "askpass");
    no_way_to_ask(process).record(process, "no way to ask");
    ssh(process).record(process, "ssh");
    gpg_signing(process).record(process, "gpg signing");
    gpg_asking_for_a_passphrase(process).record(process, "gpg passphrase");
}

/// Prompts no offline test can reach, under both flags.
#[test]
#[ignore = "needs GitHub's port 22"]
fn record_ssh_prompts() {
    const WINDOWS_SSH: &str = "C:/Windows/System32/OpenSSH/ssh.exe";
    const ASK: &str = "-o StrictHostKeyChecking=ask";
    for process in [WindowsProcess::NoWindow, WindowsProcess::Detached] {
        ssh_asking_about_a_host(process, "ssh", "/dev/null", ASK)
            .record(process, "Git for Windows ssh asking about a host");
        ssh_asking_about_a_host(process, WINDOWS_SSH, "NUL", ASK)
            .record(process, "Windows ssh asking about a host");
        ssh_asking_about_a_host(process, WINDOWS_SSH, "NUL", "-o StrictHostKeyChecking=no")
            .record(process, "control: Windows ssh with nothing to ask");
        ssh_asking_about_a_host(process, WINDOWS_SSH, "NUL", "-o BatchMode=yes")
            .record(process, "Windows ssh told never to ask");
    }
}

/// Git Credential Manager, the helper Git for Windows installs, and gpg's own pinentry. Both
/// can put a dialog on the desktop, and nobody answers it, so this runs only when asked for.
#[test]
#[ignore = "can open dialogs on the desktop"]
fn record_real_dialogs() {
    for process in [WindowsProcess::NoWindow, WindowsProcess::Detached] {
        let case = Case::new();
        fetch_with(process, &case, "credential.helper=manager")
            .record(process, "Git Credential Manager");
        Gpg::new("cairn-secret")
            .sign(process)
            .seen
            .record(process, "gpg's own pinentry");
    }
}

/// The median cost of a call under each flag, directly and through a shell. Run it with
/// `--release`.
#[test]
#[ignore = "a measurement, not a test"]
fn record_spawn_cost() {
    let case = Case::new();
    let median = |process, args: &[&str]| {
        let mut runs: Vec<Duration> = (0..200)
            .map(|_| {
                let invocation = Invocation::new(JobClass::Read, &case.repo.path)
                    .args(args)
                    .windows_process(process);
                let started = Instant::now();
                ok(git().run(&invocation).unwrap());
                started.elapsed()
            })
            .collect();
        runs.sort();
        runs[runs.len() / 2]
    };
    let shell = ["-c", "alias.nothing=!true", "nothing"];
    for round in 0..3 {
        for process in [WindowsProcess::NoWindow, WindowsProcess::Detached] {
            println!(
                "RESULT round {round} {process:?}: git {:?}, git with a shell alias {:?}",
                median(process, &["--version"]),
                median(process, &shell),
            );
        }
    }
}
