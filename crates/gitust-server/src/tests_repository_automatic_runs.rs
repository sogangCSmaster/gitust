use super::tests_verification::fixture;
use super::*;
use crate::automatic_run_model::{Job, Origin, Phase};
use crate::automatic_run_store::Store;
use chrono::{Duration, Utc};
use gitust_core::CommitSha;
use serde_json::{json, Value};

const ROUTE: &str = "/api/v1/repo/automatic-runs";

async fn page(app: &Router, token: &str, query: &str) -> (StatusCode, Value, Option<String>) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{ROUTE}{query}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let cache = response
        .headers()
        .get(header::CACHE_CONTROL)
        .map(|v| v.to_str().unwrap().to_owned());
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into_owned()));
    (status, value, cache)
}

fn targets(state: &AppState, template: &AgentWorktree, count: u128) -> Vec<AgentWorktree> {
    let rows: Vec<_> = (1..=count)
        .map(|i| {
            let mut wt = template.clone();
            wt.id = WorktreeId(uuid::Uuid::from_u128(i));
            wt.branch = format!("agent/test/diagnostic-{i}");
            wt
        })
        .collect();
    *state.worktrees.write() = rows.clone().into();
    rows
}

fn save(store: &Store, wt: &AgentWorktree, user: UserId) {
    let now = Utc::now();
    store
        .update(wt.id, &wt.branch, &wt.base_branch, |doc| {
            for offset in [0, 1] {
                let mut job = Job::new(
                    user,
                    Origin::Mcp,
                    CommitSha("1".repeat(40)),
                    None,
                    "2".repeat(40),
                );
                job.created_at = now - Duration::seconds(offset);
                job.changed_at = job.created_at;
                job.transition(Phase::Completed);
                doc.recent.push(job);
            }
            doc.recent.reverse();
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn repository_pages_are_bounded_grouped_and_continue_by_worktree_not_global_time() {
    let (_tmp, state, app, token, template, _) = fixture().await;
    let service = crate::automatic_checks::start(&state).await.unwrap();
    let store = state.automatic_checks.store().unwrap();
    let user = state
        .auth_store
        .authenticate_session(&token)
        .unwrap()
        .user_id;
    let rows = targets(&state, &template, 23);
    for wt in &rows {
        save(&store, wt, user);
    }
    let mut foreign = template.clone();
    foreign.id = WorktreeId(uuid::Uuid::from_u128(100));
    foreign.repo_id = RepoId::new();
    state.worktrees.write().push_back(foreign);
    store
        .document_reads
        .store(0, std::sync::atomic::Ordering::Relaxed);
    let (status, first, cache) = page(&app, &token, "").await;
    assert_eq!(
        store
            .document_reads
            .load(std::sync::atomic::Ordering::Relaxed),
        10
    );
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(cache.as_deref(), Some("no-store"));
    assert_eq!(first["repo_id"], json!(state.repo_id));
    assert_eq!(first["order"], "worktree_id_asc");
    assert_eq!(first["scanned_worktrees"], 10);
    assert_eq!(first["worktrees"].as_array().unwrap().len(), 10);
    let mut found = Vec::new();
    let mut current = first;
    loop {
        for group in current["worktrees"].as_array().unwrap() {
            found.push(group["worktree_id"].as_str().unwrap().to_owned());
            let runs = group["runs"].as_array().unwrap();
            assert_eq!(runs.len(), 2);
            assert!(
                runs[0]["created_at"].as_str().unwrap() > runs[1]["created_at"].as_str().unwrap()
            );
        }
        let Some(cursor) = current["next_cursor"].as_str() else {
            break;
        };
        let (status, next, _) = page(&app, &token, &format!("?cursor={cursor}")).await;
        assert_eq!(status, StatusCode::OK, "{next}");
        current = next;
    }
    assert_eq!(
        found,
        rows.iter().map(|w| w.id.0.to_string()).collect::<Vec<_>>()
    );
    assert_eq!(current["scanned_worktrees"], 3);
    store
        .document_reads
        .store(0, std::sync::atomic::Ordering::Relaxed);
    let (status, max, _) = page(&app, &token, "?limit=20").await;
    assert_eq!(
        store
            .document_reads
            .load(std::sync::atomic::Ordering::Relaxed),
        20
    );
    assert_eq!(status, StatusCode::OK, "{max}");
    assert_eq!(max["scanned_worktrees"], 20);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn repository_pages_validate_cursor_and_role_even_when_no_runs_exist() {
    let (_tmp, state, app, token, _wt, _) = fixture().await;
    let service = crate::automatic_checks::start(&state).await.unwrap();
    let principal = state.auth_store.authenticate_session(&token).unwrap();
    let pat = state
        .auth_store
        .create_personal_access_token(&principal, "diagnostics", None)
        .unwrap();
    let (status, body, _) = page(&app, &pat.raw_token, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["worktrees"][0]["runs"].as_array().unwrap().is_empty());
    state.worktrees.write()[0].base_branch.clear();
    let (status, legacy, _) = page(&app, &token, "").await;
    assert_eq!(status, StatusCode::OK, "{legacy}");
    assert_eq!(legacy["worktrees"][0]["base_branch"], "");
    assert!(legacy["worktrees"][0]["runs"]
        .as_array()
        .unwrap()
        .is_empty());
    let mcp = state
        .auth_store
        .issue_oauth_session(principal.user_id)
        .unwrap();
    assert_eq!(
        page(&app, &mcp.raw_session_token, "").await.0,
        StatusCode::UNAUTHORIZED
    );
    let invite = state
        .auth_store
        .create_invite_link(
            &principal,
            "reader@company.internal",
            EnterpriseRole::Developer,
            vec![],
            None,
        )
        .unwrap();
    state
        .auth_store
        .redeem_invite_link(&invite.raw_token, "reader", "Password!2026")
        .unwrap();
    let dev = state.auth_store.login("reader", "Password!2026").unwrap();
    assert_eq!(
        page(&app, &dev.raw_session_token, "").await.0,
        StatusCode::FORBIDDEN
    );
    for query in [
        "?limit=0".into(),
        "?limit=21".into(),
        "?phase=running".into(),
        "?cursor=bad".into(),
        format!("?cursor=v2.{}.{}", state.repo_id.0, uuid::Uuid::new_v4()),
        format!(
            "?cursor=v1.{}.{}",
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4()
        ),
        format!("?cursor=v1.{}.{}", state.repo_id.0, uuid::Uuid::nil()),
        format!("?cursor={}", "x".repeat(1000)),
    ] {
        assert_eq!(
            page(&app, &token, &query).await.0,
            StatusCode::BAD_REQUEST,
            "{query}"
        );
    }
    state.worktrees.write().clear();
    assert_eq!(page(&app, &token, "").await.1["scanned_worktrees"], 0);
    let metadata = state
        .auth_store
        .create_team_repository(&principal, "collision", gitust_core::TeamId::new(), true)
        .unwrap();
    let mut collision = state.clone();
    collision.repo_id = metadata.id;
    assert_eq!(
        page(&build_router(collision), &token, "").await.0,
        StatusCode::FORBIDDEN
    );
    service.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repository_pages_recheck_original_session_and_pat_after_reading_including_empty_pages() {
    use crate::automatic_run_page::{TestHook, TestStage};
    for use_pat in [false, true] {
        let (_tmp, state, app, token, wt, _) = fixture().await;
        let service = crate::automatic_checks::start(&state).await.unwrap();
        let principal = state.auth_store.authenticate_session(&token).unwrap();
        let pat = use_pat.then(|| {
            state
                .auth_store
                .create_personal_access_token(&principal, "page-revocation", None)
                .unwrap()
        });
        let credential = pat
            .as_ref()
            .map_or_else(|| token.clone(), |p| p.raw_token.clone());
        if use_pat {
            save(
                &state.automatic_checks.store().unwrap(),
                &wt,
                principal.user_id,
            );
        } else {
            state.worktrees.write().clear();
        }
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, waiting) = std::sync::mpsc::channel();
        *state.automatic_run_page_hook.lock() = Some(TestHook {
            stage: TestStage::Read,
            entered,
            release: waiting,
        });
        let request = tokio::spawn(async move { page(&app, &credential, "").await });
        tokio::time::timeout(std::time::Duration::from_secs(5), ready)
            .await
            .unwrap()
            .unwrap();
        match pat {
            Some(pat) => {
                state
                    .auth_store
                    .revoke_personal_access_token(&principal, pat.metadata.id)
                    .unwrap();
            }
            None => state.auth_store.revoke_session(&principal, &token).unwrap(),
        }
        assert_eq!(
            state
                .auth_store
                .verify_active_principal(&principal)
                .unwrap()
                .user_id,
            principal.user_id
        );
        release.send(()).unwrap();
        let (status, body, _) = tokio::time::timeout(std::time::Duration::from_secs(5), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert!(body.get("worktrees").is_none());
        service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn repository_pages_read_only_selected_documents_and_fail_closed_on_storage_errors() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, state, app, token, template, _) = fixture().await;
    let service = crate::automatic_checks::start(&state).await.unwrap();
    let store = state.automatic_checks.store().unwrap();
    let user = state
        .auth_store
        .authenticate_session(&token)
        .unwrap()
        .user_id;
    let rows = targets(&state, &template, 3);
    for wt in &rows {
        save(&store, wt, user);
    }
    let path = store.path().join(format!("{}.json", rows[2].id.0));
    let original = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"{").unwrap();
    store
        .document_reads
        .store(0, std::sync::atomic::Ordering::Relaxed);
    let (status, body, _) = page(&app, &token, "?limit=2").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        store
            .document_reads
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"{");
    let query = format!("?cursor={}", body["next_cursor"].as_str().unwrap());
    assert_eq!(
        page(&app, &token, &query).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        page(&app, &token, &query).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    std::fs::write(&path, &original).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        page(&app, &token, &query).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut wrong: Value = serde_json::from_slice(&original).unwrap();
    wrong["worktree_id"] = json!(WorktreeId::new());
    std::fs::write(&path, serde_json::to_vec(&wrong).unwrap()).unwrap();
    assert_eq!(
        page(&app, &token, &query).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    wrong = serde_json::from_slice(&original).unwrap();
    wrong["branch"] = json!("agent/test/different-target");
    std::fs::write(&path, serde_json::to_vec(&wrong).unwrap()).unwrap();
    assert_eq!(
        page(&app, &token, &query).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    std::fs::write(&path, &original).unwrap();
    assert_eq!(page(&app, &token, &query).await.0, StatusCode::OK);
    assert_eq!(std::fs::read(path).unwrap(), original);
    service.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repository_pages_omit_only_positive_retirement_and_keep_the_examined_cursor() {
    let (_tmp, state, app, token, template, _) = fixture().await;
    let service = crate::automatic_checks::start(&state).await.unwrap();
    let store = state.automatic_checks.store().unwrap();
    let user = state
        .auth_store
        .authenticate_session(&token)
        .unwrap()
        .user_id;
    let rows = targets(&state, &template, 3);
    for wt in &rows {
        save(&store, wt, user);
    }
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, waiting) = std::sync::mpsc::channel();
    *store.diagnostic_read_hook.lock() = Some((entered, waiting));
    store
        .document_reads
        .store(0, std::sync::atomic::Ordering::Relaxed);
    let request_app = app.clone();
    let request_token = token.clone();
    let request = tokio::spawn(async move { page(&request_app, &request_token, "?limit=1").await });
    tokio::time::timeout(std::time::Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    let discard_store = store.clone();
    let discard_state = state.clone();
    let discarded = rows[0].id;
    let removal = tokio::task::spawn_blocking(move || {
        discard_store.remove(discarded).unwrap();
        discard_state.remove_worktree_entry(discarded);
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), removal)
        .await
        .unwrap()
        .unwrap();
    release.send(()).unwrap();
    let (status, body, _) = tokio::time::timeout(std::time::Duration::from_secs(5), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["scanned_worktrees"], 1);
    assert!(body["worktrees"].as_array().unwrap().is_empty());
    assert_eq!(
        store
            .document_reads
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    let cursor = body["next_cursor"].as_str().unwrap();
    assert_eq!(cursor, format!("v1.{}.{}", state.repo_id.0, rows[0].id.0));
    let (status, next, _) = page(&app, &token, &format!("?limit=1&cursor={cursor}")).await;
    assert_eq!(status, StatusCode::OK, "{next}");
    assert_eq!(next["worktrees"][0]["worktree_id"], json!(rows[1].id));
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn repository_pages_preserve_long_refs_and_bound_only_selected_metadata_bytes() {
    let (_tmp, state, app, token, template, _) = fixture().await;
    let service = crate::automatic_checks::start(&state).await.unwrap();
    let mut rows = targets(&state, &template, 2);
    let branch = format!(
        "agent/{}",
        (0..24)
            .map(|i| format!("segment{i:03}{}", "x".repeat(40)))
            .collect::<Vec<_>>()
            .join("/")
    );
    let oversized_base = format!("base/{}tip", "長/".repeat(8000));
    for name in [&branch, &oversized_base] {
        let checked = std::process::Command::new("git")
            .args(["check-ref-format", "--branch", name])
            .output()
            .unwrap();
        assert!(checked.status.success());
    }
    assert!(branch.len() > 1024);
    assert!(branch.len() + oversized_base.chars().count() < 32 * 1024);
    assert!(branch.len() + oversized_base.len() > 32 * 1024);
    rows[0].branch = branch.clone();
    rows[1].branch = branch.clone();
    rows[1].base_branch = oversized_base;
    *state.worktrees.write() = rows.clone().into();
    let (status, first, _) = page(&app, &token, "?limit=1").await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["worktrees"][0]["branch"], branch);
    assert!(first["worktrees"][0]["runs"].as_array().unwrap().is_empty());
    let cursor = first["next_cursor"].as_str().unwrap();
    assert_eq!(
        page(&app, &token, &format!("?limit=1&cursor={cursor}"))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        page(&app, &token, "?limit=2").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    service.shutdown().await.unwrap();
}
