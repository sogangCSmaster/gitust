//! Offline create contracts use only synthetic private local fixtures.
use clap::Parser;
#[test]
fn create_cli_requires_explicit_destination_auth_privacy_and_default_branch() {
    let arguments = [
        "gitust",
        "repository",
        "create",
        "--repo",
        "/synthetic/new.git",
        "--auth-db",
        "/synthetic/auth/db.json",
        "--admin-username",
        "admin",
        "--admin-password",
        "synthetic-password",
        "--name",
        "Created",
        "--owner-team-id",
        "4e6b8a16-bf87-4e79-911d-d63cead01378",
        "--private",
        "true",
        "--default-branch",
        "trunk",
    ];
    let parsed = crate::Cli::try_parse_from(arguments);
    assert!(
        parsed.is_ok(),
        "explicit offline repository create must be supported"
    );
    for required in ["--private", "--default-branch", "--repo", "--auth-db"] {
        let mut missing = arguments.to_vec();
        let index = missing.iter().position(|v| *v == required).unwrap();
        missing.drain(index..index + 2);
        assert!(crate::Cli::try_parse_from(missing).is_err());
    }
}

use crate::{
    repository_create_cmd::{run, CreateArgs},
    repository_provision::{complete_registration_and_publish, ProvisionTransaction},
    tests_repository_import::{fixture as import_fixture, git},
};
use gitust_db::AuthStore;
use gitust_git::{GitEngine, RegistrationReceipt, RepositoryHead};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

fn fixture(root: &Path) -> CreateArgs {
    let old = import_fixture(root);
    CreateArgs {
        repo: old.repo,
        auth_db: old.auth_db,
        admin_username: old.admin_username,
        admin_password: old.admin_password,
        name: "Created".into(),
        owner_team_id: old.owner_team_id,
        private: true,
        default_branch: "trunk".into(),
        acknowledge_stopped_create_processes: false,
    }
}
fn registration(
    args: &CreateArgs,
    tx: &ProvisionTransaction,
    engine: &GitEngine,
    receipt: bool,
) -> gitust_core::RepositoryRegistration {
    let store = AuthStore::open_persistent(&args.auth_db).unwrap();
    crate::admin_cmd::with_admin_session(
        &store,
        &args.admin_username,
        &args.admin_password,
        |admin| {
            let row = store.register_local_repository(
                admin,
                tx.id(),
                &args.name,
                args.owner_team_id,
                args.private,
                None,
            )?;
            if receipt {
                engine.complete_repository_registration(row.binding)?;
            }
            Ok(row.binding)
        },
    )
    .unwrap()
}
fn assert_empty(args: &CreateArgs) {
    assert_eq!(
        git(&args.repo, &["rev-parse", "--is-bare-repository"]),
        "true"
    );
    assert_eq!(
        git(&args.repo, &["symbolic-ref", "--no-recurse", "HEAD"]),
        format!("refs/heads/{}", args.default_branch)
    );
    assert!(git(&args.repo, &["for-each-ref"]).is_empty());
    git(&args.repo, &["fsck", "--full", "--strict"]);
}
#[test]
fn create_empty_registered_bare_and_identical_retry_preserve_identity_binding_and_inode() {
    let root = tempfile::tempdir().unwrap();
    let args = fixture(root.path());
    let id = run(&args).unwrap();
    assert_empty(&args);
    let engine = GitEngine::new(&args.repo);
    assert!(
        matches!(engine.inspect_repository_head().unwrap(),RepositoryHead::Unborn{branch_name} if branch_name=="trunk")
    );
    let RegistrationReceipt::Complete(binding) = engine.repository_registration_state(id).unwrap()
    else {
        panic!("missing fixture registration")
    };
    let store = AuthStore::open_persistent(&args.auth_db).unwrap();
    store
        .configured_repository_access(id, Some(binding))
        .unwrap();
    let inode = fs::metadata(&args.repo).unwrap().ino();
    let identity = fs::read(args.repo.join("gitust-repository/identity.json")).unwrap();
    let receipt = fs::read(args.repo.join("gitust-repository/registration.json")).unwrap();
    assert_eq!(run(&args).unwrap(), id);
    assert_eq!(fs::metadata(&args.repo).unwrap().ino(), inode);
    assert_eq!(
        fs::read(args.repo.join("gitust-repository/identity.json")).unwrap(),
        identity
    );
    assert_eq!(
        fs::read(args.repo.join("gitust-repository/registration.json")).unwrap(),
        receipt
    );
    assert_empty(&args);
}
#[test]
fn create_and_import_share_claim_and_never_adopt_each_others_operation_journal() {
    let root = tempfile::tempdir().unwrap();
    let old = import_fixture(root.path());
    let mut args = CreateArgs {
        repo: old.repo.clone(),
        auth_db: old.auth_db.clone(),
        admin_username: old.admin_username.clone(),
        admin_password: old.admin_password.clone(),
        name: old.name.clone(),
        owner_team_id: old.owner_team_id,
        private: old.private,
        default_branch: old.default_branch.clone(),
        acknowledge_stopped_create_processes: true,
    };
    let tx = ProvisionTransaction::open(&args.provision()).unwrap();
    assert!(crate::repository_import_cmd::ImportTransaction::open(&old).is_err());
    assert!(ProvisionTransaction::open(&args.provision()).is_err());
    let intent = fs::read(tx.envelope.path.join("intent.json")).unwrap();
    drop(tx);
    assert!(crate::repository_import_cmd::run(&old).is_err());
    let tx = ProvisionTransaction::open(&args.provision()).unwrap();
    assert_eq!(
        fs::read(tx.envelope.path.join("intent.json")).unwrap(),
        intent
    );
    drop(tx);
    args.repo = root.path().join("other.git");
    let mut old = old;
    old.repo = args.repo.clone();
    let tx = crate::repository_import_cmd::ImportTransaction::open(&old).unwrap();
    assert!(ProvisionTransaction::open(&args.provision()).is_err());
    drop(tx);
    assert!(run(&args).is_err());
    // Existing import-v1 documents are still encoded without an operation field.
    let tx = crate::repository_import_cmd::ImportTransaction::open(&old).unwrap();
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(tx.envelope.path.join("intent.json")).unwrap()).unwrap();
    assert_eq!(record["version"], 1);
    assert!(record.get("operation").is_none());
    drop(tx);
    crate::repository_import_cmd::run(&old).unwrap();
}
#[test]
fn interrupted_create_requires_ack_and_preserves_fixed_id() {
    let root = tempfile::tempdir().unwrap();
    let mut args = fixture(root.path());
    let tx = ProvisionTransaction::open(&args.provision()).unwrap();
    let id = tx.id();
    tx.mark_preparing().unwrap();
    drop(tx);
    assert!(run(&args).is_err());
    assert!(!args.repo.exists());
    args.acknowledge_stopped_create_processes = true;
    assert_eq!(run(&args).unwrap(), id);
    assert_empty(&args);
}
#[test]
fn db_only_registration_and_rename_before_directory_sync_resume_the_same_pair() {
    for boundary in ["db_only", "pending_receipt", "renamed"] {
        let root = tempfile::tempdir().unwrap();
        let args = fixture(root.path());
        let tx = ProvisionTransaction::open(&args.provision()).unwrap();
        let id = tx.id();
        let engine = tx.prepare(&args.provision()).unwrap();
        let expected_binding = registration(&args, &tx, &engine, boundary != "db_only");
        let store = AuthStore::open_persistent(&args.auth_db).unwrap();
        let before = store
            .configured_repository_access(id, Some(expected_binding))
            .unwrap();
        if boundary == "pending_receipt" {
            let metadata = tx.envelope.path.join("repository/gitust-repository");
            fs::rename(
                metadata.join("registration.json"),
                metadata.join("registration.pending"),
            )
            .unwrap();
        }
        if boundary == "renamed" {
            tx.envelope
                .rename_exclusive("repository", &tx.parent, &tx.leaf)
                .unwrap();
        }
        drop(engine);
        drop(tx);
        assert_eq!(run(&args).unwrap(), id);
        assert_empty(&args);
        let RegistrationReceipt::Complete(binding) = GitEngine::new(&args.repo)
            .repository_registration_state(id)
            .unwrap()
        else {
            panic!("missing fixture receipt")
        };
        let after = store
            .configured_repository_access(id, Some(binding))
            .unwrap();
        assert_eq!(before, after);
        assert_eq!(binding, expected_binding);
    }
}
#[test]
fn existing_destination_race_cannot_overwrite_and_retry_keeps_registered_pair() {
    let root = tempfile::tempdir().unwrap();
    let args = fixture(root.path());
    let tx = ProvisionTransaction::open(&args.provision()).unwrap();
    let id = tx.id();
    let engine = tx.prepare(&args.provision()).unwrap();
    let stage = tx.envelope.path.join("repository");
    assert!(GitEngine::new(&stage)
        .prepare_empty_repository("changed")
        .is_err());
    assert_eq!(git(&stage, &["symbolic-ref", "HEAD"]), "refs/heads/trunk");
    registration(&args, &tx, &engine, true);
    let receipt = fs::read(
        tx.envelope
            .path
            .join("repository/gitust-repository/registration.json"),
    )
    .unwrap();
    fs::create_dir(&args.repo).unwrap();
    fs::write(args.repo.join("sentinel"), b"preserve").unwrap();
    assert!(tx.publish(&engine).is_err());
    drop(engine);
    drop(tx);
    assert_eq!(fs::read(args.repo.join("sentinel")).unwrap(), b"preserve");
    fs::remove_file(args.repo.join("sentinel")).unwrap();
    fs::remove_dir(&args.repo).unwrap();
    assert_eq!(run(&args).unwrap(), id);
    assert_eq!(
        fs::read(args.repo.join("gitust-repository/registration.json")).unwrap(),
        receipt
    );
}
#[test]
fn populated_or_dangling_symbolic_ref_is_not_an_empty_snapshot_even_with_unborn_default() {
    for dangling in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let args = fixture(root.path());
        let id = run(&args).unwrap();
        if dangling {
            git(
                &args.repo,
                &["symbolic-ref", "refs/heads/dangling", "refs/heads/missing"],
            );
        } else {
            let source = root.path().join("source");
            git(
                &source,
                &["push", args.repo.to_str().unwrap(), "main:refs/heads/topic"],
            );
        }
        assert!(matches!(
            GitEngine::new(&args.repo)
                .inspect_repository_head()
                .unwrap(),
            RepositoryHead::Unborn { .. }
        ));
        let head = fs::read(args.repo.join("HEAD")).unwrap();
        let receipt = fs::read(args.repo.join("gitust-repository/registration.json")).unwrap();
        assert!(run(&args).is_err());
        assert_eq!(
            GitEngine::new(&args.repo)
                .load_existing_repository_id()
                .unwrap(),
            id
        );
        assert_eq!(fs::read(args.repo.join("HEAD")).unwrap(), head);
        assert_eq!(
            fs::read(args.repo.join("gitust-repository/registration.json")).unwrap(),
            receipt
        );
    }
}
#[test]
fn create_denies_credentials_paths_and_changed_request_without_adopting_destination() {
    let root = tempfile::tempdir().unwrap();
    let mut args = fixture(root.path());
    args.admin_password = "wrong synthetic credential".into();
    assert!(run(&args).is_err());
    assert!(!args.repo.exists());
    assert!(!fs::read_dir(root.path()).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".gitust-import-")));
    args.admin_password = "Fixture-password-123".into();
    let tx = ProvisionTransaction::open(&args.provision()).unwrap();
    let id = tx.id();
    drop(tx);
    args.default_branch = "agent/unsafe".into();
    assert!(run(&args).is_err());
    args.default_branch = "other".into();
    assert!(run(&args).is_err());
    args.default_branch = "trunk".into();
    assert_eq!(run(&args).unwrap(), id);
    let outside = root.path().join("untouched");
    fs::create_dir(&outside).unwrap();
    args.repo = root.path().join("alias.git");
    std::os::unix::fs::symlink(&outside, &args.repo).unwrap();
    assert!(run(&args).is_err());
    assert!(args.repo.is_symlink());
    args.repo = root.path().join("nonexistent-parent/new.git");
    assert!(run(&args).is_err());
    assert!(!args.repo.parent().unwrap().exists());
    args.repo = root.path().join("db-target");
    args.auth_db = args.repo.join("db.json");
    assert!(run(&args).is_err());
    assert!(!args.repo.exists());
}
#[test]
fn create_rejects_corrupt_or_replaced_prepared_stage_without_repair() {
    for mutation in ["operation", "pending", "inode", "mode"] {
        let root = tempfile::tempdir().unwrap();
        let args = fixture(root.path());
        let tx = ProvisionTransaction::open(&args.provision()).unwrap();
        let engine = tx.prepare(&args.provision()).unwrap();
        let ready = tx.envelope.path.join("prepared.json");
        match mutation {
            "operation" => {
                let mut v: serde_json::Value =
                    serde_json::from_slice(&fs::read(&ready).unwrap()).unwrap();
                v["operation"] = serde_json::Value::Null;
                fs::write(&ready, serde_json::to_vec(&v).unwrap()).unwrap();
            }
            "pending" => {
                fs::rename(&ready, tx.envelope.path.join("prepared.json.pending")).unwrap();
            }
            "inode" => {
                fs::rename(
                    tx.envelope.path.join("repository"),
                    tx.envelope.path.join("old-stage"),
                )
                .unwrap();
                fs::create_dir(tx.envelope.path.join("repository")).unwrap();
                fs::set_permissions(
                    tx.envelope.path.join("repository"),
                    fs::Permissions::from_mode(0o700),
                )
                .unwrap();
            }
            _ => fs::set_permissions(&ready, fs::Permissions::from_mode(0o644)).unwrap(),
        }
        drop(engine);
        drop(tx);
        assert!(
            run(&args).is_err(),
            "unsafe prepared state must stay denied"
        );
        assert!(!args.repo.exists());
    }
}
#[test]
fn revoked_originating_session_cannot_register_or_publish_a_prepared_create() {
    let root = tempfile::tempdir().unwrap();
    let args = fixture(root.path());
    let tx = ProvisionTransaction::open(&args.provision()).unwrap();
    let engine = tx.prepare(&args.provision()).unwrap();
    let store = AuthStore::open_persistent(&args.auth_db).unwrap();
    let result = crate::admin_cmd::with_admin_session_token(
        &store,
        &args.admin_username,
        &args.admin_password,
        |admin, token| {
            store.revoke_session(admin, token)?;
            assert!(complete_registration_and_publish(
                &args.provision(),
                &store,
                admin,
                token,
                &tx,
                &engine
            )
            .is_err());
            Ok(())
        },
    );
    assert!(result.is_err()); // The helper also reports that its already-revoked temporary token cannot be revoked again.
    assert!(matches!(
        engine.repository_registration_state(tx.id()).unwrap(),
        RegistrationReceipt::Absent
    ));
    assert!(!args.repo.exists());
}
#[test]
fn create_requires_existing_private_auth_database_without_changing_exposed_or_linked_files() {
    let root = tempfile::tempdir().unwrap();
    let mut args = fixture(root.path());
    let original = fs::read(&args.auth_db).unwrap();
    fs::set_permissions(&args.auth_db, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(run(&args).is_err());
    assert!(!args.repo.exists());
    assert_eq!(fs::metadata(&args.auth_db).unwrap().mode() & 0o7777, 0o644);
    fs::set_permissions(&args.auth_db, fs::Permissions::from_mode(0o600)).unwrap();
    let linked = root.path().join("linked-db.json");
    fs::hard_link(&args.auth_db, &linked).unwrap();
    args.auth_db = linked;
    assert!(run(&args).is_err());
    assert!(!args.repo.exists());
    assert_eq!(fs::read(&args.auth_db).unwrap(), original);
}
#[test]
fn invalid_names_fail_before_auth_or_journal_and_corrected_name_can_use_same_destination() {
    for importing in [false, true] {
        for invalid in ["bad name".to_owned(), "x".repeat(101)] {
            let root = tempfile::tempdir().unwrap();
            let mut old = import_fixture(root.path());
            let mut args = CreateArgs {
                repo: old.repo.clone(),
                auth_db: old.auth_db.clone(),
                admin_username: old.admin_username.clone(),
                admin_password: old.admin_password.clone(),
                name: invalid.clone(),
                owner_team_id: old.owner_team_id,
                private: old.private,
                default_branch: old.default_branch.clone(),
                acknowledge_stopped_create_processes: false,
            };
            old.name = invalid;
            let before = fs::read(&old.auth_db).unwrap();
            let result = if importing {
                crate::repository_import_cmd::run(&old)
            } else {
                run(&args)
            };
            assert!(result.is_err());
            assert!(
                !fs::read_dir(root.path()).unwrap().any(|e| e
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".gitust-import-")),
                "invalid name must create no destination claim or journal"
            );
            assert_eq!(
                fs::read(&old.auth_db).unwrap(),
                before,
                "invalid name must not start an auth session"
            );
            assert!(!old.repo.exists());
            old.name = "corrected-name".into();
            args.name = old.name.clone();
            let result = if importing {
                crate::repository_import_cmd::run(&old)
            } else {
                run(&args)
            };
            assert!(
                result.is_ok(),
                "corrected name must be usable at the unchanged destination"
            );
        }
    }
}
