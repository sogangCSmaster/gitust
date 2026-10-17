use crate::{AuthStore, GroupMemberError as Error};
use gitust_core::{AccountStatus, EnterpriseRole, RepoAccess, RepoId, TeamId, UserId};

fn fixture() -> (
    tempfile::TempDir,
    AuthStore,
    String,
    UserId,
    RepoAccess,
    RepoAccess,
) {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("auth")).unwrap();
    let store = AuthStore::open_persistent(temp.path().join("auth/db.json")).unwrap();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Password!2026")
        .unwrap();
    let token = store
        .login("admin", "Password!2026")
        .unwrap()
        .raw_session_token;
    let admin = store.authenticate_session(&token).unwrap();
    let team = TeamId::new();
    let mut contexts = Vec::new();
    for name in ["one", "two"] {
        let row = store
            .register_local_repository(&admin, RepoId::new(), name, team, true, None)
            .unwrap();
        contexts.push(RepoAccess {
            repo_id: row.metadata.id,
            registration: Some(row.binding),
        });
    }
    let invite = store
        .create_invite_link(
            &admin,
            "member@company.internal",
            EnterpriseRole::Developer,
            vec![TeamId::new()],
            None,
        )
        .unwrap();
    let target = store
        .redeem_invite_link(&invite.raw_token, "member", "Password!2026")
        .unwrap();
    (temp, store, token, target.id, contexts[0], contexts[1])
}
#[test]
fn membership_cas_concurrency_same_group_aba_and_restart_preserve_profiles() {
    let (_temp, store, token, user, one, two) = fixture();
    let admin = store.authenticate_session(&token).unwrap();
    let before = store
        .list_users(&admin)
        .unwrap()
        .into_iter()
        .find(|u| u.id == user)
        .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let jobs: Vec<_> = (0..2)
        .map(|_| {
            let (store, token, barrier) = (store.clone(), token.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                store.change_registered_group_member(&token, admin.user_id, one, user, 0, true)
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = jobs.into_iter().map(|j| j.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(Error::Conflict)))
            .count(),
        1
    );
    let member = store.login("member", "Password!2026").unwrap();
    let current = store
        .authenticate_session(&member.raw_session_token)
        .unwrap();
    assert!(store
        .authorize_repository_access(&current, two.repo_id)
        .is_ok());
    let noop = store
        .change_registered_group_member(&token, admin.user_id, two, user, 1, true)
        .unwrap();
    assert_eq!(noop.membership_version, 1);
    let removed = store
        .change_registered_group_member(&token, admin.user_id, two, user, 1, false)
        .unwrap();
    assert_eq!(removed.membership_version, 2);
    assert!(matches!(
        store.change_registered_group_member(&token, admin.user_id, one, user, 0, true),
        Err(Error::Conflict)
    ));
    let reopened = AuthStore::open_persistent(store.persistence_path().unwrap()).unwrap();
    assert_eq!(
        reopened
            .lookup_registered_group_member(&token, admin.user_id, one, " MEMBER ")
            .unwrap()
            .membership_version,
        2
    );
    let after = reopened
        .list_users(&admin)
        .unwrap()
        .into_iter()
        .find(|u| u.id == user)
        .unwrap();
    assert!(before == after);
}
#[test]
fn membership_bounds_inactive_cleanup_and_exact_authority_are_enforced() {
    let (_temp, store, token, user, one, _) = fixture();
    let admin = store.authenticate_session(&token).unwrap();
    let team = store
        .authorize_repository_access(&admin, one.repo_id)
        .unwrap()
        .owner_team_id;
    let mut groups: Vec<_> = (0..33).map(|_| TeamId::new()).collect();
    groups.push(team);
    let pending = {
        let mut guard = store.write_state().unwrap();
        guard.users.get_mut(&user).unwrap().team_ids = groups.clone();
        store.maybe_snapshot(guard)
    };
    store.persist_snapshot(pending).unwrap();
    let shrunk = store
        .change_registered_group_member(&token, admin.user_id, one, user, 0, false)
        .unwrap();
    assert_eq!(shrunk.membership_version, 1);
    let reopened = AuthStore::open_persistent(store.persistence_path().unwrap()).unwrap();
    assert!(matches!(
        reopened.change_registered_group_member(&token, admin.user_id, one, user, 1, true),
        Err(Error::Limit)
    ));
    let pending = {
        let mut guard = store.write_state().unwrap();
        let target = guard.users.get_mut(&user).unwrap();
        target.membership_version = u64::MAX;
        target.team_ids = vec![team];
        store.maybe_snapshot(guard)
    };
    store.persist_snapshot(pending).unwrap();
    assert!(matches!(
        store.change_registered_group_member(&token, admin.user_id, one, user, u64::MAX, false),
        Err(Error::VersionExhausted)
    ));
    assert_eq!(
        store
            .change_registered_group_member(&token, admin.user_id, one, user, u64::MAX, true)
            .unwrap()
            .membership_version,
        u64::MAX
    );
    let pending = {
        let mut guard = store.write_state().unwrap();
        guard.users.get_mut(&user).unwrap().membership_version = 4;
        store.maybe_snapshot(guard)
    };
    store.persist_snapshot(pending).unwrap();
    store.deactivate_user(&admin, user).unwrap();
    assert!(matches!(
        store.change_registered_group_member(&token, admin.user_id, one, user, 4, true),
        Err(Error::Inactive)
    ));
    assert!(store
        .change_registered_group_member(&token, admin.user_id, one, user, 4, false)
        .is_ok());
    assert_eq!(
        store
            .list_users(&admin)
            .unwrap()
            .into_iter()
            .find(|u| u.id == user)
            .unwrap()
            .status,
        AccountStatus::Suspended
    );
    let mcp = store.issue_oauth_session(admin.user_id).unwrap();
    assert!(matches!(
        store.lookup_registered_group_member(&mcp.raw_session_token, admin.user_id, one, "member"),
        Err(Error::Auth(_))
    ));
    assert!(matches!(
        store.change_registered_group_member(&token, user, one, user, 5, true),
        Err(Error::Auth(_))
    ));
    let mut mismatched = one;
    mismatched.registration = Some(gitust_core::RepositoryRegistration::new(one.repo_id));
    assert!(matches!(
        store.lookup_registered_group_member(&token, admin.user_id, mismatched, "member"),
        Err(Error::Auth(_))
    ));
    let mut wrong = one;
    wrong.repo_id = RepoId::new();
    assert!(matches!(
        store.lookup_registered_group_member(&token, admin.user_id, wrong, "member"),
        Err(Error::Auth(_))
    ));
}
#[test]
fn membership_storage_failure_does_not_relax_cas_or_claim_unconfirmed_success() {
    let (temp, store, token, user, one, _) = fixture();
    let admin = store.authenticate_session(&token).unwrap();
    let db = store.persistence_path().unwrap();
    let long = temp.path().join(format!("{}.json", "x".repeat(225)));
    std::fs::copy(db, &long).unwrap();
    let writer = AuthStore::open_persistent(&long).unwrap();
    let before = std::fs::read(&long).unwrap();
    assert!(matches!(
        writer.change_registered_group_member(&token, admin.user_id, one, user, 0, true),
        Err(Error::NotPublished(_))
    ));
    assert!(std::fs::read(&long).unwrap() == before);
    assert_eq!(
        writer
            .lookup_registered_group_member(&token, admin.user_id, one, "member")
            .unwrap()
            .membership_version,
        0
    );
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::persistence::FAIL_DIRECTORY_SYNC.with(|p| *p.borrow_mut() = None);
        }
    }
    let reset = Reset;
    crate::persistence::FAIL_DIRECTORY_SYNC.with(|p| *p.borrow_mut() = Some(db.to_owned()));
    assert!(matches!(
        store.change_registered_group_member(&token, admin.user_id, one, user, 0, true),
        Err(Error::Unconfirmed(_))
    ));
    assert!(matches!(
        store.change_registered_group_member(&token, admin.user_id, one, user, 0, true),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store.change_registered_group_member(&token, admin.user_id, one, user, 1, true),
        Err(Error::Unconfirmed(_))
    ));
    drop(reset);
    assert_eq!(
        store
            .change_registered_group_member(&token, admin.user_id, one, user, 1, true)
            .unwrap()
            .membership_version,
        1
    );
    let reopened = AuthStore::open_persistent(db).unwrap();
    assert!(
        reopened
            .lookup_registered_group_member(&token, admin.user_id, one, "member")
            .unwrap()
            .is_member
    );
}
#[test]
fn legacy_user_shape_and_versioned_shape_are_strict_and_non_interchangeable() {
    let (_temp, store, token, user, one, _) = fixture();
    let admin = store.authenticate_session(&token).unwrap();
    // This required-field decoder is the relevant unchanged legacy StoredUser contract.
    #[derive(serde::Deserialize)]
    struct OldUser {
        team_ids: Vec<TeamId>,
    }
    let user_value =
        || serde_json::to_value(store.read_state().unwrap().users.get(&user).unwrap()).unwrap();
    let old = user_value();
    assert!(old.get("membership").is_none());
    let decoded: OldUser = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(decoded.team_ids.len(), 1);
    store
        .change_registered_group_member(&token, admin.user_id, one, user, 0, true)
        .unwrap();
    let nested = user_value();
    assert!(nested.get("team_ids").is_none());
    assert!(serde_json::from_value::<OldUser>(nested.clone()).is_err());
    let parse =
        |v: serde_json::Value| serde_json::from_value::<crate::models::StoredUser>(v).is_err();
    let mut mixed = nested.clone();
    mixed["team_ids"] = serde_json::json!([]);
    assert!(parse(mixed));
    let mut zero = nested.clone();
    zero["membership"]["version"] = serde_json::json!(0);
    assert!(parse(zero));
    let mut unknown = nested.clone();
    unknown["membership"]["other"] = serde_json::json!(true);
    assert!(parse(unknown));
    let mut null = old.clone();
    null["membership"] = serde_json::Value::Null;
    assert!(parse(null));
    let mut nil = old.clone();
    nil["team_ids"] = serde_json::json!([TeamId(uuid::Uuid::nil())]);
    assert!(parse(nil));
    let mut dup = old.clone();
    dup["team_ids"] = serde_json::json!([old["team_ids"][0], old["team_ids"][0]]);
    assert!(parse(dup));
}
