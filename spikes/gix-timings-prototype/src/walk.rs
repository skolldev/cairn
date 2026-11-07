//! History walks: topo order, HEAD only and all refs, with and without the commit-graph.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::Result;
use gix::ObjectId;
use gix::revision::walk::Sorting;

use crate::{Args, measure, open, table_header, table_row, time_git, wants};

struct Case {
    name: &'static str,
    all_refs: bool,
    limit: Option<usize>,
    graph: bool,
    floor: Option<Duration>,
}

const CASES: &[Case] = &[
    Case { name: "walk HEAD first 5000, graph", all_refs: false, limit: Some(5000), graph: true, floor: Some(Duration::from_millis(100)) },
    Case { name: "walk all refs first 5000, graph", all_refs: true, limit: Some(5000), graph: true, floor: Some(Duration::from_millis(1500)) },
    Case { name: "walk all refs everything, graph", all_refs: true, limit: None, graph: true, floor: Some(Duration::from_secs(6)) },
    Case { name: "walk all refs first 5000, no graph", all_refs: true, limit: Some(5000), graph: false, floor: Some(Duration::from_secs(12)) },
    Case { name: "walk HEAD first 5000, no graph", all_refs: false, limit: Some(5000), graph: false, floor: None },
    Case { name: "walk HEAD everything, graph", all_refs: false, limit: None, graph: true, floor: None },
    Case { name: "walk HEAD everything, no graph", all_refs: false, limit: None, graph: false, floor: None },
];

pub fn run(args: &Args) -> Result<()> {
    println!("## Walks (topo order, layout excluded)\n");
    let (open_sample, _) = measure(args, || {
        let start = Instant::now();
        let repo = open(&args.repo)?;
        Ok((start.elapsed(), repo.head_id()?.detach()))
    })?;
    println!(
        "Open, with git binary config: worst {} ({}). Not part of the walk figures, which start on a freshly opened repo.\n",
        crate::ms(open_sample.worst()),
        open_sample.runs()
    );

    let mut notes = Vec::new();
    table_header();
    for case in CASES.iter().filter(|c| wants(args, c.name)) {
        let (gix, ids) = measure(args, || {
            let repo = open(&args.repo)?;
            let start = Instant::now();
            let ids = walk(&repo, case.all_refs, case.limit, case.graph)?;
            Ok((start.elapsed(), ids))
        })?;

        let mut git_args: Vec<String> = Vec::new();
        if !case.graph {
            git_args.extend(["-c".into(), "core.commitGraph=false".into()]);
        }
        git_args.extend(["rev-list".into(), "--topo-order".into()]);
        if let Some(n) = case.limit {
            git_args.push(format!("--max-count={n}"));
        }
        git_args.push(if case.all_refs { "--all".into() } else { "HEAD".into() });
        let git_args: Vec<&str> = git_args.iter().map(String::as_str).collect();
        let (git, out) = time_git(args, &args.repo, &git_args)?;

        table_row(case.name, case.floor, &gix, Some(&git));
        notes.push(format!("{}: {} commits, {}", case.name, ids.len(), compare_order(&ids, &out)));
    }
    println!("\n`git` is `git rev-list --topo-order`, with `-c core.commitGraph=false` for no graph, process spawn included.\n");
    for n in notes {
        println!("- {n}");
    }
    println!();
    Ok(())
}

/// The tips for a walk. All refs means HEAD plus every ref that peels to a commit, as `--all`.
fn tips(repo: &gix::Repository, all_refs: bool) -> Result<Vec<ObjectId>> {
    let head = repo.head_id()?.detach();
    let mut tips = vec![head];
    if !all_refs {
        return Ok(tips);
    }
    let mut seen: HashSet<ObjectId> = HashSet::from([head]);
    for reference in repo.references()?.all()? {
        let mut reference = reference.map_err(|e| anyhow::anyhow!("{e}"))?;
        let id = reference.peel_to_id()?.detach();
        if repo.find_header(id)?.kind() != gix::object::Kind::Commit {
            continue;
        }
        if seen.insert(id) {
            tips.push(id);
        }
    }
    Ok(tips)
}

fn walk(repo: &gix::Repository, all_refs: bool, limit: Option<usize>, graph: bool) -> Result<Vec<ObjectId>> {
    let tips = tips(repo, all_refs)?;
    let walk = repo
        .rev_walk(tips)
        .sorting(Sorting::TopoOrder)
        .use_commit_graph(graph)
        .all()?;
    let mut ids = Vec::with_capacity(limit.unwrap_or(1 << 21));
    for info in walk {
        ids.push(info?.id);
        if Some(ids.len()) == limit {
            break;
        }
    }
    Ok(ids)
}

/// How gix's order compares with git's. Not a Floor: topo order allows ties to differ.
fn compare_order(gix: &[ObjectId], git_out: &[u8]) -> String {
    let git: Vec<ObjectId> = git_out
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .filter_map(|l| ObjectId::from_hex(l).ok())
        .collect();
    if gix == git.as_slice() {
        return "identical order to git".into();
    }
    let first_diff = gix.iter().zip(&git).position(|(a, b)| a != b);
    let a: HashSet<_> = gix.iter().collect();
    let b: HashSet<_> = git.iter().collect();
    let only_gix = a.difference(&b).count();
    let only_git = b.difference(&a).count();
    format!(
        "differs from git ({} vs {} commits), first difference at position {}, {only_gix} only in gix, {only_git} only in git",
        gix.len(),
        git.len(),
        first_diff.map(|p| p.to_string()).unwrap_or_else(|| "end".into()),
    )
}
