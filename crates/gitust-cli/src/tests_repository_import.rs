//! Observable import transaction boundaries; all sources and credentials are synthetic.
use crate::repository_import_cmd::{ImportArgs, ImportTransaction};
use gitust_core::{EnterpriseRole, TeamId};
use gitust_db::AuthStore;
use gitust_git::GitEngine;
use std::{fs, path::Path, process::Command};

pub(crate) fn git(path: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let out = command
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@company.internal")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@company.internal")
        .output()
        .unwrap();
    assert!(out.status.success(), "synthetic git operation failed");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
pub(crate) fn fixture(root: &Path) -> ImportArgs {
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    git(&source, &["init", "-q", "-b", "main"]);
    fs::write(source.join("hello.txt"), "hello\n").unwrap();
    git(&source, &["add", "."]);
    git(&source, &["commit", "-qm", "fixture"]);
    git(&source, &["tag", "-a", "v1", "-m", "release"]);
    let auth_db = root.join("auth/db.json");
    AuthStore::open_persistent(&auth_db)
        .unwrap()
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Fixture-password-123")
        .unwrap();
    ImportArgs {
        source: source.to_str().unwrap().into(),
        repo: root.join("imported.git"),
        auth_db,
        admin_username: "admin".into(),
        admin_password: "Fixture-password-123".into(),
        name: "Imported".into(),
        owner_team_id: TeamId::new(),
        private: true,
        default_branch: "main".into(),
        acknowledge_stopped_import_processes: false,
    }
}
#[test]
fn import_preserves_head_and_tag_without_copying_source_identity_and_retries_same_id() {
    let root = tempfile::tempdir().unwrap();
    let args = fixture(root.path());
    let source = GitEngine::new(&args.source);
    let old_id = source.load_or_create_repository_id().unwrap();
    let first = crate::repository_import_cmd::run(&args).unwrap();
    assert_ne!(first, old_id);
    assert_eq!(
        git(&args.repo, &["rev-parse", "HEAD"]),
        git(Path::new(&args.source), &["rev-parse", "HEAD"])
    );
    assert_eq!(
        git(&args.repo, &["rev-parse", "refs/tags/v1"]),
        git(Path::new(&args.source), &["rev-parse", "refs/tags/v1"])
    );
    assert_eq!(crate::repository_import_cmd::run(&args).unwrap(), first);
    git(&args.repo, &["fsck", "--full"]);
    git(Path::new(&args.source), &["fsck", "--full"]);
}
#[test]
fn uncertain_preparation_requires_ack_and_never_changes_the_intent_id() {
    let root = tempfile::tempdir().unwrap();
    let mut args = fixture(root.path());
    let tx = ImportTransaction::open(&args).unwrap();
    let id = tx.id();
    tx.mark_preparing().unwrap();
    drop(tx);
    assert!(crate::repository_import_cmd::run(&args).is_err());
    assert!(!args.repo.exists());
    args.acknowledge_stopped_import_processes = true;
    assert_eq!(crate::repository_import_cmd::run(&args).unwrap(), id);
}
#[test]
fn registered_stage_survives_destination_race_and_retries_without_refetch_or_new_binding() {
    let root = tempfile::tempdir().unwrap();
    let args = fixture(root.path());
    let tx = ImportTransaction::open(&args).unwrap();
    let id = tx.id();
    let engine = tx.prepare(&args).unwrap();
    let store = AuthStore::open_persistent(&args.auth_db).unwrap();
    crate::admin_cmd::with_admin_session(
        &store,
        &args.admin_username,
        &args.admin_password,
        |admin| {
            let row = store.register_local_repository(
                admin,
                id,
                &args.name,
                args.owner_team_id,
                true,
                None,
            )?;
            engine.complete_repository_registration(row.binding)?;
            Ok(())
        },
    )
    .unwrap();
    fs::create_dir(&args.repo).unwrap();
    fs::write(args.repo.join("sentinel"), "untouched").unwrap();
    assert!(tx.publish(&engine).is_err());
    drop(tx);
    drop(engine);
    assert_eq!(fs::read(args.repo.join("sentinel")).unwrap(), b"untouched");
    fs::remove_file(args.repo.join("sentinel")).unwrap();
    fs::remove_dir(&args.repo).unwrap();
    let expected = git(Path::new(&args.source), &["rev-parse", "HEAD"]);
    fs::write(Path::new(&args.source).join("hello.txt"), "newer\n").unwrap();
    git(
        Path::new(&args.source),
        &["commit", "-qam", "upstream changed"],
    );
    assert_eq!(crate::repository_import_cmd::run(&args).unwrap(), id);
    assert_eq!(git(&args.repo, &["rev-parse", "HEAD"]), expected);
}
#[test]
fn destination_claim_is_exclusive_and_existing_or_symlink_targets_are_never_adopted() {
    let root = tempfile::tempdir().unwrap();
    let args = fixture(root.path());
    let tx = ImportTransaction::open(&args).unwrap();
    assert!(ImportTransaction::open(&args).is_err());
    drop(tx);
    let other = root.path().join("existing");
    fs::create_dir(&other).unwrap();
    std::os::unix::fs::symlink(&other, &args.repo).unwrap();
    assert!(crate::repository_import_cmd::run(&args).is_err());
    assert!(args.repo.is_symlink());
}
#[test]
fn source_credentials_bad_default_and_auth_db_inside_destination_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let mut args = fixture(root.path());
    args.source = "ssh://user:synthetic-secret@example.invalid/repo".into();
    assert!(crate::repository_import_cmd::run(&args).is_err());
    assert!(!args.repo.exists());
    let mut args = fixture(&{
        let p = root.path().join("second");
        fs::create_dir(&p).unwrap();
        p
    });
    args.auth_db = args.repo.join("db.json");
    assert!(crate::repository_import_cmd::run(&args).is_err());
    assert!(!args.repo.exists());
    args.auth_db = root.path().join("second/auth/db.json");
    args.default_branch = "agent/unsafe".into();
    assert!(crate::repository_import_cmd::run(&args).is_err());
    assert!(!args.repo.exists());
}
#[test]
fn originating_admin_session_must_still_be_current_before_publication() {
    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Fixture-password-123")
        .unwrap();
    crate::admin_cmd::with_admin_session_token(
        &store,
        "admin",
        "Fixture-password-123",
        |caller, token| {
            assert_eq!(caller.role, EnterpriseRole::SiteAdmin);
            store.revoke_session(caller, token)?;
            assert!(crate::repository_import_cmd::recheck_admin(&store, caller, token).is_err());
            Ok(())
        },
    )
    .unwrap();
}

#[test]
fn failed_fetch_keeps_one_id_and_requires_ack_before_reusing_the_private_stage() {
    let root = tempfile::tempdir().unwrap();
    let mut args = fixture(root.path());
    let missing_git = root.path().join("not yet a repository");
    fs::create_dir(&missing_git).unwrap();
    args.source = missing_git.to_str().unwrap().into();
    assert!(crate::repository_import_cmd::run(&args).is_err());
    let tx = ImportTransaction::open(&args).unwrap();
    let id = tx.id();
    drop(tx);
    assert!(!args.repo.exists());
    git(&missing_git, &["init", "-q", "-b", "main"]);
    git(
        &missing_git,
        &["commit", "--allow-empty", "-qm", "now available"],
    );
    assert!(crate::repository_import_cmd::run(&args).is_err());
    args.acknowledge_stopped_import_processes = true;
    assert_eq!(crate::repository_import_cmd::run(&args).unwrap(), id);
}
#[test]
fn rejected_credentials_create_no_import_journal_and_changed_inputs_never_resume_one() {
    let root = tempfile::tempdir().unwrap();
    let mut args = fixture(root.path());
    args.admin_password = "wrong synthetic password".into();
    assert!(crate::repository_import_cmd::run(&args).is_err());
    assert!(!args.repo.exists());
    assert!(!fs::read_dir(root.path()).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".gitust-import-")));
    args.admin_password = "Fixture-password-123".into();
    let tx = ImportTransaction::open(&args).unwrap();
    let id = tx.id();
    drop(tx);
    args.name = "Different request".into();
    assert!(crate::repository_import_cmd::run(&args).is_err());
    args.name = "Imported".into();
    assert_eq!(crate::repository_import_cmd::run(&args).unwrap(), id);
}
