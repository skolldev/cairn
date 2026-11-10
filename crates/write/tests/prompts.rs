//! A Job never waits on a Prompt Cairn can't show (ADR 0008).
//!
//! Some tests here set variables in this process's environment, which every git the runner
//! starts inherits. So every test in this file holds [`one_at_a_time`] while git runs.

mod common;

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cairn_write::{Invocation, JobClass, RunError, Stop};
use common::{Repo, git};

/// Far longer than `ssh` takes to refuse a Prompt.
const HANG: Duration = Duration::from_secs(30);

fn one_at_a_time() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Sets `vars` in this process's environment until dropped, then removes them again.
struct UserEnv {
    names: Vec<&'static str>,
    _one: MutexGuard<'static, ()>,
}

impl UserEnv {
    fn set(vars: &[(&'static str, &str)]) -> Self {
        let one = one_at_a_time();
        git(); // The hermetic config is in place before anything here touches the environment.
        for (name, value) in vars {
            // SAFETY: every test in this binary holds `one_at_a_time` while git runs, so no
            // other thread reads the environment meanwhile.
            unsafe { std::env::set_var(name, value) };
        }
        Self {
            names: vars.iter().map(|(name, _)| *name).collect(),
            _one: one,
        }
    }
}

impl Drop for UserEnv {
    fn drop(&mut self) {
        for name in &self.names {
            // SAFETY: as in `set`, under the same lock.
            unsafe { std::env::remove_var(name) };
        }
    }
}

/// What a Network Job did: its exit code, or `None` if it outlasted [`HANG`] and was stopped.
#[derive(Debug)]
struct Ended {
    exit: Option<i32>,
    stderr: String,
    took: Duration,
}

/// Runs `args` as a Network Job in `repo`, stopping it if it outlasts [`HANG`].
fn network(repo: &Repo, args: &[&str]) -> Ended {
    let stop = Stop::new();
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
    let started = Instant::now();
    let result = git().run(
        &Invocation::new(JobClass::Network, &repo.path)
            .args(args)
            .stop_on(&stop),
    );
    let took = started.elapsed();
    finished.store(true, Ordering::SeqCst);
    watchdog.join().unwrap();
    let (exit, stderr) = match result {
        Ok(out) => (out.status.code(), out.stderr_text().into_owned()),
        Err(RunError::Stopped { stderr }) => (None, String::from_utf8_lossy(&stderr).into_owned()),
        Err(other) => panic!("git didn't run: {other}"),
    };
    Ended { exit, stderr, took }
}

fn slashes(path: &Path) -> String {
    path.to_str().unwrap().replace('\\', "/")
}

/// `program` meeting GitHub's host key for the first time, which it asks whether to trust.
/// Needs GitHub's port 22. No key is offered: `ssh` stops at the question.
fn ssh_meeting_an_unknown_host(program: &str, null: &str) -> Ended {
    let _one = one_at_a_time();
    let repo = Repo::new();
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = slashes(&dir.path().join("known_hosts"));
    let command = format!(
        "core.sshCommand='{program}' -F {null} -o UserKnownHostsFile='{known_hosts}' \
         -o GlobalKnownHostsFile={null} -o IdentitiesOnly=yes -o IdentityAgent=none \
         -o ConnectTimeout=10 -o StrictHostKeyChecking=ask"
    );
    let ended = network(
        &repo,
        &["-c", &command, "ls-remote", "git@github.com:git/git.git"],
    );
    println!("RESULT {program}: {ended:?}");
    ended
}

#[track_caller]
fn assert_refused_with_ssh_s_own_words(ended: &Ended) {
    assert!(
        ended.exit.is_some(),
        "the Job waited on a Prompt: {ended:?}"
    );
    assert!(ended.took < Duration::from_secs(15), "{ended:?}");
    assert!(
        ended.stderr.starts_with("Host key verification failed."),
        "{ended:?}"
    );
}

/// Windows' own `ssh.exe` waited, silently, until stopped (#44).
#[cfg(windows)]
#[test]
fn windows_ssh_refuses_an_unknown_host_instead_of_waiting() {
    const WINDOWS_SSH: &str = "C:/Windows/System32/OpenSSH/ssh.exe";
    assert!(
        Path::new(WINDOWS_SSH).exists(),
        "this test needs Windows' OpenSSH client"
    );
    let ended = ssh_meeting_an_unknown_host(WINDOWS_SSH, "NUL");
    assert_refused_with_ssh_s_own_words(&ended);
}

/// The `ssh` git finds by itself: macOS's, or Git for Windows' own. Run from a terminal, it
/// would ask there and wait.
#[test]
fn the_default_ssh_refuses_an_unknown_host_instead_of_waiting() {
    let ended = ssh_meeting_an_unknown_host("ssh", "/dev/null");
    // Seen red: with `cairn-askpass` answering `yes`, which shows `ssh` asks it.
    assert_refused_with_ssh_s_own_words(&ended);
}

/// An `ssh` command that only reports the askpass variables it was given, and exits.
fn askpass_seen_by_ssh() -> String {
    let repo = Repo::new();
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("ssh.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\necho \"SSH_ASKPASS=$SSH_ASKPASS\" >&2\n\
         echo \"SSH_ASKPASS_REQUIRE=$SSH_ASKPASS_REQUIRE\" >&2\nexit 1\n",
    )
    .unwrap();
    let command = format!("core.sshCommand=sh '{}'", slashes(&script));
    let ended = network(
        &repo,
        &["-c", &command, "ls-remote", "ssh://git@cairn.invalid/r.git"],
    );
    ended.stderr
}

#[test]
fn a_user_s_own_ssh_askpass_reaches_ssh_unchanged() {
    let _env = UserEnv::set(&[("SSH_ASKPASS", "/the users/own askpass")]);
    let seen = askpass_seen_by_ssh();
    // Seen red: with the runner setting SSH_ASKPASS whatever the user's environment says.
    assert!(
        seen.lines()
            .any(|l| l == "SSH_ASKPASS=/the users/own askpass"),
        "{seen}"
    );
    assert!(
        seen.lines().any(|l| l == "SSH_ASKPASS_REQUIRE=force"),
        "{seen}"
    );
}

#[test]
fn a_user_s_own_ssh_askpass_require_reaches_ssh_unchanged() {
    let _env = UserEnv::set(&[("SSH_ASKPASS_REQUIRE", "prefer")]);
    let seen = askpass_seen_by_ssh();
    // Seen red: with the runner setting SSH_ASKPASS_REQUIRE whatever the user's environment
    // says.
    assert!(
        seen.lines().any(|l| l == "SSH_ASKPASS_REQUIRE=prefer"),
        "{seen}"
    );
    let helper = seen
        .lines()
        .find_map(|l| l.strip_prefix("SSH_ASKPASS="))
        .unwrap_or_default();
    assert!(is_cairns_helper(OsStr::new(helper)), "{seen}");
}

/// Whether `path` is the `cairn-askpass` this test binary was built with.
fn is_cairns_helper(path: &OsStr) -> bool {
    let built = Path::new(env!("CARGO_BIN_EXE_cairn-askpass"));
    std::fs::canonicalize(path).ok() == std::fs::canonicalize(built).ok()
}

/// A server that refuses every request and asks for Basic auth, as a private remote does.
/// Returns its URL. It ends with the test binary.
fn refusing_http_remote() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://127.0.0.1:{}/repo.git",
        listener.local_addr().unwrap().port()
    );
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
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"cairn\"\r\n\
                  Content-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    url
}

fn ls_remote_over_https(extra: &[&str]) -> Ended {
    let repo = Repo::new();
    let url = refusing_http_remote();
    let mut args = vec!["-c", "http.proxy=", "-c", "credential.helper="];
    args.extend_from_slice(extra);
    args.extend(["ls-remote", url.as_str()]);
    network(&repo, &args)
}

/// git's own username prompt is refused as it always was. git asks `SSH_ASKPASS` when nothing
/// else is set, and an askpass that fails adds `error: unable to read askpass response`, so an
/// unchanged error also shows that Cairn's helper wasn't run.
#[test]
fn git_s_own_https_prompt_fails_as_before_without_asking_cairn_s_helper() {
    let _one = one_at_a_time();
    let ended = ls_remote_over_https(&[]);
    assert_eq!(ended.exit, Some(128), "{ended:?}");
    // Seen red: with the runner no longer setting GIT_ASKPASS to empty.
    assert!(
        ended
            .stderr
            .starts_with("fatal: could not read Username for 'http://127.0.0.1:")
            && ended.stderr.ends_with("': terminal prompts disabled\n"),
        "{ended:?}"
    );
}

/// A user's own `GIT_ASKPASS` still answers git's prompts.
#[test]
fn a_user_s_own_git_askpass_still_answers_git() {
    let dir = tempfile::tempdir().unwrap();
    let askpass = dir.path().join("askpass.sh");
    std::fs::write(&askpass, "#!/bin/sh\necho asked >&2\necho cairn-user\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&askpass, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let askpass = slashes(&askpass);
    let _env = UserEnv::set(&[("GIT_ASKPASS", askpass.as_str())]);
    let ended = ls_remote_over_https(&[]);
    // Seen red: with the runner setting GIT_ASKPASS to empty whatever the user's environment
    // says.
    assert!(ended.stderr.starts_with("asked\n"), "{ended:?}");
}
