//! Binary resolution: Cairn runs the real git, never a shim, and refuses one it can't use.

use cairn_write::{Git, GitVersion, ResolveError};

#[test]
fn resolves_a_git_at_least_the_minimum() {
    let git = Git::shared().expect("a usable git is installed");
    assert!(git.path().is_file(), "{} is a file", git.path().display());
    assert!(git.version() >= GitVersion::MINIMUM, "{}", git.version());
}

#[cfg(target_os = "macos")]
#[test]
fn never_resolves_the_macos_shim() {
    let git = Git::shared().expect("a usable git is installed");
    assert_ne!(git.path(), std::path::Path::new("/usr/bin/git"));
}

#[cfg(windows)]
#[test]
fn resolves_the_real_git_for_windows_binary_not_the_cmd_shim() {
    let git = Git::shared().expect("a usable git is installed");
    let parts: Vec<String> = git
        .path()
        .components()
        .rev()
        .take(3)
        .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    assert_eq!(
        parts,
        ["git.exe", "bin", "mingw64"],
        "{}",
        git.path().display()
    );
}

/// The oldest-git CI job sets `CAIRN_EXPECT_GIT_VERSION`, so a job that silently picked up a
/// newer system git fails instead of passing on the wrong version.
#[test]
fn resolves_the_git_version_ci_expects() {
    let Some(expected) = std::env::var_os("CAIRN_EXPECT_GIT_VERSION") else {
        return;
    };
    let git = Git::shared().expect("a usable git is installed");
    assert_eq!(git.version().to_string(), expected.to_string_lossy());
}

#[test]
fn a_missing_git_is_not_found_rather_than_a_spawn_failure() {
    let dir = tempfile::tempdir().unwrap();
    let err = Git::at(dir.path().join("git")).unwrap_err();
    assert!(matches!(err, ResolveError::NotFound { .. }), "{err:?}");
}

#[cfg(unix)]
#[test]
fn a_git_older_than_2_23_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    // Real old gits aren't installable everywhere; this stands in for `git --version` only.
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("git");
    std::fs::write(&old, "#!/bin/sh\necho 'git version 2.22.5'\n").unwrap();
    std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o755)).unwrap();

    let err = Git::at(&old).unwrap_err();
    assert_eq!(
        err,
        ResolveError::TooOld {
            path: old.clone(),
            version: GitVersion {
                major: 2,
                minor: 22,
                patch: 5
            },
        }
    );
    assert_eq!(
        err.to_string(),
        format!(
            "Cairn needs git 2.23.0 or newer, but {} is git 2.22.5",
            old.display()
        )
    );
}
