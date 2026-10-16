//! Regression tests for cross-process `AuthStore` persistence races.

use std::fs;
use std::sync::mpsc;
use std::time::Duration;

use gitust_core::{AuthError, EnterpriseRole};

use crate::persistence::acquire_advisory_lock;
use crate::AuthStore;

/// Red-team P1: `open_persistent()` used to read the snapshot and the `DiskStamp` separately and
/// without the advisory lock. If another process atomically renamed a newer snapshot (e.g. one
/// that deactivates a user) in between, the store paired stale content with a fresh stamp,
/// skipped every later sync, and revoked sessions authenticated again (and were re-persisted).
#[test]
fn gate_open_persistent_startup_race_cannot_resurrect_revoked_session() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let db_path = temp_dir.path().join("gitust-auth.json");
    let stale_path = temp_dir.path().join("stale.json");
    let revoked_path = temp_dir.path().join("revoked.json");

    let writer = AuthStore::open_persistent(&db_path).expect("open writer store");
    writer
        .bootstrap_initial_admin_cli("admin1", "admin1@company.internal", "Admin1StrongPass!2026")
        .expect("bootstrap admin");
    let admin_login = writer
        .login("admin1", "Admin1StrongPass!2026")
        .expect("login admin");
    let admin = writer
        .authenticate_session(&admin_login.raw_session_token)
        .expect("auth admin");
    let inv = writer
        .create_invite_link(
            &admin,
            "dev1@company.internal",
            EnterpriseRole::Developer,
            vec![],
            None,
        )
        .expect("create invite");
    let dev = writer
        .redeem_invite_link(&inv.raw_token, "dev1", "Dev1StrongPass!2026")
        .expect("redeem invite");
    let dev_session = writer
        .login("dev1", "Dev1StrongPass!2026")
        .expect("login dev1");

    // Snapshot where dev1's session is still valid.
    fs::copy(&db_path, &stale_path).expect("copy stale snapshot");
    writer
        .deactivate_user(&admin, dev.id)
        .expect("deactivate dev1");
    // Snapshot where dev1 is deactivated and the session revoked.
    fs::copy(&db_path, &revoked_path).expect("copy revoked snapshot");
    fs::rename(&stale_path, &db_path).expect("restore stale snapshot");

    // Simulate another process in the middle of a locked write.
    let held_lock = acquire_advisory_lock(&db_path).expect("hold advisory lock");
    let (tx, rx) = mpsc::channel();
    let open_path = db_path.clone();
    let opener = std::thread::spawn(move || {
        let res = AuthStore::open_persistent(&open_path);
        let _ = tx.send(());
        res
    });

    // open_persistent must wait for the lock instead of reading mid-write.
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "open_persistent read the auth db without acquiring the advisory lock"
    );

    fs::rename(&revoked_path, &db_path).expect("atomic rename revoked snapshot");
    drop(held_lock);

    let reader = opener
        .join()
        .expect("opener thread")
        .expect("open reader store");
    assert_eq!(
        reader.authenticate_session(&dev_session.raw_session_token),
        Err(AuthError::AccountDeactivated)
    );

    // A subsequent write must not re-persist the revoked session to disk.
    reader
        .create_personal_access_token(&admin, "ci-token", None)
        .expect("create admin PAT");
    let reopened = AuthStore::open_persistent(&db_path).expect("reopen store");
    assert_eq!(
        reopened.authenticate_session(&dev_session.raw_session_token),
        Err(AuthError::AccountDeactivated)
    );
}

/// Red-team P1 #2: lock acquisition failure used to be swallowed (`.ok()` / "no change"),
/// so a CLI revocation could report success while the server kept accepting the old session.
/// Both sides must now fail closed with `StorageError`.
#[test]
fn gate_lock_failure_fails_closed_for_reads_and_writes() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let db_path = temp_dir.path().join("gitust-auth.json");
    let lock_path = temp_dir.path().join("gitust-auth.json.lock");

    let server = AuthStore::open_persistent(&db_path).expect("open server store");
    server
        .bootstrap_initial_admin_cli("admin1", "admin1@company.internal", "Admin1StrongPass!2026")
        .expect("bootstrap admin");
    let login = server
        .login("admin1", "Admin1StrongPass!2026")
        .expect("login admin");
    let admin = server
        .authenticate_session(&login.raw_session_token)
        .expect("auth admin");
    let cli = AuthStore::open_persistent(&db_path).expect("open cli store");

    // Make the lockfile un-openable for writing.
    fs::remove_file(&lock_path).expect("remove lockfile");
    fs::create_dir(&lock_path).expect("replace lockfile with directory");

    // Writes must not report success without the cross-process lock.
    assert!(matches!(
        cli.revoke_session(&admin, &login.raw_session_token),
        Err(AuthError::StorageError(_))
    ));

    // Simulate an external change landing on disk; the reader must fail closed rather than
    // treating the lock failure as "no change" and serving stale auth state.
    let mut bytes = fs::read(&db_path).expect("read db");
    bytes.extend_from_slice(b"\n");
    let tmp = temp_dir.path().join("replace.tmp");
    fs::write(&tmp, &bytes).expect("write replacement");
    fs::rename(&tmp, &db_path).expect("atomic replace db");
    assert!(matches!(
        server.authenticate_session(&login.raw_session_token),
        Err(AuthError::StorageError(_))
    ));
}

/// A DB file that was observed and then vanished must not be treated as "unchanged".
#[test]
fn gate_vanished_db_fails_closed() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let db_path = temp_dir.path().join("gitust-auth.json");

    let server = AuthStore::open_persistent(&db_path).expect("open server store");
    server
        .bootstrap_initial_admin_cli("admin1", "admin1@company.internal", "Admin1StrongPass!2026")
        .expect("bootstrap admin");
    let login = server
        .login("admin1", "Admin1StrongPass!2026")
        .expect("login admin");

    fs::remove_file(&db_path).expect("delete db");
    assert!(matches!(
        server.authenticate_session(&login.raw_session_token),
        Err(AuthError::StorageError(_))
    ));
    assert!(matches!(
        server.login("admin1", "Admin1StrongPass!2026"),
        Err(AuthError::StorageError(_))
    ));
}

fn bootstrap_admin(db_path: &std::path::Path) -> (AuthStore, gitust_core::AuthenticatedPrincipal) {
    let store = AuthStore::open_persistent(db_path).expect("open store");
    store
        .bootstrap_initial_admin_cli("admin1", "admin1@company.internal", "Admin1StrongPass!2026")
        .expect("bootstrap admin");
    let login = store
        .login("admin1", "Admin1StrongPass!2026")
        .expect("login admin");
    let admin = store
        .authenticate_session(&login.raw_session_token)
        .expect("auth admin");
    (store, admin)
}

/// Gate B P1 (2026-10-16): a lock held by another process must not block callers forever
/// (it froze async workers). Acquisition is bounded and fails closed with `StorageError`.
#[test]
fn gate_foreign_lock_holder_times_out_fail_closed() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let db_path = temp_dir.path().join("gitust-auth.json");
    let (store, admin) = bootstrap_admin(&db_path);

    let held = acquire_advisory_lock(&db_path).expect("hold lock");
    let start = std::time::Instant::now();
    let res = store.create_personal_access_token(&admin, "blocked", None);
    let waited = start.elapsed();
    drop(held);

    assert!(matches!(res, Err(AuthError::StorageError(_))), "{res:?}");
    assert!(
        waited < crate::disk_lock::LOCK_WAIT_DEADLINE + Duration::from_secs(2),
        "lock wait was not bounded: {waited:?}"
    );
    // Works again once the holder releases.
    store
        .create_personal_access_token(&admin, "after", None)
        .expect("create PAT after release");
}

/// Gate B P1: other local users must not be able to open (and hold) the lockfile.
#[cfg(unix)]
#[test]
fn gate_lockfile_is_private_and_legacy_lockfiles_are_tightened() {
    use std::os::unix::fs::PermissionsExt;
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let db_path = temp_dir.path().join("sub").join("gitust-auth.json");
    let lock_path = temp_dir.path().join("sub").join("gitust-auth.json.lock");
    let (store, admin) = bootstrap_admin(&db_path);

    let mode = |p: &std::path::Path| fs::metadata(p).expect("stat").permissions().mode() & 0o777;
    assert_eq!(mode(&lock_path), 0o600);
    assert_eq!(mode(&temp_dir.path().join("sub")), 0o700);

    fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o644)).expect("loosen");
    store
        .create_personal_access_token(&admin, "tighten", None)
        .expect("write");
    assert_eq!(mode(&lock_path), 0o600);
}

/// Gate B P2: a sparse / oversized DB must be rejected before allocating or reading it.
#[test]
fn gate_oversized_auth_db_is_rejected_without_reading() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let db_path = temp_dir.path().join("gitust-auth.json");
    let file = fs::File::create(&db_path).expect("create db");
    file.set_len(crate::persistence::MAX_AUTH_DB_BYTES + 1)
        .expect("sparse");
    drop(file);
    let start = std::time::Instant::now();
    assert!(matches!(
        AuthStore::open_persistent(&db_path),
        Err(AuthError::StorageError(_))
    ));
    assert!(start.elapsed() < Duration::from_secs(2));
}

/// Gate A/B P2: a mutation whose persist failed must be rolled back in memory, so it is neither
/// visible only in this process nor silently persisted by a later unrelated write.
#[cfg(unix)]
#[test]
fn gate_failed_persist_rolls_back_in_memory_mutation() {
    use std::os::unix::fs::PermissionsExt;
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let dir = temp_dir.path().join("data");
    let db_path = dir.join("gitust-auth.json");
    let (store, admin) = bootstrap_admin(&db_path);
    let pat = store
        .create_personal_access_token(&admin, "ci", None)
        .expect("create PAT");

    // Make temp-file creation fail.
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).expect("read-only dir");
    let revoke = store.revoke_personal_access_token(&admin, pat.metadata.id);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).expect("restore dir");
    assert!(
        matches!(revoke, Err(AuthError::StorageError(_))),
        "{revoke:?}"
    );

    // In-memory state matches disk: the failed revoke is not applied here...
    store
        .authenticate_pat(&pat.raw_token)
        .expect("failed revoke must be rolled back in memory");
    // ...and a later unrelated write does not persist it either.
    store
        .create_personal_access_token(&admin, "other", None)
        .expect("unrelated write");
    let reopened = AuthStore::open_persistent(&db_path).expect("reopen");
    reopened
        .authenticate_pat(&pat.raw_token)
        .expect("failed revoke must not be persisted later");
}

/// Gate A R2 P1: if `write_snapshot_atomically` fails AND the existing DB file cannot be read
/// (e.g. `chmod 0000` on the DB file + `0555` on the directory), rollback must still succeed in
/// memory without leaving the uncommitted mutation live, and `DiskStamp::of_path` must fail closed
/// with `StorageError` whenever `stat(2)` fails with `PermissionDenied`.
#[cfg(unix)]
#[test]
fn gate_double_io_failure_rolls_back_in_memory_and_fails_closed() {
    use std::os::unix::fs::PermissionsExt;
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let dir = temp_dir.path().join("data");
    let db_path = dir.join("gitust-auth.json");
    let (store, admin) = bootstrap_admin(&db_path);

    // Make the DB file unreadable (0000) AND the parent directory read-only (0555) so both
    // temp-file creation and any disk re-read fail.
    fs::set_permissions(&db_path, fs::Permissions::from_mode(0o000)).expect("chmod 000 db");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).expect("chmod 555 dir");
    let attempted = store.create_personal_access_token(&admin, "uncommitted-pat", None);
    assert!(
        matches!(attempted, Err(AuthError::StorageError(_))),
        "{attempted:?}"
    );

    // If the directory becomes un-traversable (0000), `DiskStamp::of_path` must return
    // `Err(StorageError)` rather than `None` (which used to compare equal to a cleared stamp!).
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).expect("chmod 000 dir");
    assert!(matches!(
        store.list_personal_access_tokens(&admin),
        Err(AuthError::StorageError(_))
    ));

    // Restore permissions and verify the failed PAT creation was rolled back in memory.
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).expect("restore dir");
    fs::set_permissions(&db_path, fs::Permissions::from_mode(0o600)).expect("restore db");
    let pats = store
        .list_personal_access_tokens(&admin)
        .expect("list PATs after recovery");
    assert!(
        pats.is_empty(),
        "uncommitted PAT remained in memory after double I/O failure: {pats:?}"
    );
}
