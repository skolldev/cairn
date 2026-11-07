//! A small repo with `core.autocrlf=true`, `.gitattributes` eol rules, a binary file and a
//! clean filter, rebuilt on every run. Checks status and hunk parity with `git`.
//!
//! Every file must show the same status line and the same hunks as `git`. That includes
//! files whose only change is one the filters undo: git hashes them equal to the index and
//! shows no diff, but still reports them `.M`, even after a refresh, so gix must too.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;

use crate::{Args, git_run, hunks, status};

const ATTRIBUTES: &str = "\
* text=auto
*.crlf text eol=crlf
*.lf text eol=lf
*.raw -text
*.bin binary
*.up filter=upper
";

pub fn run(args: &Args) -> Result<()> {
    let dir = std::path::absolute(&args.fixture_dir)?;
    build(args, &dir)?;
    println!("## autocrlf and filters fixture at {}\n", dir.display());
    println!(
        "`core.autocrlf=true`, `.gitattributes` with `text=auto`, `eol=crlf`, `eol=lf`, `-text`, `binary`, \
         and a clean filter `upper` (`tr a-z A-Z`).\n"
    );
    let report = status::parity(args, &dir, &BTreeMap::new())?;
    report.print("autocrlf and filters");
    let paths: Vec<(String, bool)> = [
        ("a.txt", false),
        ("b.crlf", false),
        ("c.lf", false),
        ("e.up", false),
        ("e2.up", false),
        ("mixed.txt", false),
        ("g.raw", false),
        ("d.bin", false),
        ("f.txt", false),
        ("f.txt", true),
    ]
    .into_iter()
    .map(|(p, s)| (p.to_owned(), s))
    .collect();
    hunks::compare_and_print(args, &dir, &paths)?;

    // Again with the stat cache refreshed, so gix can't match git only by both trusting stat data.
    git_run(&args.git, &dir, &["update-index", "-q", "--refresh"])?;
    let report = status::parity(args, &dir, &BTreeMap::new())?;
    report.print("autocrlf and filters, after `git update-index --refresh`");
    Ok(())
}

fn lines(n: usize, tag: &str, eol: &str) -> String {
    (1..=n).map(|i| format!("{tag} line {i}{eol}")).collect()
}

fn build(args: &Args, dir: &Path) -> Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::create_dir_all(dir)?;
    let git = |a: &[&str]| git_run(&args.git, dir, a);
    let write = |p: &str, data: &[u8]| std::fs::write(dir.join(p), data);

    git(&["init", "-q", "-b", "main"])?;
    for (k, v) in [
        ("user.name", "cairn spike"),
        ("user.email", "spike@example.invalid"),
        ("core.autocrlf", "true"),
        ("filter.upper.clean", "tr a-z A-Z"),
        ("filter.upper.smudge", "cat"),
        ("filter.upper.required", "true"),
    ] {
        git(&["config", k, v])?;
    }
    write(".gitattributes", ATTRIBUTES.as_bytes())?;
    write("a.txt", lines(30, "a", "\n").as_bytes())?;
    write("b.crlf", lines(30, "b", "\n").as_bytes())?;
    write("c.lf", lines(30, "c", "\n").as_bytes())?;
    write("d.bin", b"\x00\x01\x02binary\r\n\x00\xff\xfe")?;
    write("e.up", b"hello\nworld\n")?;
    write("e2.up", lines(20, "e", "\n").as_bytes())?;
    write("mixed.txt", lines(30, "m", "\n").as_bytes())?;
    write("g.raw", lines(10, "g", "\r\n").as_bytes())?;
    write("f.txt", lines(30, "f", "\n").as_bytes())?;
    git(&["add", "-A"])?;
    git(&["commit", "-q", "-m", "base"])?;

    // Check out again, so smudge and eol conversion shape the worktree as a clone would.
    for p in ["a.txt", "b.crlf", "c.lf", "d.bin", "e.up", "e2.up", "mixed.txt", "g.raw", "f.txt"] {
        std::fs::remove_file(dir.join(p))?;
    }
    git(&["checkout", "--", "."])?;
    std::thread::sleep(Duration::from_millis(1100));

    // Only line endings or filter-undone changes: same blob as the index, no hunks.
    write("b.crlf", lines(30, "b", "\n").as_bytes())?;
    write("c.lf", lines(30, "c", "\r\n").as_bytes())?;
    write("e.up", b"hello\nworld\n")?;
    // Real changes.
    let mut e2 = lines(20, "e", "\n");
    e2 = e2.replace("e line 7\n", "e line seven\n");
    write("e2.up", e2.as_bytes())?;
    let mixed = lines(30, "m", "\r\n").replace("m line 15\r\n", "m line fifteen\r\n");
    write("mixed.txt", mixed.as_bytes())?;
    write("g.raw", lines(10, "g", "\n").as_bytes())?;
    write("d.bin", b"\x00\x01\x02binary\r\n\x00\xff\xfd")?;
    let f1 = lines(30, "f", "\r\n").replace("f line 5\r\n", "f line five\r\n");
    write("f.txt", f1.as_bytes())?;
    git(&["add", "--", "f.txt"])?;
    write("f.txt", f1.replace("f line 25\r\n", "f line twenty-five\r\n").as_bytes())?;
    write("new.txt", b"untracked\r\n")?;
    std::fs::create_dir_all(dir.join("newdir"))?;
    write("newdir/x.txt", b"untracked\n")?;
    // Past the racy second, so stat-dirty files must be content-checked rather than trusted.
    std::thread::sleep(Duration::from_millis(1100));
    Ok(())
}
