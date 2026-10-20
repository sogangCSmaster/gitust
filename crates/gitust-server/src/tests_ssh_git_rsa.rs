use super::{
    generate_ssh_key, git, init_bare_with_commit, install_host_key, register_repo, ssh_git_env,
};
use crate::{ssh_git, AppState};
use gitust_core::{RepoId, TeamId};
use gitust_db::AuthStore;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_rsa_sha2_256_and_512_clone_fetch_and_push_and_reject_unregistered_or_revoked() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let keys_dir = tmp.path().join("keys");
    fs::create_dir_all(&keys_dir).expect("keys dir");
    fs::set_permissions(&keys_dir, fs::Permissions::from_mode(0o700)).expect("chmod 700");

    let primary = init_bare_with_commit(tmp.path(), "rsa-primary", false);
    let store = AuthStore::open_persistent(tmp.path().join("auth.json")).expect("auth db");
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "StrongPassword123!")
        .expect("bootstrap");
    let admin_token = store
        .login("admin", "StrongPassword123!")
        .expect("login")
        .raw_session_token;
    let admin = store.authenticate_session(&admin_token).expect("admin");
    let primary_id = register_repo(&store, &admin, &primary, "rsa-primary", TeamId::new());

    let (rsa2048_path, rsa2048_pub) =
        generate_ssh_key(&keys_dir, "admin_rsa2048", "rsa", Some("2048"));
    let (rsa4096_path, rsa4096_pub) =
        generate_ssh_key(&keys_dir, "admin_rsa4096", "rsa", Some("4096"));
    let (unreg_rsa_path, _) = generate_ssh_key(&keys_dir, "unreg_rsa2048", "rsa", Some("2048"));

    let rsa2048_view = store
        .register_ssh_public_key(&admin, "rsa-2048", &rsa2048_pub)
        .expect("reg rsa 2048");
    store
        .register_ssh_public_key(&admin, "rsa-4096", &rsa4096_pub)
        .expect("reg rsa 4096");

    let known_hosts = install_host_key(&primary, &keys_dir);
    let state = AppState::with_auth_store(
        primary.clone(),
        "https://example.invalid/rsa-primary.git".into(),
        store.clone(),
    )
    .expect("state");
    let listener = ssh_git::Listener::bind("127.0.0.1:0".parse().expect("addr"), state)
        .await
        .expect("bind ssh");
    let ssh_port = listener.local_addr().expect("local_addr").port();
    let running = listener.start();

    let url = |id: RepoId| format!("ssh://git@127.0.0.1:{ssh_port}/{}.git", id.0);
    let rsa256_env = ssh_git_env(&rsa2048_path, &known_hosts, "rsa-sha2-256");
    let rsa512_env = ssh_git_env(&rsa4096_path, &known_hosts, "rsa-sha2-512");
    let unreg_env = ssh_git_env(&unreg_rsa_path, &known_hosts, "rsa-sha2-512,rsa-sha2-256");

    // 1. Unregistered RSA key must be rejected.
    assert!(
        !git(
            tmp.path(),
            &["ls-remote", "--refs", &url(primary_id)],
            Some(&unreg_env)
        )
        .status
        .success(),
        "unregistered RSA key must be rejected"
    );

    // 2. Registered 2048-bit RSA key authenticates via rsa-sha2-256 for clone & push.
    let clone_dir = tmp.path().join("clone-rsa");
    assert!(
        git(
            tmp.path(),
            &["clone", &url(primary_id), clone_dir.to_str().expect("path")],
            Some(&rsa256_env)
        )
        .status
        .success(),
        "RSA 2048 with rsa-sha2-256 clone must succeed"
    );
    fs::write(clone_dir.join("rsa256.txt"), "pushed via rsa-sha2-256\n").expect("write");
    assert!(git(&clone_dir, &["add", "rsa256.txt"], None)
        .status
        .success());
    assert!(git(
        &clone_dir,
        &["commit", "-q", "-m", "rsa-sha2-256 push"],
        None
    )
    .status
    .success());
    assert!(
        git(&clone_dir, &["push", "origin", "main"], Some(&rsa256_env))
            .status
            .success(),
        "RSA 2048 with rsa-sha2-256 push must succeed"
    );

    // 3. Registered 4096-bit RSA key authenticates via rsa-sha2-512 for fetch & push.
    fs::write(clone_dir.join("rsa512.txt"), "pushed via rsa-sha2-512\n").expect("write");
    assert!(git(&clone_dir, &["add", "rsa512.txt"], None)
        .status
        .success());
    assert!(git(
        &clone_dir,
        &["commit", "-q", "-m", "rsa-sha2-512 push"],
        None
    )
    .status
    .success());
    assert!(
        git(&clone_dir, &["push", "origin", "main"], Some(&rsa512_env))
            .status
            .success(),
        "RSA 4096 with rsa-sha2-512 push must succeed"
    );
    assert_eq!(
        git(&clone_dir, &["rev-parse", "HEAD"], None).stdout,
        git(&primary, &["rev-parse", "refs/heads/main"], None).stdout
    );

    // 4. Legacy SHA-1 `ssh-rsa` signature algorithm is rejected even for a valid 4096-bit RSA key.
    let legacy_sha1_env = ssh_git_env(&rsa4096_path, &known_hosts, "ssh-rsa");
    assert!(
        !git(
            tmp.path(),
            &["ls-remote", "--refs", &url(primary_id)],
            Some(&legacy_sha1_env)
        )
        .status
        .success(),
        "legacy SHA-1 ssh-rsa signature algorithm must be rejected"
    );

    // 5. Weak <2048-bit RSA key (1024-bit) is rejected at SSH handshake even if registered in AuthStore.
    let (rsa1024_path, rsa1024_pub) =
        generate_ssh_key(&keys_dir, "admin_rsa1024", "rsa", Some("1024"));
    store
        .register_ssh_public_key(&admin, "rsa-1024-weak", &rsa1024_pub)
        .expect("reg rsa 1024");
    let rsa1024_env = ssh_git_env(&rsa1024_path, &known_hosts, "rsa-sha2-512,rsa-sha2-256");
    assert!(
        !git(
            tmp.path(),
            &["ls-remote", "--refs", &url(primary_id)],
            Some(&rsa1024_env)
        )
        .status
        .success(),
        "weak 1024-bit RSA key must be rejected at SSH handshake (< 2048 bits)"
    );

    // 6. Revoked RSA key is immediately rejected.
    store
        .revoke_ssh_public_key(&admin, rsa2048_view.id)
        .expect("revoke rsa 2048");
    assert!(
        !git(
            tmp.path(),
            &["ls-remote", "--refs", &url(primary_id)],
            Some(&rsa256_env)
        )
        .status
        .success(),
        "revoked RSA key must be rejected"
    );

    // 7. Deactivating a user account immediately invalidates their active 2048-bit RSA SSH key.
    let invite = store
        .create_invite_link(
            &admin,
            "rsadev@company.internal",
            gitust_core::EnterpriseRole::Developer,
            vec![],
            None,
        )
        .expect("invite");
    let dev_user = store
        .redeem_invite_link(&invite.raw_token, "rsadev", "StrongPassword123!")
        .expect("redeem");
    let dev_token = store
        .login("rsadev", "StrongPassword123!")
        .expect("dev login")
        .raw_session_token;
    let dev_principal = store.authenticate_session(&dev_token).expect("dev auth");
    let (dev_rsa_path, dev_rsa_pub) =
        generate_ssh_key(&keys_dir, "dev_rsa2048", "rsa", Some("2048"));
    store
        .register_ssh_public_key(&dev_principal, "dev-rsa-2048", &dev_rsa_pub)
        .expect("reg dev rsa");
    store
        .deactivate_user(&admin, dev_user.id)
        .expect("deactivate dev");
    let dev_rsa_env = ssh_git_env(&dev_rsa_path, &known_hosts, "rsa-sha2-512,rsa-sha2-256");
    assert!(
        !git(
            tmp.path(),
            &["ls-remote", "--refs", &url(primary_id)],
            Some(&dev_rsa_env)
        )
        .status
        .success(),
        "deactivated user's RSA SSH key must be immediately rejected"
    );

    running.shutdown().await.expect("shutdown ssh");
}
