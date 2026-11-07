//! Finding the git binary Cairn runs, once per run.
//!
//! Both OSes put a shim on `PATH` that costs a few milliseconds a call (#12): Git for
//! Windows' `Git\cmd\git.exe` and macOS's `/usr/bin/git`. Resolution looks past them to the
//! real binary, then checks its version.
//! See `docs/adr/0005-runner-runs-the-real-git-binary.md`.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;

use crate::{ResolveError, git_command};

/// A git version, as `git --version` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

/// The git binary Cairn runs, with the version it reported.
#[derive(Debug, Clone)]
pub struct Git {
    path: PathBuf,
    version: GitVersion,
}

impl Git {
    /// The git for this run, resolved on first use and shared after that.
    pub fn shared() -> Result<&'static Git, ResolveError> {
        static SHARED: OnceLock<Result<Git, ResolveError>> = OnceLock::new();
        SHARED
            .get_or_init(Git::resolve)
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Finds the real git binary on this machine and checks its version.
    fn resolve() -> Result<Git, ResolveError> {
        Git::at(find()?)
    }

    /// Uses the git binary at `path`, after checking that it runs and is new enough.
    pub fn at(path: impl Into<PathBuf>) -> Result<Git, ResolveError> {
        let path = path.into();
        if !path.is_file() {
            return Err(ResolveError::NotFound {
                reason: format!("there is no git at {}", path.display()),
            });
        }
        let out = git_command(&path)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| ResolveError::Unusable {
                path: path.clone(),
                reason: e.to_string(),
            })?;
        let text = String::from_utf8_lossy(&out.stdout);
        let version = match (out.status.success(), GitVersion::parse(&text)) {
            (true, Some(version)) => version,
            _ => {
                return Err(ResolveError::Unusable {
                    path,
                    reason: format!(
                        "`git --version` printed {:?}, {}",
                        text.trim(),
                        String::from_utf8_lossy(&out.stderr).trim()
                    ),
                });
            }
        };
        if version < GitVersion::MINIMUM {
            return Err(ResolveError::TooOld { path, version });
        }
        Ok(Git { path, version })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn version(&self) -> GitVersion {
        self.version
    }
}

impl GitVersion {
    /// The oldest git Cairn runs: `switch` and `restore` arrived in 2.23.
    pub const MINIMUM: GitVersion = GitVersion {
        major: 2,
        minor: 23,
        patch: 0,
    };

    /// Reads `git --version` output such as `git version 2.39.3 (Apple Git-145)` or
    /// `git version 2.45.1.windows.1`.
    fn parse(text: &str) -> Option<GitVersion> {
        let rest = text.trim().strip_prefix("git version ")?;
        let mut parts = rest
            .split(|c: char| !c.is_ascii_digit())
            .map(|p| p.parse::<u32>().ok());
        Some(GitVersion {
            major: parts.next()??,
            minor: parts.next()??,
            patch: parts.next().flatten().unwrap_or(0),
        })
    }
}

impl fmt::Display for GitVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

const EXE: &str = if cfg!(windows) { "git.exe" } else { "git" };

/// Every `git` on `PATH`, in order.
fn on_path() -> Vec<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join(EXE))
        .filter(|p| p.is_file())
        .collect()
}

/// macOS: the first git on `PATH` that isn't the `/usr/bin/git` shim, such as one from
/// Homebrew, or else the one `xcrun --find git` names.
#[cfg(target_os = "macos")]
fn find() -> Result<PathBuf, ResolveError> {
    const SHIM: &str = "/usr/bin/git";
    let is_shim =
        |p: &Path| p == Path::new(SHIM) || p.canonicalize().is_ok_and(|c| c == Path::new(SHIM));
    if let Some(git) = on_path().into_iter().find(|p| !is_shim(p)) {
        return Ok(git);
    }
    let out = std::process::Command::new("/usr/bin/xcrun")
        .args(["--find", "git"])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output();
    let not_found = || ResolveError::NotFound {
        reason: "no git was found on PATH and `xcrun --find git` found none. Install the \
                 Xcode Command Line Tools (`xcode-select --install`) or another git."
            .to_owned(),
    };
    match out {
        Ok(out) if out.status.success() => {
            let git = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
            if git.is_file() {
                Ok(git)
            } else {
                Err(not_found())
            }
        }
        _ => Err(not_found()),
    }
}

/// The MSYS2 environments Git for Windows ships its real binary in: `mingw64` historically,
/// `ucrt64` in current releases, `clangarm64` on ARM.
#[cfg(windows)]
const WINDOWS_ENVIRONMENTS: [&str; 3] = ["mingw64", "ucrt64", "clangarm64"];

/// Windows: the real `<environment>\bin\git.exe` behind the first git on `PATH`, found next to
/// the `cmd` (or `bin`) shim or through `git --exec-path`.
#[cfg(windows)]
fn find() -> Result<PathBuf, ResolveError> {
    let Some(first) = on_path().into_iter().next() else {
        return Err(ResolveError::NotFound {
            reason: "no git.exe was found on PATH. Install Git for Windows.".to_owned(),
        });
    };
    // Git\cmd\git.exe and Git\bin\git.exe: the real binary is Git\<environment>\bin\git.exe.
    if let Some(root) = first.parent().and_then(Path::parent) {
        let real = WINDOWS_ENVIRONMENTS
            .iter()
            .map(|env| root.join(env).join("bin").join(EXE))
            .find(|candidate| candidate.is_file());
        if let Some(real) = real {
            return Ok(real);
        }
    }
    // Otherwise ask git: its exec path is <prefix>\libexec\git-core, beside <prefix>\bin.
    if let Ok(out) = git_command(&first)
        .arg("--exec-path")
        .stdin(Stdio::null())
        .output()
        && out.status.success()
    {
        let exec_path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        if let Some(prefix) = exec_path.parent().and_then(Path::parent) {
            let candidate = prefix.join("bin").join(EXE);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    let in_cmd = first
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|d| d.eq_ignore_ascii_case("cmd"));
    if in_cmd {
        return Err(ResolveError::NotFound {
            reason: format!(
                "only the {} shim was found, and the git.exe it runs couldn't be located",
                first.display()
            ),
        });
    }
    Ok(first)
}

/// Other OSes have no shim to skip: the first git on `PATH`.
#[cfg(not(any(target_os = "macos", windows)))]
fn find() -> Result<PathBuf, ResolveError> {
    on_path()
        .into_iter()
        .next()
        .ok_or_else(|| ResolveError::NotFound {
            reason: "no git was found on PATH".to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_version_lines_git_prints() {
        let v = |major, minor, patch| GitVersion {
            major,
            minor,
            patch,
        };
        assert_eq!(
            GitVersion::parse("git version 2.39.3 (Apple Git-145)\n"),
            Some(v(2, 39, 3))
        );
        assert_eq!(
            GitVersion::parse("git version 2.45.1.windows.1"),
            Some(v(2, 45, 1))
        );
        assert_eq!(GitVersion::parse("git version 2.23.0"), Some(v(2, 23, 0)));
        assert_eq!(GitVersion::parse("git version 3.0"), Some(v(3, 0, 0)));
        assert_eq!(GitVersion::parse("hello"), None);
    }

    #[test]
    fn orders_versions_numerically() {
        let v = |major, minor, patch| GitVersion {
            major,
            minor,
            patch,
        };
        assert!(v(2, 22, 9) < GitVersion::MINIMUM);
        assert!(v(2, 23, 0) >= GitVersion::MINIMUM);
        assert!(v(2, 100, 0) > v(2, 23, 0));
    }
}
