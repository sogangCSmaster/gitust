use crate::{
    automatic_checks, local_checks_automatic, repository_registry::RepositoryRegistry, ssh_git,
    tests::call, AppState,
};
use axum::http::StatusCode;
use gitust_core::{AgentKind, EnterpriseRole, RepoId, TeamId};
use gitust_db::AuthStore;
use gitust_git::GitEngine;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

fn git(cwd: &Path, args: &[&str], ssh_env: Option<&[(String, String)]>) -> std::process::Output {
    let mut cmd = crate::tests::fixture_git();
    cmd.arg("-C")
        .arg(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Gitust SSH Test")
        .env("GIT_AUTHOR_EMAIL", "ssh@company.internal")
        .env("GIT_COMMITTER_NAME", "Gitust SSH Test")
        .env("GIT_COMMITTER_EMAIL", "ssh@company.internal")
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args);
    if let Some(envs) = ssh_env {
        for (k, v) in envs {
            cmd.env(k, v);
        }
    }
    cmd.output().expect("git command")
}

fn generate_ssh_key(
    dir: &Path,
    name: &str,
    key_type: &str,
    bits: Option<&str>,
) -> (PathBuf, String) {
    let key_path = dir.join(name);
    let mut cmd = Command::new("ssh-keygen");
    cmd.args(["-q", "-t", key_type, "-N", "", "-C", "", "-f"])
        .arg(&key_path);
    if let Some(b) = bits {
        cmd.args(["-b", b]);
    }
    assert!(cmd.status().expect("ssh-keygen").success());
    fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).expect("chmod 600");
    let pub_key = fs::read_to_string(format!("{}.pub", key_path.display()))
        .expect("read pub")
        .trim()
        .to_owned();
    (key_path, pub_key)
}

fn install_host_key(common_git_dir: &Path, keys_dir: &Path) -> PathBuf {
    let host_dir = common_git_dir.join("gitust-ssh");
    fs::create_dir_all(&host_dir).expect("mkdir gitust-ssh");
    fs::set_permissions(&host_dir, fs::Permissions::from_mode(0o700)).expect("chmod 700");
    let (_, pub_key) = generate_ssh_key(&host_dir, "host_key", "ed25519", None);
    let known_hosts = keys_dir.join("known_hosts");
    let mut parts = pub_key.split_whitespace();
    let algo = parts.next().expect("algo");
    let blob = parts.next().expect("blob");
    fs::write(&known_hosts, format!("gitust-ssh-test {algo} {blob}\n")).expect("write known_hosts");
    fs::set_permissions(&known_hosts, fs::Permissions::from_mode(0o600)).expect("chmod 600");
    known_hosts
}

fn ssh_git_env(key_path: &Path, known_hosts: &Path, algo: &str) -> Vec<(String, String)> {
    let ssh_cmd = format!(
        "ssh -F /dev/null -i '{}' -o BatchMode=yes -o IdentitiesOnly=yes -o IdentityAgent=none \
         -o PreferredAuthentications=publickey -o PasswordAuthentication=no \
         -o KbdInteractiveAuthentication=no -o HostKeyAlgorithms=ssh-ed25519 \
         -o PubkeyAcceptedAlgorithms={} -o StrictHostKeyChecking=yes \
         -o UserKnownHostsFile='{}' -o GlobalKnownHostsFile=/dev/null \
         -o HostKeyAlias=gitust-ssh-test -o UpdateHostKeys=no -o VerifyHostKeyDNS=no \
         -o ProxyCommand=none -o ProxyJump=none -o ClearAllForwardings=yes \
         -o ConnectionAttempts=1 -o ConnectTimeout=5",
        key_path.display(),
        algo,
        known_hosts.display()
    );
    vec![
        ("GIT_SSH_COMMAND".into(), ssh_cmd),
        ("GIT_SSH_VARIANT".into(), "ssh".into()),
        ("SSH_AUTH_SOCK".into(), String::new()),
        ("SSH_ASKPASS".into(), "/usr/bin/false".into()),
        ("GIT_ALLOW_PROTOCOL".into(), "ssh:file".into()),
    ]
}

fn init_bare_with_commit(root: &Path, name: &str, with_checks: bool) -> PathBuf {
    let seed = root.join(format!("{name}-seed"));
    fs::create_dir_all(&seed).expect("seed dir");
    assert!(git(&seed, &["init", "-q", "-b", "main"], None)
        .status
        .success());
    fs::write(seed.join("README.md"), format!("{name}\n")).expect("write README");
    if with_checks {
        fs::write(
            seed.join(".gitust-checks.json"),
            r#"{"version":1,"auto_run":true,"commands":["true"]}"#,
        )
        .expect("write checks");
    }
    assert!(git(&seed, &["add", "."], None).status.success());
    assert!(git(&seed, &["commit", "-q", "-m", "initial"], None)
        .status
        .success());
    let bare = root.join(format!("{name}.git"));
    assert!(git(
        root,
        &[
            "clone",
            "--bare",
            "--no-local",
            seed.to_str().expect("seed"),
            bare.to_str().expect("bare"),
        ],
        None,
    )
    .status
    .success());
    bare
}

fn register_repo(
    store: &AuthStore,
    admin: &gitust_core::AuthenticatedPrincipal,
    path: &Path,
    name: &str,
    team: TeamId,
) -> RepoId {
    let engine = GitEngine::new(path);
    let id = engine.load_or_create_repository_id().expect("repo id");
    let reg = store
        .register_local_repository(admin, id, name, team, true, None)
        .expect("register repo");
    engine
        .complete_repository_registration(reg.binding)
        .expect("complete registration");
    id
}

#[tokio::test]
async fn catalog_exposes_ssh_port_only_when_configured_and_nonzero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = init_bare_with_commit(tmp.path(), "catalog", false);
    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "StrongPassword123!")
        .expect("bootstrap");
    let token = store
        .login("admin", "StrongPassword123!")
        .expect("login")
        .raw_session_token;
    let base_state =
        AppState::with_auth_store(repo, "https://example.invalid/repo.git".into(), store)
            .expect("state");

    for (configured, expected) in [(None, None), (Some(0), None), (Some(2222), Some(2222u64))] {
        let app = crate::build_router(base_state.clone().with_ssh_port(configured));
        let (status, body) = call(&app, "GET", "/api/v1/repos".into(), &token, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("json");
        let entry = &parsed["repositories"][0];
        assert_eq!(entry.get("ssh_port").and_then(|v| v.as_u64()), expected);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_receive_pack_ed25519_and_ecdsa_push_primary_and_git_write_secondary_and_reject_readonly(
) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let keys_dir = tmp.path().join("keys");
    fs::create_dir_all(&keys_dir).expect("keys dir");
    fs::set_permissions(&keys_dir, fs::Permissions::from_mode(0o700)).expect("chmod 700");

    let primary = init_bare_with_commit(tmp.path(), "primary", false);
    let writable = init_bare_with_commit(tmp.path(), "writable", false);
    let readonly = init_bare_with_commit(tmp.path(), "readonly", false);

    let store = AuthStore::open_persistent(tmp.path().join("auth.json")).expect("auth db");
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "StrongPassword123!")
        .expect("bootstrap");
    let admin_token = store
        .login("admin", "StrongPassword123!")
        .expect("login")
        .raw_session_token;
    let admin = store.authenticate_session(&admin_token).expect("admin");
    let team = TeamId::new();

    let primary_id = register_repo(&store, &admin, &primary, "primary", team);
    let writable_id = register_repo(&store, &admin, &writable, "writable", team);
    let readonly_id = register_repo(&store, &admin, &readonly, "readonly", team);

    let invite = store
        .create_invite_link(
            &admin,
            "dev@company.internal",
            EnterpriseRole::Developer,
            vec![team],
            None,
        )
        .expect("invite");
    store
        .redeem_invite_link(&invite.raw_token, "devuser", "StrongPassword123!")
        .expect("redeem");
    let dev_token = store
        .login("devuser", "StrongPassword123!")
        .expect("dev login")
        .raw_session_token;
    let dev = store.authenticate_session(&dev_token).expect("dev");

    let (admin_ed_path, admin_ed_pub) = generate_ssh_key(&keys_dir, "admin_ed", "ed25519", None);
    let (admin_p256_path, admin_p256_pub) =
        generate_ssh_key(&keys_dir, "admin_p256", "ecdsa", Some("256"));
    let (admin_p384_path, admin_p384_pub) =
        generate_ssh_key(&keys_dir, "admin_p384", "ecdsa", Some("384"));
    let (admin_p521_path, admin_p521_pub) =
        generate_ssh_key(&keys_dir, "admin_p521", "ecdsa", Some("521"));
    let (dev_ed_path, dev_ed_pub) = generate_ssh_key(&keys_dir, "dev_ed", "ed25519", None);

    store
        .register_ssh_public_key(&admin, "ed25519", &admin_ed_pub)
        .expect("reg admin ed");
    store
        .register_ssh_public_key(&admin, "p256", &admin_p256_pub)
        .expect("reg admin p256");
    store
        .register_ssh_public_key(&admin, "p384", &admin_p384_pub)
        .expect("reg admin p384");
    store
        .register_ssh_public_key(&admin, "p521", &admin_p521_pub)
        .expect("reg admin p521");
    store
        .register_ssh_public_key(&dev, "dev-ed", &dev_ed_pub)
        .expect("reg dev ed");

    let known_hosts = install_host_key(&primary, &keys_dir);
    let mut state = AppState::with_auth_store(
        primary.clone(),
        "https://example.invalid/primary.git".into(),
        store,
    )
    .expect("state");
    state.repository_registry = Some(
        RepositoryRegistry::load_with_git_write(
            &state,
            vec![readonly.clone()],
            vec![writable.clone()],
        )
        .expect("registry"),
    );
    let listener = ssh_git::Listener::bind("127.0.0.1:0".parse().expect("addr"), state.clone())
        .await
        .expect("bind ssh");
    let ssh_port = listener.local_addr().expect("local_addr").port();
    let running = listener.start();

    let url = |id: RepoId| format!("ssh://git@127.0.0.1:{ssh_port}/{}.git", id.0);
    let admin_ed_env = ssh_git_env(&admin_ed_path, &known_hosts, "ssh-ed25519");
    let admin_p256_env = ssh_git_env(&admin_p256_path, &known_hosts, "ecdsa-sha2-nistp256");
    let admin_p384_env = ssh_git_env(&admin_p384_path, &known_hosts, "ecdsa-sha2-nistp384");
    let admin_p521_env = ssh_git_env(&admin_p521_path, &known_hosts, "ecdsa-sha2-nistp521");
    let dev_ed_env = ssh_git_env(&dev_ed_path, &known_hosts, "ssh-ed25519");

    // Verify P-384 and P-521 ECDSA user keys authenticate over SSH.
    for env in [&admin_p384_env, &admin_p521_env] {
        assert!(git(
            tmp.path(),
            &["ls-remote", "--refs", &url(primary_id)],
            Some(env)
        )
        .status
        .success());
    }

    // Clone primary over ECDSA P-256 and push a commit to main + annotated tag.
    let clone_primary = tmp.path().join("clone-primary");
    assert!(git(
        tmp.path(),
        &[
            "clone",
            &url(primary_id),
            clone_primary.to_str().expect("path")
        ],
        Some(&admin_p256_env),
    )
    .status
    .success());
    fs::write(clone_primary.join("ecdsa.txt"), "pushed with ecdsa-p256\n").expect("write");
    assert!(git(&clone_primary, &["add", "ecdsa.txt"], None)
        .status
        .success());
    assert!(
        git(&clone_primary, &["commit", "-q", "-m", "ecdsa push"], None)
            .status
            .success()
    );
    assert!(git(
        &clone_primary,
        &["tag", "-a", "v1.0-ssh", "-m", "annotated ssh tag"],
        None
    )
    .status
    .success());
    assert!(git(
        &clone_primary,
        &["push", "origin", "main", "refs/tags/v1.0-ssh"],
        Some(&admin_p256_env),
    )
    .status
    .success());
    assert_eq!(
        git(&clone_primary, &["rev-parse", "HEAD"], None).stdout,
        git(&primary, &["rev-parse", "refs/heads/main"], None).stdout
    );

    // Clone writable secondary over Ed25519 and push to main; reject agent/* ref on secondary.
    let clone_writable = tmp.path().join("clone-writable");
    assert!(git(
        tmp.path(),
        &[
            "clone",
            &url(writable_id),
            clone_writable.to_str().expect("path")
        ],
        Some(&admin_ed_env),
    )
    .status
    .success());
    fs::write(clone_writable.join("sec.txt"), "secondary ssh write\n").expect("write");
    assert!(git(&clone_writable, &["add", "sec.txt"], None)
        .status
        .success());
    assert!(
        git(&clone_writable, &["commit", "-q", "-m", "sec push"], None)
            .status
            .success()
    );
    assert!(git(
        &clone_writable,
        &["push", "origin", "main"],
        Some(&admin_ed_env)
    )
    .status
    .success());
    assert!(!git(
        &clone_writable,
        &["push", "origin", "HEAD:refs/heads/agent/denied"],
        Some(&admin_ed_env),
    )
    .status
    .success());
    assert!(!git(
        &clone_writable,
        &["push", "origin", "HEAD:refs/heads/dev-denied"],
        Some(&dev_ed_env),
    )
    .status
    .success());

    // Read-only secondary must reject git-receive-pack even for SiteAdmin.
    let ro_before = git(&readonly, &["for-each-ref"], None).stdout;
    assert!(!git(
        &clone_primary,
        &["push", &url(readonly_id), "HEAD:refs/heads/main"],
        Some(&admin_ed_env),
    )
    .status
    .success());
    assert_eq!(git(&readonly, &["for-each-ref"], None).stdout, ro_before);

    // Developer RBAC on primary: allowed on new/owned agent/* branch, denied on non-agent branch or deletion.
    assert!(!git(
        &clone_primary,
        &["push", "origin", "HEAD:refs/heads/protected-branch"],
        Some(&dev_ed_env),
    )
    .status
    .success());
    assert!(git(
        &clone_primary,
        &["push", "origin", "HEAD:refs/heads/agent/dev-task"],
        Some(&dev_ed_env),
    )
    .status
    .success());
    assert!(!git(
        &clone_primary,
        &["push", "origin", ":refs/heads/agent/dev-task"],
        Some(&dev_ed_env),
    )
    .status
    .success());

    running.shutdown().await.expect("shutdown ssh");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_receive_pack_rejects_mid_stream_key_revocation_and_triggers_automatic_checks() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let keys_dir = tmp.path().join("keys");
    fs::create_dir_all(&keys_dir).expect("keys dir");
    fs::set_permissions(&keys_dir, fs::Permissions::from_mode(0o700)).expect("chmod 700");

    let _ = init_bare_with_commit(tmp.path(), "auto-primary", true);
    let primary = tmp.path().join("auto-primary-seed");
    let store = AuthStore::open_persistent(tmp.path().join("auth.json")).expect("auth db");
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "StrongPassword123!")
        .expect("bootstrap");
    let admin_token = store
        .login("admin", "StrongPassword123!")
        .expect("login")
        .raw_session_token;
    let admin = store.authenticate_session(&admin_token).expect("admin");
    let primary_id = register_repo(&store, &admin, &primary, "auto-primary", TeamId::new());

    let (revoke_path, revoke_pub) = generate_ssh_key(&keys_dir, "revoke_ed", "ed25519", None);
    let revoked_view = store
        .register_ssh_public_key(&admin, "revoke-me", &revoke_pub)
        .expect("register revoke key");
    let (admin_path, admin_pub) = generate_ssh_key(&keys_dir, "keep_ed", "ed25519", None);
    store
        .register_ssh_public_key(&admin, "keep-me", &admin_pub)
        .expect("register keep key");

    let known_hosts = install_host_key(&primary.join(".git"), &keys_dir);
    let state = AppState::with_auth_store(
        primary.clone(),
        "https://example.invalid/primary.git".into(),
        store.clone(),
    )
    .expect("state")
    .with_trusted_sandbox_exec(true);
    let auto_svc = automatic_checks::start(&state).await.expect("start auto");
    let local_svc = local_checks_automatic::start(&state)
        .await
        .expect("start local auto");
    let app = crate::build_router(state.clone());

    let (status, wt_body) = call(
        &app,
        "POST",
        "/api/v1/swarm/worktrees".into(),
        &admin_token,
        Some(serde_json::json!({
            "agent_kind": AgentKind::ClaudeCode,
            "branch": "agent/ssh-auto",
            "prompt_summary": "ssh auto checks"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{wt_body}");
    let wt: gitust_core::AgentWorktree = serde_json::from_str(&wt_body).expect("wt json");
    let before_main =
        String::from_utf8(git(&primary, &["rev-parse", "refs/heads/main"], None).stdout)
            .expect("utf8")
            .trim()
            .to_owned();
    crate::state::set_worktree_status(&state, wt.id, |w| {
        w.status = gitust_core::WorktreeStatus::Verified;
        w.verified_commit = Some(gitust_core::CommitSha(before_main.clone()));
    });

    let listener = ssh_git::Listener::bind("127.0.0.1:0".parse().expect("addr"), state.clone())
        .await
        .expect("bind ssh");
    let ssh_port = listener.local_addr().expect("local_addr").port();
    let running = listener.start();
    let remote = format!("ssh://git@127.0.0.1:{ssh_port}/{}.git", primary_id.0);

    let revoke_env = ssh_git_env(&revoke_path, &known_hosts, "ssh-ed25519");
    let admin_env = ssh_git_env(&admin_path, &known_hosts, "ssh-ed25519");

    let clone_dir = tmp.path().join("clone-auto");
    assert!(git(
        tmp.path(),
        &["clone", &remote, clone_dir.to_str().expect("path")],
        Some(&admin_env),
    )
    .status
    .success());
    fs::write(clone_dir.join("change.txt"), "trigger checks\n").expect("write");
    assert!(git(&clone_dir, &["add", "change.txt"], None)
        .status
        .success());
    assert!(
        git(&clone_dir, &["commit", "-q", "-m", "ssh trigger"], None)
            .status
            .success()
    );

    // 1. Mid-stream key revocation inside repository_read_hook must abort push, reject intents, and preserve Verified status.
    let hook_store = store.clone();
    let hook_admin = admin.clone();
    let revoke_id = revoked_view.id;
    *state.repository_read_hook.lock() = Some(Arc::new(move || {
        hook_store
            .revoke_ssh_public_key(&hook_admin, revoke_id)
            .expect("revoke key in hook");
    }));
    assert!(!git(
        &clone_dir,
        &[
            "push",
            "origin",
            "HEAD:refs/heads/agent/ssh-auto",
            "HEAD:refs/heads/main"
        ],
        Some(&revoke_env),
    )
    .status
    .success());
    assert_eq!(
        String::from_utf8(git(&primary, &["rev-parse", "refs/heads/main"], None).stdout)
            .expect("utf8")
            .trim(),
        before_main
    );
    let wt_after_deny = state.worktrees.read()[0].clone();
    assert_eq!(wt_after_deny.status, gitust_core::WorktreeStatus::Verified);
    assert_eq!(
        wt_after_deny.verified_commit.as_ref().map(|c| c.0.as_str()),
        Some(before_main.as_str())
    );

    // 2. Authorized SSH push to agent/ssh-auto and main schedules automatic Checks.
    assert!(git(
        &clone_dir,
        &[
            "push",
            "origin",
            "HEAD:refs/heads/agent/ssh-auto",
            "HEAD:refs/heads/main"
        ],
        Some(&admin_env),
    )
    .status
    .success());

    let mut saw_local = false;
    let mut saw_worktree = false;
    for _ in 0..200 {
        let (_, body) = call(
            &app,
            "GET",
            format!("/api/v1/repos/{}/local-check-automatic-runs", primary_id.0),
            &admin_token,
            None,
        )
        .await;
        if body.contains("\"phase\":\"completed\"") {
            saw_local = true;
        }
        let (_, wt_runs) = call(
            &app,
            "GET",
            "/api/v1/repo/automatic-runs".into(),
            &admin_token,
            None,
        )
        .await;
        if wt_runs.contains("\"phase\":\"completed\"") {
            saw_worktree = true;
        }
        if saw_local && saw_worktree {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        saw_local,
        "expected local automatic Checks run to complete after SSH push to main"
    );
    assert!(
        saw_worktree,
        "expected worktree automatic Checks run to complete after SSH push to agent/ssh-auto"
    );

    running.shutdown().await.expect("shutdown ssh");
    local_svc.shutdown().await.expect("shutdown local");
    auto_svc.shutdown().await.expect("shutdown auto");
}

#[path = "tests_ssh_git_rsa.rs"]
mod tests_ssh_git_rsa;
