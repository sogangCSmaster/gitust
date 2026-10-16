use crate::AuthStore;

#[test]
fn session_purpose_survives_reload_and_legacy_sessions_require_login() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("auth.json");
    let store = AuthStore::open_persistent(&path).unwrap();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Password!2026")
        .unwrap();
    let login = store.login("admin", "Password!2026").unwrap();
    let admin = store
        .authenticate_session(&login.raw_session_token)
        .unwrap();
    let oauth = store.issue_oauth_session(admin.user_id).unwrap();
    drop(store);
    let reloaded = AuthStore::open_persistent(&path).unwrap();
    assert!(reloaded
        .authenticate_session(&login.raw_session_token)
        .is_ok());
    assert!(reloaded
        .authenticate_mcp_session(&login.raw_session_token)
        .is_ok());
    assert!(reloaded
        .authenticate_session(&oauth.raw_session_token)
        .is_err());
    assert!(reloaded
        .authenticate_mcp_session(&oauth.raw_session_token)
        .is_ok());
    reloaded
        .revoke_session(&admin, &oauth.raw_session_token)
        .unwrap();
    assert!(reloaded
        .authenticate_mcp_session(&oauth.raw_session_token)
        .is_err());
    drop(reloaded);

    // Upgrade from the previous schema cannot identify whether old tokens came from OAuth.
    let mut snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for session in snapshot["sessions"].as_array_mut().unwrap() {
        session.as_object_mut().unwrap().remove("purpose");
    }
    std::fs::write(&path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
    let legacy = AuthStore::open_persistent(&path).unwrap();
    assert!(legacy
        .authenticate_session(&login.raw_session_token)
        .is_err());
    assert!(legacy
        .authenticate_mcp_session(&login.raw_session_token)
        .is_err());
    let new_login = legacy.login("admin", "Password!2026").unwrap();
    assert!(legacy
        .authenticate_session(&new_login.raw_session_token)
        .is_ok());
}
