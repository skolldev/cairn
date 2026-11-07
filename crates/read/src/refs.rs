//! Refs through gix, field for field what `git for-each-ref` and `git stash list` report.

use cairn_core::{Head, Oid, RefEntry, RefName, Refs, Stash};

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
