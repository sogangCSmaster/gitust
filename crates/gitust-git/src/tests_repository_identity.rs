use std::{fs, path::Path, process::Command, sync::Arc};

use gitust_core::{AgentKind, WorktreeId};
use tempfile::TempDir;

use crate::{GitEngine, GitEngineError};

fn git(path: &Path, args: &[&str]) {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.as_encoded_bytes().starts_with(b"GIT_") {
            command.env_remove(key);
        }
    }
    let output = command
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Identity fixture")
        .env("GIT_AUTHOR_EMAIL", "identity@example.invalid")
        .env("GIT_COMMITTER_NAME", "Identity fixture")
        .env("GIT_COMMITTER_EMAIL", "identity@example.invalid")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture() -> (TempDir, GitEngine) {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("source");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("README.md"), "identity fixture\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-qm", "initial"]);
    (temp, GitEngine::new(repo))
}

fn identity_path(engine: &GitEngine) -> std::path::PathBuf {
    engine
        .repo_path()
        .join(".git/gitust-repository/identity.json")
}

#[test]
fn identity_survives_process_restart() {
    const PATH_ENV: &str = "GITUST_IDENTITY_RESTART_FIXTURE";
    const ID_ENV: &str = "GITUST_IDENTITY_RESTART_EXPECTED";
    if let Some(path) = std::env::var_os(PATH_ENV) {
        let expected = std::env::var(ID_ENV).unwrap();
        assert_eq!(
            GitEngine::new(path)
                .load_or_create_repository_id()
                .unwrap()
                .0
                .to_string(),
            expected
        );
        return;
    }
    let (_temp, engine) = fixture();
    let id = engine.load_or_create_repository_id().unwrap();
    assert!(!id.0.is_nil());
    for _ in 0..2 {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests_repository_identity::identity_survives_process_restart",
                "--nocapture",
            ])
            .env(PATH_ENV, engine.repo_path())
            .env(ID_ENV, id.0.to_string())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[test]
fn concurrent_clones_initialize_one_identity_and_other_service_owners_are_refused() {
    let (_temp, engine) = fixture();
    let service = engine.lock_worktree_service().unwrap();
    let rival = GitEngine::new(engine.repo_path());
    assert!(matches!(
        rival.load_or_create_repository_id(),
        Err(GitEngineError::WorktreeServiceBusy(_))
    ));
    assert!(!identity_path(&engine).exists());
    let barrier = Arc::new(std::sync::Barrier::new(12));
    let threads: Vec<_> = (0..12)
        .map(|_| {
            let engine = engine.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                engine.load_or_create_repository_id().unwrap()
            })
        })
        .collect();
    let ids: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert!(ids.iter().all(|id| *id == ids[0]));
    drop(service);
    assert_eq!(rival.load_or_create_repository_id().unwrap(), ids[0]);
}

#[test]
fn linked_common_and_relocated_repository_keep_identity_but_git_clones_are_independent() {
    let (temp, engine) = fixture();
    let id = engine.load_or_create_repository_id().unwrap();
    let worktree = engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            id,
            AgentKind::CodexSwarm,
            "agent/test/identity",
            "main",
            "identity",
        )
        .unwrap();
    assert_eq!(
        GitEngine::new(&worktree.worktree_dir)
            .load_or_create_repository_id()
            .unwrap(),
        id
    );
    engine
        .remove_agent_worktree(
            &worktree.worktree_dir,
            &worktree.worktree.branch,
            &worktree.worktree.base_commit,
        )
        .unwrap();
    let original = engine.repo_path().to_path_buf();
    drop(engine);
    let moved = temp.path().join("relocated");
    fs::rename(&original, &moved).unwrap();
    assert_eq!(
        GitEngine::new(&moved)
            .load_or_create_repository_id()
            .unwrap(),
        id
    );
    let clone = temp.path().join("clone");
    git(
        temp.path(),
        &[
            "clone",
            "--quiet",
            "--no-hardlinks",
            moved.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    assert!(!clone.join(".git/gitust-repository").exists());
    assert_ne!(
        GitEngine::new(&clone)
            .load_or_create_repository_id()
            .unwrap(),
        id
    );
    #[cfg(unix)]
    {
        let alias = temp.path().join("path-alias");
        std::os::unix::fs::symlink(&moved, &alias).unwrap();
        assert_eq!(
            GitEngine::new(alias)
                .load_or_create_repository_id()
                .unwrap(),
            id
        );
    }
}

#[test]
fn invalid_identity_never_mints_a_replacement_or_swallows_an_incomplete_migration() {
    let (_temp, engine) = fixture();
    let id = engine.load_or_create_repository_id().unwrap();
    let path = identity_path(&engine);
    let original = fs::read(&path).unwrap();
    let candidates = [
        Vec::new(),
        b"not json".to_vec(),
        vec![b' '; 4097],
        serde_json::to_vec(&serde_json::json!({"version":2,"repo_id":id})).unwrap(),
        serde_json::to_vec(
            &serde_json::json!({"version":1,"repo_id":"00000000-0000-0000-0000-000000000000"}),
        )
        .unwrap(),
        serde_json::to_vec(&serde_json::json!({"version":1,"repo_id":id,"extra":true})).unwrap(),
    ];
    for bytes in candidates {
        fs::write(&path, &bytes).unwrap();
        assert!(engine.load_or_create_repository_id().is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    fs::write(&path, &original).unwrap();
    assert_eq!(engine.load_or_create_repository_id().unwrap(), id);
    fs::remove_file(&path).unwrap();
    let staging = path.with_file_name("identity.pending");
    fs::write(&staging, &original).unwrap();
    assert!(GitEngine::new(engine.repo_path())
        .load_or_create_repository_id()
        .is_err());
    assert!(!path.exists());
    assert_eq!(fs::read(staging).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn unsafe_permissions_symlinks_and_hardlinks_are_rejected_without_repairing_them() {
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    let (temp, engine) = fixture();
    let id = engine.load_or_create_repository_id().unwrap();
    let path = identity_path(&engine);
    let directory = path.parent().unwrap().to_path_buf();
    assert_eq!(fs::metadata(&directory).unwrap().mode() & 0o7777, 0o700);
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o600);
    for (target, unsafe_mode, safe_mode) in [(&path, 0o644, 0o600), (&directory, 0o755, 0o700)] {
        fs::set_permissions(target, fs::Permissions::from_mode(unsafe_mode)).unwrap();
        assert!(engine.load_or_create_repository_id().is_err());
        assert_eq!(fs::metadata(target).unwrap().mode() & 0o7777, unsafe_mode);
        fs::set_permissions(target, fs::Permissions::from_mode(safe_mode)).unwrap();
    }
    let link = temp.path().join("identity-link");
    fs::hard_link(&path, &link).unwrap();
    assert!(engine.load_or_create_repository_id().is_err());
    fs::remove_file(&link).unwrap();
    assert_eq!(engine.load_or_create_repository_id().unwrap(), id);
    fs::rename(&path, &link).unwrap();
    symlink(&link, &path).unwrap();
    assert!(engine.load_or_create_repository_id().is_err());
    assert!(path.is_symlink());
    fs::remove_file(&path).unwrap();
    fs::rename(&link, &path).unwrap();
    let moved = temp.path().join("private-identity");
    fs::rename(&directory, &moved).unwrap();
    symlink(&moved, &directory).unwrap();
    assert!(engine.load_or_create_repository_id().is_err());
    assert!(directory.is_symlink());
    assert!(moved.join("identity.json").is_file());
}
