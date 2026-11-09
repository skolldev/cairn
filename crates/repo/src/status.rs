//! The Staged and Unstaged lists, read through the git CLI runner (#13 put both on it).

use std::path::Path;

use cairn_core::{ChangeKind, FileChange, RepoPath};
use cairn_write::{Git, Invocation, JobClass, Output, RunError, Stop};

/// HEAD vs index: `git diff-index --cached`, with renames found as `git status` finds them.
pub(crate) fn staged(workdir: &Path, stop: &Stop) -> Result<Vec<FileChange>, String> {
    let git = Git::shared().map_err(|e| e.to_string())?;
    let against_head = diff_index(git, workdir, stop, "HEAD")?;
    let out = match against_head.status.success() {
        true => against_head,
        // An unborn branch has no HEAD to compare with, so everything staged is Added,
        // as `git status` shows it. Any other failure is git's to report.
        false if head_is_unborn(git, workdir, stop)? => {
            let empty_tree = empty_tree(git, workdir, stop)?;
            diff_index(git, workdir, stop, &empty_tree)?
        }
        false => against_head,
    };
    let out = out.success().map_err(text)?;
    let mut changes: Vec<FileChange> = parse_raw(&out)?.into_iter().map(Raw::into_change).collect();
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

/// Index vs worktree: `git diff-files`, without the entries that are only stat-dirty.
///
/// `diff-files` doesn't refresh the index, so a file whose stat info changed but whose
/// content didn't (touched, copied, checked out by another tool) is listed as modified.
/// `git status` checks such files' content, so this does too, with one `git hash-object`
/// for every candidate. It applies the same clean filters and line-ending conversion.
pub(crate) fn unstaged(workdir: &Path, stop: &Stop) -> Result<Vec<FileChange>, String> {
    unstaged_of(workdir, stop, Vec::new())
}

/// [`unstaged`] again after the worktree changed only at `changed`, relative paths with `/`
/// separators, starting from `last`, the list it gave before. Only right if the index is as
/// it was when `last` was read: an entry's answer changes only if it or its file did.
///
/// A path inside a submodule or a nested repository asks about that repository's own entry,
/// whose answer depends on what's inside it.
pub(crate) fn unstaged_within(
    workdir: &Path,
    stop: &Stop,
    last: &[FileChange],
    changed: &[&[u8]],
) -> Result<Vec<FileChange>, String> {
    let asked: Vec<&[u8]> = changed
        .iter()
        .map(|&p| outermost_repo(workdir, p))
        .collect();
    let mut pathspecs = Vec::with_capacity(asked.len());
    for path in &asked {
        // Case-insensitive, so that on a case-insensitive filesystem every entry that names
        // the changed file in another case is asked about too. Asking about more entries
        // than changed is only slower; [`covers`] folds case the same way.
        let mut spec = b":(literal,icase)".to_vec();
        spec.extend_from_slice(path);
        pathspecs.push(os(spec).ok_or("a changed path isn't valid UTF-8")?);
    }
    let read = unstaged_of(workdir, stop, pathspecs)?;
    // git may fold more than ASCII, so it can answer for an entry that `covers` keeps.
    let mut changes: Vec<FileChange> = last
        .iter()
        .filter(|c| !asked.iter().any(|p| covers(p, c.path.as_bytes())))
        .filter(|c| !read.iter().any(|r| r.path == c.path))
        .cloned()
        .collect();
    changes.extend(read);
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

/// The topmost directory above `path` that holds a `.git`, such as a submodule, or `path`
/// itself if there is none.
fn outermost_repo<'p>(workdir: &Path, path: &'p [u8]) -> &'p [u8] {
    let path = path.strip_suffix(b"/").unwrap_or(path);
    path.iter()
        .enumerate()
        .filter(|&(_, &b)| b == b'/')
        .map(|(i, _)| &path[..i])
        .find(|dir| {
            std::str::from_utf8(dir).is_ok_and(|dir| workdir.join(dir).join(".git").exists())
        })
        .unwrap_or(path)
}

/// Whether git's `:(literal,icase)` pathspec `spec` matches `path`: the same path or one
/// under it, ignoring ASCII case as git's `icase` does.
fn covers(spec: &[u8], path: &[u8]) -> bool {
    path.get(..spec.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(spec))
        && (path.len() == spec.len() || path[spec.len()] == b'/')
}

/// Bytes as an argument. Paths come from the filesystem, so they're Unicode on Windows.
fn os(bytes: Vec<u8>) -> Option<std::ffi::OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        Some(std::ffi::OsString::from_vec(bytes))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes).ok().map(Into::into)
    }
}

/// `diff-files` for every entry, or only those `pathspecs` match, without the stat-only ones.
fn unstaged_of(
    workdir: &Path,
    stop: &Stop,
    pathspecs: Vec<std::ffi::OsString>,
) -> Result<Vec<FileChange>, String> {
    let git = Git::shared().map_err(|e| e.to_string())?;
    let mut args: Vec<std::ffi::OsString> = vec!["diff-files".into(), "-z".into()];
    if !pathspecs.is_empty() {
        args.push("--".into());
        args.extend(pathspecs);
    }
    let out = git
        .run(&read(workdir, stop).args(args))
        .map_err(text)?
        .success()
        .map_err(text)?;
    let raw = parse_raw(&out)?;

    // Only a same-mode modification of a regular file can be stat-only. git can't hash a
    // symlink's target through `hash-object`, and a submodule's change is its own.
    let candidates: Vec<usize> = raw
        .iter()
        .enumerate()
        .filter(|(_, r)| r.status == b'M' && r.src_mode == r.dst_mode && is_regular(&r.src_mode))
        .map(|(i, _)| i)
        .collect();
    let mut unchanged = vec![false; raw.len()];
    if !candidates.is_empty() {
        let mut stdin = Vec::new();
        for &i in &candidates {
            c_quote(raw[i].path.as_bytes(), &mut stdin);
            stdin.push(b'\n');
        }
        let hashed = git
            .run(
                &read(workdir, stop)
                    .args(["hash-object", "--stdin-paths"])
                    .stdin(stdin),
            )
            .map_err(text)?
            .success()
            .map_err(text)?;
        let ids: Vec<&[u8]> = hashed
            .stdout
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .collect();
        if ids.len() != candidates.len() {
            return Err(format!(
                "git hash-object returned {} ids for {} paths",
                ids.len(),
                candidates.len()
            ));
        }
        for (&i, id) in candidates.iter().zip(ids) {
            unchanged[i] = id == raw[i].src_oid.as_bytes();
        }
    }
    let mut changes: Vec<FileChange> = raw
        .into_iter()
        .zip(unchanged)
        .filter(|(_, unchanged)| !unchanged)
        .map(|(r, _)| r.into_change())
        .collect();
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

fn diff_index(git: &Git, workdir: &Path, stop: &Stop, tree: &str) -> Result<Output, String> {
    git.run(&read(workdir, stop).args(["diff-index", "--cached", "-z", "-M", tree]))
        .map_err(text)
}

/// True if HEAD names no commit yet. `rev-parse -q --verify` exits 1 for that, and 128
/// when it can't read the repository at all.
fn head_is_unborn(git: &Git, workdir: &Path, stop: &Stop) -> Result<bool, String> {
    let out = git
        .run(&read(workdir, stop).args(["rev-parse", "-q", "--verify", "HEAD"]))
        .map_err(text)?;
    Ok(out.status.code() == Some(1))
}

/// The empty tree's id in this repository's hash.
fn empty_tree(git: &Git, workdir: &Path, stop: &Stop) -> Result<String, String> {
    let out = git
        .run(
            &read(workdir, stop)
                .args(["hash-object", "-t", "tree", "--stdin"])
                .stdin(Vec::new()),
        )
        .map_err(text)?
        .success()
        .map_err(text)?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// A Read invocation that `stop` kills, with everything it started such as a clean
/// filter, once a newer Read Job Supersedes this one.
fn read(workdir: &Path, stop: &Stop) -> Invocation {
    Invocation::new(JobClass::Read, workdir).stop_on(stop)
}

fn text(e: RunError) -> String {
    e.to_string()
}

fn is_regular(mode: &str) -> bool {
    mode == "100644" || mode == "100755"
}

/// One record of git's raw diff format.
struct Raw {
    src_mode: String,
    dst_mode: String,
    src_oid: String,
    status: u8,
    score: u8,
    /// The source of a rename.
    from: Option<RepoPath>,
    path: RepoPath,
}

impl Raw {
    fn into_change(self) -> FileChange {
        let kind = match (self.status, self.from) {
            (b'R', Some(from)) => ChangeKind::Renamed {
                from,
                score: self.score,
            },
            (b'A', _) => ChangeKind::Added,
            (b'D', _) => ChangeKind::Deleted,
            (b'T', _) => ChangeKind::TypeChanged,
            (b'U', _) => ChangeKind::Unmerged,
            _ => ChangeKind::Modified,
        };
        FileChange {
            path: self.path,
            kind,
        }
    }
}

/// Parses `-z` raw output: `:srcmode dstmode srcoid dstoid STATUS[score]`, then the path,
/// then for a rename the destination path, each its own record.
fn parse_raw(out: &Output) -> Result<Vec<Raw>, String> {
    let malformed = |what: &[u8]| {
        format!(
            "unexpected git diff output: {:?}",
            String::from_utf8_lossy(what)
        )
    };
    let mut records = out.records();
    let mut parsed = Vec::new();
    while let Some(header) = records.next() {
        let fields: Vec<&str> =
            std::str::from_utf8(header.strip_prefix(b":").ok_or_else(|| malformed(header))?)
                .map_err(|_| malformed(header))?
                .split(' ')
                .collect();
        let [src_mode, dst_mode, src_oid, _dst_oid, status] = fields[..] else {
            return Err(malformed(header));
        };
        let (&letter, score) = status
            .as_bytes()
            .split_first()
            .ok_or_else(|| malformed(header))?;
        // Only a rename carries a score. An unreadable one means malformed output.
        let score = match score {
            [] => 0,
            digits => std::str::from_utf8(digits)
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| malformed(header))?,
        };
        let mut next_path = || {
            records
                .next()
                .map(RepoPath::new)
                .ok_or_else(|| malformed(header))
        };
        let first = next_path()?;
        // `diff-index -M` finds renames but never copies, so no `C` record arrives.
        let (from, path) = if letter == b'R' {
            (Some(first), next_path()?)
        } else {
            (None, first)
        };
        parsed.push(Raw {
            src_mode: src_mode.to_owned(),
            dst_mode: dst_mode.to_owned(),
            src_oid: src_oid.to_owned(),
            status: letter,
            score,
            from,
            path,
        });
    }
    Ok(parsed)
}

/// Quotes a path the way `hash-object --stdin-paths` unquotes it, so newlines, a leading
/// `"` and control characters all survive. Other bytes, UTF-8 included, pass through.
fn c_quote(path: &[u8], out: &mut Vec<u8>) {
    out.push(b'"');
    for &b in path {
        match b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\r' => out.extend_from_slice(b"\\r"),
            0x00..=0x1f | 0x7f => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
            _ => out.push(b),
        }
    }
    out.push(b'"');
}
