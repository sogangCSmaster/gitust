use super::*;

fn with_label(engine: &GitEngine, branch: &str, label: &str) -> MaterializedWorktree {
    engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            RepoId::new(),
            AgentKind::CodexSwarm,
            branch,
            "main",
            label,
        )
        .unwrap()
}

#[test]
fn worktree_labels_survive_restarts_including_empty_unicode_and_escaped_byte_limit() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let labels = [
        String::new(),
        "검증된 변경을 리뷰합니다\nline two".to_owned(),
        format!("{}a", "한".repeat(1365)),
        "\0".repeat(4096),
    ];
    for (index, label) in labels.iter().enumerate() {
        let task = with_label(&engine, &format!("agent/test/label-{index}"), label);
        let recovered = GitEngine::new(&repo)
            .recover_agent_worktrees(RepoId::new())
            .unwrap()
            .into_iter()
            .find(|m| m.worktree.id == task.worktree.id)
            .unwrap();
        assert_eq!(recovered.worktree.prompt_summary, *label);
        assert_eq!(recovered.worktree.branch, task.worktree.branch);
        assert_eq!(recovered.worktree.base_commit, task.worktree.base_commit);
        assert_eq!(recovered.worktree.status, WorktreeStatus::Pending);
        assert!(recovered.worktree.verified_commit.is_none());
        let bytes = fs::read(task.worktree_admin_dir.join("gitust-origin.json")).unwrap();
        let record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(record["prompt_summary"], *label);
        assert!(bytes.len() <= 32 * 1024);
        if index == 3 {
            assert!(
                bytes.len() > 8192,
                "JSON escaping must exercise the old cap"
            );
        }
    }
    GitEngine::verify_git_fsck(&repo).unwrap();
}

#[test]
fn worktree_label_byte_limit_rejects_before_creating_a_branch_or_checkout() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let before = git(&repo, &["show-ref"]);
    for (index, label) in ["a".repeat(4097), "한".repeat(1366)].iter().enumerate() {
        let id = WorktreeId::new();
        let result = engine.materialize_agent_worktree(
            id,
            RepoId::new(),
            AgentKind::CodexSwarm,
            &format!("agent/test/oversized-label-{index}"),
            "main",
            label.clone(),
        );
        assert!(result.is_err(), "oversized label must be refused");
        assert_eq!(git(&repo, &["show-ref"]), before);
        assert!(!temp
            .path()
            .join(".repo.gitust-worktrees")
            .join(id.0.to_string())
            .exists());
        assert!(engine.list_agent_worktrees().unwrap().is_empty());
    }
}

#[test]
fn legacy_records_and_reflog_migration_keep_the_unknown_label_fallback() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let task = with_label(&engine, "agent/test/legacy-label", "old label");
    let path = task.worktree_admin_dir.join("gitust-origin.json");
    let mut record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record.as_object_mut().unwrap().remove("prompt_summary");
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    for migrate in [false, true] {
        if migrate {
            fs::remove_file(&path).unwrap();
        }
        let recovered = engine
            .recover_agent_worktrees(RepoId::new())
            .unwrap()
            .remove(0);
        assert_eq!(recovered.worktree.prompt_summary, "recovered from disk");
        assert_eq!(recovered.worktree.base_branch, task.worktree.base_branch);
        assert_eq!(recovered.worktree.base_commit, task.worktree.base_commit);
        let migrated: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(migrated.get("prompt_summary").is_none());
    }
}

#[test]
fn invalid_persisted_labels_and_oversized_valid_json_fail_closed() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let task = with_label(&engine, "agent/test/corrupt-label", "display only");
    let path = task.worktree_admin_dir.join("gitust-origin.json");
    let saved = fs::read(&path).unwrap();
    for label in [serde_json::json!("a".repeat(4097)), serde_json::json!(7)] {
        let mut record: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        record["prompt_summary"] = label;
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(engine.recover_agent_worktrees(RepoId::new()).is_err());
    }
    let mut oversized = saved.clone();
    oversized.resize(32 * 1024 + 1, b' ');
    assert!(serde_json::from_slice::<serde_json::Value>(&oversized).is_ok());
    fs::write(&path, oversized).unwrap();
    assert!(engine.recover_agent_worktrees(RepoId::new()).is_err());
    fs::write(&path, saved).unwrap();
    assert_eq!(
        engine.recover_agent_worktrees(RepoId::new()).unwrap()[0]
            .worktree
            .prompt_summary,
        "display only"
    );
}
