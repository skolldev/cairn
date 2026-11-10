//! Refs through gix, field for field what `git for-each-ref` and `git stash list` report.

use std::collections::HashMap;

use cairn_core::{Head, Oid, RefEntry, RefName, Refs, Stash, Upstream};

use gix::bstr::ByteSlice as _;
use gix::prelude::ReferenceExt as _;

use crate::{ReadError, err};

pub(crate) fn read(repo: &gix::Repository) -> Result<Refs, ReadError> {
    let platform = repo.references().map_err(err)?;
    let packed = repo.refs.cached_packed_buffer().map_err(err)?;
    let packed = packed.as_ref().map(|p| &***p);

    let mut local = Vec::new();
    let mut remote = Vec::new();
    let mut tags = Vec::new();
    for reference in platform.all().map_err(err)? {
        let mut reference = reference.map_err(err)?;
        let name = reference.name().as_bstr().to_string();
        let list = if name.starts_with("refs/heads/") {
            &mut local
        } else if name.starts_with("refs/remotes/") {
            &mut remote
        } else if name.starts_with("refs/tags/") {
            &mut tags
        } else {
            continue;
        };
        let symref = match reference.target() {
            gix::refs::TargetRef::Symbolic(target) => {
                Some(RefName::new(target.as_bstr().to_string()))
            }
            gix::refs::TargetRef::Object(_) => None,
        };
        // Follows symbolic refs only. A tag ref stays on its tag object, as in git.
        let target = reference.follow_to_object_packed(packed).map_err(err)?;
        let peeled = peel_tag_once(repo, target.detach())?;
        list.push(RefEntry {
            name: RefName::new(name),
            target: oid(target.detach()),
            peeled,
            symref,
            upstream: None,
        });
    }
    let targets: HashMap<String, Oid> = local
        .iter()
        .chain(&remote)
        .chain(&tags)
        .map(|entry| (entry.name.as_str().to_owned(), entry.target.clone()))
        .collect();
    for entry in &mut local {
        let Some(name) = upstream_name(repo, entry.name.as_str(), &targets) else {
            continue;
        };
        let target = match targets.get(&name) {
            Some(target) => Some(target.clone()),
            // Outside the listed refs, such as `refs/notes/x`. Rare enough to look up alone.
            None => match repo.try_find_reference(name.as_str()).map_err(err)? {
                Some(mut reference) => Some(oid(reference.peel_to_id().map_err(err)?.detach())),
                None => None,
            },
        };
        entry.upstream = Some(Upstream {
            name: RefName::new(name),
            target,
        });
    }
    // git sorts by the ref name's bytes; String's order is the same.
    for list in [&mut local, &mut remote, &mut tags] {
        list.sort_by(|a, b| a.name.cmp(&b.name));
    }

    Ok(Refs {
        head: head(repo)?,
        local,
        remote,
        tags,
        stashes: stashes(repo)?,
    })
}

/// `%(upstream)` for the local branch `branch`, as git's `branch_get_upstream` finds it:
/// `branch.<name>.remote` (the last one set) and `branch.<name>.merge` (the first one set)
/// must both be set, and the merge ref must map through that remote's fetch refspecs. A
/// remote of `.` means the merge ref is itself the upstream. `refs` holds every listed ref,
/// for expanding a short merge name the way git's `dwim_ref` does for `.`.
fn upstream_name(
    repo: &gix::Repository,
    branch: &str,
    refs: &HashMap<String, Oid>,
) -> Option<String> {
    let short = branch.strip_prefix("refs/heads/")?;
    let config = repo.config_snapshot();
    let config = config.plumbing();
    let subsection: &gix::bstr::BStr = short.into();
    let remote = config.string_by("branch", Some(subsection), "remote")?;
    let merge = config
        .strings_by("branch", Some(subsection), "merge")?
        .into_iter()
        .next()?
        .to_string();
    if remote == "." {
        return Some(dwim_local(&merge, refs));
    }
    let remote = repo.try_find_remote(remote.as_bstr())?.ok()?;
    let specs = remote
        .refspecs(gix::remote::Direction::Fetch)
        .iter()
        .map(gix::refspec::RefSpec::to_ref)
        .filter(|spec| spec.source().is_some() && spec.destination().is_some());
    let null = repo.object_hash().null();
    let item = gix::refspec::match_group::Item {
        full_ref_name: merge.as_str().into(),
        target: &null,
        object: None,
    };
    gix::refspec::MatchGroup::from_fetch_specs(specs)
        .match_lhs(std::iter::once(item))
        .mappings
        .into_iter()
        .next()?
        .rhs
        .map(|name| name.to_string())
}

/// The merge ref of a branch whose remote is `.`: a full name as it is, and a short one
/// expanded as git's `dwim_ref` does when exactly one rule matches a ref.
fn dwim_local(merge: &str, refs: &HashMap<String, Oid>) -> String {
    if merge.starts_with("refs/") {
        return merge.to_owned();
    }
    let candidates = [
        format!("refs/{merge}"),
        format!("refs/tags/{merge}"),
        format!("refs/heads/{merge}"),
        format!("refs/remotes/{merge}"),
        format!("refs/remotes/{merge}/HEAD"),
    ];
    let mut found = candidates
        .into_iter()
        .filter(|name| refs.contains_key(name));
    match (found.next(), found.next()) {
        (Some(only), None) => only,
        _ => merge.to_owned(),
    }
}

fn oid(id: gix::ObjectId) -> Oid {
    Oid::from_hex(id.to_string())
}

/// `%(*objectname)`: the object a tag object points at, one level down.
fn peel_tag_once(repo: &gix::Repository, id: gix::ObjectId) -> Result<Option<Oid>, ReadError> {
    let header = repo.find_header(id).map_err(err)?;
    if header.kind() != gix::object::Kind::Tag {
        return Ok(None);
    }
    let tag = repo.find_object(id).map_err(err)?.into_tag();
    Ok(Some(oid(tag.target_id().map_err(err)?.detach())))
}

fn head(repo: &gix::Repository) -> Result<Head, ReadError> {
    let head = repo.head().map_err(err)?;
    Ok(match head.kind {
        gix::head::Kind::Symbolic(reference) => {
            let name = RefName::new(reference.name.as_bstr().to_string());
            let mut reference = reference.attach(repo);
            let target = reference.follow_to_object().map_err(err)?;
            Head::Branch {
                name,
                target: Some(oid(target.detach())),
            }
        }
        gix::head::Kind::Unborn(name) => Head::Branch {
            name: RefName::new(name.as_bstr().to_string()),
            target: None,
        },
        gix::head::Kind::Detached { target, .. } => Head::Detached {
            target: oid(target),
        },
    })
}

/// The stash is `refs/stash` and its reflog, newest entry first.
fn stashes(repo: &gix::Repository) -> Result<Vec<Stash>, ReadError> {
    let Some(stash) = repo.try_find_reference("refs/stash").map_err(err)? else {
        return Ok(Vec::new());
    };
    let mut log = stash.log_iter();
    let Some(entries) = log.rev().map_err(err)? else {
        return Ok(Vec::new());
    };
    entries
        .map(|line| {
            let line = line.map_err(err)?;
            Ok(Stash {
                target: oid(line.new_oid),
                message: line.message.to_string(),
            })
        })
        .collect()
}
