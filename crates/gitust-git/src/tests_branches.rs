use super::*;
use std::{fs, path::Path, process::Command};
fn git(path: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    crate::process::sanitize_git_environment(&mut command);
    let output = command
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@company.internal")
        .env("GIT_COMMITTER_EMAIL", "fixture@company.internal")
        .output()
        .unwrap();
    assert!(output.status.success(), "synthetic Git operation failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
fn init_repo(path: &Path) {
    fs::create_dir(path).unwrap();
    git(path, &["init", "-q", "-b", "main"]);
    fs::create_dir(path.join("src")).unwrap();
    fs::write(path.join("src/lib.rs"), "fixture\n").unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-qm", "fixture"]);
}

#[test]
fn local_branch_keysets_match_independent_git_with_packed_loose_symbolic_and_oid_names() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("repo");
    init_repo(&path);
    let old = git(&path, &["rev-parse", "HEAD"]);
    git(&path, &["commit", "--allow-empty", "-qm", "second"]);
    let head = git(&path, &["rev-parse", "HEAD"]);
    for name in [
        "aaa",
        "agent/task",
        "z-last",
        "refs/heads/nested",
        "HEAD",
        &old,
        "unicode/hello\u{a0}world",
    ] {
        git(&path, &["update-ref", &format!("refs/heads/{name}"), &head]);
    }
    git(&path, &["tag", "-a", "aaa", "-m", "tag is not a branch"]);
    git(
        &path,
        &["symbolic-ref", "refs/heads/alias", "refs/heads/main"],
    );
    git(&path, &["pack-refs", "--all"]);
    git(&path, &["branch", "middle"]);
    let oracle = git(
        &path,
        &[
            "for-each-ref",
            "--sort=refname",
            "--format=%(refname)%00%(objectname)",
            "refs/heads/",
        ],
    );
    let expected: Vec<_> = oracle
        .lines()
        .map(|line| {
            let (r, s) = line.split_once('\0').unwrap();
            (r.to_owned(), s.to_owned())
        })
        .collect();
    let engine = GitEngine::new(&path);
    let mut cursor = None;
    let mut actual = Vec::new();
    loop {
        let page = engine.list_local_branches(2, cursor.as_deref()).unwrap();
        for row in page.branches {
            assert_eq!(row.name, row.ref_name.strip_prefix(PREFIX).unwrap());
            actual.push((row.ref_name, row.commit_sha.0));
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(actual, expected);
    assert!(actual
        .iter()
        .any(|(r, s)| r == &format!("refs/heads/{old}") && s == &head));
    git(&path, &["fsck", "--full", "--strict"]);
}
#[test]
fn unborn_default_has_no_invented_branch_but_other_existing_heads_remain_visible() {
    let root = tempfile::tempdir().unwrap();
    let empty = root.path().join("empty");
    fs::create_dir(&empty).unwrap();
    git(&empty, &["init", "-q", "--bare", "-b", "main"]);
    let page = GitEngine::new(&empty)
        .list_local_branches(50, None)
        .unwrap();
    assert!(page.branches.is_empty());
    assert!(page.next_cursor.is_none());
    let path = root.path().join("repo");
    init_repo(&path);
    git(&path, &["symbolic-ref", "HEAD", "refs/heads/unborn"]);
    let page = GitEngine::new(&path).list_local_branches(50, None).unwrap();
    assert_eq!(page.branches.len(), 1);
    assert_eq!(page.branches[0].name, "main");
}
#[test]
fn corrupt_noncommit_missing_and_dangling_refs_fail_instead_of_becoming_empty() {
    for kind in ["malformed", "missing", "blob", "symbolic"] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("repo");
        init_repo(&path);
        let data = match kind {
            "malformed" => "not-an-object\n".into(),
            "missing" => format!("{}\n", "a".repeat(40)),
            "blob" => format!("{}\n", git(&path, &["rev-parse", "HEAD:src/lib.rs"])),
            _ => "ref: refs/heads/missing\n".into(),
        };
        fs::write(path.join(".git/refs/heads/broken"), data).unwrap();
        assert!(
            GitEngine::new(&path).list_local_branches(50, None).is_err(),
            "corrupt ref must fail closed"
        );
    }
}
#[test]
fn maximum_unicode_cursor_is_canonical_bounded_and_invalid_forms_are_rejected() {
    let full = format!("{PREFIX}{}", "é".repeat(MAX_BRANCH_NAME_BYTES / 2));
    let cursor = encode(&full);
    assert_eq!(cursor.len(), 43_709);
    assert_eq!(cursor.len(), MAX_BRANCH_CURSOR_BYTES);
    assert_eq!(decode(&cursor).unwrap(), full);
    for invalid_cursor in [
        String::new(),
        format!("{cursor}="),
        "v2.YQ".into(),
        "v1._w".into(),
        encode("refs/tags/main"),
        encode("refs/heads/../bad"),
    ] {
        assert!(decode(&invalid_cursor).is_err());
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("repo");
    init_repo(&path);
    let engine = GitEngine::new(&path);
    for limit in [0, 201] {
        assert!(engine.list_local_branches(limit, None).is_err());
    }
}
#[test]
fn serialized_byte_budget_cuts_pages_without_losing_long_refs_and_scan_cap_fails_closed() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("repo");
    init_repo(&path);
    let head = git(&path, &["rev-parse", "HEAD"]);
    let names: Vec<_> = (0..24)
        .map(|i| {
            format!(
                "{PREFIX}{i:02}{}",
                "é".repeat((MAX_BRANCH_NAME_BYTES - 2) / 2)
            )
        })
        .collect();
    let packed = format!(
        "# pack-refs with: sorted\n{}",
        names
            .iter()
            .map(|r| format!("{head} {r}\n"))
            .collect::<String>()
    );
    fs::write(path.join(".git/packed-refs"), packed).unwrap();
    let engine = GitEngine::new(&path);
    let first = engine.list_local_branches(200, None).unwrap();
    assert!(!first.branches.is_empty());
    assert!(first.branches.len() < 24);
    assert!(serde_json::to_vec(&first).unwrap().len() <= MAX_BRANCH_PAGE_BYTES);
    let mut all = first.branches;
    let mut cursor = first.next_cursor;
    while let Some(key) = cursor {
        let page = engine.list_local_branches(200, Some(&key)).unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_BRANCH_PAGE_BYTES);
        all.extend(page.branches);
        cursor = page.next_cursor;
    }
    let oracle = git(
        &path,
        &[
            "for-each-ref",
            "--sort=refname",
            "--format=%(refname)",
            "refs/heads/",
        ],
    );
    assert_eq!(
        all.iter().map(|r| r.ref_name.as_str()).collect::<Vec<_>>(),
        oracle.lines().collect::<Vec<_>>()
    );
    git(&path, &["fsck", "--full", "--strict"]);
    let packed = format!(
        "# pack-refs with: sorted\n{}",
        (0..=MAX_SCANNED_HEADS)
            .map(|i| format!("{head} {PREFIX}b{i:05}\n"))
            .collect::<String>()
    );
    fs::write(path.join(".git/packed-refs"), packed).unwrap();
    assert!(GitEngine::new(&path).list_local_branches(1, None).is_err());
}
