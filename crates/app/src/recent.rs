//! The recent repositories: the ones that opened, newest first, kept in a small text file in
//! the user's app-data folder so the list survives a restart. A repository joins the list
//! once its Open succeeds, never when it is chosen, and leaves only when the user removes it.

#[cfg(test)]
mod tests;

use std::io;
use std::path::{Path, PathBuf};

use gpui::{
    Context, EventEmitter, IntoElement, ParentElement, Render, SharedString, Styled, Window, div,
    prelude::*, rgb,
};

/// How many repositories the list keeps.
pub const KEPT: usize = 20;

/// The file's first line. A file without it isn't one of ours, and reads as an empty list.
const HEADER: &str = "# Cairn recent repositories, newest first. One path per line.";

const DIM: u32 = 0x7d8590;

/// The list, and the file it is kept in. Without a file it lives only as long as the window.
pub struct RecentRepos {
    file: Option<PathBuf>,
    entries: Vec<PathBuf>,
}

impl RecentRepos {
    /// Reads the list from `file`. A missing, unreadable or corrupt file is an empty list,
    /// never a failed launch.
    pub fn load(file: Option<PathBuf>) -> Self {
        let entries = file.as_deref().map(read).unwrap_or_default();
        Self { file, entries }
    }

    /// Where the list is kept: `Cairn/recent-repos.txt` in the OS's app-data folder.
    pub fn default_file() -> Option<PathBuf> {
        let dir = if cfg!(target_os = "macos") {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join("Library/Application Support"))
        } else if cfg!(windows) {
            std::env::var_os("APPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
                })
        }?;
        Some(dir.join("Cairn").join("recent-repos.txt"))
    }

    pub fn entries(&self) -> &[PathBuf] {
        &self.entries
    }

    /// Puts the repository an Open named at the top, as its worktree root.
    pub fn record(&mut self, opened: &Path) {
        let root = worktree_root(opened);
        self.entries.retain(|entry| *entry != root);
        self.entries.insert(0, root);
        self.entries.truncate(KEPT);
        self.save();
    }

    pub fn remove(&mut self, entry: &Path) {
        self.entries.retain(|e| e != entry);
        self.save();
    }

    /// Writes the list beside the file and renames it over, so a crash mid-write leaves the
    /// old list rather than half of the new one. A failure keeps the list in memory only.
    fn save(&self) {
        let Some(file) = &self.file else {
            return;
        };
        if let Err(e) = write(file, &self.entries) {
            tracing::warn!(file = %file.display(), error = %e, "recent repositories not saved");
        }
    }
}

fn read(file: &Path) -> Vec<PathBuf> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) => {
            if e.kind() != io::ErrorKind::NotFound {
                tracing::warn!(file = %file.display(), error = %e, "recent repositories not read");
            }
            return Vec::new();
        }
    };
    let mut lines = text.lines();
    if lines.next() != Some(HEADER) {
        tracing::warn!(file = %file.display(), "recent repositories file not recognised; starting empty");
        return Vec::new();
    }
    let mut entries: Vec<PathBuf> = Vec::new();
    for path in lines.filter(|line| !line.is_empty()).map(PathBuf::from) {
        if path.is_absolute() && !entries.contains(&path) && entries.len() < KEPT {
            entries.push(path);
        }
    }
    entries
}

fn write(file: &Path, entries: &[PathBuf]) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = format!("{HEADER}\n");
    for entry in entries {
        // A path that isn't UTF-8, or has a line break in it, can't be written as a line of
        // text. Such a repository just isn't remembered.
        if let Some(line) = entry.to_str().filter(|line| !line.contains(['\n', '\r'])) {
            text.push_str(line);
            text.push('\n');
        }
    }
    let partial = file.with_extension("txt.partial");
    std::fs::write(&partial, text)?;
    std::fs::rename(&partial, file)
}

/// The worktree root of a repository that opened from `opened`: absolute, with symlinks
/// resolved, and the worktree rather than its `.git` folder, so one repository is one entry
/// however it was named.
fn worktree_root(opened: &Path) -> PathBuf {
    let path = std::fs::canonicalize(opened)
        .or_else(|_| std::path::absolute(opened))
        .unwrap_or_else(|_| opened.to_path_buf());
    match path.parent() {
        Some(worktree) if path.file_name().is_some_and(|name| name == ".git") => {
            worktree.to_path_buf()
        }
        _ => path,
    }
}

/// The user chose this recent repository to open.
pub struct OpenRecent(pub PathBuf);

/// The list as the user sees it: in the empty state, and under the header's Open Recent.
pub struct RecentView {
    repos: RecentRepos,
    /// The header's list is showing.
    menu_open: bool,
}

/// Where a list is drawn. Each place has its own selectors, since both can show at once.
#[derive(Clone, Copy)]
pub enum Place {
    Empty,
    Menu,
}

impl Place {
    fn prefix(self) -> &'static str {
        match self {
            Place::Empty => "recent:empty",
            Place::Menu => "recent:menu",
        }
    }
}

impl RecentView {
    pub fn new(repos: RecentRepos) -> Self {
        Self {
            repos,
            menu_open: false,
        }
    }

    #[cfg(test)]
    pub fn entries(&self) -> &[PathBuf] {
        self.repos.entries()
    }

    /// An Open of `workdir` succeeded.
    pub fn record(&mut self, workdir: &Path, cx: &mut Context<Self>) {
        self.repos.record(workdir);
        cx.notify();
    }

    pub fn toggle_menu(&mut self, cx: &mut Context<Self>) {
        self.menu_open = !self.menu_open;
        cx.notify();
    }

    pub fn menu_open(&self) -> bool {
        self.menu_open
    }

    fn choose(&mut self, entry: PathBuf, cx: &mut Context<Self>) {
        self.menu_open = false;
        cx.emit(OpenRecent(entry));
        cx.notify();
    }

    fn remove(&mut self, entry: &Path, cx: &mut Context<Self>) {
        self.repos.remove(entry);
        cx.notify();
    }

    /// The list, drawn at `place`.
    pub fn list(&self, place: Place, cx: &mut Context<Self>) -> impl IntoElement {
        let prefix = place.prefix();
        let mut list = div().flex().flex_col().gap_1();
        if self.repos.entries().is_empty() {
            return list.child(div().text_color(rgb(DIM)).child("No recent repositories."));
        }
        for entry in self.repos.entries() {
            let shown: SharedString = entry.display().to_string().into();
            let open = entry.clone();
            let remove = entry.clone();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id(SharedString::from(format!("{prefix}:open:{shown}")))
                            .debug_selector(|| format!("{prefix}:open:{shown}"))
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0x2a2f37)))
                            .on_click(
                                cx.listener(move |view, _, _, cx| view.choose(open.clone(), cx)),
                            )
                            .child(shown.clone()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("{prefix}:remove:{shown}")))
                            .debug_selector(|| format!("{prefix}:remove:{shown}"))
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(rgb(DIM))
                            .hover(|style| style.bg(rgb(0x2a2f37)))
                            .on_click(cx.listener(move |view, _, _, cx| view.remove(&remove, cx)))
                            .child("×"),
                    ),
            );
        }
        list
    }
}

impl EventEmitter<OpenRecent> for RecentView {}

/// The header's list, while it is open. The empty state draws its own with [`RecentView::list`].
impl Render for RecentView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menu = div();
        if !self.menu_open {
            return menu;
        }
        menu.px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(0x2a2f37))
            .child(self.list(Place::Menu, cx))
    }
}
