//! The repository samples run in: a throwaway clone of a fixture, so the fixtures themselves
//! stay untouched, with two branches whose only difference shows on screen.

use std::path::{Path, PathBuf};

use cairn_write::{Git, Invocation, JobClass, Output};

pub const BRANCH_A: &str = "budget-a";
pub const BRANCH_B: &str = "budget-b";
/// A tracked file that a save changes.
pub const SAVED: &str = "budget-saved.txt";
/// A file in the worktree, untracked on [`BRANCH_A`] and ignored on [`BRANCH_B`]. A switch leaves
/// the Staged and Unstaged lists as they were, so this is what shows that status caught up.
pub const KEEP: &str = "budget.keep";

const SAVED_CLEAN: &str = "as committed\n";
const SAVED_DIRTY: &str = "as committed\nand changed by a save\n";

fn git() -> Result<&'static Git, String> {
    Git::shared().map_err(|e| e.to_string())
}

fn run(invocation: Invocation) -> Result<Output, String> {
    git()?
        .run(&invocation)
        .and_then(Output::success)
        .map_err(|e| e.to_string())
}

fn local_write<'a>(dir: &Path, args: impl IntoIterator<Item = &'a str>) -> Result<Output, String> {
    run(Invocation::new(JobClass::LocalWrite, dir)
        // Setup commits in a large clone mustn't leave a gc running under the measurements.
        .args(["-c", "gc.auto=0", "-c", "maintenance.auto=false"])
        .args([
            "-c",
            "user.name=Cairn Budget",
            "-c",
            "user.email=budget@cairn.invalid",
        ])
        .args(args))
}

/// `git rev-parse HEAD` in a repository.
pub fn head_of(repo: &Path) -> Result<String, String> {
    let out = run(Invocation::new(JobClass::Read, repo).args(["rev-parse", "HEAD"]))?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// A throwaway clone of a fixture, on [`BRANCH_A`].
pub struct Sandbox {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    fixture_head: String,
}

impl Sandbox {
    /// Clones `fixture` and sets up the branches. A local clone also copies its commit-graph.
    pub fn clone_of(fixture: &Path) -> Result<Self, String> {
        let fixture_head = head_of(fixture)?;
        let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
        let repo = dir.path().join("copy");
        let (from, to) = (path_text(fixture)?, path_text(&repo)?);
        local_write(dir.path(), ["clone", "--quiet", from, to])?;

        local_write(&repo, ["switch", "--quiet", "-c", BRANCH_A])?;
        std::fs::write(repo.join(SAVED), SAVED_CLEAN).map_err(|e| e.to_string())?;
        local_write(&repo, ["add", "--", SAVED])?;
        local_write(
            &repo,
            ["commit", "--quiet", "--no-verify", "-m", "budget: a"],
        )?;

        local_write(&repo, ["switch", "--quiet", "-c", BRANCH_B])?;
        let ignore = repo.join(".gitignore");
        let mut ignored = std::fs::read_to_string(&ignore).unwrap_or_default();
        if !ignored.is_empty() && !ignored.ends_with('\n') {
            ignored.push('\n');
        }
        ignored.push_str(&format!("/{KEEP}\n"));
        std::fs::write(&ignore, ignored).map_err(|e| e.to_string())?;
        local_write(&repo, ["add", "--", ".gitignore"])?;
        local_write(
            &repo,
            ["commit", "--quiet", "--no-verify", "-m", "budget: b"],
        )?;

        local_write(&repo, ["switch", "--quiet", BRANCH_A])?;
        std::fs::write(repo.join(KEEP), "kept\n").map_err(|e| e.to_string())?;
        Ok(Self {
            _dir: dir,
            repo,
            fixture_head,
        })
    }

    pub fn repo(&self) -> &Path {
        &self.repo
    }

    /// The commit the fixture itself was at.
    pub fn fixture_head(&self) -> &str {
        &self.fixture_head
    }

    /// `git switch`, run outside Cairn.
    pub fn switch(&self, branch: &str) -> Result<(), String> {
        local_write(
            &self.repo,
            ["switch", "--quiet", "--no-guess", "--", branch],
        )
        .map(drop)
    }

    /// Saves [`SAVED`] as a modified file or back as committed.
    pub fn save(&self, modified: bool) -> Result<(), String> {
        let content = if modified { SAVED_DIRTY } else { SAVED_CLEAN };
        std::fs::write(self.repo.join(SAVED), content).map_err(|e| e.to_string())
    }
}

fn path_text(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("{} isn't UTF-8", path.display()))
}
