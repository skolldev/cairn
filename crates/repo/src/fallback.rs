//! Git CLI reads when gix cannot represent a repository reliably.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use cairn_write::{Git, Invocation, JobClass, Output, Stop};

use crate::{Head, JobContext, Oid, RefEntry, RefName, Refs, RepoPath, Stash};

#[derive(Default)]
pub(super) struct ReadMode {
    cli: AtomicBool,
    failures: AtomicUsize,
    detected: Mutex<Option<bool>>,
}

impl ReadMode {
    fn detect(path: &Path, stop: &Stop) -> Result<bool, String> {
        if cairn_read::Repo::open(path).is_ok_and(|repo| repo.reduced_trust()) {
            return Ok(true);
        }
        let output = run_raw(
            path,
            stop,
            &[
                "config",
                "--get-regexp",
                "^(index\\.sparse|extensions\\.partialclone|remote\\..*\\.promisor|core\\.safecrlf)$",
            ],
        )?;
        // Exit 1 is "no key matches", an answer. Any other failure is git failing, whose
        // empty stdout says nothing about the repository.
        let Some(output) = answer(output)? else {
            return Ok(false);
        };
        let config = String::from_utf8_lossy(&output.stdout);
        let enabled = config.lines().any(|line| {
            let Some((key, value)) = line.split_once(' ') else {
                return false;
            };
            let value = value.trim();
            match key.to_ascii_lowercase().as_str() {
                "index.sparse" | "core.safecrlf" => value.eq_ignore_ascii_case("true"),
                "extensions.partialclone" => !value.is_empty(),
                key if key.starts_with("remote.") && key.ends_with(".promisor") => {
                    value.eq_ignore_ascii_case("true")
                }
                _ => false,
            }
        });
        Ok(enabled)
    }

    pub(super) fn read<T>(
        &self,
        path: &Path,
        job: &JobContext,
        cli: fn(&Path, &Stop) -> Result<T, String>,
        gix: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let detected = {
            let mut detected = self.detected.lock().unwrap();
            match *detected {
                Some(value) => value,
                None => {
                    let value = Self::detect(path, job.cli_stop())?;
                    *detected = Some(value);
                    value
                }
            }
        };
        if detected || self.cli.load(Ordering::Relaxed) {
            return cli(path, job.cli_stop());
        }
        match gix() {
            Ok(value) => Ok(value),
            Err(error) => {
                if job.is_current() {
                    if self.failures.fetch_add(1, Ordering::Relaxed) + 1 >= 2 {
                        self.cli.store(true, Ordering::Relaxed);
                    }
                    tracing::warn!(%error, "gix read failed; retrying with git");
                    cli(path, job.cli_stop())
                } else {
                    Err(error)
                }
            }
        }
    }
}

fn run_raw(path: &Path, stop: &Stop, args: &[&str]) -> Result<Output, String> {
    Git::shared()
        .map_err(|e| e.to_string())?
        .run(
            &Invocation::new(JobClass::Read, path)
                .args(args)
                .stop_on(stop),
        )
        .map_err(|e| e.to_string())
}

/// Git's answer from a command that exits 1 for "no", such as `symbolic-ref -q`: the output
/// on exit 0, `None` on exit 1. Any other exit is git failing, and its stderr is the error.
fn answer(output: Output) -> Result<Option<Output>, String> {
    match output.status.code() {
        Some(0) => Ok(Some(output)),
        Some(1) => Ok(None),
        _ => Err(output.stderr_text().into_owned()),
    }
}

/// The error when both HEAD commands answer "no" (#77). `-q` usually leaves their stderr
/// empty, so the error says what that means, then adds whatever either did print.
fn neither_branch_nor_commit(symbolic: &Output, target: &Output) -> String {
    let mut error = "HEAD names neither a branch nor a commit".to_owned();
    for output in [symbolic, target] {
        let stderr = output.stderr_text();
        let stderr = stderr.trim();
        if !stderr.is_empty() {
            error.push('\n');
            error.push_str(stderr);
        }
    }
    error
}

fn run(path: &Path, stop: &Stop, args: &[&str]) -> Result<Output, String> {
    run_raw(path, stop, args)?
        .success()
        .map_err(|e| e.to_string())
}

/// History from all refs and HEAD. With no commit-graph this is the walk's engine rather
/// than a fallback (#13). `--date-order` overrides `--topo-order` and implies it.
pub(super) fn walk(path: &Path, stop: &Stop) -> Result<Vec<Oid>, String> {
    let output = run(
        path,
        stop,
        &["rev-list", "--topo-order", "--date-order", "--all"],
    )?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(Oid::from_hex)
        .collect())
}

pub(super) fn untracked(path: &Path, stop: &Stop) -> Result<Vec<RepoPath>, String> {
    let output = run(path, stop, &["status", "--porcelain=v2", "-z"])?;
    Ok(output
        .records()
        .filter_map(|record| record.strip_prefix(b"? "))
        .map(|bytes| RepoPath::new(bytes.to_vec()))
        .collect())
}

pub(super) fn refs(path: &Path, stop: &Stop) -> Result<Refs, String> {
    let output = run(
        path,
        stop,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(*objectname)%00%(symref)",
            "refs/heads/",
            "refs/remotes/",
            "refs/tags/",
        ],
    )?;
    let mut local = Vec::new();
    let mut remote = Vec::new();
    let mut tags = Vec::new();
    for line in output
        .stdout
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let fields: Vec<_> = line.split(|&byte| byte == 0).collect();
        if fields.len() != 4 {
            return Err("git for-each-ref returned an invalid record".into());
        }
        let name = String::from_utf8_lossy(fields[0]).into_owned();
        let target = Oid::from_hex(String::from_utf8_lossy(fields[1]).into_owned());
        let peeled = (!fields[2].is_empty())
            .then(|| Oid::from_hex(String::from_utf8_lossy(fields[2]).into_owned()));
        let symref = (!fields[3].is_empty())
            .then(|| RefName::new(String::from_utf8_lossy(fields[3]).into_owned()));
        let entry = RefEntry {
            name: RefName::new(&name),
            target,
            peeled,
            symref,
        };
        if name.starts_with("refs/heads/") {
            local.push(entry)
        } else if name.starts_with("refs/remotes/") {
            remote.push(entry)
        } else if name.starts_with("refs/tags/") {
            tags.push(entry)
        }
    }
    let symbolic = run_raw(path, stop, &["symbolic-ref", "-q", "HEAD"])?;
    let target = run_raw(path, stop, &["rev-parse", "-q", "--verify", "HEAD"])?;
    let neither = neither_branch_nor_commit(&symbolic, &target);
    let target = answer(target)?
        .map(|target| Oid::from_hex(String::from_utf8_lossy(&target.stdout).trim().to_owned()));
    let head = match (answer(symbolic)?, target) {
        (Some(symbolic), target) => Head::Branch {
            name: RefName::new(String::from_utf8_lossy(&symbolic.stdout).trim().to_owned()),
            target,
        },
        (None, Some(target)) => Head::Detached { target },
        (None, None) => return Err(neither),
    };
    let stash = run(path, stop, &["stash", "list", "--format=%H%x00%gs"])?;
    let stashes = stash
        .stdout
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let separator = line
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| "git stash list returned an invalid record".to_owned())?;
            let (id, message) = (&line[..separator], &line[separator + 1..]);
            Ok(Stash {
                target: Oid::from_hex(String::from_utf8_lossy(id).into_owned()),
                message: String::from_utf8_lossy(message).into_owned(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Refs {
        head,
        local,
        remote,
        tags,
        stashes,
    })
}
