use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::{GitEngine, GitEngineError, SearchMatchKind, SymbolKind};

fn git(dir: &Path, args: &[&str]) {
    let mut cmd = std::process::Command::new("git");
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(k);
        }
    }
    let out = cmd
        .arg("-C")
        .arg(dir)
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "SearchTest")
        .env("GIT_AUTHOR_EMAIL", "search@company.internal")
        .env("GIT_COMMITTER_NAME", "SearchTest")
        .env("GIT_COMMITTER_EMAIL", "search@company.internal")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn search_finds_committed_symbol_and_code_matches_and_reuses_commit_index_cache() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    fs::create_dir_all(repo_dir.join("src")).expect("mkdir");
    git(&repo_dir, &["init", "-q", "-b", "main"]);

    fs::write(
        repo_dir.join("src/lib.rs"),
        "pub struct AuthVerifier {\n    pub realm: String,\n}\n\npub fn verify_enterprise_assertion(token: &str) -> bool {\n    !token.is_empty()\n}\n",
    )
    .expect("write lib.rs");
    fs::write(
        repo_dir.join("src/client.ts"),
        "export interface SearchQueryOptions {\n  revision?: string;\n}\nexport function executeSearchQuery(q: string): string {\n  return q.trim();\n}\n",
    )
    .expect("write client.ts");
    git(&repo_dir, &["add", "."]);
    git(&repo_dir, &["commit", "-q", "-m", "initial search corpus"]);

    let engine = GitEngine::new(&repo_dir);
    let head = engine.inspect_head_identity().expect("head");

    let first = engine
        .search_repository(None, "verify_enterprise_assertion", SearchMatchKind::All)
        .expect("first search");
    assert_eq!(first.commit_sha, head.head_sha);
    assert_eq!(first.indexed_file_count, 2);
    assert!(!first.index_cache_hit, "first search must build index");
    assert_eq!(first.hits.len(), 1);
    assert_eq!(first.hits[0].path, "src/lib.rs");
    assert_eq!(first.hits[0].line_number, 5);
    assert_eq!(first.hits[0].match_kind, SearchMatchKind::Symbol);
    assert_eq!(
        first.hits[0].symbol_name.as_deref(),
        Some("verify_enterprise_assertion")
    );
    assert_eq!(first.hits[0].symbol_kind, Some(SymbolKind::Function));

    let second = engine
        .search_repository(Some("main"), "SearchQueryOptions", SearchMatchKind::Symbol)
        .expect("second search");
    assert_eq!(second.commit_sha, head.head_sha);
    assert!(
        second.index_cache_hit,
        "second search on same commit SHA must reuse cached index"
    );
    assert_eq!(second.hits.len(), 1);
    assert_eq!(second.hits[0].path, "src/client.ts");
    assert_eq!(second.hits[0].line_number, 1);
    assert_eq!(
        second.hits[0].symbol_name.as_deref(),
        Some("SearchQueryOptions")
    );
    assert_eq!(second.hits[0].symbol_kind, Some(SymbolKind::Interface));

    let code_only = engine
        .search_repository(Some(&head.head_sha.0), "q.trim()", SearchMatchKind::Code)
        .expect("code search");
    assert!(code_only.index_cache_hit);
    assert_eq!(code_only.hits.len(), 1);
    assert_eq!(code_only.hits[0].path, "src/client.ts");
    assert_eq!(code_only.hits[0].line_number, 5);
    assert_eq!(code_only.hits[0].match_kind, SearchMatchKind::Code);
    assert!(code_only.hits[0].symbol_name.is_none());
}

#[test]
fn search_excludes_uncommitted_worktree_modifications_and_untracked_files() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    fs::create_dir_all(&repo_dir).expect("mkdir");
    git(&repo_dir, &["init", "-q", "-b", "main"]);

    fs::write(
        repo_dir.join("committed.rs"),
        "pub fn committed_baseline_marker() {}\n",
    )
    .expect("write committed.rs");
    git(&repo_dir, &["add", "."]);
    git(&repo_dir, &["commit", "-q", "-m", "committed baseline"]);

    // Modify tracked file and create untracked file in working tree without committing.
    fs::write(
        repo_dir.join("committed.rs"),
        "pub fn dirty_worktree_marker_never_indexed() {}\n",
    )
    .expect("overwrite committed.rs");
    fs::write(
        repo_dir.join("untracked_secret.rs"),
        "pub const UNTRACKED_WORKTREE_SECRET: &str = \"leak\";\n",
    )
    .expect("write untracked");

    let engine = GitEngine::new(&repo_dir);

    let dirty = engine
        .search_repository(
            None,
            "dirty_worktree_marker_never_indexed",
            SearchMatchKind::All,
        )
        .expect("search dirty marker");
    assert!(
        dirty.hits.is_empty(),
        "uncommitted working tree edits must never be indexed"
    );

    let untracked = engine
        .search_repository(None, "UNTRACKED_WORKTREE_SECRET", SearchMatchKind::All)
        .expect("search untracked");
    assert!(
        untracked.hits.is_empty(),
        "untracked files must never be indexed"
    );

    let baseline = engine
        .search_repository(None, "committed_baseline_marker", SearchMatchKind::All)
        .expect("search committed baseline");
    assert_eq!(baseline.hits.len(), 1);
    assert_eq!(baseline.hits[0].path, "committed.rs");
}

#[test]
fn search_rejects_agent_branches_and_skips_binary_blobs() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    fs::create_dir_all(&repo_dir).expect("mkdir");
    git(&repo_dir, &["init", "-q", "-b", "main"]);

    fs::write(repo_dir.join("main.rs"), "pub fn valid_main_symbol() {}\n").expect("write text");
    fs::write(
        repo_dir.join("binary.bin"),
        b"\x00\x01\x02fn binary_embedded_marker() {}\x00",
    )
    .expect("write binary");
    git(&repo_dir, &["add", "."]);
    git(&repo_dir, &["commit", "-q", "-m", "main with binary"]);

    git(&repo_dir, &["checkout", "-q", "-b", "agent/claude/task-1"]);
    fs::write(
        repo_dir.join("agent_only.rs"),
        "pub fn secret_agent_only_symbol() {}\n",
    )
    .expect("write agent_only");
    git(&repo_dir, &["add", "."]);
    git(&repo_dir, &["commit", "-q", "-m", "agent commit"]);
    let agent_sha_out = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("rev-parse");
    let agent_sha = String::from_utf8_lossy(&agent_sha_out.stdout)
        .trim()
        .to_owned();
    git(&repo_dir, &["checkout", "-q", "main"]);

    let engine = GitEngine::new(&repo_dir);

    for forbidden_rev in [
        "agent/claude/task-1",
        "refs/heads/agent/claude/task-1",
        "AGENT/claude/task-1",
        "agent/claude/task-1~1",
        "refs/stash",
        agent_sha.as_str(),
    ] {
        let err = engine
            .search_repository(
                Some(forbidden_rev),
                "valid_main_symbol",
                SearchMatchKind::All,
            )
            .expect_err("agent branch revision or agent-only commit SHA must be rejected");
        assert!(matches!(err, GitEngineError::InvalidRevision(_)));
    }

    let bin_res = engine
        .search_repository(Some("main"), "binary_embedded_marker", SearchMatchKind::All)
        .expect("search skipping binary");
    assert_eq!(bin_res.indexed_file_count, 1);
    assert!(bin_res.hits.is_empty(), "binary blobs must be skipped");
}
