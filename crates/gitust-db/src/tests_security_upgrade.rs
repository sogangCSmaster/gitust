use crate::secret_storage::{SecretKey, StoredSecret};
use crate::*;
use gitust_core::{RepoAccess, RepoId, TeamId};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const TEST_KEY: [u8; 32] = [49; 32];
fn persistent() -> (TempDir, AuthStore, AuthenticatedPrincipal, RepoAccess) {
    let dir = tempfile::tempdir().unwrap();
    let store =
        AuthStore::open_persistent_with_secret_key(dir.path().join("auth.json"), TEST_KEY).unwrap();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "FixturePassword!2026")
        .unwrap();
    let token = store
        .login("admin", "FixturePassword!2026")
        .unwrap()
        .raw_session_token;
    let admin = store.authenticate_session(&token).unwrap();
    let id = RepoId::new();
    let repo = store
        .register_local_repository(&admin, id, "fixture", TeamId::new(), true, None)
        .unwrap();
    (
        dir,
        store,
        admin,
        RepoAccess {
            repo_id: id,
            registration: Some(repo.binding),
        },
    )
}

#[test]
fn aead_secrets_survive_restart_and_reject_wrong_keys_contexts_and_corruption() {
    let (dir, store, admin, access) = persistent();
    store
        .set_workflow_secret(&admin, access, "FIXTURE", "synthetic-value")
        .unwrap();
    let bytes = std::fs::read(dir.path().join("auth.json")).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains("key_hex"));
    assert!(!text.contains("synthetic-value"));
    assert!(!text.contains(&hex::encode(TEST_KEY)));
    let reopened =
        AuthStore::open_persistent_with_secret_key(dir.path().join("auth.json"), TEST_KEY).unwrap();
    assert_eq!(
        reopened.resolve_workflow_secrets(&admin, access).unwrap()["FIXTURE"],
        "synthetic-value"
    );
    assert!(
        AuthStore::open_persistent_with_secret_key(dir.path().join("auth.json"), [50; 32]).is_err()
    );
    let key = SecretKey::supplied(TEST_KEY);
    let mut record = key.seal("totp:one", b"fixture").unwrap();
    assert!(key.open("totp:two", &record).is_err());
    let changed = if record.ciphertext_hex.starts_with("00") {
        "01"
    } else {
        "00"
    };
    record.ciphertext_hex.replace_range(0..2, changed);
    assert!(key.open("totp:one", &record).is_err());
}

#[test]
fn concurrent_handles_cannot_mix_encryption_keys_or_publish_after_key_failure() {
    let (dir, writer, admin, access) = persistent();
    let path = dir.path().join("auth.json");
    let other = AuthStore::open_persistent_with_secret_key(&path, [50; 32]).unwrap();
    writer
        .set_workflow_secret(&admin, access, "FIRST", "fixture-one")
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(other
        .set_workflow_secret(&admin, access, "SECOND", "fixture-two")
        .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        writer
            .resolve_workflow_secrets(&admin, access)
            .unwrap()
            .len(),
        1
    );
}

fn legacy_secret(family: &str, binding: &[u8], plain: &[u8]) -> StoredSecret {
    let key = [11u8; 32];
    let nonce = [12u8; 16];
    let mut cipher = Vec::new();
    for (i, chunk) in plain.chunks(32).enumerate() {
        let mut stream = Sha256::new();
        stream.update(format!("gitust-{family}-secret-stream-v1:"));
        stream.update(key);
        stream.update(nonce);
        stream.update((i as u64).to_le_bytes());
        cipher.extend(chunk.iter().zip(stream.finalize()).map(|(a, b)| a ^ b));
    }
    let mut mac = Sha256::new();
    mac.update(format!("gitust-{family}-secret-mac-v1:"));
    mac.update(key);
    mac.update(nonce);
    mac.update(binding);
    mac.update((cipher.len() as u64).to_le_bytes());
    mac.update(&cipher);
    StoredSecret {
        secret_version: 0,
        key_hex: hex::encode(key),
        nonce_hex: hex::encode(nonce),
        ciphertext_hex: hex::encode(cipher),
        tag_sha256: Some(mac.finalize().into()),
    }
}

#[test]
fn legacy_secret_families_migrate_with_bound_identity_and_no_embedded_key() {
    let key = SecretKey::supplied(TEST_KEY);
    for family in ["wf", "sso", "totp"] {
        let mut old = legacy_secret(family, b"identity", b"synthetic-secret");
        key.migrate("new-bound-identity", family, b"identity", &mut old)
            .unwrap();
        assert_eq!(
            key.open("new-bound-identity", &old).unwrap(),
            b"synthetic-secret"
        );
        assert!(!serde_json::to_string(&old).unwrap().contains("key_hex"));
        let mut invalid = legacy_secret(family, b"identity", b"synthetic-secret");
        assert!(key
            .migrate("new-bound-identity", family, b"wrong", &mut invalid)
            .is_err());
    }
}

#[test]
fn legacy_upgrade_disables_shared_password_and_revokes_previous_credentials() {
    let (dir, store, admin, access) = persistent();
    let invite = store
        .create_invite_link(
            &admin,
            "sso@company.internal",
            EnterpriseRole::Developer,
            vec![],
            None,
        )
        .unwrap();
    let profile = store
        .redeem_invite_link(&invite.raw_token, "ssofixture", "FixturePassword!2026")
        .unwrap();
    let session = store.login("ssofixture", "FixturePassword!2026").unwrap();
    let principal = store
        .authenticate_session(&session.raw_session_token)
        .unwrap();
    let pat = store
        .create_personal_access_token(&principal, "fixture", None)
        .unwrap();
    // Simulate the old on-disk representation; never authenticate with its shared password.
    let snapshot = {
        let mut guard = store.write_state().unwrap();
        guard.security_version = 0;
        guard.users.get_mut(&profile.id).unwrap().password_hash =
            crate::crypto::dummy_password_hash().to_owned();
        let mut binding = access.repo_id.0.as_bytes().to_vec();
        binding.extend_from_slice(&7u64.to_le_bytes());
        binding.extend_from_slice(b"FIXTURE");
        guard.workflow_secrets.insert(
            access.repo_id,
            vec![crate::workflow_secrets::StoredWorkflowSecret {
                name: "FIXTURE".into(),
                updated_at: chrono::Utc::now(),
                secret: legacy_secret("wf", &binding, b"preserved-value"),
            }],
        );
        store.maybe_snapshot(guard)
    };
    store.persist_snapshot(snapshot).unwrap();
    let path = dir.path().join("auth.json");
    let migrated = AuthStore::open_persistent_with_secret_key(&path, TEST_KEY).unwrap();
    assert_eq!(
        migrated.state.read().users[&profile.id].password_hash,
        crate::security_migration::DISABLED_SSO_PASSWORD
    );
    assert!(migrated
        .authenticate_session(&session.raw_session_token)
        .is_err());
    assert!(migrated.authenticate_pat(&pat.raw_token).is_err());
    assert_eq!(
        migrated.resolve_workflow_secrets(&admin, access).unwrap()["FIXTURE"],
        "preserved-value"
    );
    assert!(!String::from_utf8(std::fs::read(&path).unwrap())
        .unwrap()
        .contains("key_hex"));
    let reset = migrated
        .create_password_reset_link(&admin, profile.id, None)
        .unwrap();
    migrated
        .redeem_password_reset_link(&reset.raw_token, "NewFixturePassword!2026")
        .unwrap();
    assert!(migrated
        .login("ssofixture", "NewFixturePassword!2026")
        .is_ok());
    assert!(migrated.login("admin", "FixturePassword!2026").is_ok());
}

#[test]
fn current_storage_version_rejects_legacy_envelopes_without_rewriting_disk() {
    let (dir, store, _admin, access) = persistent();
    let snapshot = {
        let mut guard = store.write_state().unwrap();
        guard.workflow_secrets.insert(
            access.repo_id,
            vec![crate::workflow_secrets::StoredWorkflowSecret {
                name: "FIXTURE".into(),
                updated_at: chrono::Utc::now(),
                secret: legacy_secret("wf", b"identity", b"fixture"),
            }],
        );
        store.maybe_snapshot(guard)
    };
    store.persist_snapshot(snapshot).unwrap();
    let path = dir.path().join("auth.json");
    let before = std::fs::read(&path).unwrap();
    assert!(AuthStore::open_persistent_with_secret_key(&path, TEST_KEY).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn persistent_secret_write_requires_external_key_and_leaves_store_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let store =
        AuthStore::open_with_key(&dir.path().join("auth.json"), SecretKey::absent()).unwrap();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "FixturePassword!2026")
        .unwrap();
    let login = store.login("admin", "FixturePassword!2026").unwrap();
    let admin = store
        .authenticate_session(&login.raw_session_token)
        .unwrap();
    let id = RepoId::new();
    let repo = store
        .register_local_repository(&admin, id, "fixture", TeamId::new(), true, None)
        .unwrap();
    let before = std::fs::read(dir.path().join("auth.json")).unwrap();
    assert!(store
        .set_workflow_secret(
            &admin,
            RepoAccess {
                repo_id: id,
                registration: Some(repo.binding)
            },
            "FIXTURE",
            "fixture"
        )
        .is_err());
    assert_eq!(std::fs::read(dir.path().join("auth.json")).unwrap(), before);
}
