//! A Job never waits on a Prompt Cairn can't show (ADR 0008).
//!
//! Some tests here set variables in this process's environment, which every git the runner
//! starts inherits. So every test in this file takes [`one_at_a_time`] first, and holds it
//! until it has finished with git and the environment.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cairn_write::{Invocation, JobClass, RunError, Stop};
use common::{Repo, forward_slashes, git, is_cairns_askpass, ssh_askpass_in};

/// Far longer than `ssh` takes to refuse a Prompt.
const HANG: Duration = Duration::from_secs(30);

fn one_at_a_time() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Sets `vars` in this process's environment, as a user's shell would, until dropped. The
/// hermetic config has removed every variable set here, so removing it again restores it.
struct UserEnv {
    names: Vec<&'static str>,
    _one: MutexGuard<'static, ()>,
}

impl UserEnv {
    fn set(vars: &[(&'static str, &str)]) -> Self {
        let one = one_at_a_time();
        git(); // The hermetic config is in place before anything here touches the environment.
        for (name, value) in vars {
            // SAFETY: every test in this binary holds `one_at_a_time` from its start, so no
            // other test thread reads the environment meanwhile. The only threads that
            // outlive a test are `refusing_http_remote`'s servers, which never read it.
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

/// How a Network Job ended: its exit code, or `None` if it outlasted [`HANG`] and was stopped.
#[derive(Debug)]
struct JobEnd {
    exit: Option<i32>,
    stderr: String,
    took: Duration,
}

/// Runs `args` as a Network Job in `repo`, stopping it if it outlasts [`HANG`].
fn run_network_job(repo: &Repo, args: &[&str]) -> JobEnd {
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
    JobEnd { exit, stderr, took }
}

/// `program` meeting GitHub's host key for the first time, which it asks whether to trust.
/// Needs GitHub's port 22. No key is offered: `ssh` stops at the question. The caller holds
/// [`one_at_a_time`].
fn ssh_meeting_an_unknown_host(program: &str, null: &str) -> JobEnd {
    let repo = Repo::new();
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = forward_slashes(&dir.path().join("known_hosts"));
    let command = format!(
        "core.sshCommand='{program}' -F {null} -o UserKnownHostsFile='{known_hosts}' \
         -o GlobalKnownHostsFile={null} -o IdentitiesOnly=yes -o IdentityAgent=none \
         -o ConnectTimeout=10 -o StrictHostKeyChecking=ask"
    );
    run_network_job(
        &repo,
        &["-c", &command, "ls-remote", "git@github.com:git/git.git"],
    )
}

#[track_caller]
fn assert_refused_with_ssh_s_own_words(end: &JobEnd) {
    assert!(end.exit.is_some(), "the Job waited on a Prompt: {end:?}");
    assert!(end.took < Duration::from_secs(15), "{end:?}");
    assert!(
        end.stderr.starts_with("Host key verification failed."),
        "{end:?}"
    );
}

#[cfg(windows)]
const WINDOWS_SSH: &str = "C:/Windows/System32/OpenSSH/ssh.exe";

#[cfg(windows)]
fn windows_ssh_meeting_an_unknown_host() -> JobEnd {
    assert!(
        std::path::Path::new(WINDOWS_SSH).exists(),
        "this test needs Windows' OpenSSH client"
    );
    ssh_meeting_an_unknown_host(WINDOWS_SSH, "NUL")
}

/// Windows' own `ssh.exe` waited, silently, until stopped (#44).
#[cfg(windows)]
#[test]
fn windows_ssh_refuses_an_unknown_host_instead_of_waiting() {
    let _one = one_at_a_time();
    let end = windows_ssh_meeting_an_unknown_host();
    assert_refused_with_ssh_s_own_words(&end);
}

/// The helper is started from wherever the app is, which on Windows can be a path with spaces
/// and backslashes. Here it is a copy of `cairn-askpass` at such a path, named the way a user's
/// own askpass is.
#[cfg(windows)]
#[test]
fn windows_ssh_starts_the_helper_from_a_path_with_spaces() {
    let dir = tempfile::tempdir().unwrap();
    let spaced = dir.path().join("Program Files").join("Cairn App");
    std::fs::create_dir_all(&spaced).unwrap();
    let helper = spaced.join("cairn askpass.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_cairn-askpass"), &helper).unwrap();
    let missing = spaced.join("missing askpass.exe");

    let started = {
        let _env = UserEnv::set(&[("SSH_ASKPASS", helper.to_str().unwrap())]);
        windows_ssh_meeting_an_unknown_host()
    };
    assert_refused_with_ssh_s_own_words(&started);
    // Control: an askpass `ssh` can't start reads differently, so the refusal above came from
    // the helper.
    let not_started = {
        let _env = UserEnv::set(&[("SSH_ASKPASS", missing.to_str().unwrap())]);
        windows_ssh_meeting_an_unknown_host()
    };
    assert_ne!(started.stderr, not_started.stderr, "{not_started:?}");
}

/// The `ssh` git finds by itself: macOS's, or Git for Windows' own. Run from a terminal, it
/// would ask there and wait.
#[test]
fn the_default_ssh_refuses_an_unknown_host_instead_of_waiting() {
    let _one = one_at_a_time();
    let end = ssh_meeting_an_unknown_host("ssh", "/dev/null");
    // Seen red: with `cairn-askpass` answering `yes`, which shows `ssh` asks it.
    assert_refused_with_ssh_s_own_words(&end);
}

/// The askpass variables an `ssh` command is given: it reports them, one per line, and exits.
/// The caller holds [`one_at_a_time`].
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
    let command = format!("core.sshCommand=sh '{}'", forward_slashes(&script));
    run_network_job(
        &repo,
        &["-c", &command, "ls-remote", "ssh://git@cairn.invalid/r.git"],
    )
    .stderr
}

#[test]
fn a_user_s_own_ssh_askpass_reaches_ssh_unchanged() {
    let _env = UserEnv::set(&[("SSH_ASKPASS", "/the users/own askpass")]);
    let seen = askpass_seen_by_ssh();
    // Seen red: with the runner setting SSH_ASKPASS whatever the user's environment says.
    assert_eq!(
        ssh_askpass_in(&seen),
        Some("/the users/own askpass"),
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
    assert!(
        ssh_askpass_in(&seen).is_some_and(is_cairns_askpass),
        "{seen}"
    );
}

/// An empty value names no askpass and no mode, so it is no choice to keep.
#[test]
fn an_empty_ssh_askpass_or_require_counts_as_unset() {
    let _env = UserEnv::set(&[("SSH_ASKPASS", ""), ("SSH_ASKPASS_REQUIRE", "")]);
    let seen = askpass_seen_by_ssh();
    // Seen red: with the runner keeping an empty value as the user's choice.
    assert!(
        ssh_askpass_in(&seen).is_some_and(is_cairns_askpass),
        "{seen}"
    );
    assert!(
        seen.lines().any(|l| l == "SSH_ASKPASS_REQUIRE=force"),
        "{seen}"
    );
}

/// A server that refuses every request and asks for Basic auth, as a private remote does.
/// Returns its URL. It ends with the test binary, and never reads the environment.
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

/// `ls-remote` against a remote that wants a username, with no credential helper. Plain HTTP:
/// git asks for credentials over HTTPS through the same prompt. The caller holds
/// [`one_at_a_time`].
fn ls_remote_needing_credentials() -> JobEnd {
    let repo = Repo::new();
    let url = refusing_http_remote();
    run_network_job(
        &repo,
        &[
            "-c",
            "http.proxy=",
            "-c",
            "credential.helper=",
            "ls-remote",
            &url,
        ],
    )
}

/// git's own username prompt is refused as it always was. git asks `SSH_ASKPASS` when nothing
/// else is set, and an askpass that fails adds a line to git's error (the control below), so
/// an unchanged error also shows that Cairn's helper wasn't run.
#[test]
fn git_s_own_https_prompt_fails_as_before_without_asking_cairn_s_helper() {
    let _one = one_at_a_time();
    let end = ls_remote_needing_credentials();
    assert_eq!(end.exit, Some(128), "{end:?}");
    // Seen red: with the runner no longer setting GIT_ASKPASS to empty.
    assert!(
        end.stderr
            .starts_with("fatal: could not read Username for 'http://127.0.0.1:")
            && end.stderr.ends_with("': terminal prompts disabled\n"),
        "{end:?}"
    );
}

/// The control for the test above: when git does ask Cairn's helper, its error says so.
#[test]
fn control_git_asking_cairn_s_helper_adds_a_line_to_its_error() {
    let helper = env!("CARGO_BIN_EXE_cairn-askpass");
    let _env = UserEnv::set(&[("GIT_ASKPASS", helper)]);
    let end = ls_remote_needing_credentials();
    // Seen red: with `cairn-askpass` answering, which git takes as a username.
    assert!(
        end.stderr
            .starts_with("error: unable to read askpass response from "),
        "{end:?}"
    );
}

/// A user's own `GIT_ASKPASS` still answers git's prompts.
#[test]
fn a_user_s_own_git_askpass_still_answers_git() {
    let _one = one_at_a_time();
    let dir = tempfile::tempdir().unwrap();
    let askpass = dir.path().join("askpass.sh");
    std::fs::write(&askpass, "#!/bin/sh\necho asked >&2\necho cairn-user\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&askpass, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let askpass = forward_slashes(&askpass);
    drop(_one);
    let _env = UserEnv::set(&[("GIT_ASKPASS", askpass.as_str())]);
    let end = ls_remote_needing_credentials();
    // Seen red: with the runner setting GIT_ASKPASS to empty whatever the user's environment
    // says.
    assert!(end.stderr.starts_with("asked\n"), "{end:?}");
}
