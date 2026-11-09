//! The one place the harness starts a process: `cairn`, PresentMon and what reports the machine.
//! `scripts/check-single-git-spawn.sh` allows this file and nothing else outside `cairn-write`,
//! because the rule is that git is started with the runner's rules. So this refuses git.

use std::path::Path;
use std::process::Command;

/// A command for a program that isn't git. git goes through `cairn_write::Git`.
pub fn tool(program: impl AsRef<std::ffi::OsStr>) -> Result<Command, String> {
    let program = program.as_ref();
    // Both separators, so that a Windows path reads the same on every OS.
    let text = program.to_string_lossy();
    let name = text.rsplit(['/', '\\']).next().unwrap_or(&text);
    let stem = Path::new(name).file_stem().unwrap_or_default();
    if stem.eq_ignore_ascii_case("git") {
        return Err(format!(
            "{text} is git, which only cairn_write::Git may start"
        ));
    }
    Ok(Command::new(program))
}
