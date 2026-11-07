//! Status timings on the kernel worktree, and status parity with `git status --porcelain=v2`.
//!
//! The bench changes the worktree (edits, a staged edit, a staged rename and deletion, a
//! worktree deletion, untracked and ignored files), times status, and restores exactly
//! those paths. A manifest is written first so `restore` can undo a crashed run.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use gix::bstr::{BString, ByteSlice};
use gix::status::UntrackedFiles;

use crate::{Args, git_output, git_run, hunks, measure, open, table_header, table_row, time_git, wants};

const MANIFEST: &str = "results/kernel-mutation.manifest";
const INDEX_BACKUP: &str = "results/kernel-index.backup";

pub fn run(args: &Args) -> Result<()> {
    println!("## Status on {}\n", args.repo.display());
    let baseline = baseline(args)?;
    println!("Baseline before changes: {} paths, all `.M` with equal hashes.\n", baseline.len());

    let plan = Mutation::plan(args, &baseline)?;
    let guard = plan.apply(args)?;

    // Settle the stat cache the way a normal session would, outside the timed runs, and past
    // the racy-git second so edited files aren't re-hashed on every run.
    std::thread::sleep(Duration::from_millis(1100));
    git_run(&args.git, &args.repo, &["update-index", "-q", "--refresh"]).ok();

    let result = (|| -> Result<()> {
        let report = parity(args, &args.repo, &baseline)?;
        report.print("kernel");

        let mut hunk_paths: Vec<(String, bool)> = plan.edited.iter().map(|p| (p.clone(), false)).collect();
        hunk_paths.push((plan.staged_then_edited.clone(), false));
        hunk_paths.push((plan.staged_then_edited.clone(), true));
        hunks::compare_and_print(args, &args.repo, &hunk_paths)?;

        timings(args, &plan)
    })();
    drop(guard);
    let after = baseline_lines(args)?;
    println!(
        "Restored: worktree status is {} the baseline.\n",
        if after == baseline.values().cloned().collect::<BTreeSet<_>>() { "back to" } else { "NOT back to" }
    );
    result
}

fn timings(args: &Args, plan: &Mutation) -> Result<()> {
    let full_floor = if cfg!(windows) { Duration::from_secs(1) } else { Duration::from_millis(500) };
    let watched: Vec<BString> = plan.edited.iter().map(|p| BString::from(p.as_str())).collect();
    let watched_git: Vec<&str> = plan.edited.iter().map(String::as_str).collect();
    let nol = "--no-optional-locks";

    println!("### Status timings\n");
    let mut counts = Vec::new();
    table_header();
    let repo_path = &args.repo;

    if wants(args, "HEAD vs index") {
        let (gix, n) = measure(args, || {
            let repo = open(repo_path)?;
            let start = Instant::now();
            let n = head_vs_index(&repo)?;
            Ok((start.elapsed(), n))
        })?;
        let (git, _) = time_git(args, repo_path, &[nol, "diff-index", "--cached", "-M", "--name-status", "HEAD"])?;
        table_row("HEAD vs index", Some(Duration::from_millis(50)), &gix, Some(&git));
        counts.push(format!("{n} changes"));
    }
    if wants(args, "watcher") {
        let (gix, n) = measure(args, || {
            let repo = open(repo_path)?;
            let start = Instant::now();
            let n = index_worktree(&repo, &watched)?;
            Ok((start.elapsed(), n))
        })?;
        let mut git_args = vec![nol, "diff-files", "--name-status", "--"];
        git_args.extend(&watched_git);
        let (git, _) = time_git(args, repo_path, &git_args)?;
        table_row("watcher index vs worktree, 10 paths", Some(Duration::from_millis(30)), &gix, Some(&git));
        counts.push(format!("{n} changes"));

        // Diagnostic: a long-running app holds the index in memory between watcher events.
        let (held, _) = measure(args, || {
            let repo = open(repo_path)?;
            let index = repo.index()?;
            let start = Instant::now();
            let iter = repo
                .status(gix::progress::Discard)?
                .index(gix::worktree::IndexPersistedOrInMemory::Persisted(index))
                .untracked_files(UntrackedFiles::None)
                .into_index_worktree_iter(watched.clone())?;
            let mut n = 0;
            for item in iter {
                item?;
                n += 1;
            }
            Ok((start.elapsed(), n))
        })?;
        table_row("  watcher, index already loaded", None, &held, None);
    }
    if wants(args, "full status") {
        let (gix, n) = measure(args, || {
            let repo = open(repo_path)?;
            let start = Instant::now();
            let n = full_status(&repo, UntrackedFiles::Collapsed)?;
            Ok((start.elapsed(), n))
        })?;
        let (git, _) = time_git(args, repo_path, &[nol, "status", "--porcelain=v2"])?;
        table_row("full status with untracked", Some(full_floor), &gix, Some(&git));
        counts.push(format!("full status: {n} items"));
    }
    if wants(args, "index vs worktree, whole tree") {
        let (gix, _) = measure(args, || {
            let repo = open(repo_path)?;
            let start = Instant::now();
            let n = index_worktree(&repo, &[])?;
            Ok((start.elapsed(), n))
        })?;
        let (git, _) = time_git(args, repo_path, &[nol, "diff-files", "--name-status"])?;
        table_row("index vs worktree, whole tree", None, &gix, Some(&git));
    }
    if wants(args, "full status, no untracked") {
        let (gix, _) = measure(args, || {
            let repo = open(repo_path)?;
            let start = Instant::now();
            let n = full_status(&repo, UntrackedFiles::None)?;
            Ok((start.elapsed(), n))
        })?;
        let (git, _) = time_git(args, repo_path, &[nol, "status", "--porcelain=v2", "-uno"])?;
        table_row("full status, no untracked", None, &gix, Some(&git));
    }
    if wants(args, "HEAD vs index") {
        let (read, entries) = measure(args, || {
            let repo = open(repo_path)?;
            let start = Instant::now();
            let index = repo.index()?;
            Ok((start.elapsed(), index.entries().len()))
        })?;
        table_row("  part: read index", None, &read, None);
        let (tree, _) = measure(args, || {
            let repo = open(repo_path)?;
            let start = Instant::now();
            let tree = repo.head_tree_id()?;
            let state = repo.index_from_tree(&tree)?;
            Ok((start.elapsed(), state.entries().len()))
        })?;
        table_row("  part: index from HEAD tree", None, &tree, None);
        counts.push(format!("index has {entries} entries"));
    }
    println!("\nCounts: {}.", counts.join(", "));
    println!(
        "gix times start on a freshly opened repo and include reading the index. `git` is run with \
         `--no-optional-locks` so it doesn't rewrite the index, process spawn included. \
         HEAD vs index is `diff-index --cached -M`, the watcher case is `diff-files -- <10 paths>`, \
         full status is `status --porcelain=v2`.\n"
    );
    Ok(())
}

fn head_vs_index(repo: &gix::Repository) -> Result<usize> {
    let index = repo.index()?;
    let head_tree = repo.head_tree_id()?;
    let mut n = 0;
    repo.tree_index_status(
        &head_tree,
        &index,
        None,
        gix::status::tree_index::TrackRenames::AsConfigured,
        |_, _, _| {
            n += 1;
            Ok(std::ops::ControlFlow::Continue(()))
        },
    )?;
    Ok(n)
}

fn index_worktree(repo: &gix::Repository, paths: &[BString]) -> Result<usize> {
    let iter = repo
        .status(gix::progress::Discard)?
        .untracked_files(UntrackedFiles::None)
        .into_index_worktree_iter(paths.to_vec())?;
    let mut n = 0;
    for item in iter {
        item?;
        n += 1;
    }
    Ok(n)
}

fn full_status(repo: &gix::Repository, untracked: UntrackedFiles) -> Result<usize> {
    let iter = repo
        .status(gix::progress::Discard)?
        .untracked_files(untracked)
        .into_iter(Vec::<BString>::new())?;
    let mut n = 0;
    for item in iter {
        item?;
        n += 1;
    }
    Ok(n)
}

// ---------------------------------------------------------------- baseline

/// The 13 case-colliding paths. Every line must be `1 .M` with equal hashes, nothing else.
fn baseline(args: &Args) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for line in baseline_lines(args)? {
        let path = line_path(&line).to_owned();
        let f: Vec<&str> = line.splitn(9, ' ').collect();
        let ok = f.len() == 9 && f[0] == "1" && f[1] == ".M" && f[6] == f[7];
        if !ok {
            bail!("worktree isn't at its baseline, refusing to change it: {line}");
        }
        out.insert(path, line);
    }
    Ok(out)
}

fn baseline_lines(args: &Args) -> Result<BTreeSet<String>> {
    let out = git_output(&args.git, &args.repo, &["status", "--porcelain=v2"])?;
    Ok(String::from_utf8(out)?.lines().map(str::to_owned).collect())
}

// ---------------------------------------------------------------- mutation

struct Mutation {
    edited: Vec<String>,
    staged_then_edited: String,
    renamed: (String, String),
    staged_deleted: String,
    worktree_deleted: String,
    untracked: Vec<String>,
    ignored: String,
}

struct Guard {
    repo: PathBuf,
    git: PathBuf,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Err(e) = restore(&self.git, &self.repo) {
            eprintln!("RESTORE FAILED, run `restore`: {e:#}");
        }
    }
}

impl Mutation {
    fn plan(args: &Args, baseline: &BTreeMap<String, String>) -> Result<Self> {
        let listing = git_output(&args.git, &args.repo, &["ls-files", "-s", "-z"])?;
        let mut candidates = Vec::new();
        for rec in listing.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let rec = rec.to_str()?;
            let (meta, path) = rec.split_once('\t').context("ls-files record")?;
            let plain = path.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b));
            if meta.starts_with("100644 ") && plain && !baseline.contains_key(path) && path.ends_with(".c") {
                candidates.push(path.to_owned());
            }
        }
        // Spread the picks across the tree, keeping only small text files.
        let mut picks = Vec::new();
        let step = candidates.len() / 40;
        for path in candidates.iter().step_by(step.max(1)) {
            let data = std::fs::read(args.repo.join(path))?;
            if data.len() < 200_000 && !data.contains(&0) && data.lines().count() >= 20 {
                picks.push(path.clone());
            }
            if picks.len() == 14 {
                break;
            }
        }
        if picks.len() < 14 {
            bail!("couldn't find 14 suitable files");
        }
        let renamed_from = picks[11].clone();
        Ok(Mutation {
            edited: picks[..10].to_vec(),
            staged_then_edited: picks[10].clone(),
            renamed: (renamed_from.clone(), format!("{renamed_from}.cairn-renamed.c")),
            staged_deleted: picks[12].clone(),
            worktree_deleted: picks[13].clone(),
            untracked: vec![
                "cairn-spike-untracked.txt".into(),
                "Documentation/cairn-spike-note.txt".into(),
                "cairn-spike-dir/a.txt".into(),
                "cairn-spike-dir/sub/b.txt".into(),
            ],
            ignored: "cairn-spike-ignored.o".into(),
        })
    }

    fn apply(&self, args: &Args) -> Result<Guard> {
        let repo = &args.repo;
        if Path::new(MANIFEST).exists() {
            bail!("{MANIFEST} exists: a previous run didn't restore. Run `restore` first.");
        }
        std::fs::create_dir_all("results")?;
        let git_dir = repo.join(".git");
        std::fs::copy(git_dir.join("index"), INDEX_BACKUP)?;

        let mut manifest = format!("repo\t{}\n", repo.display());
        let mut tracked: Vec<&str> = self.edited.iter().map(String::as_str).collect();
        tracked.extend([
            self.staged_then_edited.as_str(),
            self.renamed.0.as_str(),
            self.staged_deleted.as_str(),
            self.worktree_deleted.as_str(),
        ]);
        for p in &tracked {
            manifest += &format!("tracked\t{p}\n");
        }
        let mut removed: Vec<&str> = self.untracked.iter().map(String::as_str).collect();
        removed.extend([self.renamed.1.as_str(), self.ignored.as_str(), "cairn-spike-dir"]);
        for p in &removed {
            manifest += &format!("remove\t{p}\n");
        }
        std::fs::write(MANIFEST, manifest)?;
        let guard = Guard { repo: repo.clone(), git: args.git.clone() };

        for p in &self.edited {
            edit(&repo.join(p))?;
        }
        edit(&repo.join(&self.staged_then_edited))?;
        git_run(&args.git, repo, &["add", "--", &self.staged_then_edited])?;
        edit(&repo.join(&self.staged_then_edited))?;
        git_run(&args.git, repo, &["mv", "--", &self.renamed.0, &self.renamed.1])?;
        git_run(&args.git, repo, &["rm", "-q", "--", &self.staged_deleted])?;
        std::fs::remove_file(repo.join(&self.worktree_deleted))?;
        for p in self.untracked.iter().chain([&self.ignored]) {
            let path = repo.join(p);
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(path, "cairn spike\n")?;
        }
        Ok(guard)
    }
}

/// Replaces the middle line and inserts one a quarter of the way in, so hunks land mid-file.
fn edit(path: &Path) -> Result<()> {
    let data = std::fs::read(path)?;
    let mut lines: Vec<&[u8]> = data.lines_with_terminator().collect();
    let mid = lines.len() / 2;
    lines[mid] = b"/* cairn spike: replaced line */\n";
    lines.insert(lines.len() / 4, b"/* cairn spike: inserted line */\n");
    std::fs::write(path, lines.concat())?;
    Ok(())
}

pub fn restore_from_manifest(args: &Args) -> Result<()> {
    if !Path::new(MANIFEST).exists() {
        println!("Nothing to restore.");
        return Ok(());
    }
    restore(&args.git, &args.repo)?;
    println!("Restored.");
    Ok(())
}

fn restore(git: &Path, expected_repo: &Path) -> Result<()> {
    let manifest = std::fs::read_to_string(MANIFEST)?;
    let mut repo = expected_repo.to_owned();
    let mut tracked = Vec::new();
    let mut removed = Vec::new();
    for line in manifest.lines() {
        match line.split_once('\t') {
            Some(("repo", p)) => repo = p.into(),
            Some(("tracked", p)) => tracked.push(p.to_owned()),
            Some(("remove", p)) => removed.push(p.to_owned()),
            _ => bail!("bad manifest line {line}"),
        }
    }
    std::fs::copy(INDEX_BACKUP, repo.join(".git").join("index"))?;
    let mut checkout = vec!["checkout".to_owned(), "--".to_owned()];
    checkout.extend(tracked);
    let checkout: Vec<&str> = checkout.iter().map(String::as_str).collect();
    git_run(git, &repo, &checkout)?;
    for p in removed {
        let path = repo.join(&p);
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else if path.exists() {
            std::fs::remove_file(&path)?;
        }
    }
    std::fs::remove_file(MANIFEST)?;
    std::fs::remove_file(INDEX_BACKUP).ok();
    Ok(())
}

// ---------------------------------------------------------------- parity

pub struct Parity {
    pub matched: usize,
    pub baseline_matched: usize,
    pub baseline_total: usize,
    pub only_gix: Vec<String>,
    pub only_git: Vec<String>,
    pub notes: Vec<String>,
}

impl Parity {
    pub fn print(&self, fixture: &str) {
        let pass = self.only_gix.is_empty() && self.only_git.is_empty();
        println!(
            "### Status parity, {fixture}: {}\n\n{} lines identical to `git status --porcelain=v2`, outside the baseline. \
             Baseline paths: {} of {} lines identical (excluded from the result).",
            if pass { "PASS" } else { "FAIL" },
            self.matched,
            self.baseline_matched,
            self.baseline_total
        );
        for l in &self.only_git {
            println!("- only git: `{l}`");
        }
        for l in &self.only_gix {
            println!("- only gix: `{l}`");
        }
        for n in &self.notes {
            println!("- note: {n}");
        }
        println!();
    }
}

pub fn parity(args: &Args, repo_path: &Path, baseline: &BTreeMap<String, String>) -> Result<Parity> {
    let git: BTreeSet<String> = String::from_utf8(git_output(&args.git, repo_path, &["--no-optional-locks", "status", "--porcelain=v2"])?)?
        .lines()
        .map(str::to_owned)
        .collect();
    let (gix, notes) = gix_porcelain(&open(repo_path)?)?;
    let is_base = |l: &String| baseline.contains_key(line_path(l));
    let mut p = Parity {
        matched: git.intersection(&gix).filter(|l| !is_base(l)).count(),
        baseline_matched: git.intersection(&gix).filter(|l| is_base(l)).count(),
        baseline_total: baseline.len(),
        only_gix: gix.difference(&git).filter(|l| !is_base(l)).cloned().collect(),
        only_git: git.difference(&gix).filter(|l| !is_base(l)).cloned().collect(),
        notes,
    };
    for l in gix.difference(&git).filter(|l| is_base(l)) {
        p.notes.push(format!("baseline, only gix: `{l}`"));
    }
    for l in git.difference(&gix).filter(|l| is_base(l)) {
        p.notes.push(format!("baseline, only git: `{l}`"));
    }
    Ok(p)
}

fn line_path(line: &str) -> &str {
    match line.as_bytes().first() {
        Some(b'1') => line.splitn(9, ' ').nth(8).unwrap_or(""),
        Some(b'2') => line.splitn(10, ' ').nth(9).unwrap_or("").split('\t').next().unwrap_or(""),
        Some(b'?') | Some(b'!') => line.get(2..).unwrap_or("").trim_end_matches('/'),
        _ => line,
    }
}

enum Staged {
    Add { mode: String, id: String },
    Del { mode: String, id: String },
    Mod { prev_mode: String, prev_id: String, mode: String, id: String, kind_changed: bool },
    Rename { source: String, source_mode: String, source_id: String, mode: String, id: String, copy: bool },
}

fn hex(id: &gix::oid) -> String {
    id.to_hex().to_string()
}

fn mode(m: gix::index::entry::Mode) -> String {
    format!("{:06o}", m.bits())
}

/// Renders gix's full status as `git status --porcelain=v2` lines (untracked collapsed, as `-unormal`).
pub fn gix_porcelain(repo: &gix::Repository) -> Result<(BTreeSet<String>, Vec<String>)> {
    let zero = gix::ObjectId::null(repo.object_hash()).to_string();
    let mut notes = Vec::new();
    let mut staged: BTreeMap<String, Staged> = BTreeMap::new();
    let mut worktree: BTreeMap<String, char> = BTreeMap::new();
    let mut lines = BTreeSet::new();

    let mut iter = repo
        .status(gix::progress::Discard)?
        .untracked_files(UntrackedFiles::Collapsed)
        .into_iter(Vec::<BString>::new())?;
    for item in iter.by_ref() {
        use gix::diff::index::ChangeRef as C;
        use gix::status::index_worktree::Item as W;
        use gix::status::plumbing::index_as_worktree::{Change, EntryStatus};
        match item? {
            gix::status::Item::TreeIndex(change) => match change {
                C::Addition { location, entry_mode, id, .. } => {
                    staged.insert(location.to_string(), Staged::Add { mode: mode(entry_mode), id: hex(&id) });
                }
                C::Deletion { location, entry_mode, id, .. } => {
                    staged.insert(location.to_string(), Staged::Del { mode: mode(entry_mode), id: hex(&id) });
                }
                C::Modification { location, previous_entry_mode, previous_id, entry_mode, id, .. } => {
                    staged.insert(
                        location.to_string(),
                        Staged::Mod {
                            prev_mode: mode(previous_entry_mode),
                            prev_id: hex(&previous_id),
                            mode: mode(entry_mode),
                            id: hex(&id),
                            kind_changed: previous_entry_mode.to_tree_entry_mode().map(|m| m.kind())
                                != entry_mode.to_tree_entry_mode().map(|m| m.kind()),
                        },
                    );
                }
                C::Rewrite { source_location, source_entry_mode, source_id, location, entry_mode, id, copy, .. } => {
                    staged.insert(
                        location.to_string(),
                        Staged::Rename {
                            source: source_location.to_string(),
                            source_mode: mode(source_entry_mode),
                            source_id: hex(&source_id),
                            mode: mode(entry_mode),
                            id: hex(&id),
                            copy,
                        },
                    );
                }
            },
            gix::status::Item::IndexWorktree(W::Modification { rela_path, status, .. }) => {
                let c = match status {
                    EntryStatus::Change(Change::Removed) => 'D',
                    EntryStatus::Change(Change::Type { .. }) => 'T',
                    EntryStatus::Change(Change::Modification { .. }) => 'M',
                    EntryStatus::Change(Change::SubmoduleModification(_)) => 'M',
                    EntryStatus::IntentToAdd => 'A',
                    EntryStatus::NeedsUpdate(_) => continue,
                    EntryStatus::Conflict { .. } => {
                        notes.push(format!("conflict at {rela_path}, not rendered"));
                        continue;
                    }
                };
                worktree.insert(rela_path.to_string(), c);
            }
            gix::status::Item::IndexWorktree(W::DirectoryContents { entry, collapsed_directory_status }) => {
                if collapsed_directory_status.is_none() && entry.status == gix::dir::entry::Status::Untracked {
                    let dir = entry.disk_kind.is_some_and(|k| k.is_dir());
                    lines.insert(format!("? {}{}", entry.rela_path, if dir { "/" } else { "" }));
                }
            }
            gix::status::Item::IndexWorktree(W::Rewrite { source, dirwalk_entry, .. }) => {
                notes.push(format!(
                    "index-worktree rename {} -> {} reported, not rendered",
                    source.rela_path(),
                    dirwalk_entry.rela_path
                ));
            }
        }
    }
    let outcome = iter.into_outcome().context("status outcome")?;
    let index = &outcome.worktree_index;
    let workdir = repo.workdir().context("workdir")?.to_owned();

    let paths: BTreeSet<String> = staged.keys().chain(worktree.keys()).cloned().collect();
    for path in paths {
        let y = worktree.get(&path).copied().unwrap_or('.');
        let entry = index.entry_by_path(path.as_bytes().as_bstr());
        let (im, ih) = entry.map(|e| (mode(e.mode), hex(&e.id))).unwrap_or(("000000".into(), zero.clone()));
        let (x, hm, hh, im, ih, rename) = match staged.get(&path) {
            None => ('.', im.clone(), ih.clone(), im, ih, None),
            Some(Staged::Add { mode, id }) => ('A', "000000".into(), zero.clone(), mode.clone(), id.clone(), None),
            Some(Staged::Del { mode, id }) => ('D', mode.clone(), id.clone(), "000000".into(), zero.clone(), None),
            Some(Staged::Mod { prev_mode, prev_id, mode, id, kind_changed }) => {
                (if *kind_changed { 'T' } else { 'M' }, prev_mode.clone(), prev_id.clone(), mode.clone(), id.clone(), None)
            }
            Some(Staged::Rename { source, source_mode, source_id, mode, id, copy }) => {
                let score = if source_id == id { "100".to_owned() } else { "??".to_owned() };
                let tag = format!("{}{score}", if *copy { 'C' } else { 'R' });
                (if *copy { 'C' } else { 'R' }, source_mode.clone(), source_id.clone(), mode.clone(), id.clone(), Some((tag, source.clone())))
            }
        };
        let wm = if y == 'D' || (x == 'D' && y == '.') {
            "000000".to_owned()
        } else if y == '.' {
            im.clone()
        } else {
            worktree_mode(&workdir.join(&path)).unwrap_or_else(|| "000000".into())
        };
        lines.insert(match rename {
            None => format!("1 {x}{y} N... {hm} {im} {wm} {hh} {ih} {path}"),
            Some((tag, source)) => format!("2 {x}{y} N... {hm} {im} {wm} {hh} {ih} {tag} {path}\t{source}"),
        });
    }
    Ok((lines, notes))
}

fn worktree_mode(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() {
        return Some("120000".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 != 0 {
            return Some("100755".into());
        }
    }
    Some("100644".into())
}
