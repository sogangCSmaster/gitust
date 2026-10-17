use super::tests_automatic_checks::{git, policy};
use super::tests_verification::fixture;
use super::*;
use crate::automatic_run_model::{Job, Origin, Phase};
use crate::automatic_run_store::Store;
use gitust_core::CommitSha;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn update_packet(old: &str, new: &str, branch: &str, first: bool) -> Vec<u8> {
    let capabilities = if first { "\0report-status" } else { "" };
    let command = format!("{old} {new} refs/heads/{branch}{capabilities}\n");
    format!("{:04x}{command}", command.len() + 4).into_bytes()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoked_original_git_credentials_reject_every_prepared_ref_before_receive_pack() {
    for use_pat in [false, true] {
        let (tmp, state, app, token, first, first_dir) = fixture().await;
        let repo = tmp.path().join("repo");
        policy(&repo, true, "printf must-not-run");
        let principal = state.auth_store.authenticate_session(&token).unwrap();
        let pat = use_pat.then(|| {
            state
                .auth_store
                .create_personal_access_token(&principal, "receive-revocation", None)
                .unwrap()
        });
        let credential = pat
            .as_ref()
            .map_or_else(|| token.clone(), |p| p.raw_token.clone());
        let (status, body) = call(
            &app,
            "POST",
            "/api/v1/swarm/worktrees".into(),
            &token,
            Some(serde_json::json!({
                "branch": "agent/test/revoked-second",
                "prompt_summary": "second ref in revoked receive-pack"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let second: AgentWorktree = serde_json::from_str(&body).unwrap();
        let second_dir = state.get_worktree_runtime(second.id).unwrap().worktree_dir;
        let worktrees = [first, second];
        let directories = [first_dir, second_dir];
        let mut revisions = Vec::new();
        let mut request_body = Vec::new();
        for (index, (wt, directory)) in worktrees.iter().zip(&directories).enumerate() {
            let old = git(directory, &["rev-parse", "HEAD"]);
            // Existing commit objects make this a valid receive-pack without shipping a pack.
            let new = git(
                directory,
                &[
                    "commit-tree",
                    "HEAD^{tree}",
                    "-p",
                    &old,
                    "-m",
                    "revoked push candidate",
                ],
            );
            request_body.extend(update_packet(&old, &new, &wt.branch, index == 0));
            revisions.push((old, new));
        }
        request_body.extend_from_slice(b"0000PACK\0\0\0\x02\0\0\0\0");
        // SHA-1 checksum of an empty version-2 PACK, independently produced by git pack-objects.
        request_body.extend_from_slice(&[
            0x02, 0x9d, 0x08, 0x82, 0x3b, 0xd8, 0xa8, 0xea, 0xb5, 0x10, 0xad, 0x6a, 0xc7, 0x5c,
            0x82, 0x3c, 0xfd, 0x3e, 0xd3, 0x1e,
        ]);
        let common = state.git_engine.repository_common_directory().unwrap();
        let store = Store::open(&common, state.repo_id).unwrap();
        let checks = state
            .git_engine
            .required_checks(&worktrees[0].base_branch)
            .unwrap()
            .unwrap();
        let mut queued = Job::new(
            principal.user_id,
            Origin::GitHttp,
            CommitSha(revisions[0].0.clone()),
            None,
            checks.blob_oid,
        );
        queued.transition(Phase::Queued);
        let queued_id = queued.run_id;
        store
            .update(
                worktrees[0].id,
                &worktrees[0].branch,
                &worktrees[0].base_branch,
                |doc| {
                    doc.queued = Some(queued);
                    Ok(())
                },
            )
            .unwrap();
        drop(store);

        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *state.automatic_checks.pre_receive_hook.lock() = Some((entered_tx, release_rx));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let request = tokio::spawn(async move {
            let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
            let headers = format!(
                "POST /git/default.git/git-receive-pack HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {credential}\r\nContent-Type: application/x-git-receive-pack-request\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                request_body.len()
            );
            socket.write_all(headers.as_bytes()).await.unwrap();
            socket.write_all(&request_body).await.unwrap();
            let mut response = Vec::new();
            socket.take(8192).read_to_end(&mut response).await.unwrap();
            String::from_utf8(response).unwrap()
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx)
            .await
            .expect("request reached the durable pre-receive boundary")
            .unwrap();
        let prepared = state.automatic_checks.store().unwrap().documents().unwrap();
        assert_eq!(prepared.len(), 2);
        for (wt, (old, new)) in worktrees.iter().zip(&revisions) {
            let document = prepared
                .iter()
                .find(|doc| doc.worktree_id == wt.id)
                .unwrap();
            assert_eq!(document.prepared.as_ref().unwrap().expected.0, *new);
            assert_eq!(
                git(&repo, &["rev-parse", &format!("refs/heads/{}", wt.branch)]),
                *old
            );
        }
        match pat {
            Some(pat) => {
                state
                    .auth_store
                    .revoke_personal_access_token(&principal, pat.metadata.id)
                    .unwrap();
            }
            None => state.auth_store.revoke_session(&principal, &token).unwrap(),
        }
        // The account remains active: only the exact credential accepted before the pause is gone.
        assert_eq!(
            state
                .auth_store
                .verify_active_principal(&principal)
                .unwrap()
                .user_id,
            principal.user_id
        );
        release_tx.send(()).unwrap();
        let response = tokio::time::timeout(std::time::Duration::from_secs(5), request)
            .await
            .expect("revoked push completed")
            .unwrap();
        assert!(response.starts_with("HTTP/1.1 401 "));
        assert!(response.lines().any(|line| line
            .to_ascii_lowercase()
            .starts_with("www-authenticate: basic realm=")));

        let documents = Store::open(&common, state.repo_id)
            .unwrap()
            .documents()
            .unwrap();
        for (wt, (old, new)) in worktrees.iter().zip(&revisions) {
            let document = documents
                .iter()
                .find(|doc| doc.worktree_id == wt.id)
                .unwrap();
            let attempt = prepared
                .iter()
                .find(|doc| doc.worktree_id == wt.id)
                .unwrap()
                .prepared
                .as_ref()
                .unwrap();
            assert!(document.prepared.is_none());
            assert!(document.running.is_none());
            assert!(document
                .recent
                .iter()
                .any(|job| job.run_id == attempt.run_id
                    && job.phase == Phase::Rejected
                    && job.expected.0 == *new));
            assert_eq!(
                git(&repo, &["rev-parse", &format!("refs/heads/{}", wt.branch)]),
                *old
            );
            assert!(crate::run_history::read(&common, wt.id, &wt.branch)
                .unwrap()
                .is_empty());
        }
        let retained = documents
            .iter()
            .find(|doc| doc.worktree_id == worktrees[0].id)
            .unwrap();
        assert_eq!(retained.queued.as_ref().unwrap().run_id, queued_id);
        let second = documents
            .iter()
            .find(|doc| doc.worktree_id == worktrees[1].id)
            .unwrap();
        assert!(second.queued.is_none());
        git(&repo, &["fsck", "--strict"]);
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
    }
}
