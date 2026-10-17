use gitust_core::{AuthError, EnterpriseRole, UserId};

use crate::AuthStore;

#[test]
fn automatic_actor_requires_a_real_current_admin_including_after_disk_revocation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.json");
    let store = AuthStore::open_persistent(&path).unwrap();
    assert!(matches!(
        store.authorize_automatic_checks_actor(UserId::new()),
        Err(AuthError::Unauthenticated)
    ));
    store
        .bootstrap_initial_admin_cli("operator", "operator@company.internal", "Password!2026")
        .unwrap();
    let login = store.login("operator", "Password!2026").unwrap();
    let operator = store
        .authenticate_session(&login.raw_session_token)
        .unwrap();
    for (username, role) in [
        ("job-admin", EnterpriseRole::SiteAdmin),
        ("developer", EnterpriseRole::Developer),
    ] {
        let invite = store
            .create_invite_link(
                &operator,
                &format!("{username}@company.internal"),
                role,
                Vec::new(),
                None,
            )
            .unwrap();
        let user = store
            .redeem_invite_link(&invite.raw_token, username, "Password!2026")
            .unwrap();
        if role == EnterpriseRole::Developer {
            assert!(matches!(
                store.authorize_automatic_checks_actor(user.id),
                Err(AuthError::AdminPrivilegeRequired)
            ));
            continue;
        }
        let before = store.authorize_automatic_checks_actor(user.id).unwrap();
        assert_eq!(before.user_id, user.id);
        assert_eq!(before.role, EnterpriseRole::SiteAdmin);
        let writer = AuthStore::open_persistent(&path).unwrap();
        writer.deactivate_user(&operator, user.id).unwrap();
        assert!(matches!(
            store.authorize_automatic_checks_actor(user.id),
            Err(AuthError::AccountDeactivated)
        ));
    }
    assert!(matches!(
        store.authorize_automatic_checks_actor(UserId::new()),
        Err(AuthError::Unauthenticated)
    ));
}
