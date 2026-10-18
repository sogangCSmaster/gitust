use crate::{GitEngine, SshUploadPackError};
use std::{fs, path::Path, process::Command, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

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
        &["tag", "-a", "release", "-m", "annotated"],
    );
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

#[tokio::test]
async fn upload_pack_completes_after_eof_and_preserves_repository_refs() {
    let fixture = repository();
    let before = git(fixture.path(), &["show-ref"]);
    let head = git(fixture.path(), &["rev-parse", "HEAD"]);
    let tag = git(fixture.path(), &["rev-parse", "refs/tags/release"]);
    let engine = GitEngine::new(fixture.path());
    let (server, mut client) = tokio::io::duplex(16384);
    let holder = Arc::new(());
    let run = engine
        .start_ssh_upload_pack(server, holder.clone(), || Ok(()))
        .expect("start upload-pack");
    let refs = String::from_utf8(advertisement(&mut client).await).expect("refs UTF8");
    assert!(refs.contains(&format!("{} refs/heads/main", head.trim())));
    assert!(refs.contains(&format!("{} refs/tags/release", tag.trim())));
    assert!(!refs.contains("refs/remotes/private/staging"));
    let want = format!("want {}\n", head.trim());
    client
        .write_all(format!("{:04x}{want}00000009done\n", want.len() + 4).as_bytes())
        .await
        .expect("want commit");
    client.shutdown().await.expect("client input EOF");
    let mut pack = Vec::new();
    client.read_to_end(&mut pack).await.expect("drain pack");
    assert!(pack.windows(4).any(|bytes| bytes == b"PACK"));
    assert_eq!(run.wait().await.expect("completed run").exit_code, 0);
    assert_eq!(Arc::strong_count(&holder), 1);
    assert_eq!(git(fixture.path(), &["show-ref"]), before);
    git(fixture.path(), &["fsck", "--strict"]);
}

#[tokio::test]
async fn cancelling_an_open_upload_pack_confirms_cleanup_before_releasing_holder() {
    let fixture = repository();
    let engine = GitEngine::new(fixture.path());
    for drop_handle in [false, true] {
        let (server, mut client) = tokio::io::duplex(16384);
        let holder = Arc::new(());
        let run = engine
            .start_ssh_upload_pack(server, holder.clone(), || Ok(()))
            .expect("start upload-pack");
        advertisement(&mut client).await;
        let cancellation = run.cancel_handle();
        if drop_handle {
            drop(run);
        } else {
            cancellation.cancel();
            assert_eq!(run.wait().await, Err(SshUploadPackError::Cancelled));
        }
        cancellation.wait().await.expect("cleanup confirmed");
        assert_eq!(Arc::strong_count(&holder), 1);
    }
    git(fixture.path(), &["fsck", "--strict"]);
}

#[tokio::test]
async fn denied_upload_pack_publishes_no_bytes_and_preserves_refs() {
    let fixture = repository();
    let before = git(fixture.path(), &["show-ref"]);
    let engine = GitEngine::new(fixture.path());
    let (server, mut client) = tokio::io::duplex(16384);
    let run = engine
        .start_ssh_upload_pack(server, (), || Err(()))
        .expect("supervisor starts before authorization");
    assert_eq!(run.wait().await, Err(SshUploadPackError::Denied));
    let mut bytes = Vec::new();
    client.read_to_end(&mut bytes).await.expect("closed stream");
    assert!(bytes.is_empty());
    assert_eq!(git(fixture.path(), &["show-ref"]), before);
}
