use std::fs;
use std::process::Command;

use tempfile::TempDir;

use super::{git, init_repo, materialize};
use crate::{DiffLineKind, GitEngine, GitEngineError};

#[test]
fn repository_paths_preserve_leading_trailing_and_only_spaces() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("repo");
    init_repo(&dir);
    for (path, content) in [
        ("note", "plain\n"),
        ("note ", "trailing\n"),
        (" note", "leading\n"),
        (" ", "space only\n"),
    ] {
        fs::write(dir.join(path), content).unwrap();
    }
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "distinct paths"]);
    let engine = GitEngine::new(&dir);
    for (path, content) in [
        ("note", "plain\n"),
        ("note ", "trailing\n"),
        (" note", "leading\n"),
        (" ", "space only\n"),
    ] {
        let blob = engine.read_blob(None, path).unwrap();
        assert_eq!(blob.path, path);
        assert_eq!(blob.content.as_deref(), Some(content));
        let diff = engine.inspect_diff(None, None, Some(path)).unwrap();
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].path, path);
    }
    assert_eq!(
        GitEngine::validate_repo_relative_path("/note /").unwrap(),
        "note "
    );
    for invalid in ["../note", "a/../note", "a//note", ".git/config", "a/./note"] {
        assert!(GitEngine::validate_repo_relative_path(invalid).is_err());
    }
    assert_eq!(
        engine
            .inspect_tree(None, Some("/"), false)
            .unwrap()
            .base_path,
        ""
    );
}

#[test]
fn browse_commits_tree_blob_and_diff_in_process_and_adversarial_bounds() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_repo(&repo_dir);

    fs::create_dir_all(repo_dir.join("crates/core")).expect("mkdir");
    fs::write(
        repo_dir.join("crates/core/mod.rs"),
        "pub const VERSION: &str = \"0.1.0\";\n",
    )
    .expect("write mod.rs");
    fs::write(repo_dir.join("crates/core/bin.dat"), b"\x00\x01\x02binary")
        .expect("write binary blob");
    fs::write(repo_dir.join("crates/core/my file.txt"), "spaced path\n")
        .expect("write spaced file");
    fs::write(repo_dir.join("q.sql"), "x\n-- old sql comment\n").expect("write q.sql");
    fs::write(
        repo_dir.join("oversized.txt"),
        vec![b'a'; crate::browse::MAX_BLOB_VIEW_BYTES + 1024],
    )
    .expect("write oversized blob");
    git(&repo_dir, &["add", "."]);
    git(&repo_dir, &["commit", "-q", "-m", "feat: add core module"]);

    fs::write(repo_dir.join("q.sql"), "x\n++ evil();\n").expect("update q.sql");
    fs::write(repo_dir.join("한글파일.sql"), "SELECT 1;\n").expect("write korean file");
    fs::write(repo_dir.join("a.txt"), "file a\n").expect("write a.txt");
    fs::write(repo_dir.join("b.txt"), "file b\n").expect("write b.txt");
    fs::write(repo_dir.join("[ab].txt"), "file bracket ab\n").expect("write [ab].txt");
    fs::write(repo_dir.join("quoted\"name.txt"), "quoted content\n").expect("write quoted file");
    git(&repo_dir, &["add", "."]);
    git(
        &repo_dir,
        &["commit", "-q", "-m", "fix: sql comment and korean path"],
    );

    let engine = GitEngine::new(&repo_dir);

    let commit_list = engine.list_commits(Some("main"), 10).expect("list_commits");
    assert_eq!(commit_list.commits.len(), 3);
    assert_eq!(
        commit_list.commits[0].summary,
        "fix: sql comment and korean path"
    );
    assert_eq!(commit_list.commits[1].summary, "feat: add core module");
    assert_eq!(commit_list.commits[2].summary, "initial commit");

    let root_tree = engine
        .inspect_tree(Some("main"), None, false)
        .expect("root tree");
    assert!(root_tree
        .entries
        .iter()
        .any(|e| e.filename == "crates" && e.is_dir));

    let recursive_tree = engine
        .inspect_tree(Some("main"), None, true)
        .expect("recursive tree");
    assert!(recursive_tree
        .entries
        .iter()
        .any(|e| e.path == "crates/core/mod.rs" && !e.is_dir));

    let text_blob = engine
        .read_blob(Some("main"), "crates/core/mod.rs")
        .expect("read text blob");
    assert!(!text_blob.is_binary);
    assert_eq!(text_blob.line_count, 1);
    assert_eq!(
        text_blob.content.as_deref(),
        Some("pub const VERSION: &str = \"0.1.0\";\n")
    );

    let bin_blob = engine
        .read_blob(Some("main"), "crates/core/bin.dat")
        .expect("read binary blob");
    assert!(bin_blob.is_binary);
    assert!(bin_blob.content.is_none());

    let oversized_blob = engine
        .read_blob(Some("main"), "oversized.txt")
        .expect("read oversized blob via header check");
    assert!(oversized_blob.truncated);
    assert!(oversized_blob.content.is_none());
    assert_eq!(
        oversized_blob.size,
        crate::browse::MAX_BLOB_VIEW_BYTES + 1024
    );

    for bad_path in [
        "",
        "../Cargo.toml",
        "crates/../../etc/passwd",
        ".git/config",
    ] {
        assert!(
            matches!(
                engine.read_blob(Some("main"), bad_path),
                Err(GitEngineError::InvalidRepoPath(_))
            ),
            "bad blob path must be rejected: {bad_path}"
        );
    }

    let head_short_sha = &commit_list.commits[0].short_sha;
    let sql_diff = engine
        .inspect_diff(None, Some(head_short_sha), None)
        .expect("inspect_diff on sql and korean commit");
    let q_summary = sql_diff
        .files
        .iter()
        .find(|f| f.path == "q.sql")
        .expect("q.sql summary must exist");
    assert_eq!(
        (q_summary.additions, q_summary.deletions),
        (1, 1),
        "P1 #2: `-- old sql comment` deletion and `++ evil();` addition must be counted"
    );
    assert!(
        sql_diff
            .lines
            .iter()
            .any(|l| l.kind == DiffLineKind::Del && l.content == "-- old sql comment"),
        "P1 #2: deleted `-- old sql comment` line must be visible in diff lines: {:?}",
        sql_diff.lines
    );
    assert!(
        sql_diff
            .lines
            .iter()
            .any(|l| l.kind == DiffLineKind::Add && l.content == "++ evil();"),
        "P1 #2: added `++ evil();` line must be visible in diff lines: {:?}",
        sql_diff.lines
    );
    assert!(
        sql_diff.files.iter().any(|f| f.path == "한글파일.sql"),
        "P2: non-ASCII Korean filename must not be octal-escaped: {:?}",
        sql_diff.files
    );

    let quoted_entry = sql_diff
        .files
        .iter()
        .find(|f| f.path == "quoted\"name.txt")
        .expect("C-quoted path must be decoded to exact repo-relative path `quoted\"name.txt`");
    let quoted_blob = engine
        .read_blob(Some("main"), &quoted_entry.path)
        .expect("read_blob on decoded C-quoted path must succeed");
    assert_eq!(quoted_blob.content.as_deref(), Some("quoted content\n"));

    let bracket_diff = engine
        .inspect_diff(None, Some(head_short_sha), Some("[ab].txt"))
        .expect("inspect_diff with literal pathspec [ab].txt");
    assert_eq!(
        bracket_diff.files.len(),
        1,
        "literal pathspec [ab].txt must not match a.txt or b.txt: {:?}",
        bracket_diff.files
    );
    assert_eq!(bracket_diff.files[0].path, "[ab].txt");

    let prev_short_sha = &commit_list.commits[1].short_sha;
    let diff = engine
        .inspect_diff(None, Some(prev_short_sha), None)
        .expect("inspect_diff via 7-char short_sha");
    assert!(
        diff.files
            .iter()
            .any(|f| f.path == "crates/core/my file.txt"),
        "diff parser must preserve spaces in file paths: {:?}",
        diff.files
    );

    let root_sha = &commit_list.commits[2].sha.0;
    let root_diff = engine
        .inspect_diff(None, Some(root_sha), None)
        .expect("inspect_diff root commit");
    assert_eq!(root_diff.files_changed, 1);
    assert_eq!(root_diff.additions, 1);

    for bad_rev in [
        "--output=/tmp/pwn",
        "HEAD@{0}",
        "main..HEAD",
        "-n",
        "HEAD~600~600",
    ] {
        assert!(
            matches!(
                engine.inspect_diff(Some(bad_rev), Some("main"), None),
                Err(GitEngineError::InvalidRevision(_))
            ),
            "bad revision must be rejected: {bad_rev}"
        );
    }
}

/// Benchmark: `cargo test -p gitust-git --release -- --ignored --nocapture bench_`
#[test]
#[ignore = "benchmark; run explicitly"]
fn bench_materialize_agent_worktree() {
    for file_count in [10_usize, 1_000, 10_000] {
        let tmp = TempDir::new().expect("tempdir");
        let repo_dir = tmp.path().join("repo");
        fs::create_dir_all(&repo_dir).expect("mkdir");
        git(&repo_dir, &["init", "-q", "-b", "main"]);
        for i in 0..file_count {
            let dir = repo_dir.join(format!("d{}", i % 100));
            fs::create_dir_all(&dir).expect("mkdir");
            fs::write(
                dir.join(format!("f{i}.rs")),
                format!("pub const X: usize = {i};\n"),
            )
            .expect("write");
        }
        git(&repo_dir, &["add", "."]);
        git(&repo_dir, &["commit", "-q", "-m", "bench"]);

        let engine = GitEngine::new(&repo_dir);
        let _service_lock = engine.lock_worktree_service().expect("service lock");
        let mut samples = Vec::new();
        for run in 0..5 {
            let m = materialize(&engine, &format!("agent/bench/r{run}")).expect("materialize");
            samples.push(m.worktree.materialize_micros);
        }
        samples.sort_unstable();
        println!(
            "files={file_count:>6} materialize_micros min={} median={} max={}",
            samples[0], samples[2], samples[4]
        );
    }
}

/// Benchmark (60,000 commits + 50 fast-forward + 10 local-ahead + 10 diverged branches):
/// `cargo test -p gitust-git --release -- --ignored --nocapture bench_sync_50_branches_on_deep_history`
#[test]
#[ignore = "benchmark; run explicitly"]
fn bench_sync_50_branches_on_deep_history() {
    use std::io::Write;
    use std::process::Stdio;
    use std::time::Instant;

    let tmp = TempDir::new().expect("tempdir");
    let upstream_dir = tmp.path().join("upstream-60k");
    let mirror_dir = tmp.path().join("mirror-60k");
    fs::create_dir_all(&upstream_dir).expect("mkdir");
    git(&upstream_dir, &["init", "--bare", "-q", "-b", "main"]);

    let mut fi = Command::new("git")
        .arg("-C")
        .arg(&upstream_dir)
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn fast-import");
    {
        let stdin = fi.stdin.as_mut().expect("stdin");
        writeln!(stdin, "blob\nmark :1\ndata 6\nhello\n").expect("blob");
        for i in 1..=60_000_u32 {
            let ts = 1_700_000_000_u64 + u64::from(i);
            writeln!(
                stdin,
                "commit refs/heads/main\nmark :{}\ncommitter Bench <b@c> {ts} +0000\ndata 8\nc-{i:05}\n",
                i + 1
            )
            .expect("commit header");
            if i > 1 {
                writeln!(stdin, "from :{}", i).expect("from");
            } else {
                writeln!(stdin, "M 100644 :1 README.md").expect("tree");
            }
        }
        for b in 0..50_u32 {
            writeln!(stdin, "reset refs/heads/ff-branch-{b:02}\nfrom :60001\n").expect("reset");
        }
        for b in 0..10_u32 {
            writeln!(stdin, "reset refs/heads/ahead-branch-{b:02}\nfrom :60001\n").expect("reset");
            writeln!(stdin, "reset refs/heads/div-branch-{b:02}\nfrom :60001\n").expect("reset");
        }
    }
    assert!(fi.wait().expect("wait fast-import").success());

    let mirror_engine = GitEngine::new(&mirror_dir);
    let upstream_str = upstream_dir.to_str().expect("utf8");
    mirror_engine
        .sync_inbound_read_only(upstream_str)
        .expect("initial 60k sync");

    let head_sha = git(&upstream_dir, &["rev-parse", "refs/heads/main"]);

    // Advance 10 local-ahead and 10 diverged branches locally in mirror_dir
    let mut fi_local = Command::new("git")
        .arg("-C")
        .arg(&mirror_dir)
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn fast-import local");
    {
        let stdin = fi_local.stdin.as_mut().expect("stdin");
        for b in 0..10_u32 {
            let ts = 1_700_090_000_u64 + u64::from(b);
            writeln!(
                stdin,
                "commit refs/heads/ahead-branch-{b:02}\ncommitter Local <l@c> {ts} +0000\ndata 6\nla-{b:02}\nfrom {head_sha}\n"
            )
            .expect("local ahead commit");
            writeln!(
                stdin,
                "commit refs/heads/div-branch-{b:02}\ncommitter Local <l@c> {ts} +0000\ndata 6\nld-{b:02}\nfrom {head_sha}\n"
            )
            .expect("local div commit");
        }
    }
    assert!(fi_local.wait().expect("wait fast-import local").success());

    // Advance all 50 FF branches and 10 diverged branches on upstream
    let mut fi2 = Command::new("git")
        .arg("-C")
        .arg(&upstream_dir)
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn fast-import 2");
    {
        let stdin = fi2.stdin.as_mut().expect("stdin");
        for b in 0..50_u32 {
            let ts = 1_700_100_000_u64 + u64::from(b);
            writeln!(
                stdin,
                "commit refs/heads/ff-branch-{b:02}\ncommitter Bench <b@c> {ts} +0000\ndata 6\nff-{b:02}\nfrom {head_sha}\n"
            )
            .expect("ff commit");
        }
        for b in 0..10_u32 {
            let ts = 1_700_110_000_u64 + u64::from(b);
            writeln!(
                stdin,
                "commit refs/heads/div-branch-{b:02}\ncommitter Bench <b@c> {ts} +0000\ndata 6\nud-{b:02}\nfrom {head_sha}\n"
            )
            .expect("upstream div commit");
        }
    }
    assert!(fi2.wait().expect("wait fast-import 2").success());

    let t_total = Instant::now();
    mirror_engine
        .fetch_inbound_mirror_staging(upstream_str)
        .expect("fetch staging");
    let fetch_ms = t_total.elapsed().as_secs_f64() * 1000.0;
    let t_apply = Instant::now();
    let outcome = mirror_engine
        .apply_inbound_mirror_refs(upstream_str, t_total)
        .expect("apply 50 FF + 10 ahead + 10 diverged branches on 60k history");
    let apply_ms = t_apply.elapsed().as_secs_f64() * 1000.0;
    let total_ms = t_total.elapsed().as_secs_f64() * 1000.0;
    println!(
        "60k_commits 50_ff_10_ahead_10_diverged fetch_ms={fetch_ms:.2} apply_under_lock_ms={apply_ms:.2} total_ms={total_ms:.2} synced={} skipped={}",
        outcome.synced_branches_count,
        outcome.skipped_branches.len()
    );
    assert_eq!(outcome.synced_branches_count, 50);
    assert_eq!(outcome.skipped_branches.len(), 20);
    assert!(
        apply_ms < 500.0,
        "50 FF + 10 ahead + 10 diverged branches on 60k history must complete in <500ms, took {apply_ms:.2}ms"
    );
}
