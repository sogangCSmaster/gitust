use super::tests_repository_visibility::fixture;
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn credential_mutations_recheck_original_credentials_under_the_auth_write_lock() {
    let mut failures = Vec::new();
    for use_pat in [false, true] {
        let (temp, state, bootstrap, repository) = fixture();
        let store = &state.auth_store;
        let admin = store.authenticate_session(&bootstrap).unwrap();
        let invitation = store
            .create_invite_link(
                &admin,
                "target@company.internal",
                EnterpriseRole::Developer,
                vec![],
                None,
            )
            .unwrap();
        let target = store
            .redeem_invite_link(&invitation.raw_token, "target", "Password!2026")
            .unwrap();
        let target_pat = store
            .create_personal_access_token(&admin, "target", None)
            .unwrap();
        let key = store
            .register_ssh_public_key(
                &admin,
                "target",
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl",
            )
            .unwrap();
        for operation in [
            "pat_create",
            "pat_revoke",
            "ssh_register",
            "ssh_revoke",
            "repo_create",
            "rename",
            "deactivate",
        ] {
            let session = store
                .login("admin", "Password!2026")
                .unwrap()
                .raw_session_token;
            let pat = store
                .create_personal_access_token(&admin, "origin", None)
                .unwrap();
            let credential = if use_pat {
                pat.raw_token.clone()
            } else {
                session.clone()
            };
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
            let expected_bytes = bytes.clone();
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
                "pat_create" => (
                    "POST",
                    "/api/v1/auth/pats".into(),
                    Some(serde_json::json!({"name":"unexpected","ttl_days":null})),
                ),
                "pat_revoke" => (
                    "DELETE",
                    format!("/api/v1/auth/pats/{}", target_pat.metadata.id.0),
                    None,
                ),
                "ssh_register" => (
                    "POST",
                    "/api/v1/auth/ssh-keys".into(),
                    Some(
                        serde_json::json!({"title":"unexpected","public_key_openssh":
                        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAAAQE="}),
                    ),
                ),
                "ssh_revoke" => (
                    "DELETE",
                    format!("/api/v1/auth/ssh-keys/{}", key.id.0),
                    None,
                ),
                "repo_create" => (
                    "POST",
                    "/api/v1/repos".into(),
                    Some(
                        serde_json::json!({"name":"unexpected","owner_team_id":TeamId::new(),"is_private":true}),
                    ),
                ),
                "rename" => (
                    "PATCH",
                    format!("/api/v1/repos/{}", repository.0),
                    Some(serde_json::json!({"name":"unexpected"})),
                ),
                _ => (
                    "POST",
                    format!("/api/v1/admin/users/{}/deactivate", target.id.0),
                    None,
                ),
            };
            let result = call(&build_router(state.clone()), method, uri, &credential, body).await;
            completed.store(true, Ordering::SeqCst);
            worker.lock().unwrap().take().unwrap().join().unwrap();
            assert!(held.load(Ordering::SeqCst));
            if result.0 != StatusCode::UNAUTHORIZED || std::fs::read(&db).unwrap() != expected_bytes
            {
                failures.push((operation, use_pat, result.0));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "revoked credential mutation results: {failures:?}"
    );
}
