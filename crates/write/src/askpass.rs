//! Where the runner finds `cairn-askpass`, the helper every git run gets as `SSH_ASKPASS`
//! (ADR 0008).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const HELPER: &str = "cairn-askpass";

/// The helper's path: beside the running program, as the app and the helper are built and
/// shipped, or one directory up, where cargo puts test binaries. Found once per run. If it
/// isn't there, the path beside the program is still named: `ssh` then fails to start it and
/// stops with its own error, so a Prompt still never waits.
pub(crate) fn helper() -> &'static Path {
    static HELPER_PATH: OnceLock<PathBuf> = OnceLock::new();
    HELPER_PATH.get_or_init(|| {
        let name = format!("{HELPER}{}", std::env::consts::EXE_SUFFIX);
        let exe = std::env::current_exe().unwrap_or_default();
        let beside = exe.parent().map(|dir| dir.join(&name));
        let above = exe
            .parent()
            .and_then(Path::parent)
            .map(|dir| dir.join(&name));
        [&beside, &above]
            .into_iter()
            .flatten()
            .find(|path| path.is_file())
            .or(beside.as_ref())
            .cloned()
            .unwrap_or_else(|| PathBuf::from(name))
    })
}
