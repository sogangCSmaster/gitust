use super::tests_repository_visibility::fixture;
use super::*;
use serde_json::{json, Value};

async fn read(app: &Router, token: &str, id: RepoId) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/repos/{}/local-check-automatic-runs", id.0))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn local_auto_read_only_diagnostics_keep_primary_admin_scope_without_creating_storage() {
    let (_temp, state, token, secondary) = fixture();
    let app = build_router(state.clone());
    let common = state.git_engine.repository_common_directory().unwrap();
    let (status, value) = read(&app, &token, state.repo_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        value,
        json!({"version":1,"repo_id":state.repo_id,"runs":[]})
    );
    assert!(!common.join("gitust-local-check-automatic-runs").exists());
    assert_eq!(
        read(&app, &token, secondary).await,
        (
            StatusCode::NOT_FOUND,
            json!({"code":"local_checks_automatic_not_hosted"})
        )
    );
    assert_eq!(
        read(&app, "invalid-session", state.repo_id).await,
        (
            StatusCode::UNAUTHORIZED,
            json!({"code":"local_checks_automatic_unauthorized"})
        )
    );
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let member =
        super::tests_repository_visibility::user(&state, &admin, "auto-reader", TeamId::new());
    assert_eq!(
        read(&app, &member, state.repo_id).await,
        (
            StatusCode::FORBIDDEN,
            json!({"code":"local_checks_automatic_forbidden"})
        )
    );
}

use crate::{
    local_checks_automatic as automatic,
    local_checks_automatic_model::{Document, History, Job, Phase, Reason, DIRECTORY},
    local_checks_automatic_store::Store,
};
use gitust_core::CommitSha;

fn sample_job() -> Job {
    let mut job = Job::new(
        UserId::new(),
        "refs/heads/main".into(),
        CommitSha("a".repeat(40)),
        CommitSha("b".repeat(40)),
        "c".repeat(40),
    );
    job.checks_blob_oid = Some("d".repeat(40));
    job.transition(Phase::Queued, None, History::NotAttempted);
    job
}
#[test]
fn local_auto_store_preserves_latest_queue_unique_ids_retention_and_malformed_errors() {
    let dir = tempfile::tempdir().unwrap();
    let id = RepoId::new();
    let store = Store::open(dir.path(), id).unwrap();
    assert!(store.read().unwrap().public().is_empty());
    assert!(!dir.path().join(DIRECTORY).exists());
    let mut running = sample_job();
    running.transition(Phase::Running, None, History::NotAttempted);
    let first = sample_job();
    let latest = sample_job();
    let first_id = first.run_id;
    store
        .update(|doc| {
            doc.running = Some(running.clone());
            doc.queue(first);
            doc.queue(latest.clone());
            Ok(())
        })
        .unwrap();
    let snapshot = store.read().unwrap();
    assert_eq!(snapshot.running.unwrap().run_id, running.run_id);
    assert_eq!(snapshot.queued.unwrap().run_id, latest.run_id);
    assert_eq!(snapshot.recent[0].run_id, first_id);
    assert_eq!(
        snapshot.recent[0].reason,
        Some(Reason::LatestQueuedReplaced)
    );
    for _ in 0..6 {
        store
            .update(|doc| {
                doc.finish(sample_job(), Phase::Completed, None, History::Saved);
                Ok(())
            })
            .unwrap();
    }
    let reopened = Store::open(dir.path(), id).unwrap();
    let snapshot = reopened.read().unwrap();
    assert_eq!(snapshot.recent.len(), 5);
    assert_eq!(snapshot.public().len(), 7);
    assert!(snapshot.valid(id));
    assert!(!snapshot.valid(RepoId::new()));
    assert!(Store::open(dir.path(), RepoId::new())
        .unwrap()
        .read()
        .is_err());
    let bytes = std::fs::read(dir.path().join(DIRECTORY).join("state.json")).unwrap();
    let duplicate: Document = serde_json::from_slice(&bytes).unwrap();
    assert!(reopened
        .update(|doc| {
            doc.recent[0] = duplicate.queued.clone().unwrap();
            Ok(())
        })
        .is_err());
    assert_eq!(
        std::fs::read(dir.path().join(DIRECTORY).join("state.json")).unwrap(),
        bytes
    );
    std::fs::write(
        dir.path().join(DIRECTORY).join("state.json"),
        "malformed ordinary storage",
    )
    .unwrap();
    assert!(reopened.read().is_err());
}
#[test]
fn local_auto_startup_never_promotes_prepared_or_replays_running_and_disabled_queue() {
    let (_temp, state, _token, _) = fixture();
    let state = state.with_trusted_sandbox_exec(true);
    let _owner = state.git_engine.lock_worktree_service().unwrap();
    let common = state.git_engine.repository_common_directory().unwrap();
    let store = Store::open(&common, state.repo_id).unwrap();
    let prepared = Job::new(
        UserId::new(),
        "refs/heads/main".into(),
        CommitSha("a".repeat(40)),
        CommitSha("e".repeat(40)),
        "c".repeat(40),
    );
    let mut running = sample_job();
    running.transition(Phase::Running, None, History::NotAttempted);
    let queued = sample_job();
    let queued_id = queued.run_id;
    store
        .update(|doc| {
            doc.prepared = Some(prepared);
            doc.running = Some(running);
            doc.queue(queued);
            Ok(())
        })
        .unwrap();
    crate::local_checks_automatic_worker::recover(&state, &store).unwrap();
    let doc = store.read().unwrap();
    assert!(doc.prepared.is_none() && doc.running.is_none());
    assert_eq!(doc.queued.unwrap().run_id, queued_id);
    assert!(doc
        .recent
        .iter()
        .all(|j| j.phase == Phase::Interrupted && j.history == History::NotAttempted));
    assert!(doc
        .recent
        .iter()
        .any(|j| j.reason == Some(Reason::AcceptanceUnconfirmed)));
    assert!(doc
        .recent
        .iter()
        .any(|j| j.reason == Some(Reason::UnconfirmedExecution)));
    crate::local_checks_automatic_worker::recover(
        &state.clone().with_trusted_sandbox_exec(false),
        &store,
    )
    .unwrap();
    let doc = store.read().unwrap();
    assert!(doc.queued.is_none());
    assert_eq!(doc.recent[0].reason, Some(Reason::ExecutionDisabled));
    assert!(!common.join("gitust-local-check-runs").exists());
    assert!(state.worktrees.read().is_empty());
}
fn git(state: &AppState, args: &[&str]) -> String {
    super::tests_automatic_checks::git(state.git_engine.repo_path(), args)
}
fn stage_policy(state: &AppState, enabled: bool, commands: &[&str]) -> (String, String) {
    let old = git(state, &["rev-parse", "HEAD"]);
    git(state, &["checkout", "-B", "ordinary-candidate"]);
    std::fs::write(
        state.git_engine.repo_path().join(".gitust-checks.json"),
        json!({"version":1,"auto_run":enabled,"commands":commands}).to_string(),
    )
    .unwrap();
    std::fs::write(
        state.git_engine.repo_path().join("feature-only.txt"),
        "selected snapshot\n",
    )
    .unwrap();
    git(state, &["add", ".gitust-checks.json", "feature-only.txt"]);
    git(state, &["commit", "-qm", "normal policy update"]);
    let new = git(state, &["rev-parse", "HEAD"]);
    git(state, &["checkout", "main"]);
    (old, new)
}
fn update(old: String, new: String) -> gitust_git::ReceivePackRefUpdate {
    gitust_git::ReceivePackRefUpdate {
        old_oid: old,
        new_oid: new,
        refname: "refs/heads/main".into(),
        is_agent_branch: false,
        is_deletion: false,
    }
}
async fn settle(state: &AppState, run_id: gitust_core::SandboxRunId) -> Job {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let doc = state
                .local_checks_automatic
                .store(state)
                .unwrap()
                .read()
                .unwrap();
            if let Some(job) = doc.recent.into_iter().find(|j| j.run_id == run_id) {
                return job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("normal automatic attempt completed")
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_auto_observed_normal_updates_run_exact_accepted_policy_and_save_stable_outcomes() {
    let (_temp, state, token, _) = fixture();
    let state = state.with_trusted_sandbox_exec(true);
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let service = automatic::start(&state).await.unwrap();
    let (old, new) = stage_policy(&state, true, &["printf activation"]);
    assert!(matches!(
        automatic::prepare(&state, &admin, &[update(old, new)]).unwrap(),
        automatic::Eligibility::Ineligible
    ));
    git(&state, &["merge", "--ff-only", "ordinary-candidate"]);
    let app = build_router(state.clone());
    let mut ids = Vec::new();
    for failed in [false, true] {
        let commands = if failed {
            vec!["printf expected-failure; exit 7", "printf must-not-run"]
        } else {
            vec![
                "test -f feature-only.txt && printf actual-selected",
                "printf second",
            ]
        };
        let (old, new) = stage_policy(&state, true, &commands);
        let automatic::Eligibility::Prepared(intent) =
            automatic::prepare(&state, &admin, &[update(old, new.clone())]).unwrap()
        else {
            panic!("old policy opted in")
        };
        let id = intent.run_id;
        ids.push(id);
        git(&state, &["merge", "--ff-only", "ordinary-candidate"]);
        let refs = git(&state, &["show-ref", "--head"]);
        let head = git(&state, &["symbolic-ref", "HEAD"]);
        automatic::confirm(&state, intent).unwrap();
        let job = settle(&state, id).await;
        assert_eq!(
            job.phase,
            if failed {
                Phase::Failed
            } else {
                Phase::Completed
            }
        );
        assert_eq!(job.history, History::Saved);
        assert_eq!(job.expected_commit.0, new);
        let (status, wire) = read(&app, &token, state.repo_id).await;
        assert_eq!(status, StatusCode::OK);
        let row = &wire["runs"][0];
        assert_eq!(row.as_object().unwrap().len(), 9);
        assert_eq!(row["run_id"], id.0.to_string());
        assert_eq!(row["expected_commit"], new);
        let common = state.git_engine.repository_common_directory().unwrap();
        let history =
            crate::local_checks_history::LocalChecksHistoryStore::open(&common, state.repo_id)
                .unwrap();
        let record = history.get(id).unwrap().unwrap();
        assert_eq!(record.result.run_id, id);
        assert_eq!(
            record.checks_blob_oid,
            git(&state, &["rev-parse", "HEAD:.gitust-checks.json"])
        );
        assert_eq!(record.result.exit_code, if failed { 7 } else { 0 });
        assert!(record.result.steps.iter().all(|step| step.required));
        assert_eq!(
            record.result.steps[1].exit_code,
            if failed { None } else { Some(0) }
        );
        assert_eq!(git(&state, &["show-ref", "--head"]), refs);
        assert_eq!(git(&state, &["symbolic-ref", "HEAD"]), head);
        assert!(state.worktrees.read().is_empty());
        assert!(state.worktree_runtime.read().is_empty());
    }
    let (old, new) = stage_policy(&state, false, &["printf disabled"]);
    let automatic::Eligibility::Prepared(intent) =
        automatic::prepare(&state, &admin, &[update(old, new)]).unwrap()
    else {
        panic!("previous accepted policy opted in")
    };
    let id = intent.run_id;
    git(&state, &["merge", "--ff-only", "ordinary-candidate"]);
    automatic::confirm(&state, intent).unwrap();
    assert_eq!(
        settle(&state, id).await.reason,
        Some(Reason::PolicyDisabled)
    );
    let common = state.git_engine.repository_common_directory().unwrap();
    let history =
        crate::local_checks_history::LocalChecksHistoryStore::open(&common, state.repo_id).unwrap();
    assert_eq!(history.list().unwrap().len(), 2);
    assert_ne!(ids[0], ids[1]);
    service.shutdown().await.unwrap();
    // Stopped startup metadata can be reconciled from an already confirmed exact result.
    let store = state.local_checks_automatic.store(&state).unwrap();
    let snapshot = history.list().unwrap();
    store
        .update(|doc| {
            let index = doc.recent.iter().position(|j| j.run_id == ids[1]).unwrap();
            let mut job = doc.recent.remove(index);
            job.transition(Phase::Running, None, History::NotAttempted);
            doc.running = Some(job);
            Ok(())
        })
        .unwrap();
    crate::local_checks_automatic_worker::recover(&state, &store).unwrap();
    assert_eq!(
        serde_json::to_value(history.list().unwrap()).unwrap(),
        serde_json::to_value(snapshot).unwrap()
    );
    git(&state, &["fsck", "--full", "--strict"]);
}

#[tokio::test]
async fn local_auto_stale_preparation_cleans_checkout_before_reporting_target_changed() {
    use crate::ephemeral_sandbox::{run_prepared_sandbox, RunCommands, RunContext};
    let (temp, state, _token, _) = fixture();
    let old = git(&state, &["rev-parse", "HEAD"]);
    std::fs::write(
        state.git_engine.repo_path().join("README.md"),
        "a newer ordinary commit\n",
    )
    .unwrap();
    git(&state, &["commit", "-qam", "normal newer default commit"]);
    let refs = git(&state, &["show-ref", "--head"]);
    let registrations = git(&state, &["worktree", "list", "--porcelain"]);
    let base = temp.path().join("prepared-checkouts");
    let engine = state.git_engine.clone().with_worktree_base_dir(&base);
    let owner = engine.lock_worktree_service().unwrap();
    let outcome = run_prepared_sandbox(
        engine,
        owner,
        "main".into(),
        RunCommands::Checks {
            expected: CommitSha(old),
            commands: vec!["printf must-not-start".into()],
        },
        false,
        RunContext {
            repo_id: state.repo_id,
            executor: state.sandbox_executor.clone(),
            auth_store: Some(state.auth_store.clone()),
            admission: None,
            operation: None,
            run_id: None,
        },
        std::future::pending(),
    )
    .await;
    assert_eq!(
        outcome.err().map(|error| error.to_string()).as_deref(),
        Some("Checks source changed before execution")
    );
    assert_eq!(git(&state, &["show-ref", "--head"]), refs);
    assert_eq!(
        git(&state, &["worktree", "list", "--porcelain"]),
        registrations
    );
    assert!(!base.exists() || std::fs::read_dir(base).unwrap().count() == 0);
    let common = state.git_engine.repository_common_directory().unwrap();
    for leaf in ["gitust-active-executions", "gitust-sandbox-scratch"] {
        let path = common.join(leaf);
        assert!(!path.exists() || std::fs::read_dir(path).unwrap().count() == 0);
    }
}
