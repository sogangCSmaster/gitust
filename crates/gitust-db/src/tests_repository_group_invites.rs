use crate::{
    AuthStore, RepositoryGroupInviteError as Error, RepositoryGroupInviteOutcome as Outcome,
    RepositoryGroupInviteRequest as Request,
};
use gitust_core::{EnterpriseRole, InviteId, RepoAccess, RepoId, TeamId};

fn fixture() -> (tempfile::TempDir, AuthStore, String, RepoAccess) {
    let temp = tempfile::tempdir().unwrap();
    let store = AuthStore::open_persistent(temp.path().join("auth.json")).unwrap();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Password!2026")
        .unwrap();
    let token = store
        .login("admin", "Password!2026")
        .unwrap()
        .raw_session_token;
    let admin = store.authenticate_session(&token).unwrap();
    let row = store
        .register_local_repository(&admin, RepoId::new(), "repo", TeamId::new(), true, None)
        .unwrap();
    (
        temp,
        store,
        token,
        RepoAccess {
            repo_id: row.metadata.id,
            registration: Some(row.binding),
        },
    )
}
fn request(id: InviteId) -> Request {
    Request {
        invite_id: id,
        target_email: " INVITED@COMPANY.INTERNAL ".into(),
        assigned_role: EnterpriseRole::Developer,
    }
}
fn issue(
    store: &AuthStore,
    token: &str,
    access: RepoAccess,
    id: InviteId,
) -> Result<crate::RepositoryGroupInvite, Error> {
    let admin = store.authenticate_session(token).unwrap();
    store.create_registered_repository_group_invite(token, admin.user_id, access, request(id))
}

#[test]
fn group_invite_concurrency_restart_and_semantic_conflicts_preserve_one_token_and_deadline() {
    let (_temp, store, token, access) = fixture();
    let id = InviteId::new();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let jobs: Vec<_> = (0..2)
        .map(|_| {
            let (store, token, barrier) = (store.clone(), token.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                issue(&store, &token, access, id).unwrap()
            })
        })
        .collect();
    barrier.wait();
    let mut created = None;
    let mut replay_count = 0;
    for job in jobs {
        match job.join().unwrap().outcome {
            Outcome::Created(link) => {
                assert!(created.is_none());
                created = Some(link);
            }
            Outcome::AlreadyIssued(_) => replay_count += 1,
        }
    }
    assert_eq!(replay_count, 1);
    let issued = created.unwrap();
    assert_eq!(
        (issued.invite.expires_at - issued.invite.created_at).num_hours(),
        72
    );
    let disk = std::fs::read(store.persistence_path().unwrap()).unwrap();
    assert!(!String::from_utf8_lossy(&disk).contains(&issued.raw_token));
    let record = store.read_state().unwrap().invites_by_id[&id].clone();
    let restarted = AuthStore::open_persistent(store.persistence_path().unwrap()).unwrap();
    assert!(matches!(
        issue(&restarted, &token, access, id).unwrap().outcome,
        Outcome::AlreadyIssued(_)
    ));
    let current = restarted.read_state().unwrap().invites_by_id[&id].clone();
    assert!(
        serde_json::to_value(record).unwrap() == serde_json::to_value(current).unwrap(),
        "stored issuance changed on replay"
    );
    let admin = restarted.authenticate_session(&token).unwrap();
    let original_count = restarted.list_invites(&admin).unwrap().len();
    for (email, role) in [
        ("different@company.internal", EnterpriseRole::Developer),
        ("invited@company.internal", EnterpriseRole::SiteAdmin),
    ] {
        assert!(matches!(
            restarted.create_registered_repository_group_invite(
                &token,
                admin.user_id,
                access,
                Request {
                    invite_id: id,
                    target_email: email.into(),
                    assigned_role: role
                }
            ),
            Err(Error::Conflict)
        ));
    }
    let other = restarted
        .register_local_repository(
            &admin,
            RepoId::new(),
            "same-group",
            issued.invite.team_ids[0],
            true,
            None,
        )
        .unwrap();
    assert!(matches!(
        issue(
            &restarted,
            &token,
            RepoAccess {
                repo_id: other.metadata.id,
                registration: Some(other.binding)
            },
            id
        ),
        Err(Error::Conflict)
    ));
    let administrator = restarted
        .create_invite_link(
            &admin,
            "second-admin@company.internal",
            EnterpriseRole::SiteAdmin,
            vec![],
            None,
        )
        .unwrap();
    restarted
        .redeem_invite_link(&administrator.raw_token, "second-admin", "Password!2026")
        .unwrap();
    let second = restarted
        .login("second-admin", "Password!2026")
        .unwrap()
        .raw_session_token;
    assert!(matches!(
        issue(&restarted, &second, access, id),
        Err(Error::Conflict)
    ));
    let generic = restarted
        .create_invite_link(
            &admin,
            "ordinary@company.internal",
            EnterpriseRole::Developer,
            vec![],
            Some(12),
        )
        .unwrap();
    assert!(matches!(
        issue(&restarted, &token, access, generic.invite.id),
        Err(Error::Conflict)
    ));
    let reset = restarted
        .create_password_reset_link(&admin, admin.user_id, None)
        .unwrap();
    assert!(matches!(
        issue(&restarted, &token, access, reset.invite.id),
        Err(Error::Conflict)
    ));
    assert_eq!(
        restarted.list_invites(&admin).unwrap().len(),
        original_count + 3
    );
}

#[test]
fn consumed_revoked_expired_and_legacy_origin_keys_never_remint() {
    let (_temp, store, token, access) = fixture();
    let id = InviteId::new();
    let issued = match issue(&store, &token, access, id).unwrap().outcome {
        Outcome::Created(link) => link,
        _ => panic!("first issuance"),
    };
    let admin = store.authenticate_session(&token).unwrap();
    store
        .redeem_invite_link(&issued.raw_token, "invited", "Password!2026")
        .unwrap();
    store.revoke_invite_link(&admin, id).unwrap();
    let mut guard = store.write_state().unwrap();
    guard.invites_by_id.get_mut(&id).unwrap().expires_at =
        chrono::Utc::now() - chrono::Duration::hours(1);
    let pending = store.maybe_snapshot(guard);
    store.persist_snapshot(pending).unwrap();
    let before = serde_json::to_value(&store.read_state().unwrap().invites_by_id[&id]).unwrap();
    assert!(matches!(
        issue(&store, &token, access, id).unwrap().outcome,
        Outcome::AlreadyIssued(_)
    ));
    assert!(
        serde_json::to_value(&store.read_state().unwrap().invites_by_id[&id]).unwrap() == before,
        "terminal replay changed the issuance"
    );
    let mut guard = store.write_state().unwrap();
    guard.invites_by_id.get_mut(&id).unwrap().repository_origin = None;
    let pending = store.maybe_snapshot(guard);
    store.persist_snapshot(pending).unwrap();
    assert!(matches!(
        issue(&store, &token, access, id),
        Err(Error::Conflict)
    ));
    // Legacy snapshots without the new optional provenance still load, but cannot remint the key.
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&std::fs::read(store.persistence_path().unwrap()).unwrap()).unwrap();
    legacy["invites"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .for_each(|v| {
            v.as_object_mut().unwrap().remove("repository_origin");
        });
    assert!(serde_json::from_value::<crate::models::PersistedSnapshot>(legacy).is_ok());
}

#[test]
fn invitation_publication_failures_never_return_a_secret_and_replay_reconfirms_durability() {
    let (temp, store, token, access) = fixture();
    // The existing DB and lock names fit NAME_MAX; its generated temp filename cannot.
    // This faults the real pre-publication write without any production injection hook.
    let long = temp.path().join(format!("{}.json", "a".repeat(230)));
    std::fs::copy(store.persistence_path().unwrap(), &long).unwrap();
    // A persistent store includes its SQLite checkpoint, even when the JSON window is small.
    std::fs::copy(
        crate::sqlite_wal_path(store.persistence_path().unwrap()),
        crate::sqlite_wal_path(&long),
    )
    .unwrap();
    let blocked = AuthStore::open_persistent(&long).unwrap();
    let old = std::fs::read(&long).unwrap();
    let id = InviteId::new();
    assert!(matches!(
        issue(&blocked, &token, access, id),
        Err(Error::NotPublished(_))
    ));
    assert!(
        std::fs::read(&long).unwrap() == old,
        "previous auth checkpoint changed"
    );
    assert!(!blocked
        .read_state()
        .unwrap()
        .invites_by_id
        .contains_key(&id));
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::persistence::FAIL_DIRECTORY_SYNC.with(|p| *p.borrow_mut() = None);
        }
    }
    let reset = Reset;
    let db = store.persistence_path().unwrap().to_owned();
    crate::persistence::FAIL_DIRECTORY_SYNC.with(|p| *p.borrow_mut() = Some(db.clone()));
    assert!(matches!(
        issue(&store, &token, access, id),
        Err(Error::Unconfirmed(_))
    ));
    let disk: serde_json::Value = serde_json::from_slice(&std::fs::read(&db).unwrap()).unwrap();
    let record = disk["invites"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id.0.to_string())
        .unwrap()
        .clone();
    assert!(matches!(
        issue(&store, &token, access, id),
        Err(Error::Unconfirmed(_))
    ));
    let again: serde_json::Value = serde_json::from_slice(&std::fs::read(&db).unwrap()).unwrap();
    assert!(
        again["invites"][0] == record,
        "uncertain retry changed the issuance"
    );
    drop(reset);
    assert!(matches!(
        issue(&store, &token, access, id).unwrap().outcome,
        Outcome::AlreadyIssued(_)
    ));
    let final_state = store.read_state().unwrap();
    assert!(
        serde_json::to_value(&final_state.invites_by_id[&id]).unwrap() == record,
        "confirmed replay changed the issuance"
    );
}

#[test]
fn group_invite_authority_and_registration_are_current_even_for_an_existing_key() {
    let (_temp, store, token, access) = fixture();
    let admin = store.authenticate_session(&token).unwrap();
    let id = InviteId::new();
    issue(&store, &token, access, id).unwrap();
    let count = store.list_invites(&admin).unwrap().len();
    let wrong = RepoAccess {
        registration: Some(gitust_core::RepositoryRegistration::new(access.repo_id)),
        ..access
    };
    assert!(matches!(
        issue(&store, &token, wrong, id),
        Err(Error::Auth(gitust_core::AuthError::StorageError(_)))
    ));
    assert!(matches!(
        issue(
            &store,
            &token,
            RepoAccess {
                registration: None,
                ..access
            },
            id
        ),
        Err(Error::Auth(
            gitust_core::AuthError::ForbiddenRepositoryAccess
        ))
    ));
    assert!(matches!(
        store.create_registered_repository_group_invite(
            &token,
            gitust_core::UserId::new(),
            access,
            request(id)
        ),
        Err(Error::Auth(gitust_core::AuthError::AdminPrivilegeRequired))
    ));
    let mcp = store.issue_oauth_session(admin.user_id).unwrap();
    assert!(matches!(
        store.create_registered_repository_group_invite(
            &mcp.raw_session_token,
            admin.user_id,
            access,
            request(id)
        ),
        Err(Error::Auth(gitust_core::AuthError::Unauthenticated))
    ));
    let pat = store
        .create_personal_access_token(&admin, "invite", None)
        .unwrap();
    store
        .revoke_personal_access_token(&admin, pat.metadata.id)
        .unwrap();
    assert!(matches!(
        store.create_registered_repository_group_invite(
            &pat.raw_token,
            admin.user_id,
            access,
            request(id)
        ),
        Err(Error::Auth(gitust_core::AuthError::Unauthenticated))
    ));
    store.revoke_session(&admin, &token).unwrap();
    assert!(matches!(
        store.create_registered_repository_group_invite(&token, admin.user_id, access, request(id)),
        Err(Error::Auth(gitust_core::AuthError::Unauthenticated))
    ));
    assert_eq!(store.list_invites(&admin).unwrap().len(), count);
}
