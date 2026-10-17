use super::*;
use crate::SmartHttpService;

#[test]
fn managed_checkout_pointer_cannot_redirect_to_another_repository_or_checkout() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("repo");
    init_repo(&path);
    let engine = GitEngine::new(&path);
    let m = materialize(&engine, "agent/test/pointer").unwrap();
    let other = materialize(&engine, "agent/test/other").unwrap();
    let original_other_head = fs::read(other.worktree_admin_dir.join("HEAD")).unwrap();
    // Forge identical HEAD/commit metadata; branch-name/SHA checks alone cannot catch this.
    fs::write(
        other.worktree_admin_dir.join("HEAD"),
        format!("ref: refs/heads/{}\n", m.worktree.branch),
    )
    .unwrap();
    let pointer = m.worktree_dir.join(".git");
    let original = fs::read(&pointer).unwrap();
    for replacement in [
        fs::read(other.worktree_dir.join(".git")).unwrap(),
        format!("gitdir: {}\n", path.join(".git").display()).into_bytes(),
    ] {
        fs::write(&pointer, replacement).unwrap();
        assert!(engine
            .clean_worktree_commit(m.worktree.id, &m.worktree.branch)
            .is_err());
        assert!(engine.recover_agent_worktrees(RepoId::new()).is_err());
    }
    fs::write(&pointer, original).unwrap();
    assert!(engine
        .clean_worktree_commit(m.worktree.id, &m.worktree.branch)
        .is_ok());
    assert!(
        engine.recover_agent_worktrees(RepoId::new()).is_err(),
        "the second checkout still disagrees with its immutable creation branch"
    );
    fs::write(other.worktree_admin_dir.join("HEAD"), original_other_head).unwrap();
    assert!(engine.recover_agent_worktrees(RepoId::new()).is_ok());
}

#[test]
fn verification_rejects_self_ignored_untracked_gitignore_but_allows_build_outputs() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("repo");
    init_repo(&path);
    fs::write(path.join(".gitignore"), "target/\n").unwrap();
    git(&path, &["add", ".gitignore"]);
    git(&path, &["commit", "-m", "ignore artifacts"]);
    let engine = GitEngine::new(&path);
    let m = materialize(&engine, "agent/test/ignored").unwrap();
    fs::create_dir(m.worktree_dir.join("target")).unwrap();
    fs::write(m.worktree_dir.join("target/.gitignore"), "*\n").unwrap();
    fs::write(m.worktree_dir.join("target/output"), "build artifact").unwrap();
    assert!(engine
        .clean_worktree_commit(m.worktree.id, &m.worktree.branch)
        .is_ok());
    fs::create_dir(m.worktree_dir.join("src/hidden")).unwrap();
    fs::write(m.worktree_dir.join("src/hidden/.gitignore"), "*\n").unwrap();
    fs::write(
        m.worktree_dir.join("src/hidden/source.rs"),
        "hidden test input",
    )
    .unwrap();
    assert!(engine
        .clean_worktree_commit(m.worktree.id, &m.worktree.branch)
        .is_err());
}

#[test]
fn managed_approval_rejects_dirty_candidate_without_a_server_expected_sha() {
    for flag in [None, Some("--skip-worktree"), Some("--assume-unchanged")] {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("repo");
        init_repo(&path);
        let engine = GitEngine::new(&path);
        let m = materialize(&engine, "agent/test/dirty").unwrap();
        git(
            &m.worktree_dir,
            &["commit", "--allow-empty", "-m", "candidate"],
        );
        let before = git(&path, &["rev-parse", "main"]);
        if let Some(flag) = flag {
            git(&m.worktree_dir, &["update-index", flag, "src/lib.rs"]);
        }
        fs::write(
            m.worktree_dir.join("src/lib.rs"),
            "uncommitted test input\n",
        )
        .unwrap();
        assert!(engine
            .approve_managed_worktree(m.worktree.id, &m.worktree.branch, "main")
            .is_err());
        assert_eq!(git(&path, &["rev-parse", "main"]), before);
        assert!(engine
            .load_worktree_approval(m.worktree.id)
            .unwrap()
            .is_none());
    }
}

#[test]
fn unsafe_execution_and_host_credential_config_is_rejected() {
    for text in [
        "[filter \"source\"]\n clean = /tmp/command\n",
        "[filter \"source\"]\n smudge = command\n",
        "[diff \"source\"]\n command = command\n",
        "[core]\n sshCommand = command\n",
        "[core]\n askPass = command\n",
        "[http]\n cookieFile = /tmp/cookies\n",
        "[http]\n sslKey = /tmp/key\n",
        "[url \"https://other.example/\"]\n insteadOf = https://example/\n",
    ] {
        assert!(config_contains_credentials(text), "accepted {text}");
    }
}

#[test]
fn smart_http_does_not_advertise_private_mirror_staging_refs() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("repo");
    init_repo(&path);
    for name in [
        "refs/remotes/upstream/main",
        "refs/remotes/upstream-tags/v1",
    ] {
        git(&path, &["update-ref", name, "HEAD"]);
    }
    let engine = GitEngine::new(&path);
    let bytes = engine
        .smart_http_advertise_refs(SmartHttpService::UploadPack)
        .unwrap();
    let advertisement = String::from_utf8_lossy(&bytes);
    assert!(advertisement.contains("refs/heads/main"));
    assert!(!advertisement.contains("refs/remotes/"));
}

#[test]
fn deleted_diff_path_with_embedded_b_prefix_is_exact() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("repo");
    init_repo(&path);
    fs::create_dir_all(path.join("a b")).unwrap();
    fs::create_dir_all(path.join("a b/deleted b")).unwrap();
    let name = "a b/deleted b/name.txt";
    fs::write(path.join(name), "fixture\n").unwrap();
    let binary = "a b/deleted b/binary.bin";
    fs::write(path.join(binary), b"\0fixture").unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-m", "add tricky file"]);
    fs::remove_file(path.join(name)).unwrap();
    fs::remove_file(path.join(binary)).unwrap();
    git(&path, &["commit", "-am", "delete tricky file"]);
    let diff = GitEngine::new(&path)
        .inspect_diff(None, None, None)
        .unwrap();
    assert!(diff.files.iter().any(|f| f.path == name));
    assert!(diff.files.iter().any(|f| f.path == binary));
}

#[test]
fn inherited_git_config_cannot_override_host_commands() {
    if std::env::var_os("GITUST_ENV_PROBE").is_some() {
        let temp = TempDir::new().unwrap();
        init_repo(temp.path());
        assert!(crate::run_git(temp.path(), ["config", "--get", "gitust.injected"]).is_err());
        let command = crate::git_command(temp.path());
        for key in [
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_KEY_0",
            "GIT_CONFIG_VALUE_0",
            "GIT_EXEC_PATH",
        ] {
            assert!(command
                .get_envs()
                .any(|(name, value)| name == key && value.is_none()));
        }
        return;
    }
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::tests_review_security::inherited_git_config_cannot_override_host_commands",
        ])
        .env("GITUST_ENV_PROBE", "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "gitust.injected")
        .env("GIT_CONFIG_VALUE_0", "unexpected")
        .env("GIT_EXEC_PATH", "/nonexistent-gitust-test")
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn bounded_process_capture_drains_output_and_reaps_descendants_on_success_and_timeout() {
    use std::time::{Duration, Instant};
    let mut verbose = Command::new("/bin/sh");
    verbose.args(["-c", "dd if=/dev/zero bs=1024 count=128 2>/dev/null; dd if=/dev/zero bs=1024 count=128 >&2 2>/dev/null"]);
    let (output, truncated) =
        crate::process::run_capped(verbose, Duration::from_secs(2), 1024).unwrap();
    assert_eq!(output.len(), 1024);
    assert!(truncated);
    for script in ["sleep 5 & exit 0", "sleep 5 & wait"] {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        let start = Instant::now();
        let result = crate::process::run_capped(command, Duration::from_millis(100), 1024);
        assert_eq!(result.is_ok(), script.ends_with("exit 0"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}

#[test]
fn verbose_fetch_keeps_draining_stderr_beyond_diagnostic_limit() {
    use std::time::Duration;
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("upstream");
    init_repo(&path);
    let mut command = crate::git_command(&path);
    command.args(["update-ref", "--stdin"]);
    let updates = (0..500).map(|i| format!("update refs/heads/many-long-branch-names-to-exercise-diagnostic-pipe-{i:04} HEAD\n")).collect::<String>();
    crate::process::run_capped_with_input(
        command,
        Duration::from_secs(10),
        1024,
        Some(updates.into_bytes()),
    )
    .unwrap();
    let mirror = temp.path().join("mirror");
    fs::create_dir(&mirror).unwrap();
    git(&mirror, &["init", "--bare"]);
    crate::config_guard::run_git_fetch_with_timeout(
        &mirror,
        path.to_str().unwrap(),
        Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(
        git(
            &mirror,
            &[
                "rev-parse",
                "refs/remotes/upstream/many-long-branch-names-to-exercise-diagnostic-pipe-0499"
            ]
        ),
        git(&path, &["rev-parse", "HEAD"])
    );
}

#[test]
fn recovery_bounds_untrusted_reflog_and_skips_unmanaged_directory_names() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("repo");
    init_repo(&path);
    let engine = GitEngine::new(&path);
    let m = materialize(&engine, "agent/test/reflog").unwrap();
    let log = m
        .common_git_dir
        .join("logs/refs/heads")
        .join(&m.worktree.branch);
    fs::write(&log, vec![b'x'; 100_000]).unwrap();
    let recovered = engine.recover_agent_worktrees(RepoId::new()).unwrap();
    assert_eq!(
        recovered[0].worktree.base_commit, m.worktree.base_commit,
        "trusted creation metadata takes precedence over an expired or invalid reflog"
    );
    fs::remove_file(m.worktree_admin_dir.join("gitust-origin.json")).unwrap();
    let recovered = engine.recover_agent_worktrees(RepoId::new()).unwrap();
    assert!(recovered[0].worktree.base_commit.0.is_empty());
    assert!(recovered[0].worktree.base_branch.is_empty());
    let other = m.worktrees_base.join("not-a-uuid");
    git(
        &path,
        &[
            "worktree",
            "add",
            other.to_str().unwrap(),
            "-b",
            "agent/test/unmanaged",
        ],
    );
    assert_eq!(
        engine.recover_agent_worktrees(RepoId::new()).unwrap().len(),
        1
    );
}
