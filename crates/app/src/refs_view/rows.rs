//! The sidebar's rows: the Refs Snapshot as a tree of sections, remote groups and folders,
//! flattened through the collapse state and the filter into the rows a list draws.

use std::collections::HashSet;

use cairn_repo::{Head, Oid, RefEntry, RefName, Refs, Upstream};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Local,
    Remotes,
    Tags,
    Stashes,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Local => "Local",
            Section::Remotes => "Remotes",
            Section::Tags => "Tags",
            Section::Stashes => "Stashes",
        }
    }

    fn id(self) -> String {
        format!("section:{}", self.title())
    }
}

/// One row of the sidebar. Its [`Row::id`] is what later actions hang on.
#[derive(Clone, Debug)]
pub enum Row {
    Section {
        section: Section,
        /// Refs in the section, or while filtering, refs that match.
        count: usize,
        open: bool,
    },
    /// A remote's group or a folder made by a `/` in names.
    Folder {
        /// The ref name prefix it stands for, ending in `/`: `refs/heads/feature/`.
        prefix: String,
        label: String,
        depth: usize,
        open: bool,
    },
    Ref {
        name: RefName,
        label: String,
        depth: usize,
        current: bool,
        /// What the ref points at.
        target: Oid,
        /// A local branch's upstream.
        upstream: Option<Upstream>,
    },
    Stash {
        index: usize,
        message: String,
    },
}

impl Row {
    /// The full ref name for a ref, its prefix for a folder, `stash@{n}` for a stash, and
    /// `section:<title>` for a section header.
    pub fn id(&self) -> String {
        match self {
            Row::Section { section, .. } => section.id(),
            Row::Folder { prefix, .. } => prefix.clone(),
            Row::Ref { name, .. } => name.as_str().to_owned(),
            Row::Stash { index, .. } => stash_name(*index),
        }
    }

    /// The short name of the local branch it stands for, if it is one.
    pub fn local_branch(&self) -> Option<String> {
        match self {
            Row::Ref { name, .. } if name.as_str().starts_with("refs/heads/") => {
                Some(name.short().to_owned())
            }
            _ => None,
        }
    }

    /// Whether clicking it opens or closes something.
    pub fn toggles(&self) -> bool {
        matches!(self, Row::Section { .. } | Row::Folder { .. })
    }
}

fn stash_name(index: usize) -> String {
    format!("stash@{{{index}}}")
}

enum Node {
    Folder {
        prefix: String,
        label: String,
        children: Vec<Node>,
    },
    Ref {
        name: RefName,
        label: String,
        /// The short name, lowercased, that the filter matches against.
        haystack: String,
        target: Oid,
        upstream: Option<Upstream>,
    },
    Stash {
        index: usize,
        message: String,
        haystack: String,
    },
}

/// The refs as a tree, built once per Snapshot.
pub struct Tree {
    sections: [(Section, Vec<Node>, usize); 4],
    current: Option<RefName>,
}

impl Tree {
    pub fn new(refs: &Refs) -> Self {
        let mut local = Vec::new();
        for entry in &refs.local {
            insert(&mut local, "refs/heads/", entry);
        }
        let mut remotes = Vec::new();
        let mut remote_count = 0;
        // A remote's symbolic HEAD names its default branch, and isn't a branch itself.
        for entry in refs.remote.iter().filter(|entry| entry.symref.is_none()) {
            let Some(rest) = entry.name.as_str().strip_prefix("refs/remotes/") else {
                continue;
            };
            let Some((remote, _)) = rest.split_once('/') else {
                continue;
            };
            let prefix = format!("refs/remotes/{remote}/");
            let group = folder(&mut remotes, &prefix, remote);
            insert(group, &prefix, entry);
            remote_count += 1;
        }
        let mut tags = Vec::new();
        for entry in &refs.tags {
            insert(&mut tags, "refs/tags/", entry);
        }
        let stashes = refs
            .stashes
            .iter()
            .enumerate()
            .map(|(index, stash)| Node::Stash {
                index,
                message: stash.message.clone(),
                haystack: stash_name(index),
            })
            .collect();
        let current = match &refs.head {
            Head::Branch { name, .. } => Some(name.clone()),
            Head::Detached { .. } => None,
        };
        Self {
            sections: [
                (Section::Local, local, refs.local.len()),
                (Section::Remotes, remotes, remote_count),
                (Section::Tags, tags, refs.tags.len()),
                (Section::Stashes, stashes, refs.stashes.len()),
            ],
            current,
        }
    }

    /// The branch HEAD is on, if it is on one.
    pub fn current(&self) -> Option<&RefName> {
        self.current.as_ref()
    }

    /// The rows to draw. `closed` holds the ids of closed sections and folders, and is
    /// ignored while `filter` is set: then every row leading to a match is open.
    pub fn rows(&self, closed: &HashSet<String>, filter: &str) -> Vec<Row> {
        let needle = filter.to_lowercase();
        let mut rows = Vec::new();
        for (section, nodes, count) in &self.sections {
            if needle.is_empty() {
                let open = !closed.contains(&section.id());
                rows.push(Row::Section {
                    section: *section,
                    count: *count,
                    open,
                });
                if open {
                    self.push_all(&mut rows, nodes, 0, closed);
                }
            } else {
                let header = rows.len();
                rows.push(Row::Section {
                    section: *section,
                    count: 0,
                    open: true,
                });
                let matches = self.push_matches(&mut rows, nodes, 0, &needle);
                if let Row::Section { count, .. } = &mut rows[header] {
                    *count = matches;
                }
            }
        }
        rows
    }

    fn push_all(
        &self,
        rows: &mut Vec<Row>,
        nodes: &[Node],
        depth: usize,
        closed: &HashSet<String>,
    ) {
        for node in nodes {
            match node {
                Node::Folder {
                    prefix,
                    label,
                    children,
                } => {
                    let open = !closed.contains(prefix);
                    rows.push(Row::Folder {
                        prefix: prefix.clone(),
                        label: label.clone(),
                        depth,
                        open,
                    });
                    if open {
                        self.push_all(rows, children, depth + 1, closed);
                    }
                }
                _ => rows.push(self.leaf(node, depth)),
            }
        }
    }

    /// Pushes the matching refs under `nodes` with the folders that lead to them, and
    /// returns how many refs matched.
    fn push_matches(
        &self,
        rows: &mut Vec<Row>,
        nodes: &[Node],
        depth: usize,
        needle: &str,
    ) -> usize {
        let mut matches = 0;
        for node in nodes {
            match node {
                Node::Folder {
                    prefix,
                    label,
                    children,
                } => {
                    let at = rows.len();
                    rows.push(Row::Folder {
                        prefix: prefix.clone(),
                        label: label.clone(),
                        depth,
                        open: true,
                    });
                    let inside = self.push_matches(rows, children, depth + 1, needle);
                    if inside == 0 {
                        rows.truncate(at);
                    }
                    matches += inside;
                }
                Node::Ref { haystack, .. } | Node::Stash { haystack, .. } => {
                    if haystack.contains(needle) {
                        rows.push(self.leaf(node, depth));
                        matches += 1;
                    }
                }
            }
        }
        matches
    }

    fn leaf(&self, node: &Node, depth: usize) -> Row {
        match node {
            Node::Ref {
                name,
                label,
                target,
                upstream,
                ..
            } => Row::Ref {
                name: name.clone(),
                label: label.clone(),
                depth,
                current: self.current.as_ref() == Some(name),
                target: target.clone(),
                upstream: upstream.clone(),
            },
            Node::Stash { index, message, .. } => Row::Stash {
                index: *index,
                message: message.clone(),
            },
            Node::Folder { .. } => unreachable!("a folder isn't a leaf"),
        }
    }
}

/// Inserts `entry` under `nodes`, which stand for the names starting with `prefix`. Each
/// `/` after the prefix opens a folder.
fn insert(nodes: &mut Vec<Node>, prefix: &str, entry: &RefEntry) {
    let name = entry.name.as_str();
    let Some(rest) = name.strip_prefix(prefix) else {
        return;
    };
    let mut nodes = nodes;
    let mut folder_prefix = prefix.to_owned();
    let mut segments = rest.split('/').peekable();
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            nodes.push(Node::Ref {
                name: entry.name.clone(),
                label: segment.to_owned(),
                haystack: entry.name.short().to_lowercase(),
                target: entry.target.clone(),
                upstream: entry.upstream.clone(),
            });
            return;
        }
        folder_prefix.push_str(segment);
        folder_prefix.push('/');
        nodes = folder(nodes, &folder_prefix, segment);
    }
}

/// The children of the folder for `prefix`, made last in `nodes` if it isn't already.
/// Git sorts refs by name, so the names under one prefix arrive together, and only the
/// last node can be that folder.
fn folder<'a>(nodes: &'a mut Vec<Node>, prefix: &str, label: &str) -> &'a mut Vec<Node> {
    let is_last = matches!(nodes.last(), Some(Node::Folder { prefix: last, .. }) if last == prefix);
    if !is_last {
        nodes.push(Node::Folder {
            prefix: prefix.to_owned(),
            label: label.to_owned(),
            children: Vec::new(),
        });
    }
    match nodes.last_mut() {
        Some(Node::Folder { children, .. }) => children,
        _ => unreachable!("the folder was just made"),
    }
}
