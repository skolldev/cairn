//! The repositories samples run in. Each fixture is proved to have the condition it names by
//! real git, because a sample that can't change anything would wait for nothing.

mod common;

use cairn_budget::fixture::{BRANCH_A, BRANCH_B, KEEP, KEEP_DIR, SAVED, Sandbox, TRACKED};
use common::{git_read, small_fixture};

fn untracked(repo: &std::path::Path) -> Vec<String> {
    git_read(
        repo,
        &["ls-files", "--others", "--exclude-standard", "--directory"],
    )
    .lines()
    .map(str::to_owned)
    .collect()
}

#[test]
fn switching_branches_flips_the_untracked_list_and_moves_head() {
    let sandbox = Sandbox::clone_of(&small_fixture()).unwrap();
    let repo = sandbox.repo();

    assert_eq!(
        git_read(repo, &["symbolic-ref", "--short", "HEAD"]).trim(),
        BRANCH_A
    );
    assert_eq!(
        untracked(repo),
        [KEEP_DIR],
        "on {BRANCH_A} nothing beside the file is tracked"
    );
    assert_eq!(Sandbox::untracked_on(BRANCH_A), KEEP_DIR);

    sandbox.switch(BRANCH_B).unwrap();
    assert_eq!(
        git_read(repo, &["symbolic-ref", "--short", "HEAD"]).trim(),
        BRANCH_B
    );
    // Seen red: with BRANCH_B tracking its file outside the directory, here; with it changing
    // `.gitignore` too, in the diff below.
    assert_eq!(untracked(repo), [KEEP], "on {BRANCH_B} a file beside it is");
    assert_eq!(Sandbox::untracked_on(BRANCH_B), KEEP);
    assert_eq!(
        git_read(repo, &["diff", "--name-only", BRANCH_A, BRANCH_B]).trim(),
        TRACKED,
        "the index flips it, and `.gitignore` is the same on both"
    );

    sandbox.switch(BRANCH_A).unwrap();
    assert_eq!(untracked(repo), [KEEP_DIR], "and back");
    assert!(
        git_read(repo, &["status", "--porcelain", "--untracked-files=no"]).is_empty(),
        "a switch leaves the worktree clean"
    );
}

#[test]
fn a_save_makes_the_file_unstaged_and_restoring_it_clears_it() {
    let sandbox = Sandbox::clone_of(&small_fixture()).unwrap();
    let repo = sandbox.repo();

    sandbox.save(true).unwrap();
    assert_eq!(git_read(repo, &["diff", "--name-only"]).trim(), SAVED);
    sandbox.save(false).unwrap();
    assert!(git_read(repo, &["diff", "--name-only"]).is_empty());
}

#[test]
fn the_sandbox_is_a_copy_and_leaves_the_fixture_alone() {
    let fixture = small_fixture();
    let before = git_read(&fixture, &["rev-parse", "HEAD"]);
    // What the fixture's own diff holds already. A checkout made under another config can read as
    // modified under this one: a Windows clone with `core.autocrlf`, on CI.
    let diff_before = git_read(&fixture, &["diff", "--name-only"]);
    let sandbox = Sandbox::clone_of(&fixture).unwrap();
    assert_eq!(sandbox.fixture_head(), before.trim());
    sandbox.save(true).unwrap();
    assert!(
        !git_read(&fixture, &["branch", "--list", BRANCH_A]).contains(BRANCH_A),
        "the branches are the copy's"
    );
    assert_eq!(git_read(&fixture, &["diff", "--name-only"]), diff_before);
}
