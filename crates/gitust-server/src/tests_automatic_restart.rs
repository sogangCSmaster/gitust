use super::tests_automatic_checks::{git, policy, settle};
use super::tests_verification::fixture;
use gitust_core::CommitSha;

#[tokio::test]
async fn queued_checks_survive_service_restart_without_granting_approval() {
    let (tmp, state, _app, token, wt, dir) = fixture().await;
    policy(
        &tmp.path().join("repo"),
        true,
        "printf resumed-durable-check",
    );
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let first = crate::automatic_checks::start(&state).await.unwrap();
    let guard = state
        .get_worktree_runtime(wt.id)
        .unwrap()
        .exec_lock
        .clone()
        .lock_owned()
        .await;
    crate::automatic_checks::after_commit(
        &state,
        &caller,
        wt.id,
        CommitSha(git(&dir, &["rev-parse", "HEAD"])),
        false,
    )
    .unwrap();
    first.shutdown().await.unwrap();
    drop(guard);
    let _second = crate::automatic_checks::start(&state).await.unwrap();
    let records = settle(&state, &wt).await;
    assert_eq!(records.len(), 1, "persisted queued Checks must resume");
    assert!(
        serde_json::to_value(records).unwrap()[0]["result"]["stdout"]
            .as_str()
            .unwrap()
            .contains("resumed-durable-check")
    );
    assert!(state.worktrees.read()[0].verified_commit.is_none());
}

#[tokio::test]
async fn accepted_prepared_intent_recovers_but_unapplied_intent_does_not_run() {
    use crate::automatic_run_model::{Origin, Phase};
    let (tmp, state, _app, token, wt, dir) = fixture().await;
    policy(&tmp.path().join("repo"), true, "printf prepared-recovery");
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let first = crate::automatic_checks::start(&state).await.unwrap();
    let before = CommitSha(git(&dir, &["rev-parse", "HEAD"]));
    let absent = crate::automatic_checks::prepare(
        &state,
        &caller,
        wt.id,
        CommitSha("c".repeat(40)),
        Some(before.clone()),
        Origin::Mcp,
    )
    .unwrap()
    .unwrap();
    drop(absent);
    drop(first);
    let second = crate::automatic_checks::start(&state).await.unwrap();
    let store = state.automatic_checks.store().unwrap();
    let doc = store
        .document(wt.id, &wt.branch, &wt.base_branch)
        .unwrap()
        .unwrap();
    assert_eq!(doc.recent[0].phase, Phase::NotApplied);
    assert!(settle(&state, &wt).await.is_empty());
    let guard = state
        .get_worktree_runtime(wt.id)
        .unwrap()
        .exec_lock
        .clone()
        .lock_owned()
        .await;
    std::fs::write(dir.join("README.md"), "published before crash\n").unwrap();
    let prepared = state
        .git_engine
        .prepare_worktree_commit(
            wt.id,
            &wt.branch,
            &before,
            &["README.md".into()],
            "durable intent",
            "test@example.test",
        )
        .unwrap();
    let expected = prepared.result().commit.clone();
    let intent = crate::automatic_checks::prepare(
        &state,
        &caller,
        wt.id,
        expected.clone(),
        Some(before),
        Origin::Mcp,
    )
    .unwrap()
    .unwrap();
    state.git_engine.finish_worktree_commit(prepared).unwrap();
    drop(intent);
    second.shutdown().await.unwrap();
    drop(guard);
    let _third = crate::automatic_checks::start(&state).await.unwrap();
    let history = settle(&state, &wt).await;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].checked_commit.as_ref(), Some(&expected));
    assert!(state.worktrees.read()[0].verified_commit.is_none());
}

#[tokio::test]
async fn uncertain_running_is_interrupted_without_replay_or_approval() {
    use crate::automatic_run_model::{Origin, Phase};
    let (tmp, state, app, token, wt, dir) = fixture().await;
    policy(&tmp.path().join("repo"), true, "printf must-not-replay");
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let first = crate::automatic_checks::start(&state).await.unwrap();
    let intent = crate::automatic_checks::prepare(
        &state,
        &caller,
        wt.id,
        CommitSha(git(&dir, &["rev-parse", "HEAD"])),
        None,
        Origin::Mcp,
    )
    .unwrap()
    .unwrap();
    let run_id = intent.run_id;
    state
        .automatic_checks
        .store()
        .unwrap()
        .update(wt.id, &wt.branch, &wt.base_branch, |doc| {
            let mut job = doc.prepared.take().unwrap();
            job.transition(Phase::Running);
            doc.running = Some(job);
            Ok(())
        })
        .unwrap();
    drop(intent);
    drop(first);
    let _second = crate::automatic_checks::start(&state).await.unwrap();
    assert!(settle(&state, &wt).await.is_empty());
    let (status, body) = super::call(
        &app,
        "GET",
        format!("/api/v1/swarm/worktrees/{}/automatic-runs", wt.id.0),
        &token,
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    let records: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(records[0]["run_id"], serde_json::to_value(run_id).unwrap());
    assert_eq!(records[0]["phase"], "interrupted");
    assert_eq!(records[0]["purpose"], "automatic_checks");
    assert_eq!(
        state.worktrees.read()[0].status,
        gitust_core::WorktreeStatus::Failed
    );
    assert!(state.worktrees.read()[0].verified_commit.is_none());
}

#[tokio::test]
async fn live_local_operator_runs_but_restarted_local_jobs_are_cancelled() {
    use crate::automatic_run_model::Phase;
    let (tmp, state, _app, token, wt, dir) = fixture().await;
    policy(&tmp.path().join("repo"), true, "printf live-local");
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let first = crate::automatic_checks::start(&state).await.unwrap();
    let sha = CommitSha(git(&dir, &["rev-parse", "HEAD"]));
    crate::automatic_checks::after_commit(&state, &caller, wt.id, sha.clone(), true).unwrap();
    assert_eq!(settle(&state, &wt).await.len(), 1);
    let guard = state
        .get_worktree_runtime(wt.id)
        .unwrap()
        .exec_lock
        .clone()
        .lock_owned()
        .await;
    for _ in 0..20 {
        crate::automatic_checks::after_commit(&state, &caller, wt.id, sha.clone(), true).unwrap();
    }
    assert_eq!(state.automatic_checks.live_local_actor_count(), 1);
    drop(first);
    assert_eq!(state.automatic_checks.live_local_actor_count(), 0);
    drop(guard);
    let _second = crate::automatic_checks::start(&state).await.unwrap();
    assert_eq!(settle(&state, &wt).await.len(), 1);
    let doc = state
        .automatic_checks
        .store()
        .unwrap()
        .document(wt.id, &wt.branch, &wt.base_branch)
        .unwrap()
        .unwrap();
    assert_eq!(doc.recent[0].phase, Phase::Cancelled);
    assert!(doc.queued.is_none());
}

#[tokio::test]
async fn persisted_account_job_cannot_use_an_empty_auth_store_as_operator_authority() {
    use crate::automatic_run_model::Phase;
    let (tmp, state, _app, token, wt, dir) = fixture().await;
    policy(
        &tmp.path().join("repo"),
        true,
        "printf forbidden-empty-store",
    );
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let first = crate::automatic_checks::start(&state).await.unwrap();
    let guard = state
        .get_worktree_runtime(wt.id)
        .unwrap()
        .exec_lock
        .clone()
        .lock_owned()
        .await;
    crate::automatic_checks::after_commit(
        &state,
        &caller,
        wt.id,
        CommitSha(git(&dir, &["rev-parse", "HEAD"])),
        false,
    )
    .unwrap();
    first.shutdown().await.unwrap();
    let recovered = crate::state::AppState::with_engine_and_auth_store(
        state.git_engine.clone(),
        "https://example.test/repo.git".into(),
        gitust_db::AuthStore::new_in_memory(),
    )
    .unwrap()
    .with_trusted_sandbox_exec(true);
    let _second = crate::automatic_checks::start(&recovered).await.unwrap();
    drop(guard);
    let restored = recovered.worktrees.read()[0].clone();
    assert!(settle(&recovered, &restored).await.is_empty());
    let doc = recovered
        .automatic_checks
        .store()
        .unwrap()
        .document(wt.id, &wt.branch, &wt.base_branch)
        .unwrap()
        .unwrap();
    assert_eq!(doc.recent[0].phase, Phase::Rejected);
    assert!(restored.verified_commit.is_none());
}

#[tokio::test]
async fn cancelling_start_keeps_its_blocking_initialization_exclusive() {
    let (_tmp, state, _app, _token, _wt, _dir) = fixture().await;
    let (entered, observed) = tokio::sync::oneshot::channel();
    let (release, paused) = std::sync::mpsc::channel();
    *state.automatic_checks.startup_hook.lock() = Some((entered, paused));
    let snapshot = state.clone();
    let task = tokio::spawn(async move { crate::automatic_checks::start(&snapshot).await });
    observed.await.unwrap();
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    assert!(crate::automatic_checks::start(&state).await.is_err());
    release.send(()).unwrap();
    let service = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match crate::automatic_checks::start(&state).await {
                Ok(service) => break service,
                Err(_) => tokio::task::yield_now().await,
            }
        }
    })
    .await
    .unwrap();
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn automatic_timeout_has_a_lifecycle_record_and_keeps_execution_fence() {
    let (tmp, mut state, _app, token, wt, dir) = fixture().await;
    policy(&tmp.path().join("repo"), true, "sleep 5");
    state.sandbox_executor = state.sandbox_executor.with_timeout_secs(1);
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let _service = crate::automatic_checks::start(&state).await.unwrap();
    crate::automatic_checks::after_commit(
        &state,
        &caller,
        wt.id,
        CommitSha(git(&dir, &["rev-parse", "HEAD"])),
        false,
    )
    .unwrap();
    assert!(settle(&state, &wt).await.is_empty());
    let doc = state
        .automatic_checks
        .store()
        .unwrap()
        .document(wt.id, &wt.branch, &wt.base_branch)
        .unwrap()
        .unwrap();
    assert_eq!(
        doc.recent[0].phase,
        crate::automatic_run_model::Phase::TimedOut
    );
    assert!(state.git_engine.lock_worktree_service().is_err());
    assert!(state.worktrees.read()[0].verified_commit.is_none());
}

#[tokio::test]
async fn recorded_result_reconciles_by_run_identity_without_repeating_execution() {
    use crate::automatic_run_model::Phase;
    let (tmp, state, _app, token, wt, dir) = fixture().await;
    policy(&tmp.path().join("repo"), true, "printf recorded-once");
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let first = crate::automatic_checks::start(&state).await.unwrap();
    crate::automatic_checks::after_commit(
        &state,
        &caller,
        wt.id,
        CommitSha(git(&dir, &["rev-parse", "HEAD"])),
        false,
    )
    .unwrap();
    let history = settle(&state, &wt).await;
    assert_eq!(history.len(), 1);
    let run_id = history[0].result.run_id;
    let store = state.automatic_checks.store().unwrap();
    store
        .update(wt.id, &wt.branch, &wt.base_branch, |doc| {
            let mut job = doc.recent.remove(0);
            job.transition(Phase::Running);
            doc.running = Some(job);
            Ok(())
        })
        .unwrap();
    first.shutdown().await.unwrap();
    let second = crate::automatic_checks::start(&state).await.unwrap();
    assert_eq!(settle(&state, &wt).await.len(), 1);
    let doc = state
        .automatic_checks
        .store()
        .unwrap()
        .document(wt.id, &wt.branch, &wt.base_branch)
        .unwrap()
        .unwrap();
    assert_eq!(doc.recent[0].run_id, run_id);
    assert_eq!(doc.recent[0].phase, Phase::Completed);
    assert!(doc.running.is_none());
    second.shutdown().await.unwrap();
    state
        .automatic_checks
        .store()
        .unwrap()
        .update(wt.id, &wt.branch, &wt.base_branch, |doc| {
            let mut job = doc.recent.remove(0);
            job.transition(Phase::Running);
            job.checks_oid = "d".repeat(40);
            doc.running = Some(job);
            Ok(())
        })
        .unwrap();
    assert!(
        crate::automatic_checks::start(&state).await.is_err(),
        "matching RunId with a different policy must not masquerade as this job's result"
    );
    assert!(state.worktrees.read()[0].verified_commit.is_none());
}

#[tokio::test]
async fn automatic_execution_cannot_read_or_write_its_private_scheduling_metadata() {
    let (tmp, state, _app, token, wt, dir) = fixture().await;
    let common = state.git_engine.repository_common_directory().unwrap();
    let metadata = common
        .join(crate::automatic_run_store::DIRECTORY)
        .join("store.json");
    policy(&tmp.path().join("repo"), true, &format!("if cat '{}' >/dev/null 2>&1; then exit 11; fi; if printf unsafe > '{}' 2>/dev/null; then exit 12; fi; printf scheduling-private", metadata.display(), metadata.display()));
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let _service = crate::automatic_checks::start(&state).await.unwrap();
    let before = std::fs::read(&metadata).unwrap();
    crate::automatic_checks::after_commit(
        &state,
        &caller,
        wt.id,
        CommitSha(git(&dir, &["rev-parse", "HEAD"])),
        false,
    )
    .unwrap();
    let records = settle(&state, &wt).await;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].result.exit_code, 0);
    assert_eq!(std::fs::read(metadata).unwrap(), before);
}

#[tokio::test]
async fn recovery_refuses_to_consume_a_prepared_intent_owned_by_an_inflight_mutation() {
    let (tmp, state, _app, token, wt, dir) = fixture().await;
    policy(&tmp.path().join("repo"), true, "printf later");
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let first = crate::automatic_checks::start(&state).await.unwrap();
    let guard = state
        .get_worktree_runtime(wt.id)
        .unwrap()
        .exec_lock
        .clone()
        .lock_owned()
        .await;
    let intent = crate::automatic_checks::prepare(
        &state,
        &caller,
        wt.id,
        CommitSha("c".repeat(40)),
        Some(CommitSha(git(&dir, &["rev-parse", "HEAD"]))),
        crate::automatic_run_model::Origin::Mcp,
    )
    .unwrap()
    .unwrap();
    first.shutdown().await.unwrap();
    assert!(crate::automatic_checks::start(&state).await.is_err());
    let doc = state
        .automatic_checks
        .store()
        .unwrap()
        .document(wt.id, &wt.branch, &wt.base_branch)
        .unwrap()
        .unwrap();
    assert_eq!(doc.prepared.unwrap().run_id, intent.run_id);
    crate::automatic_checks::reject(intent).unwrap();
    drop(guard);
    let service = crate::automatic_checks::start(&state).await.unwrap();
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn deleting_a_queued_target_releases_its_local_context_without_disabling_other_jobs() {
    let (tmp, state, app, token, wt, dir) = fixture().await;
    policy(&tmp.path().join("repo"), true, "printf remaining-job");
    let caller = state.auth_store.authenticate_session(&token).unwrap();
    let _service = crate::automatic_checks::start(&state).await.unwrap();
    let guard = state
        .get_worktree_runtime(wt.id)
        .unwrap()
        .exec_lock
        .clone()
        .lock_owned()
        .await;
    crate::automatic_checks::after_commit(
        &state,
        &caller,
        wt.id,
        CommitSha(git(&dir, &["rev-parse", "HEAD"])),
        true,
    )
    .unwrap();
    tokio::task::yield_now().await;
    state.automatic_checks.remove_if_initialized(wt.id).unwrap();
    state.remove_worktree_entry(wt.id);
    drop(guard);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !state.automatic_checks.workers_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(state.automatic_checks.live_local_actor_count(), 0);
    let (status, body) = super::call(&app, "POST", "/api/v1/swarm/worktrees".into(), &token, Some(serde_json::json!({"branch":"agent/test/still-operational", "prompt_summary":"another queued job"}))).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    let next: gitust_core::AgentWorktree = serde_json::from_str(&body).unwrap();
    let next_dir = state.get_worktree_runtime(next.id).unwrap().worktree_dir;
    crate::automatic_checks::after_commit(
        &state,
        &caller,
        next.id,
        CommitSha(git(&next_dir, &["rev-parse", "HEAD"])),
        false,
    )
    .unwrap();
    assert_eq!(settle(&state, &next).await.len(), 1);
}
