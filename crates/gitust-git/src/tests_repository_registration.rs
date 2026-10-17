use crate::{GitEngine, RegistrationReceipt};
use gitust_core::RepositoryRegistration;
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::Path,
    process::Command,
};

fn fixture() -> (tempfile::TempDir, GitEngine, gitust_core::RepoId) {
    let temp = tempfile::tempdir().unwrap();
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.as_encoded_bytes().starts_with(b"GIT_") {
            command.env_remove(key);
        }
    }
    assert!(command
        .args(["init", "--bare"])
        .arg(temp.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap()
        .status
        .success());
    let engine = GitEngine::new(temp.path());
    let id = engine.load_or_create_repository_id().unwrap();
    (temp, engine, id)
}
fn write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn receipt_retry_preserves_identity_and_never_overwrites_a_conflicting_anchor() {
    let (temp, engine, id) = fixture();
    let directory = temp.path().join("gitust-repository");
    let identity = fs::read(directory.join("identity.json")).unwrap();
    let binding = RepositoryRegistration::new(id);
    assert!(engine.repository_registration_state(id).unwrap() == RegistrationReceipt::Absent);
    write(
        &directory.join("registration.pending"),
        &serde_json::to_vec(&binding).unwrap(),
    );
    assert!(
        engine.repository_registration_state(id).unwrap() == RegistrationReceipt::Pending(binding)
    );
    engine.complete_repository_registration(binding).unwrap();
    let receipt = directory.join("registration.json");
    let original = fs::read(&receipt).unwrap();
    engine.complete_repository_registration(binding).unwrap();
    assert_eq!(fs::read(&receipt).unwrap(), original);
    assert_eq!(fs::read(directory.join("identity.json")).unwrap(), identity);
    assert_eq!(
        fs::metadata(&receipt).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        engine.repository_registration_state(id).unwrap() == RegistrationReceipt::Complete(binding)
    );
    assert!(engine
        .complete_repository_registration(RepositoryRegistration::new(id))
        .is_err());
    assert_eq!(fs::read(&receipt).unwrap(), original);
    let ownership = engine.lock_worktree_service().unwrap();
    let rival = GitEngine::new(temp.path());
    assert!(rival.complete_repository_registration(binding).is_err());
    assert_eq!(fs::read(&receipt).unwrap(), original);
    drop(ownership);
}

#[test]
fn unsafe_or_partial_receipts_are_not_absence_and_are_never_deleted_by_retry() {
    let (temp, engine, id) = fixture();
    let directory = temp.path().join("gitust-repository");
    let receipt = directory.join("registration.json");
    let binding = RepositoryRegistration::new(id);
    for bytes in [
        b"{".to_vec(),
        serde_json::to_vec(&serde_json::json!({
        "version":99,"repo_id":id,"registration_nonce":binding.registration_nonce}))
        .unwrap(),
        serde_json::to_vec(
            &serde_json::json!({"version":1,"repo_id":id,"registration_nonce":"00000000-0000-0000-0000-000000000000"}),
        )
        .unwrap(),
    ] {
        write(&receipt, &bytes);
        assert!(engine.repository_registration_state(id).is_err());
        assert!(engine.complete_repository_registration(binding).is_err());
        assert_eq!(fs::read(&receipt).unwrap(), bytes);
    }
    fs::remove_file(&receipt).unwrap();
    let external = temp.path().join("external.json");
    write(&external, &serde_json::to_vec(&binding).unwrap());
    symlink(&external, &receipt).unwrap();
    assert!(engine.repository_registration_state(id).is_err());
    fs::remove_file(&receipt).unwrap();
    write(&receipt, &serde_json::to_vec(&binding).unwrap());
    fs::set_permissions(&receipt, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(engine.repository_registration_state(id).is_err());
    fs::remove_file(&receipt).unwrap();
    let pending = directory.join("registration.pending");
    write(&pending, b"partial");
    assert!(engine.repository_registration_state(id).is_err());
    assert!(engine.complete_repository_registration(binding).is_err());
    assert_eq!(fs::read(&pending).unwrap(), b"partial");
    write(&pending, &serde_json::to_vec(&binding).unwrap());
    write(&receipt, &serde_json::to_vec(&binding).unwrap());
    assert!(engine.repository_registration_state(id).is_err());
}
