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

fn run(path: &Path, stop: &Stop, args: &[&str]) -> Result<Output, String> {
    run_raw(path, stop, args)?
        .success()
        .map_err(|e| e.to_string())
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
    let head = if symbolic.status.success() {
        Head::Branch {
            name: RefName::new(String::from_utf8_lossy(&symbolic.stdout).trim().to_owned()),
            target: target
                .status
                .success()
                .then(|| Oid::from_hex(String::from_utf8_lossy(&target.stdout).trim().to_owned())),
        }
    } else if target.status.success() {
        Head::Detached {
            target: Oid::from_hex(String::from_utf8_lossy(&target.stdout).trim().to_owned()),
        }
    } else {
        return Err(String::from_utf8_lossy(&target.stderr).into_owned());
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
