use gitust_core::{CommitSha, PullRequestStatus, RepoAccess, RepoId, ReviewVerdict};

use crate::{
    audit_trail::append_audit_event_locked, sqlite_wal_path, AuthStore, CreatePullRequestInput,
    PullRequestLookup, SqliteWalDb, GENESIS_AUDIT_HASH,
};

#[test]
fn sqlite_wal_retains_over_600_pull_requests_across_reopen() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("auth-db.json");
    let store = AuthStore::open_persistent(&db_path).expect("open store");

    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "SyntheticPassword!2026")
        .expect("bootstrap");
    let login = store
        .login("admin", "SyntheticPassword!2026")
        .expect("login");
    let caller = store
        .authenticate_session(&login.raw_session_token)
        .expect("principal");

    let repo_id = RepoId::new();
    let access = RepoAccess {
        repo_id,
        registration: None,
    };
    let base_sha = CommitSha("1111111111111111111111111111111111111111".to_owned());
    let head_sha = CommitSha("2222222222222222222222222222222222222222".to_owned());

    for idx in 1..=620u64 {
        let created = store
            .create_pull_request(
                &caller,
                access,
                CreatePullRequestInput {
                    repo_id,
                    title: format!("Scalable PR #{idx}"),
                    description: format!("Body for PR #{idx}"),
                    base_branch: "main".to_owned(),
                    head_branch: format!("feat/branch-{idx}"),
                    base_sha: base_sha.clone(),
                    head_sha: head_sha.clone(),
                    status: PullRequestStatus::Open,
                },
            )
            .expect("create pull request beyond 512 cap");
        assert_eq!(created.number, idx);
    }

    store
        .add_pull_request_review(
            &caller,
            access,
            PullRequestLookup::Number(1),
            ReviewVerdict::Approve,
            "LGTM on PR #1".to_owned(),
            head_sha.clone(),
        )
        .expect("review PR #1");
    store
        .add_pull_request_comment(
            &caller,
            access,
            PullRequestLookup::Number(1),
            "Inline note on PR #1".to_owned(),
            Some("src/lib.rs".to_owned()),
            Some(10),
            Some(head_sha.clone()),
        )
        .expect("comment PR #1");

    let wal_file = sqlite_wal_path(&db_path);
    assert!(wal_file.exists(), "expected SQLite WAL sidecar file");
    let wal_conn = SqliteWalDb::open(&wal_file).expect("open sqlite wal");
    assert_eq!(wal_conn.journal_mode().expect("journal_mode"), "wal");

    let reopened = AuthStore::open_persistent(&db_path).expect("reopen store");
    let pulls = reopened
        .list_pull_requests(&caller, access)
        .expect("list all 620 PRs");
    assert_eq!(pulls.len(), 620);
    assert_eq!(pulls[0].number, 620);
    assert_eq!(pulls[619].number, 1);

    let pr_one = reopened
        .get_pull_request(&caller, access, PullRequestLookup::Number(1))
        .expect("get PR #1")
        .expect("PR #1 present");
    assert_eq!(pr_one.title, "Scalable PR #1");
    assert_eq!(pr_one.reviews.len(), 1);
    assert_eq!(pr_one.comments.len(), 1);
}

#[test]
fn sqlite_wal_retains_over_4200_audit_events_with_verified_genesis_hash_chain() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("auth-db.json");
    let store = AuthStore::open_persistent(&db_path).expect("open store");

    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "SyntheticPassword!2026")
        .expect("bootstrap");
    let login = store
        .login("admin", "SyntheticPassword!2026")
        .expect("login");
    let caller = store
        .authenticate_session(&login.raw_session_token)
        .expect("principal");

    {
        let mut guard = store.write_state().expect("write_state");
        for idx in 0..4250u32 {
            append_audit_event_locked(
                &mut guard,
                Some(caller.user_id),
                &caller.username,
                "repo.HighVolumeEvent",
                &format!("target:{idx}"),
            );
        }
        let pending = store.maybe_snapshot(guard);
        store
            .persist_snapshot(pending)
            .expect("persist 4250+ audit events");
    }

    let wal_file = sqlite_wal_path(&db_path);
    let wal_conn = SqliteWalDb::open(&wal_file).expect("open sqlite wal");
    assert_eq!(wal_conn.journal_mode().expect("journal_mode"), "wal");

    let reopened = AuthStore::open_persistent(&db_path).expect("reopen store");
    let (entries, report) = reopened
        .audit_log_for_admin(&caller)
        .expect("audit_log_for_admin");
    assert!(
        entries.len() > 4250,
        "expected > 4250 audit entries, got {}",
        entries.len()
    );
    assert_eq!(entries[0].seq, 1);
    assert_eq!(entries[0].prev_hash, GENESIS_AUDIT_HASH);
    assert!(report.verified, "audit chain must verify: {report:?}");
    assert_eq!(report.entries_checked, entries.len());

    #[cfg(unix)]
    {
        drop(wal_conn);
        let count_before = entries.len();
        let wal_sidecar = temp.path().join("auth-db.json.sqlite3-wal");
        let _ = std::fs::remove_file(&wal_sidecar);
        std::os::unix::fs::symlink(temp.path().join("nonexistent-target"), &wal_sidecar)
            .expect("create symlinked wal sidecar");
        let mut guard = store.write_state().expect("write_state");
        append_audit_event_locked(
            &mut guard,
            Some(caller.user_id),
            &caller.username,
            "repo.ShouldRollback",
            "target:rollback",
        );
        let pending = store.maybe_snapshot(guard);
        assert!(
            store.persist_snapshot(pending).is_err(),
            "symlinked wal sidecar must fail closed before renaming auth-db.json"
        );
        std::fs::remove_file(&wal_sidecar).expect("remove symlinked wal sidecar");
        let (after_entries, after_report) = store
            .audit_log_for_admin(&caller)
            .expect("audit_log_for_admin after rollback");
        assert_eq!(after_entries.len(), count_before);
        assert!(after_report.verified);

        crate::persistence::FAIL_DIRECTORY_SYNC.with(|cell| {
            *cell.borrow_mut() = Some(db_path.clone());
        });
        let mut guard = store.write_state().expect("write_state");
        append_audit_event_locked(
            &mut guard,
            Some(caller.user_id),
            &caller.username,
            "repo.DirSyncUnconfirmed",
            "target:dir-sync-rollback",
        );
        let pending = store.maybe_snapshot(guard);
        let dir_sync_res = store.persist_snapshot(pending);
        crate::persistence::FAIL_DIRECTORY_SYNC.with(|cell| {
            *cell.borrow_mut() = None;
        });
        assert!(
            dir_sync_res.is_err(),
            "directory sync failure must report an unconfirmed publication"
        );
        let reopened_after_dir_fail =
            AuthStore::open_persistent(&db_path).expect("reopen after dir sync failure");
        let (reopened_entries, reopened_report) = reopened_after_dir_fail
            .audit_log_for_admin(&caller)
            .expect("audit_log after reopen");
        assert_eq!(reopened_entries.len(), count_before + 1);
        assert_eq!(
            reopened_entries
                .last()
                .expect("published audit entry")
                .action,
            "repo.DirSyncUnconfirmed"
        );
        assert!(reopened_report.verified);
    }
}

#[test]
fn sqlite_checkpoint_rejects_missing_truncated_and_changed_history() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("auth.json");
    let store = AuthStore::open_persistent(&path).expect("open");
    let mut state = store.write_state().expect("lock");
    for idx in 0..70 {
        append_audit_event_locked(&mut state, None, "fixture", "event", &idx.to_string());
    }
    store
        .persist_snapshot(store.maybe_snapshot(state))
        .expect("save");
    let sidecar = sqlite_wal_path(&path);
    let saved = temp.path().join("saved.sqlite3");
    std::fs::rename(&sidecar, &saved).expect("hold sidecar");
    assert!(
        AuthStore::open_persistent(&path).is_err(),
        "missing history must fail closed"
    );
    std::fs::rename(saved, &sidecar).expect("restore");
    assert!(AuthStore::open_persistent(&path).is_ok());
    let db = SqliteWalDb::open(&sidecar).expect("sqlite");
    db.execute_batch("DELETE FROM audit_log WHERE seq = 1;")
        .expect("truncate fixture");
    assert!(
        AuthStore::open_persistent(&path).is_err(),
        "missing prefix must fail closed"
    );
}

#[test]
fn sqlite_checkpoint_detects_a_changed_prefix_and_legacy_complete_json_still_opens() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("auth.json");
    let store = AuthStore::open_persistent(&path).expect("open");
    let mut state = store.write_state().expect("lock");
    for idx in 0..70 {
        append_audit_event_locked(&mut state, None, "fixture", "event", &idx.to_string());
    }
    store
        .persist_snapshot(store.maybe_snapshot(state))
        .expect("save");
    let db = SqliteWalDb::open(&sqlite_wal_path(&path)).expect("sqlite");
    db.execute_batch("UPDATE audit_log SET payload_json = replace(payload_json, 'fixture', 'changed') WHERE seq = 1;").expect("alter fixture prefix");
    assert!(AuthStore::open_persistent(&path).is_err());
    let legacy_path = temp.path().join("legacy.json");
    let snapshot = crate::models::PersistedSnapshot {
        security_version: 1,
        ..Default::default()
    };
    std::fs::write(&legacy_path, serde_json::to_vec(&snapshot).expect("encode")).expect("legacy");
    assert!(AuthStore::open_persistent(&legacy_path).is_ok());
}
