use super::tests_git_http_secondary::{git, oracle, request};
use super::*;
use crate::repository_registry::RepositoryRegistry;

pub(super) fn fixture() -> (tempfile::TempDir, AppState, String, PathBuf, RepoId, RepoId) {
    let (temp, mut state, session, readonly, readonly_id) =
        super::tests_git_http_secondary::fixture();
    let bare = temp.path().join("hosted.git");
    assert!(git(
        temp.path(),
        &[
            "clone",
            "--bare",
            "--no-local",
            readonly.to_str().unwrap(),
            bare.to_str().unwrap()
        ],
        None
    )
    .status
    .success());
    state.repository_registry = None;
    let admin = state.auth_store.authenticate_session(&session).unwrap();
    let team = state
        .auth_store
        .authorize_repository_access(&admin, readonly_id)
        .unwrap()
        .owner_team_id;
    let engine = GitEngine::new(&bare);
    let id = engine.load_or_create_repository_id().unwrap();
    let row = state
        .auth_store
        .register_local_repository(&admin, id, "hosted", team, true, None)
        .unwrap();
    engine
        .complete_repository_registration(row.binding)
        .unwrap();
    drop(engine);
    state.repository_registry = Some(
        RepositoryRegistry::load_with_git_write(&state, vec![readonly], vec![bare.clone()])
            .unwrap(),
    );
    (temp, state, session, bare, id, readonly_id)
}
pub(super) fn route(id: RepoId, post: bool) -> String {
    format!(
        "/git/{}.git/{}",
        id.0,
        if post {
            "git-receive-pack"
        } else {
            "info/refs?service=git-receive-pack"
        }
    )
}
pub(super) fn body(state: &AppState, id: RepoId, names: &[&str]) -> Vec<u8> {
    let sha = state
        .read_repository(id)
        .unwrap()
        .engine
        .inspect_head_identity()
        .unwrap()
        .head_sha
        .0;
    let mut body = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let capabilities = if index == 0 { "\0report-status" } else { "" };
        let command = format!("{} {sha} {name}{capabilities}\n", "0".repeat(40));
        body.extend_from_slice(format!("{:04x}{command}", command.len() + 4).as_bytes());
    }
    body.extend_from_slice(b"0000PACK\0\0\0\x02\0\0\0\0");
    body.extend_from_slice(&[
        0x02, 0x9d, 0x08, 0x82, 0x3b, 0xd8, 0xa8, 0xea, 0xb5, 0x10, 0xad, 0x6a, 0xc7, 0x5c, 0x82,
        0x3c, 0xfd, 0x3e, 0xd3, 0x1e,
    ]);
    body
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_secondary_native_push_fetch_preserves_primary_and_read_only_services() {
    let (temp, state, session, bare, id, readonly) = fixture();
    let primary = state.git_engine.repo_path().to_owned();
    let refs = ["for-each-ref", "--format=%(refname)%00%(objectname)"];
    let primary_before = oracle(&primary, &refs);
    let readonly_path = state
        .read_repository(readonly)
        .unwrap()
        .engine
        .repo_path()
        .to_owned();
    let readonly_before = oracle(&readonly_path, &refs);
    let common = state
        .read_repository(id)
        .unwrap()
        .engine
        .repository_common_directory()
        .unwrap();
    let anchors: Vec<_> = ["identity.json", "registration.json"]
        .iter()
        .map(|name| std::fs::read(common.join("gitust-repository").join(name)).unwrap())
        .collect();
    let app = build_router(state.clone());
    for (target, status) in [(id, StatusCode::OK), (readonly, StatusCode::NOT_FOUND)] {
        assert_eq!(
            request(&app, "GET", route(target, false), &session, Body::empty())
                .await
                .status(),
            status
        );
    }
    let (_, catalog) = call(&app, "GET", "/api/v1/repos".into(), &session, None).await;
    let rows: serde_json::Value = serde_json::from_str(&catalog).unwrap();
    let enabled = rows["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id.0.to_string())
        .unwrap();
    assert!(enabled["can_push"] == true && enabled["capability"] == "browse");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    struct Stop(tokio::task::JoinHandle<()>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _server = Stop(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap()
    }));
    let remote = format!("http://{address}/git/{}.git", id.0);
    let client = temp.path().join("writer");
    assert!(git(
        temp.path(),
        &["clone", &remote, client.to_str().unwrap()],
        Some(&session)
    )
    .status
    .success());
    std::fs::write(client.join("new.txt"), "ordinary secondary content\n").unwrap();
    oracle(&client, &["add", "new.txt"]);
    oracle(&client, &["commit", "-m", "ordinary secondary commit"]);
    oracle(
        &client,
        &[
            "tag",
            "-a",
            "ordinary-proof",
            "-m",
            "preserve annotated tag object",
        ],
    );
    assert!(
        git(
            &client,
            &["push", "origin", "main", "refs/tags/ordinary-proof"],
            Some(&session)
        )
        .status
        .success(),
        "ordinary push failed"
    );
    assert_eq!(
        oracle(&client, &["rev-parse", "HEAD"]),
        oracle(&bare, &["rev-parse", "HEAD"])
    );
    assert_eq!(
        oracle(&client, &["rev-parse", "refs/tags/ordinary-proof"]),
        oracle(&bare, &["rev-parse", "refs/tags/ordinary-proof"])
    );
    assert_eq!(
        oracle(&bare, &["cat-file", "-t", "refs/tags/ordinary-proof"]),
        b"tag\n"
    );
    let fetch = temp.path().join("reader.git");
    assert!(git(
        temp.path(),
        &["clone", "--bare", &remote, fetch.to_str().unwrap()],
        Some(&session)
    )
    .status
    .success());
    assert!(git(&fetch, &["fetch", "origin"], Some(&session))
        .status
        .success());
    assert_eq!(oracle(&fetch, &refs), oracle(&bare, &refs));
    let before = oracle(&bare, &refs);
    assert!(!git(
        &client,
        &["push", "origin", "HEAD:refs/heads/agent/denied"],
        Some(&session)
    )
    .status
    .success());
    let api = build_router(state.clone());
    assert_eq!(
        request(
            &api,
            "POST",
            route(id, true),
            &session,
            Body::from(body(
                &state,
                id,
                &["refs/heads/ordinary-mixed", "refs/heads/agent/denied"]
            ))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(oracle(&bare, &refs), before);
    assert_eq!(oracle(&primary, &refs), primary_before);
    assert_eq!(oracle(&readonly_path, &refs), readonly_before);
    assert_eq!(
        anchors,
        ["identity.json", "registration.json"]
            .iter()
            .map(|name| std::fs::read(common.join("gitust-repository").join(name)).unwrap())
            .collect::<Vec<_>>()
    );
    assert!(state.worktrees.read().is_empty() && state.automatic_checks.store().is_err());
    assert!(!common.join("gitust-automatic-runs").exists());
    assert_eq!(
        call(
            &api,
            "POST",
            "/api/v1/swarm/worktrees".into(),
            &session,
            Some(
                serde_json::json!({"repo_id":id,"branch":"agent/denied","prompt_summary":"denied"})
            )
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    for path in [&primary, &readonly_path, &bare, &client, &fetch] {
        oracle(path, &["fsck", "--full", "--strict"]);
    }
}
#[test]
fn git_write_startup_rejects_normal_linked_duplicate_and_unregistered_contexts() {
    let (temp, mut state, _session, bare, id, readonly) = fixture();
    let normal = state
        .read_repository(readonly)
        .unwrap()
        .engine
        .repo_path()
        .to_owned();
    state.repository_registry = None;
    assert!(RepositoryRegistry::load_with_git_write(&state, vec![], vec![normal.clone()]).is_err());
    assert!(RepositoryRegistry::load_with_git_write(
        &state,
        vec![bare.clone()],
        vec![bare.clone()]
    )
    .is_err());
    assert!(
        RepositoryRegistry::load_with_git_write(&state, vec![], vec![bare.clone(); 32]).is_err()
    );
    let unknown = temp.path().join("unregistered.git");
    oracle(
        temp.path(),
        &[
            "clone",
            "--bare",
            "--no-local",
            normal.to_str().unwrap(),
            unknown.to_str().unwrap(),
        ],
    );
    assert!(RepositoryRegistry::load_with_git_write(&state, vec![], vec![unknown]).is_err());
    let linked = temp.path().join("linked");
    oracle(
        &bare,
        &[
            "worktree",
            "add",
            "--detach",
            linked.to_str().unwrap(),
            "HEAD",
        ],
    );
    assert!(RepositoryRegistry::load_with_git_write(&state, vec![], vec![bare.clone()]).is_err());
    oracle(&bare, &["worktree", "remove", linked.to_str().unwrap()]);
    let stale_managed = bare
        .join("gitust-worktrees")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&stale_managed).unwrap();
    assert!(RepositoryRegistry::load_with_git_write(&state, vec![], vec![bare.clone()]).is_err());
    std::fs::remove_dir(&stale_managed).unwrap();
    let fence_dir = bare.join("gitust-active-executions");
    std::fs::create_dir(&fence_dir).unwrap();
    std::fs::write(fence_dir.join("unresolved.json"), b"{}").unwrap();
    assert!(RepositoryRegistry::load_with_git_write(&state, vec![], vec![bare.clone()]).is_err());
    std::fs::remove_file(fence_dir.join("unresolved.json")).unwrap();
    std::fs::remove_dir(fence_dir).unwrap();
    let owner =
        RepositoryRegistry::load_with_git_write(&state, vec![], vec![bare.clone()]).unwrap();
    assert!(GitEngine::new(&bare).lock_worktree_service().is_err());
    drop(owner);
    assert!(GitEngine::new(&bare).lock_worktree_service().is_ok());
    assert_eq!(
        GitEngine::new(bare).load_existing_repository_id().unwrap(),
        id
    );
}
#[tokio::test]
async fn secondary_receive_refuses_newly_linked_worktree_before_native_mutation() {
    let (temp, state, session, bare, id, _) = fixture();
    let before = oracle(&bare, &["show-ref", "--head"]);
    let linked = temp.path().join("outside-checkout");
    oracle(
        &bare,
        &[
            "worktree",
            "add",
            "--detach",
            linked.to_str().unwrap(),
            "HEAD",
        ],
    );
    let app = build_router(state.clone());
    for post in [false, true] {
        let payload = if post {
            Body::from(body(&state, id, &["refs/heads/must-not-publish"]))
        } else {
            Body::empty()
        };
        assert_eq!(
            request(
                &app,
                if post { "POST" } else { "GET" },
                route(id, post),
                &session,
                payload
            )
            .await
            .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    assert_eq!(oracle(&bare, &["show-ref", "--head"]), before);
    assert_eq!(oracle(&linked, &["status", "--porcelain"]), b"");
}

#[test]
fn shared_receive_refuses_symbolic_update_aliases_for_primary_and_secondary() {
    let mut unchanged = Vec::new();
    let mut refused = Vec::new();
    for primary in [false, true] {
        let (_temp, state, _session, bare, id, _) = fixture();
        let id = if primary { state.repo_id } else { id };
        let context = state.read_repository(id).unwrap();
        let path = if primary {
            state.git_engine.repo_path()
        } else {
            &bare
        };
        let old = String::from_utf8(oracle(path, &["rev-parse", "HEAD"])).unwrap();
        let tree = String::from_utf8(oracle(path, &["rev-parse", "HEAD^{tree}"])).unwrap();
        let new = String::from_utf8(oracle(
            path,
            &[
                "commit-tree",
                tree.trim(),
                "-p",
                old.trim(),
                "-m",
                "alias destination commit",
            ],
        ))
        .unwrap();
        oracle(
            path,
            &["update-ref", "refs/heads/agent/protected", old.trim()],
        );
        oracle(
            path,
            &[
                "symbolic-ref",
                "refs/heads/public-alias",
                "refs/heads/agent/protected",
            ],
        );
        let mut payload = body(&state, id, &["refs/heads/public-alias"]);
        payload[4..44].copy_from_slice(old.trim().as_bytes());
        payload[45..85].copy_from_slice(new.trim().as_bytes());
        let before = oracle(path, &["show-ref", "--head"]);
        let result = context.engine.smart_http_receive_pack(&payload);
        unchanged.push(oracle(path, &["show-ref", "--head"]) == before);
        refused.push(result.is_err());
        oracle(path, &["fsck", "--full", "--strict"]);
    }
    assert_eq!(
        unchanged, [true; 2],
        "symbolic updates must not mutate either repository"
    );
    assert_eq!(
        refused, [true; 2],
        "symbolic updates must be refused before native RPC"
    );
}

#[test]
#[cfg(unix)]
fn shared_receive_rejects_filesystem_aliases_and_nonregular_refs_but_accepts_packed_direct_refs() {
    use std::{
        ffi::CString,
        os::unix::{ffi::OsStrExt, fs::symlink},
        sync::atomic::{AtomicBool, Ordering},
    };
    let (_temp, state, _session, bare, id, _) = fixture();
    let context = state.read_repository(id).unwrap();
    let common = context.engine.repository_common_directory().unwrap();
    let old = String::from_utf8(oracle(&bare, &["rev-parse", "HEAD^"])).unwrap();
    oracle(
        &bare,
        &["update-ref", "refs/heads/agent/protected", old.trim()],
    );
    oracle(&bare, &["update-ref", "refs/heads/packed", old.trim()]);
    oracle(&bare, &["pack-refs", "--all"]);
    let mut packed = body(&state, id, &["refs/heads/packed"]);
    packed[4..44].copy_from_slice(old.trim().as_bytes());
    assert!(context.engine.smart_http_receive_pack(&packed).is_ok());
    assert_eq!(
        oracle(&bare, &["rev-parse", "refs/heads/packed"]),
        oracle(&bare, &["rev-parse", "HEAD"])
    );
    // Recreate the loose target independently; aliases must not even reach the final RPC gate.
    let target = common.join("refs/heads/agent/protected");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, old.as_bytes()).unwrap();
    let alias = common.join("refs/heads/public-alias");
    for kind in 0..4 {
        match kind {
            0 => symlink("agent/protected", &alias).unwrap(),
            1 => symlink("agent", &alias).unwrap(),
            2 => std::fs::write(&alias, vec![b'0'; 65536]).unwrap(),
            _ => {
                let path = CString::new(alias.as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            }
        }
        let name = if kind == 1 {
            "refs/heads/public-alias/protected"
        } else {
            "refs/heads/public-alias"
        };
        let mut payload = body(&state, id, &[name]);
        payload[4..44].copy_from_slice(old.trim().as_bytes());
        let reached = AtomicBool::new(false);
        assert!(context
            .engine
            .smart_http_receive_pack_with_gate(&payload, || {
                reached.store(true, Ordering::SeqCst);
                Ok(())
            })
            .is_err());
        assert!(!reached.load(Ordering::SeqCst));
        assert_eq!(std::fs::read(&target).unwrap(), old.as_bytes());
        std::fs::remove_file(&alias).unwrap();
    }
    oracle(&bare, &["fsck", "--full", "--strict"]);
}
