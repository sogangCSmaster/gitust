use crate::{GitEngine, SshUploadPackError};
use std::{
    fs,
    path::Path,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

const EMPTY_PACK: &[u8] = &[
    b'P', b'A', b'C', b'K', 0, 0, 0, 2, 0, 0, 0, 0, 0x02, 0x9d, 0x08, 0x82, 0x3b, 0xd8, 0xa8, 0xea,
    0xb5, 0x10, 0xad, 0x6a, 0xc7, 0x5c, 0x82, 0x3c, 0xfd, 0x3e, 0xd3, 0x1e,
];

fn git(path: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    crate::process::sanitize_git_environment(&mut command);
    let output = command
        .arg("-C")
        .arg(path)
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .env("GIT_AUTHOR_NAME", "Gitust Test")
        .env("GIT_AUTHOR_EMAIL", "test@company.internal")
        .env("GIT_COMMITTER_NAME", "Gitust Test")
        .env("GIT_COMMITTER_EMAIL", "test@company.internal")
        .output()
        .expect("Git fixture command");
    assert!(output.status.success(), "fixture Git operation failed");
    String::from_utf8(output.stdout).expect("Git fixture UTF8")
}

fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("fixture directory");
    git(directory.path(), &["init", "-q", "-b", "main"]);
    fs::write(directory.path().join("README"), "original content\n").expect("fixture content");
    git(directory.path(), &["add", "README"]);
    git(directory.path(), &["commit", "-q", "-m", "initial"]);
    git(
        directory.path(),
        &["update-ref", "refs/remotes/private/staging", "HEAD"],
    );
    directory
}

async fn advertisement(client: &mut DuplexStream) -> Vec<u8> {
    let mut output = Vec::new();
    loop {
        let mut header = [0; 4];
        client
            .read_exact(&mut header)
            .await
            .expect("advertisement header");
        let count = usize::from_str_radix(std::str::from_utf8(&header).expect("pkt header"), 16)
            .expect("pkt length");
        if count == 0 {
            return output;
        }
        assert!((4..=65520).contains(&count));
        let mut packet = vec![0; count - 4];
        client
            .read_exact(&mut packet)
            .await
            .expect("advertisement packet");
        output.extend(packet);
    }
}

fn command_pkt(old_oid: &str, new_oid: &str, refname: &str) -> Vec<u8> {
    let line = format!("{old_oid} {new_oid} {refname}\0report-status\n");
    let mut out = format!("{:04x}{line}0000", line.len() + 4).into_bytes();
    out.extend_from_slice(EMPTY_PACK);
    out
}

#[tokio::test]
async fn receive_pack_accepts_valid_push_and_runs_finish_hook_before_releasing_holder() {
    let fixture = repository();
    let head = git(fixture.path(), &["rev-parse", "HEAD"])
        .trim()
        .to_owned();
    let engine = GitEngine::new(fixture.path());
    let (server, mut client) = tokio::io::duplex(16384);
    let holder = Arc::new(());
    let finish_holder = holder.clone();
    let saw_holder_alive = Arc::new(AtomicBool::new(false));
    let saw_holder_clone = saw_holder_alive.clone();
    let run = engine
        .start_ssh_receive_pack(
            server,
            holder.clone(),
            || Ok(()),
            move |updates| {
                assert_eq!(updates.len(), 1);
                assert_eq!(updates[0].refname, "refs/heads/agent/ssh-pushed");
                assert!(updates[0].is_agent_branch);
                Ok(())
            },
            move |outcome| {
                assert_eq!(outcome.expect("push succeeded").exit_code, 0);
                saw_holder_clone.store(Arc::strong_count(&finish_holder) >= 2, Ordering::SeqCst);
            },
        )
        .expect("start receive-pack");
    let refs = String::from_utf8(advertisement(&mut client).await).expect("refs UTF8");
    assert!(refs.contains(&format!("{head} refs/heads/main")));
    assert!(!refs.contains("refs/remotes/private/staging"));
    let payload = command_pkt(&"0".repeat(40), &head, "refs/heads/agent/ssh-pushed");
    client.write_all(&payload).await.expect("write push");
    client.shutdown().await.expect("client EOF");
    let mut status = Vec::new();
    client.read_to_end(&mut status).await.expect("read status");
    assert!(String::from_utf8_lossy(&status).contains("unpack ok"));
    assert_eq!(run.wait().await.expect("completed").exit_code, 0);
    assert!(saw_holder_alive.load(Ordering::SeqCst));
    assert_eq!(
        git(
            fixture.path(),
            &["rev-parse", "refs/heads/agent/ssh-pushed"]
        )
        .trim(),
        head
    );
    git(fixture.path(), &["fsck", "--strict"]);
}

#[tokio::test]
async fn denied_command_gate_or_symbolic_target_never_forwards_commands_to_git() {
    let fixture = repository();
    let head = git(fixture.path(), &["rev-parse", "HEAD"])
        .trim()
        .to_owned();
    git(
        fixture.path(),
        &["symbolic-ref", "refs/heads/sym-alias", "refs/heads/main"],
    );
    let before = git(fixture.path(), &["show-ref"]);
    let engine = GitEngine::new(fixture.path());

    for (refname, gate_ok) in [
        ("refs/heads/agent/denied", false),
        ("refs/heads/sym-alias", true),
    ] {
        let (server, mut client) = tokio::io::duplex(16384);
        let called_gate = Arc::new(AtomicBool::new(false));
        let called_gate_clone = called_gate.clone();
        let run = engine
            .start_ssh_receive_pack(
                server,
                (),
                || Ok(()),
                move |_| {
                    called_gate_clone.store(true, Ordering::SeqCst);
                    if gate_ok {
                        Ok(())
                    } else {
                        Err(())
                    }
                },
                |_| {},
            )
            .expect("start receive-pack");
        advertisement(&mut client).await;
        let payload = command_pkt(&"0".repeat(40), &head, refname);
        let _ = client.write_all(&payload).await;
        let _ = client.shutdown().await;
        assert_eq!(run.wait().await, Err(SshUploadPackError::Denied));
        if refname == "refs/heads/sym-alias" {
            assert!(
                !called_gate.load(Ordering::SeqCst),
                "symbolic ref target must be rejected before command_gate runs"
            );
        }
        assert_eq!(git(fixture.path(), &["show-ref"]), before);
    }
}

#[tokio::test]
async fn noop_flush_and_clean_eof_finish_without_calling_command_gate() {
    let fixture = repository();
    let before = git(fixture.path(), &["show-ref"]);
    let engine = GitEngine::new(fixture.path());
    for send_flush in [false, true] {
        let (server, mut client) = tokio::io::duplex(16384);
        let run = engine
            .start_ssh_receive_pack(
                server,
                (),
                || Ok(()),
                |_| panic!("command_gate must not run on no-op flush or EOF"),
                |_| {},
            )
            .expect("start receive-pack");
        advertisement(&mut client).await;
        if send_flush {
            client.write_all(b"0000").await.expect("write flush");
        }
        client.shutdown().await.expect("EOF");
        let mut out = Vec::new();
        client.read_to_end(&mut out).await.expect("drain");
        assert_eq!(
            run.wait().await.expect("clean exit").exit_code,
            if send_flush { 0 } else { 128 }
        );
        assert_eq!(git(fixture.path(), &["show-ref"]), before);
    }
}

#[tokio::test]
async fn receive_pack_rejects_push_options_without_trailing_newline_and_secondary_nul_capabilities()
{
    let fixture = repository();
    let head = git(fixture.path(), &["rev-parse", "HEAD"])
        .trim()
        .to_owned();
    let before = git(fixture.path(), &["show-ref"]);
    let engine = GitEngine::new(fixture.path());
    let zero = "0".repeat(40);

    let no_lf_push_opts =
        format!("{zero} {head} refs/heads/agent/task\0report-status push-options");
    let eq_push_opts =
        format!("{zero} {head} refs/heads/agent/task\0report-status push-options=1\r\n");
    let secondary_nul = format!("{zero} {head} refs/heads/agent/task\0report-status\0hidden\n");

    for line in [no_lf_push_opts, eq_push_opts, secondary_nul] {
        let mut payload = format!("{:04x}{line}0000", line.len() + 4).into_bytes();
        payload.extend_from_slice(EMPTY_PACK);
        assert!(
            crate::inspect_receive_pack_updates(&payload).is_err(),
            "inspect_receive_pack_updates must reject capability payload: {line:?}"
        );
        let (server, mut client) = tokio::io::duplex(16384);
        let run = engine
            .start_ssh_receive_pack(
                server,
                (),
                || Ok(()),
                |_| panic!("command_gate must not run on rejected capability"),
                |_| {},
            )
            .expect("start receive-pack");
        advertisement(&mut client).await;
        let _ = client.write_all(&payload).await;
        let _ = client.shutdown().await;
        assert_eq!(run.wait().await, Err(SshUploadPackError::Denied));
        assert_eq!(git(fixture.path(), &["show-ref"]), before);
    }
}
