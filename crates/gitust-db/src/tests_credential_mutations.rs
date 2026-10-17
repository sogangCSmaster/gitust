use crate::AuthStore;
use gitust_core::{AuthError, EnterpriseRole, RepoAccess, RepoId, TeamId};

#[test]
fn credential_bound_mutations_preserve_current_roles_ownership_and_session_purpose() {
    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Password!2026")
        .unwrap();
    let session = store
        .login("admin", "Password!2026")
        .unwrap()
        .raw_session_token;
    let admin = store.authenticate_session(&session).unwrap();
    let team = TeamId::new();
    let invite = store
        .create_invite_link(
            &admin,
            "developer@company.internal",
            EnterpriseRole::Developer,
            vec![team],
            None,
        )
        .unwrap();
    let developer = store
        .redeem_invite_link(&invite.raw_token, "developer", "Password!2026")
        .unwrap();
    let token = store
        .login("developer", "Password!2026")
        .unwrap()
        .raw_session_token;
    let pat = store
        .create_personal_access_token_with_credential(&token, developer.id, "owned", None)
        .unwrap();
    let key = store
        .register_ssh_public_key_with_credential(
            &token,
            developer.id,
            "owned",
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl",
        )
        .unwrap();
    let foreign_pat = store
        .create_personal_access_token(&admin, "foreign", None)
        .unwrap();
    let foreign_key = store
        .register_ssh_public_key(
            &admin,
            "foreign",
            "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAAAQE=",
        )
        .unwrap();
    for credential in [&token, &pat.raw_token] {
        assert!(matches!(
            store.revoke_personal_access_token_with_credential(
                credential,
                developer.id,
                foreign_pat.metadata.id
            ),
            Err(AuthError::ForbiddenResourceOwnership)
        ));
        assert!(matches!(
            store.revoke_ssh_public_key_with_credential(credential, developer.id, foreign_key.id),
            Err(AuthError::ForbiddenResourceOwnership)
        ));
        assert!(matches!(
            store.deactivate_user_with_credential(credential, developer.id, admin.user_id),
            Err(AuthError::AdminPrivilegeRequired)
        ));
        assert!(matches!(
            store.create_team_repository_with_credential(
                credential,
                developer.id,
                "foreign",
                TeamId::new(),
                true
            ),
            Err(AuthError::ForbiddenRepositoryAccess)
        ));
    }
    store
        .create_team_repository_with_credential(&token, developer.id, "owned", team, true)
        .unwrap();
    store
        .revoke_ssh_public_key_with_credential(&token, developer.id, key.id)
        .unwrap();
    store
        .revoke_personal_access_token_with_credential(&token, developer.id, pat.metadata.id)
        .unwrap();
    assert!(matches!(
        store.deactivate_user_with_credential(&session, admin.user_id, admin.user_id),
        Err(AuthError::CannotDeactivateLastAdmin)
    ));
    assert!(matches!(
        store.create_personal_access_token_with_credential(
            &session,
            developer.id,
            "wrong-user",
            None
        ),
        Err(AuthError::Unauthenticated)
    ));
    let mcp = store.issue_oauth_session(admin.user_id).unwrap();
    assert!(matches!(
        store.create_personal_access_token_with_credential(
            &mcp.raw_session_token,
            admin.user_id,
            "mcp",
            None
        ),
        Err(AuthError::Unauthenticated)
    ));
    let row = store
        .register_local_repository(&admin, RepoId::new(), "registered", team, true, None)
        .unwrap();
    let access = RepoAccess {
        repo_id: row.metadata.id,
        registration: Some(row.binding),
    };
    assert!(matches!(
        store.rename_registered_repository_with_credential(&token, developer.id, access, "denied"),
        Err(AuthError::AdminPrivilegeRequired)
    ));
    let changed = store
        .rename_registered_repository_with_credential(&session, admin.user_id, access, "renamed")
        .unwrap();
    assert!(changed.name == "renamed" && changed.id == row.metadata.id);
    assert!(
        store
            .configured_repository_access(changed.id, Some(row.binding))
            .unwrap()
            == access
    );
}
