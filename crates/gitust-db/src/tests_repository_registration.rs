use crate::AuthStore;
use gitust_core::{EnterpriseRole, RepoAccess, RepoId, RepositoryRegistration, TeamId};
use serde_json::Value;

#[test]
fn snapshots_reject_duplicate_or_unrecognized_registration_records() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.json");
    let store = AuthStore::open_persistent(&path).unwrap();
    store
        .bootstrap_initial_admin_cli(
            "operator",
            "operator@company.internal",
            "OperatorPassword!2026",
        )
        .unwrap();
    let login = store.login("operator", "OperatorPassword!2026").unwrap();
    let caller = store
        .authenticate_session(&login.raw_session_token)
        .unwrap();
    let record = store
        .create_team_repository(&caller, "metadata", TeamId::new(), true)
        .unwrap();
    let original: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let mut duplicate = original.clone();
    duplicate["repositories"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::to_value(&record).unwrap());
    std::fs::write(&path, serde_json::to_vec(&duplicate).unwrap()).unwrap();
    assert!(
        AuthStore::open_persistent(&path).is_err(),
        "duplicate repository IDs must not overwrite authority"
    );
    for binding in [
        Value::Null,
        serde_json::json!({"version":99,"repo_id":record.id,"registration_nonce":uuid::Uuid::new_v4()}),
        serde_json::json!({"version":1,"repo_id":record.id,"registration_nonce":uuid::Uuid::nil()}),
        serde_json::json!({"version":1,"repo_id":RepoId::new(),"registration_nonce":uuid::Uuid::new_v4()}),
        serde_json::json!({"version":1,"repo_id":record.id,"registration_nonce":uuid::Uuid::new_v4(),"unknown":true}),
    ] {
        let mut unknown = original.clone();
        unknown["repositories"][0]["binding"] = binding;
        std::fs::write(&path, serde_json::to_vec(&unknown).unwrap()).unwrap();
        assert!(
            AuthStore::open_persistent(&path).is_err(),
            "invalid registration must not become metadata-only"
        );
    }
    assert_eq!(caller.role, EnterpriseRole::SiteAdmin);
}

#[test]
fn registration_pairs_retry_without_adoption_and_pinned_access_never_falls_back() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.json");
    let store = AuthStore::open_persistent(&path).unwrap();
    store
        .bootstrap_initial_admin_cli(
            "operator",
            "operator@company.internal",
            "OperatorPassword!2026",
        )
        .unwrap();
    let login = store.login("operator", "OperatorPassword!2026").unwrap();
    let admin = store
        .authenticate_session(&login.raw_session_token)
        .unwrap();
    let id = RepoId::new();
    let team = TeamId::new();
    let legacy = RepoAccess {
        repo_id: id,
        registration: None,
    };
    store
        .authorize_service_repository(&admin, legacy, id)
        .unwrap();
    let registered = store
        .register_local_repository(&admin, id, "actual", team, true, None)
        .unwrap();
    assert!(store.configured_repository_access(id, None).is_err());
    let wire: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(serde_json::from_value::<gitust_core::RepositoryRecord>(
        wire["repositories"][0].clone()
    )
    .is_err());
    let legacy_wire = serde_json::to_value(&registered.metadata).unwrap();
    assert!(serde_json::from_value::<gitust_core::RepositoryRecord>(legacy_wire).is_ok());
    let expected = store
        .configured_repository_access(id, Some(registered.binding))
        .unwrap();
    let retried = store
        .register_local_repository(&admin, id, "actual", team, true, None)
        .unwrap();
    assert!(registered.binding == retried.binding);
    assert_eq!(registered.metadata, retried.metadata);
    let before = std::fs::read(&path).unwrap();
    assert!(store
        .register_local_repository(&admin, id, "different", team, true, None)
        .is_err());
    assert!(store
        .register_local_repository(&admin, id, "actual", team, false, None)
        .is_err());
    assert!(store
        .register_local_repository(
            &admin,
            id,
            "actual",
            team,
            true,
            Some(RepositoryRegistration::new(id))
        )
        .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(store
        .authorize_service_repository(&admin, legacy, id)
        .is_err());
    store
        .authorize_service_repository(&admin, expected, id)
        .unwrap();
    for teams in [
        vec![TeamId(uuid::Uuid::nil())],
        vec![team, team],
        (0..33).map(|_| TeamId::new()).collect(),
    ] {
        assert!(store
            .create_invite_link(
                &admin,
                "invalid@company.internal",
                EnterpriseRole::Developer,
                teams,
                None
            )
            .is_err());
    }
    for (username, member_team, allowed) in
        [("member", team, true), ("outsider", TeamId::new(), false)]
    {
        let invite = store
            .create_invite_link(
                &admin,
                &format!("{username}@company.internal"),
                EnterpriseRole::Developer,
                vec![member_team],
                None,
            )
            .unwrap();
        store
            .redeem_invite_link(&invite.raw_token, username, "MemberPassword!2026")
            .unwrap();
        let login = store.login(username, "MemberPassword!2026").unwrap();
        let caller = store
            .authenticate_session(&login.raw_session_token)
            .unwrap();
        assert_eq!(
            store
                .authorize_service_repository(&caller, expected, id)
                .is_ok(),
            allowed
        );
        assert!(store
            .authorize_service_repository(&caller, legacy, id)
            .is_err());
        if allowed {
            store.deactivate_user(&admin, caller.user_id).unwrap();
            assert!(store
                .authorize_service_repository(&caller, expected, id)
                .is_err());
        }
    }
    let metadata = store
        .create_team_repository(&admin, "unbound", team, true)
        .unwrap();
    assert!(store
        .register_local_repository(&admin, metadata.id, "unbound", team, true, None)
        .is_err());
    assert!(store
        .configured_repository_access(RepoId::new(), Some(registered.binding))
        .is_err());
    let reopened = AuthStore::open_persistent(&path).unwrap();
    reopened
        .authorize_service_repository(&admin, expected, id)
        .unwrap();
    let removed = reopened.state.write().repositories.remove(&id).unwrap();
    assert!(reopened
        .authorize_service_repository(&admin, expected, id)
        .is_err());
    let mut changed = removed;
    changed.binding = Some(RepositoryRegistration::new(id));
    reopened.state.write().repositories.insert(id, changed);
    assert!(reopened
        .authorize_service_repository(&admin, expected, id)
        .is_err());
}

#[test]
fn registration_retry_cannot_skip_a_failed_directory_sync() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.json");
    let store = AuthStore::open_persistent(&path).unwrap();
    store
        .bootstrap_initial_admin_cli(
            "operator",
            "operator@company.internal",
            "OperatorPassword!2026",
        )
        .unwrap();
    let login = store.login("operator", "OperatorPassword!2026").unwrap();
    let admin = store
        .authenticate_session(&login.raw_session_token)
        .unwrap();
    let id = RepoId::new();
    let team = TeamId::new();
    struct ResetFault;
    impl Drop for ResetFault {
        fn drop(&mut self) {
            crate::persistence::FAIL_DIRECTORY_SYNC.with(|path| *path.borrow_mut() = None);
        }
    }
    let fault = ResetFault;
    crate::persistence::FAIL_DIRECTORY_SYNC.with(|fault| *fault.borrow_mut() = Some(path.clone()));
    assert!(store
        .register_local_repository(&admin, id, "actual", team, true, None)
        .is_err());
    let bytes: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let binding: RepositoryRegistration =
        serde_json::from_value(bytes["repositories"][0]["binding"].clone()).unwrap();
    assert!(store.configured_repository_access(id, None).is_err());
    assert!(store.register_local_repository(&admin, id, "actual", team, true, None).is_err(),
        "idempotent retry must reconfirm DB publication durability before a Git receipt can be published");
    drop(fault);
    let completed = store
        .register_local_repository(&admin, id, "actual", team, true, None)
        .unwrap();
    assert!(completed.binding == binding);
    AuthStore::open_persistent(&path)
        .unwrap()
        .configured_repository_access(id, Some(binding))
        .unwrap();
}
