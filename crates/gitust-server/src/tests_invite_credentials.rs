use super::tests_repository_visibility::fixture;
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invitation_mutations_recheck_original_credentials_after_auth_database_lock_wait() {
    let mut failures = Vec::new();
    for use_pat in [false, true] {
        for operation in ["create", "reset", "revoke"] {
            let (temp, state, session, _) = fixture();
            let store = &state.auth_store;
            let admin = store.authenticate_session(&session).unwrap();
            let pat = store
                .create_personal_access_token(&admin, "invite", None)
                .unwrap();
            let credential = if use_pat {
                pat.raw_token.clone()
            } else {
                session.clone()
            };
            let existing = store
                .create_invite_link(
                    &admin,
                    "pending@company.internal",
                    EnterpriseRole::Developer,
                    vec![],
                    None,
                )
                .unwrap();
            let db = store.persistence_path().unwrap().to_owned();
            // A second legitimate auth writer supplies a fully persisted revoked checkpoint.
            // No token/hash is fabricated or printed, and the original fixture is self-owned.
            let copy = temp.path().join("revoked-auth.json");
            std::fs::copy(&db, &copy).unwrap();
            let revoked = AuthStore::open_persistent(&copy).unwrap();
            if use_pat {
                revoked
                    .revoke_personal_access_token(&admin, pat.metadata.id)
                    .unwrap();
            } else {
                revoked.revoke_session(&admin, &session).unwrap();
            }
            let bytes = std::fs::read(&copy).unwrap();
            let completed = Arc::new(AtomicBool::new(false));
            let held = Arc::new(AtomicBool::new(false));
            let worker = Arc::new(Mutex::new(None));
            let (done, locked, join) = (completed.clone(), held.clone(), worker.clone());
            let hook_db = db.clone();
            *state.repository_read_hook.lock() = Some(Arc::new(move || {
                let db = &hook_db;
                let mut name = db.as_os_str().to_owned();
                name.push(".lock");
                let lock = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(std::path::PathBuf::from(name))
                    .unwrap();
                lock.lock().unwrap();
                let (tx, rx) = std::sync::mpsc::sync_channel(0);
                let db = db.clone();
                let bytes = bytes.clone();
                let done = done.clone();
                let held = locked.clone();
                *join.lock().unwrap() = Some(std::thread::spawn(move || {
                    tx.send(()).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(80));
                    assert!(
                        !done.load(Ordering::SeqCst),
                        "request must remain pending while auth lock is held"
                    );
                    let replacement = db.with_extension("revoked");
                    std::fs::write(&replacement, &bytes).unwrap();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(
                            &replacement,
                            std::fs::Permissions::from_mode(0o600),
                        )
                        .unwrap();
                    }
                    std::fs::File::open(&replacement)
                        .unwrap()
                        .sync_all()
                        .unwrap();
                    std::fs::rename(replacement, &db).unwrap();
                    std::fs::File::open(db.parent().unwrap())
                        .unwrap()
                        .sync_all()
                        .unwrap();
                    held.store(true, Ordering::SeqCst);
                    lock.unlock().unwrap();
                }));
                rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
            }));
            let (method, uri, body) = match operation {
                "create" => (
                    "POST",
                    "/api/v1/admin/invites".to_owned(),
                    Some(
                        serde_json::json!({"target_email":"new@company.internal","assigned_role":"Developer","team_ids":[],"ttl_hours":null}),
                    ),
                ),
                "reset" => (
                    "POST",
                    format!("/api/v1/admin/users/{}/password-reset", admin.user_id.0),
                    None,
                ),
                _ => (
                    "DELETE",
                    format!("/api/v1/admin/invites/{}", existing.invite.id.0),
                    None,
                ),
            };
            let result = call(&build_router(state.clone()), method, uri, &credential, body).await;
            completed.store(true, Ordering::SeqCst);
            worker.lock().unwrap().take().unwrap().join().unwrap();
            assert!(held.load(Ordering::SeqCst));
            let reopened = AuthStore::open_persistent(&db).unwrap();
            let view = reopened.list_invites(&admin).unwrap();
            if result.0 != StatusCode::UNAUTHORIZED || view != [existing.invite] {
                failures.push((operation, use_pat, result.0));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "revoked credential mutation results: {failures:?}"
    );
}
