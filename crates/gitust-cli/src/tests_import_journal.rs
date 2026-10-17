//! Exercise durable boundary states using real filesystem operations, without production fault hooks.
use super::*;
use crate::tests_repository_import::{fixture, git};
fn registered_stage(args: &ImportArgs) -> (ImportTransaction, GitEngine) {
    let tx = ImportTransaction::open(args).unwrap();
    let engine = tx.prepare(args).unwrap();
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
            engine.complete_repository_registration(row.binding)?;
            Ok(())
        },
    )
    .unwrap();
    (tx, engine)
}
#[test]
fn process_interruption_after_directory_rename_resumes_exact_final_pair_without_refetch() {
    let root = tempfile::tempdir().unwrap();
    let args = fixture(root.path());
    let (tx, engine) = registered_stage(&args);
    let id = tx.id();
    let receipt = fs::read(
        tx.envelope
            .path
            .join("repository/gitust-repository/registration.json"),
    )
    .unwrap();
    let expected = git(&tx.envelope.path.join("repository"), &["rev-parse", "HEAD"]);
    // Stop at the real rename boundary before either directory fsync, as process death can do.
    tx.envelope
        .rename_exclusive("repository", &tx.parent, &tx.leaf)
        .unwrap();
    drop(engine);
    drop(tx);
    fs::write(
        Path::new(&args.source).join("hello.txt"),
        "upstream moved\n",
    )
    .unwrap();
    git(Path::new(&args.source), &["commit", "-qam", "new upstream"]);
    assert_eq!(run(&args).unwrap(), id);
    assert_eq!(
        fs::read(args.repo.join("gitust-repository/registration.json")).unwrap(),
        receipt
    );
    assert_eq!(git(&args.repo, &["rev-parse", "HEAD"]), expected);
}
#[test]
fn missing_prepared_record_or_replaced_stage_cannot_be_adopted_or_reinitialized() {
    let root = tempfile::tempdir().unwrap();
    let args = fixture(root.path());
    let (tx, engine) = registered_stage(&args);
    let id = tx.id();
    let ready = tx.envelope.path.join("prepared.json");
    let saved = fs::read(&ready).unwrap();
    fs::rename(&ready, tx.envelope.path.join("prepared.json.pending")).unwrap();
    drop(engine);
    drop(tx);
    assert!(run(&args).is_err());
    assert!(!args.repo.exists());
    let tx = ImportTransaction::open(&args).unwrap();
    assert_eq!(tx.id(), id);
    fs::remove_file(tx.envelope.path.join("prepared.json.pending")).unwrap();
    fs::write(&ready, saved).unwrap();
    fs::set_permissions(&ready, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    fs::rename(
        tx.envelope.path.join("repository"),
        tx.envelope.path.join("original-stage"),
    )
    .unwrap();
    fs::create_dir(tx.envelope.path.join("repository")).unwrap();
    fs::set_permissions(
        tx.envelope.path.join("repository"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    drop(tx);
    assert!(run(&args).is_err());
    assert!(!args.repo.exists());
}
#[test]
fn corrupt_symlinked_or_nonprivate_journals_are_never_repaired_automatically() {
    let root = tempfile::tempdir().unwrap();
    let args = fixture(root.path());
    let tx = ImportTransaction::open(&args).unwrap();
    let journal = tx.envelope.path.clone();
    drop(tx);
    let record = journal.join("intent.json");
    let original = fs::read(&record).unwrap();
    fs::write(&record, b"{torn").unwrap();
    assert!(run(&args).is_err());
    assert_eq!(fs::read(&record).unwrap(), b"{torn");
    fs::write(&record, &original).unwrap();
    fs::set_permissions(&record, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    assert!(run(&args).is_err());
    fs::remove_file(&record).unwrap();
    let unrelated = root.path().join("unrelated");
    fs::write(&unrelated, &original).unwrap();
    std::os::unix::fs::symlink(&unrelated, &record).unwrap();
    assert!(run(&args).is_err());
    assert!(record.is_symlink());
    assert!(!args.repo.exists());
}
