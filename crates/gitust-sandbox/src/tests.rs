use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use gitust_core::{AgentKind, RepoId, WorktreeId};
use gitust_git::{GitEngine, MaterializedWorktree};
use tempfile::TempDir;

use crate::policy::{bwrap_args, resolve_rules, seatbelt_quote};
use crate::{
    LocalSandboxExecutor, PathAccess, PathRule, SandboxError, SandboxSpec, MAX_SANDBOX_STREAM_BYTES,
};

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@company.internal")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@company.internal")
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn spec_for(m: &MaterializedWorktree) -> SandboxSpec {
    SandboxSpec::for_linked_worktree(
        &m.worktree_dir,
        &m.worktree_admin_dir,
        &m.common_git_dir,
        m.main_workdir.as_deref(),
        &m.worktrees_base,
        &m.sandbox_scratch_base,
    )
}

/// Serves one minimal HTTP response per connection on 127.0.0.1; returns the port.
fn spawn_local_http_server() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0_u8; 1024];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nok");
        }
    });
    port
}

#[tokio::test]
async fn gate_7_4_sandbox_isolation() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    let secret_dir = tmp.path().join("host-secrets");
    fs::create_dir_all(&repo_dir).expect("mkdir");
    fs::create_dir_all(&secret_dir).expect("mkdir");
    let secret_file = secret_dir.join("prod.key");
    fs::write(&secret_file, "TOP_SECRET_ON_PREM_KEY_999").expect("write");
    let outside_target = secret_dir.join("pwned.txt");

    git(&repo_dir, &["init", "-q", "-b", "main"]);
    fs::write(repo_dir.join("app.rs"), "fn main() {}\n").expect("write");
    git(&repo_dir, &["add", "."]);
    git(&repo_dir, &["commit", "-q", "-m", "initial"]);
    fs::write(repo_dir.join(".env"), "STASHED_SECRET_1").expect("write");
    git(&repo_dir, &["stash", "push", "-q", "-u", "-m", "wip"]);
    fs::write(repo_dir.join("staged.env"), "STAGED_SECRET_2").expect("write");
    git(&repo_dir, &["add", "staged.env"]);
    fs::write(repo_dir.join("MAIN_ONLY.txt"), "MAIN_WORKTREE_CONTENT").expect("write");

    let engine = GitEngine::new(&repo_dir);
    let mk = |branch: &str| {
        engine
            .materialize_agent_worktree(
                WorktreeId::new(),
                RepoId::new(),
                AgentKind::ClaudeCode,
                branch,
                "main",
                "sandbox gate",
            )
            .expect("materialize")
    };
    let m = mk("agent/claude/sandbox-gate");
    let sibling = mk("agent/cursor/sibling");
    fs::write(sibling.worktree_dir.join("SIBLING.txt"), "SIBLING_CONTENT").expect("write");
    let spec = spec_for(&m);
    let exec = LocalSandboxExecutor::new(4).with_timeout_secs(10);
    let run = |cmd: String| {
        let exec = exec.clone();
        let spec = spec.clone();
        let id = m.worktree.id;
        async move { exec.execute(id, &spec, &cmd).await.expect("execute") }
    };

    let ok = run("cat app.rs && echo '// edit' >> app.rs && git status --short".into()).await;
    assert_eq!(ok.exit_code, 0, "stderr: {}", ok.stderr);
    assert!(ok.stdout.contains("fn main()"));
    assert!(
        ok.stdout.contains("app.rs"),
        "git status must work: {}",
        ok.stdout
    );
    assert!(ok.network_isolated);

    assert!(
        !m.sandbox_scratch_base
            .join(ok.run_id.0.to_string())
            .exists(),
        "per-run scratch dir must be removed"
    );

    let commit = run("git -c user.name=a -c user.email=a@a commit -qam x".into()).await;
    assert_ne!(
        commit.exit_code, 0,
        "commit must be blocked inside the sandbox"
    );
    let gitfile = run("echo 'gitdir: /etc' > .git".into()).await;
    assert_ne!(gitfile.exit_code, 0, ".git pointer must be read-only");

    for (path, marker) in [
        (repo_dir.join("MAIN_ONLY.txt"), "MAIN_WORKTREE_CONTENT"),
        (sibling.worktree_dir.join("SIBLING.txt"), "SIBLING_CONTENT"),
        (secret_file.clone(), "TOP_SECRET_ON_PREM_KEY_999"),
    ] {
        let res = run(format!("cat '{}'", path.display())).await;
        assert_ne!(res.exit_code, 0, "read of {} must fail", path.display());
        assert!(!res.stdout.contains(marker), "{} leaked", path.display());
    }

    let common = m.common_git_dir.display().to_string();
    for probe in [
        format!("git --git-dir='{common}' ls-files -s staged.env"),
        format!("cat '{common}/index'"),
        format!("git --git-dir='{common}' show stash@{{0}}^3:.env"),
        format!("cat '{common}/refs/stash'"),
        format!("cat '{common}/logs/HEAD'"),
        format!("ls '{common}/worktrees'"),
        format!("cat '{}/index'", sibling.worktree_admin_dir.display()),
    ] {
        let res = run(probe.clone()).await;
        assert!(
            res.exit_code != 0 || res.stdout.trim().is_empty(),
            "probe must not succeed: {probe}\nstdout: {}",
            res.stdout
        );
        assert!(!res.stdout.contains("SECRET"), "{probe} leaked a secret");
    }

    git(&repo_dir, &["pack-refs", "--all"]);
    let packed = run(format!(
        "git status --short && test -r '{common}/packed-refs'"
    ))
    .await;
    assert_eq!(packed.exit_code, 0, "stderr: {}", packed.stderr);
    let logs_still_hidden = run(format!("cat '{common}/logs/HEAD'")).await;
    assert_ne!(logs_still_hidden.exit_code, 0);

    let write = run(format!("echo pwned > '{}'", outside_target.display())).await;
    assert_ne!(write.exit_code, 0);
    assert!(!outside_target.exists());

    std::env::set_var("GITUST_TEST_HOST_SECRET_TOKEN", "SUPER_SENSITIVE_777");
    let env = run("env; echo HOME=$HOME".into()).await;
    assert_eq!(env.exit_code, 0);
    assert!(!env.stdout.contains("SUPER_SENSITIVE_777"));
    assert!(
        env.stdout
            .contains(&format!("HOME={}", m.sandbox_scratch_base.display())),
        "HOME must be under the scratch base: {}",
        env.stdout
    );

    let port = spawn_local_http_server();
    let curl = format!("curl -s --max-time 2 http://127.0.0.1:{port}/");
    let host = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(&curl)
        .output()
        .expect("host curl");
    assert_eq!(
        String::from_utf8_lossy(&host.stdout),
        "ok",
        "sanity: server must be reachable without sandbox"
    );
    let net = run(curl).await;
    assert_ne!(net.exit_code, 0, "network must be blocked");
    assert!(!net.stdout.contains("ok"));

    let started = Instant::now();
    let bg = run("(sleep 30 &) ; echo lead-exited".into()).await;
    assert_eq!(bg.exit_code, 0);
    assert!(bg.stdout.contains("lead-exited"));
    assert!(started.elapsed() < Duration::from_secs(5));

    let fast = LocalSandboxExecutor::new(1).with_timeout_secs(1);
    let timed_out = fast.execute(m.worktree.id, &spec, "sleep 30").await;
    assert!(matches!(timed_out, Err(SandboxError::TimedOut { .. })));
}

#[tokio::test]
async fn gate_rust_toolchain_works_in_sandbox() {
    let has_rustup = std::env::var_os("HOME")
        .map(|h| Path::new(&h).join(".rustup").is_dir())
        .unwrap_or(false);
    if !has_rustup {
        eprintln!("skipping: host has no ~/.rustup");
        return;
    }
    let tmp = TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("crate");
    fs::create_dir_all(repo_dir.join("src")).expect("mkdir");
    git(&repo_dir, &["init", "-q", "-b", "main"]);
    fs::write(
        repo_dir.join("Cargo.toml"),
        "[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write");
    fs::write(repo_dir.join("src/lib.rs"), "pub fn one() -> u8 { 1 }\n").expect("write");
    git(&repo_dir, &["add", "."]);
    git(&repo_dir, &["commit", "-q", "-m", "tiny crate"]);

    let m = GitEngine::new(&repo_dir)
        .materialize_agent_worktree(
            WorktreeId::new(),
            RepoId::new(),
            AgentKind::ClaudeCode,
            "agent/claude/cargo",
            "main",
            "cargo",
        )
        .expect("materialize");
    let res = LocalSandboxExecutor::new(1)
        .with_timeout_secs(120)
        .execute(
            m.worktree.id,
            &spec_for(&m),
            "cargo build --offline -q && cargo --version",
        )
        .await
        .expect("execute");
    assert_eq!(
        res.exit_code, 0,
        "stdout: {}\nstderr: {}",
        res.stdout, res.stderr
    );
    assert!(res.stdout.contains("cargo "), "{}", res.stdout);
}

#[test]
fn rules_are_ordered_shallow_to_deep() {
    let tmp = TempDir::new().expect("tempdir");
    let deep = tmp.path().join("a/b/c");
    fs::create_dir_all(&deep).expect("mkdir");
    let rules = resolve_rules(vec![
        PathRule::new(&deep, PathAccess::ReadWrite),
        PathRule::new(tmp.path().join("a"), PathAccess::Hidden),
        PathRule::new(tmp.path().join("missing-ro"), PathAccess::ReadOnly),
        PathRule::new(
            tmp.path().join("a/not-yet-created-secret"),
            PathAccess::Hidden,
        ),
    ])
    .expect("resolve");
    assert_eq!(
        rules.len(),
        3,
        "missing ReadOnly paths are dropped, but missing Hidden leaves under an existing ancestor are preserved"
    );
    assert_eq!(rules[0].access, PathAccess::Hidden);
    assert_eq!(rules[1].access, PathAccess::Hidden);
    assert!(rules[1].path.ends_with("a/not-yet-created-secret"));
    assert_eq!(rules[2].access, PathAccess::ReadWrite);
}

#[test]
fn seatbelt_quote_rejects_control_chars() {
    assert!(seatbelt_quote(Path::new("/tmp/a\nb")).is_err());
    assert_eq!(
        seatbelt_quote(Path::new("/tmp/a\"b")).expect("quote"),
        "\"/tmp/a\\\"b\""
    );
}

#[test]
fn bwrap_args_orders_mounts_and_network_flags() {
    let tmp = TempDir::new().expect("tempdir");
    let hidden_dir = tmp.path().join("hidden-dir");
    let hidden_file = tmp.path().join("hidden-file");
    let ro_dir = tmp.path().join("ro");
    let rw_dir = tmp.path().join("rw");
    fs::create_dir_all(&hidden_dir).expect("mkdir");
    fs::write(&hidden_file, "secret").expect("write");
    fs::create_dir_all(&ro_dir).expect("mkdir");
    fs::create_dir_all(&rw_dir).expect("mkdir");

    let rules = vec![
        PathRule::new(&hidden_dir, PathAccess::Hidden),
        PathRule::new(&hidden_file, PathAccess::Hidden),
        PathRule::new(&ro_dir, PathAccess::ReadOnly),
        PathRule::new(&rw_dir, PathAccess::ReadWrite),
    ];
    let offline: Vec<String> = bwrap_args(&rules, &rw_dir, "echo hi", false)
        .into_iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    assert!(!offline.iter().any(|a| a == "--share-net"));
    assert!(offline
        .windows(2)
        .any(|w| w[0] == "--tmpfs" && w[1] == hidden_dir.to_string_lossy()));
    assert!(offline.windows(3).any(|w| w[0] == "--ro-bind"
        && w[1] == "/dev/null"
        && w[2] == hidden_file.to_string_lossy()));
    assert!(offline.windows(3).any(|w| w[0] == "--ro-bind"
        && w[1] == ro_dir.to_string_lossy()
        && w[2] == ro_dir.to_string_lossy()));
    assert!(offline.windows(3).any(|w| w[0] == "--bind"
        && w[1] == rw_dir.to_string_lossy()
        && w[2] == rw_dir.to_string_lossy()));

    let online: Vec<String> = bwrap_args(&rules, &rw_dir, "echo hi", true)
        .into_iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    assert!(online.iter().any(|a| a == "--share-net"));
}

#[tokio::test]
async fn read_capped_drains_beyond_1mib_without_sigpipe() {
    let tmp = TempDir::new().expect("tempdir");
    let workdir = tmp.path().join("wt");
    let scratch = tmp.path().join("scratch");
    fs::create_dir_all(&workdir).expect("mkdir");
    fs::create_dir_all(&scratch).expect("mkdir");
    let spec = SandboxSpec {
        workdir: workdir.clone(),
        scratch_base: scratch,
        rules: vec![PathRule::new(&workdir, PathAccess::ReadWrite)],
    };
    let exec = LocalSandboxExecutor::new(1);
    let res = exec
        .execute(
            WorktreeId::new(),
            &spec,
            "dd if=/dev/zero bs=1024 count=2048 2>/dev/null | tr '\\0' 'a'",
        )
        .await
        .expect("execute 2MiB stdout command");
    assert_eq!(
        res.exit_code, 0,
        "2MiB output must not fail with SIGPIPE (141)"
    );
    assert_eq!(
        res.stdout.len(),
        usize::try_from(MAX_SANDBOX_STREAM_BYTES).expect("usize")
    );
}

/// Gate A P1 (2026-10-16): a Hidden rule whose only existing ancestor is `/` (non-existent
/// `$HOME`, hosts without `/mnt` or `/media`) must be KEPT lexically, not dropped and not
/// turned into an error that aborts every sandbox spawn.
#[test]
fn hidden_rule_without_existing_ancestor_is_kept_lexically() {
    let missing = format!(
        "/gitust-missing-{}-{:?}/x",
        std::process::id(),
        Instant::now()
    );
    let rules = resolve_rules(vec![PathRule::new(&missing, PathAccess::Hidden)])
        .expect("missing hidden path under / must resolve");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].path, Path::new(&missing));
    assert_eq!(rules[0].access, PathAccess::Hidden);

    // Non-normalized Hidden paths still fail closed instead of being dropped.
    let dotted = format!("/gitust-missing-{}/../etc", std::process::id());
    assert!(matches!(
        resolve_rules(vec![PathRule::new(&dotted, PathAccess::Hidden)]),
        Err(SandboxError::UnsupportedPath(_))
    ));
}
