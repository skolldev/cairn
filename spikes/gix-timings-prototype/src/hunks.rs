//! Full hunk parity: gix's unified hunks against `git diff -U3`, line terminators kept.

use std::path::Path;

use anyhow::{Context, Result, bail};
use gix::bstr::{BStr, ByteSlice};
use gix::diff::blob::pipeline::{Mode, WorktreeRoots};
use gix::diff::blob::platform::prepare_diff::Operation;
use gix::diff::blob::unified_diff::{ConsumeHunk, ContextSize, DiffLineKind, HunkHeader};
use gix::diff::blob::{InternedInput, ResourceKind, UnifiedDiff};

use crate::{Args, git_output, open};

#[derive(Debug, PartialEq, Eq)]
pub struct Hunk {
    /// Before start, before length, after start, after length.
    header: (u32, u32, u32, u32),
    lines: Vec<(u8, Vec<u8>)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FileDiff {
    Binary,
    Text(Vec<Hunk>),
}

/// Compares each `(path, staged)`: staged is HEAD vs index, otherwise index vs worktree.
pub fn compare_and_print(args: &Args, repo_path: &Path, paths: &[(String, bool)]) -> Result<bool> {
    let repo = open(repo_path)?;
    let mut all = true;
    let mut lines = Vec::new();
    let mut hunks = 0;
    for (path, staged) in paths {
        let gix = gix_diff(&repo, path, *staged).with_context(|| format!("gix diff of {path}"))?;
        let mut git_args = vec!["diff", "--no-color", "--no-ext-diff", "--no-textconv", "-U3"];
        if *staged {
            git_args.push("--cached");
        }
        git_args.extend(["--", path.as_str()]);
        let git = parse_git(&git_output(&args.git, repo_path, &git_args)?)?;
        let side = if *staged { "staged" } else { "unstaged" };
        if let FileDiff::Text(h) = &gix {
            hunks += h.len();
        }
        if gix == git {
            lines.push(format!("- {side} `{path}`: identical ({})", describe(&gix)));
        } else {
            all = false;
            lines.push(format!("- {side} `{path}`: DIFFERENT. gix {}, git {}", describe(&gix), describe(&git)));
            lines.push(format!("  - gix: {gix:?}"));
            lines.push(format!("  - git: {git:?}"));
        }
    }
    println!(
        "### Hunk parity: {}\n\n{} files, {hunks} gix hunks, compared with `git diff -U3` byte for byte, terminators included.\n",
        if all { "PASS" } else { "FAIL" },
        paths.len()
    );
    for l in lines {
        println!("{l}");
    }
    println!();
    Ok(all)
}

fn describe(d: &FileDiff) -> String {
    match d {
        FileDiff::Binary => "binary".into(),
        FileDiff::Text(h) => format!(
            "{} hunks at {}",
            h.len(),
            h.iter().map(|h| format!("-{},{} +{},{}", h.header.0, h.header.1, h.header.2, h.header.3)).collect::<Vec<_>>().join(" ")
        ),
    }
}

pub fn gix_diff(repo: &gix::Repository, path: &str, staged: bool) -> Result<FileDiff> {
    let rela: &BStr = path.as_bytes().as_bstr();
    let index = repo.index()?;
    let entry = index.entry_by_path(rela).context("path not in index")?;
    let index_kind = entry.mode.to_tree_entry_mode().context("index mode")?.kind();
    let workdir = repo.workdir().context("workdir")?.to_owned();
    let roots = if staged {
        WorktreeRoots::default()
    } else {
        WorktreeRoots { old_root: None, new_root: Some(workdir) }
    };
    let mut cache = repo.diff_resource_cache(Mode::ToGit, roots)?;
    if staged {
        let head = repo.head_tree()?.peel_to_entry_by_path(path)?.context("path not in HEAD")?;
        cache.set_resource(head.object_id(), head.mode().kind(), rela, ResourceKind::OldOrSource, &repo.objects).map_err(ex)?;
        cache.set_resource(entry.id, index_kind, rela, ResourceKind::NewOrDestination, &repo.objects).map_err(ex)?;
    } else {
        cache.set_resource(entry.id, index_kind, rela, ResourceKind::OldOrSource, &repo.objects).map_err(ex)?;
        let null = gix::ObjectId::null(repo.object_hash());
        cache.set_resource(null, index_kind, rela, ResourceKind::NewOrDestination, &repo.objects).map_err(ex)?;
    }
    let out = cache.prepare_diff().map_err(ex)?;
    let algorithm = match out.operation {
        Operation::InternalDiff { algorithm } => algorithm,
        Operation::SourceOrDestinationIsBinary => return Ok(FileDiff::Binary),
        Operation::ExternalCommand { .. } => bail!("external diff configured"),
    };
    let old = out.old.data.as_slice().unwrap_or_default();
    let new = out.new.data.as_slice().unwrap_or_default();
    // Byte lines keep their terminators, so a CRLF to LF change isn't read as no change.
    let input = InternedInput::new(old, new);
    let diff = gix::diff::blob::diff_with_slider_heuristics(algorithm, &input);
    let hunks = UnifiedDiff::new(&diff, &input, Collect(Vec::new()), ContextSize::symmetrical(3)).consume()?;
    Ok(FileDiff::Text(hunks))
}

struct Collect(Vec<Hunk>);

impl ConsumeHunk for Collect {
    type Out = Vec<Hunk>;
    fn consume_hunk(&mut self, h: HunkHeader, lines: &[(DiffLineKind, &[u8])]) -> std::io::Result<()> {
        self.0.push(Hunk {
            header: (h.before_hunk_start, h.before_hunk_len, h.after_hunk_start, h.after_hunk_len),
            lines: lines.iter().map(|(k, l)| (k.to_prefix() as u8, l.to_vec())).collect(),
        });
        Ok(())
    }
    fn finish(self) -> Vec<Hunk> {
        self.0
    }
}

/// Parses one file's `git diff` output into hunks, counting lines from each header.
fn parse_git(out: &[u8]) -> Result<FileDiff> {
    let mut lines = out.lines_with_terminator().peekable();
    let mut hunks = Vec::new();
    while let Some(line) = lines.next() {
        if line.starts_with(b"Binary files ") {
            return Ok(FileDiff::Binary);
        }
        if !line.starts_with(b"@@ -") {
            continue;
        }
        let header = parse_header(line.to_str()?)?;
        let (mut before, mut after) = (header.1, header.3);
        let mut body: Vec<(u8, Vec<u8>)> = Vec::new();
        while before > 0 || after > 0 || lines.peek().is_some_and(|l| l.starts_with(b"\\")) {
            let l = lines.next().context("hunk ended early")?;
            match l[0] {
                b'\\' => {
                    if let Some((_, last)) = body.last_mut() {
                        if last.ends_with(b"\n") {
                            last.pop();
                        }
                    }
                    continue;
                }
                b' ' => {
                    before -= 1;
                    after -= 1;
                }
                b'-' => before -= 1,
                b'+' => after -= 1,
                other => bail!("unexpected hunk line prefix {:?}", other as char),
            }
            body.push((l[0], l[1..].to_vec()));
        }
        hunks.push(Hunk { header, lines: body });
    }
    Ok(FileDiff::Text(hunks))
}

fn parse_header(line: &str) -> Result<(u32, u32, u32, u32)> {
    // "@@ -a[,b] +c[,d] @@ ..."
    let mut parts = line.split(' ');
    parts.next();
    let range = |s: Option<&str>| -> Result<(u32, u32)> {
        let s = s.context("hunk header")?;
        let s = &s[1..];
        Ok(match s.split_once(',') {
            Some((a, b)) => (a.parse()?, b.parse()?),
            None => (s.parse()?, 1),
        })
    };
    let (a, b) = range(parts.next())?;
    let (c, d) = range(parts.next())?;
    Ok((a, b, c, d))
}

fn ex<E: std::fmt::Debug>(e: E) -> anyhow::Error {
    anyhow::anyhow!("{e:?}")
}
