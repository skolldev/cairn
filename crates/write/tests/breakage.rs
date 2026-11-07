//! #12's breakage checklist: names and messages that break naive git wrappers, each
//! arriving at git and coming back exactly as written.

mod common;

use std::collections::BTreeSet;

use common::{Repo, local_write, ok, read};

/// The untracked paths `git status --porcelain=v2 -z` reports, as raw bytes.
fn untracked(repo: &Repo) -> BTreeSet<Vec<u8>> {
    let out = ok(read(
        &repo.path,
        &["status", "--porcelain=v2", "-z", "--untracked-files=all"],
    ));
    out.records()
        .filter_map(|r| r.strip_prefix(b"? "))
        .map(<[u8]>::to_vec)
        .collect()
}

fn tracked(repo: &Repo) -> BTreeSet<Vec<u8>> {
    ok(read(&repo.path, &["ls-files", "-z"]))
        .records()
        .map(<[u8]>::to_vec)
        .collect()
}

fn names() -> Vec<&'static str> {
    let mut names = vec![
        "with space.txt",
        "umlaut äöü.txt",
        "CJK 漢字.txt",
        "emoji 🎉.txt",
        "-dash.txt",
        "semi;colon & amp.txt",
        "%PATH%.txt",
        "[brackets].txt",
        "quote'single.txt",
    ];
    // Control characters in file names are only possible off Windows.
    if cfg!(unix) {
        names.extend([
            "tab\there.txt",
            "new\nline.txt",
            "bell\x07.txt",
            "back\\slash.txt",
        ]);
    }
    names
}

#[test]
fn awkward_paths_round_trip_through_status_add_and_ls_files() {
    let repo = Repo::new();
    for name in names() {
        repo.file(name, "x\n");
    }
    let expected: BTreeSet<Vec<u8>> = names().iter().map(|n| n.as_bytes().to_vec()).collect();
    assert_eq!(
        untracked(&repo),
        expected,
        "no octal escaping, nothing lost"
    );

    // Each name as its own pathspec, after `--` so `-dash.txt` isn't read as an option.
    for name in names() {
        ok(local_write(&repo.path, &["add", "--", name], None));
    }
    assert_eq!(untracked(&repo), BTreeSet::new());
    assert_eq!(tracked(&repo), expected);
    ok(local_write(
        &repo.path,
        &["commit", "-q", "-m", "awkward names"],
        None,
    ));
}

#[test]
fn awkward_branch_names_round_trip() {
    let repo = Repo::new();
    ok(local_write(
        &repo.path,
        &["commit", "-q", "--allow-empty", "-m", "root"],
        None,
    ));
    let branches = [
        "umlaut-äöü",
        "cjk-漢字",
        "emoji-🎉",
        "semi;colon&amp",
        "%PATH%",
    ];
    for branch in branches {
        ok(local_write(&repo.path, &["branch", branch], None));
        ok(local_write(&repo.path, &["switch", "-q", branch], None));
        let head = ok(read(&repo.path, &["symbolic-ref", "--short", "HEAD"]));
        assert_eq!(head.stdout, format!("{branch}\n").as_bytes());
    }
    let listed: BTreeSet<Vec<u8>> = ok(read(
        &repo.path,
        &[
            "for-each-ref",
            "--format=%(refname:short)%00",
            "refs/heads/",
        ],
    ))
    .stdout
    .split(|&b| b == b'\n' || b == 0)
    .filter(|r| !r.is_empty())
    .map(<[u8]>::to_vec)
    .collect();
    let mut expected: BTreeSet<Vec<u8>> = branches.iter().map(|b| b.as_bytes().to_vec()).collect();
    expected.insert(
        listed
            .iter()
            .find(|b| *b == b"master" || *b == b"main")
            .unwrap()
            .clone(),
    );
    assert_eq!(listed, expected);
}

#[test]
fn a_branch_name_with_a_space_is_refused_in_gits_own_words() {
    let repo = Repo::new();
    ok(local_write(
        &repo.path,
        &["commit", "-q", "--allow-empty", "-m", "root"],
        None,
    ));
    let out = local_write(&repo.path, &["branch", "with space"], None);
    assert!(!out.status.success());
    // The wording is git's own; only its final full stop varies between versions.
    let stderr = out.stderr_text().into_owned();
    assert!(
        stderr.starts_with("fatal: 'with space' is not a valid branch name"),
        "{stderr}"
    );
    assert_eq!(out.success().unwrap_err().to_string(), stderr);
}

#[test]
fn commit_messages_on_stdin_arrive_exactly_as_typed() {
    let repo = Repo::new();
    let messages = [
        r#"double "quotes""#,
        r"back\slash",
        "trailing backslash\\",
        r#"escaped \" quote"#,
        "%PATH% stays",
        "!^&|<> shell soup",
        "first line\n\nbody line one\nbody line two",
        "tab\tseparated",
        "emoji 🎉 and 漢字",
        "-leading dash",
        "--amend looks like an option",
    ];
    for message in messages {
        // verbatim keeps git from tidying whitespace, so the bytes compare exactly.
        ok(local_write(
            &repo.path,
            &[
                "commit",
                "-q",
                "--allow-empty",
                "--cleanup=verbatim",
                "-F",
                "-",
            ],
            Some(message.as_bytes()),
        ));
        let body = ok(read(&repo.path, &["log", "-1", "--format=%B", "-z"])).stdout;
        let stored = body.strip_suffix(b"\0").unwrap_or(&body);
        let stored = stored.strip_suffix(b"\n").unwrap_or(stored);
        assert_eq!(String::from_utf8_lossy(stored), message);
    }
}

#[test]
fn patches_on_stdin_apply_exactly() {
    let repo = Repo::new();
    repo.file("a.txt", "one\n");
    ok(local_write(&repo.path, &["add", "a.txt"], None));
    ok(local_write(
        &repo.path,
        &["commit", "-q", "-m", "one"],
        None,
    ));
    let patch = "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1 +1,2 @@
 one
+two \"quoted\" \\ %PATH% !^&|<>
";
    ok(local_write(
        &repo.path,
        &["apply", "--cached", "-"],
        Some(patch.as_bytes()),
    ));
    let staged = ok(read(&repo.path, &["show", ":a.txt"]));
    assert_eq!(staged.stdout, b"one\ntwo \"quoted\" \\ %PATH% !^&|<>\n");
}

/// A short repo holding a file whose path is over 260 characters. On Windows this only
/// works because every call passes `core.longpaths=true`; the test config leaves it unset.
#[test]
fn a_file_path_over_260_characters_is_listed_added_and_committed() {
    let repo = Repo::new();
    let unset = read(
        &repo.path,
        &["config", "--global", "--get", "core.longpaths"],
    );
    assert_eq!(
        unset.status.code(),
        Some(1),
        "core.longpaths must be unset in user config"
    );

    let dir = "a-directory-name-thirty-chars-";
    let rel = format!("{}/long-file-name.txt", [dir; 9].join("/"));
    let absolute = repo.file(&rel, "long\n");
    assert!(absolute.as_os_str().len() > 260, "{}", absolute.display());

    assert_eq!(untracked(&repo), BTreeSet::from([rel.as_bytes().to_vec()]));
    ok(local_write(&repo.path, &["add", "--", &rel], None));
    ok(local_write(
        &repo.path,
        &["commit", "-q", "-m", "long path"],
        None,
    ));
    assert_eq!(tracked(&repo), BTreeSet::from([rel.as_bytes().to_vec()]));
}
