//! Real Git owns the competing locks; public engine mutations must preserve its commits.
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Stdio},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use tempfile::TempDir;

use super::{git, init_repo, materialize};
use crate::GitEngine;

pub(crate) struct Writer {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    done: bool,
}

impl Writer {
    pub(crate) fn prepare(path: &Path, name: &str, new: &str, old: Option<&str>) -> Self {
        let mut command = crate::process::git_command(path);
        command
            .args(["update-ref", "--stdin", "-z", "--no-deref"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let instruction = match old {
            Some(old) => format!("start\0update {name}\0{new}\0{old}\0prepare\0"),
            None => format!("start\0create {name}\0{new}\0prepare\0"),
        };
        input.write_all(instruction.as_bytes()).unwrap();
        input.flush().unwrap();
        let mut writer = Self {
            child,
            input: Some(input),
            output,
            done: false,
        };
        writer.ack("start: ok\n");
        writer.ack("prepare: ok\n");
        writer
    }

    fn ack(&mut self, expected: &str) {
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        assert_eq!(line, expected);
    }

    fn commit(mut self) {
        let mut input = self.input.take().unwrap();
        input.write_all(b"commit\0").unwrap();
        input.flush().unwrap();
        drop(input);
        self.ack("commit: ok\n");
        assert!(self.child.wait().unwrap().success());
        self.done = true;
    }

    pub(crate) fn delayed(self) -> JoinHandle<()> {
        std::thread::spawn(move || {
            // The competing transaction was prepared before the engine operation begins.
            // It publishes during the operation's lock wait, without changing engine state.
            std::thread::sleep(Duration::from_millis(60));
            self.commit();
        })
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        if !self.done {
            if let Some(mut input) = self.input.take() {
                let _ = input.write_all(b"abort\0");
            }
            let _ = self.child.wait();
        }
    }
}

pub(crate) fn bare_fixture() -> (TempDir, PathBuf, String) {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("source");
    init_repo(&source);
    let bare = temp.path().join("repo.git");
    git(
        temp.path(),
        &[
            "clone",
            "--bare",
            source.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let base = git(&bare, &["rev-parse", "refs/heads/main"]);
    (temp, bare, base)
}

pub(crate) fn child_commit(path: &Path, base: &str, message: &str) -> String {
    let tree = git(path, &["rev-parse", &format!("{base}^{{tree}}")]);
    git(path, &["commit-tree", &tree, "-p", base, "-m", message])
}

#[test]
fn approval_ref_cas_preserves_a_concurrent_git_commit_in_loose_and_packed_refs() {
    for packed in [false, true] {
        let (_temp, bare, base) = bare_fixture();
        let engine = GitEngine::new(&bare);
        let task = materialize(&engine, "agent/test/approval-cas").unwrap();
        git(
            &task.worktree_dir,
            &["commit", "--allow-empty", "-m", "reviewed"],
        );
        let reviewed = git(&task.worktree_dir, &["rev-parse", "HEAD"]);
        let competing = child_commit(&bare, &base, "concurrent ordinary writer");
        if packed {
            git(&bare, &["pack-refs", "--all", "--prune"]);
        }
        let writer = Writer::prepare(&bare, "refs/heads/main", &competing, Some(&base));
        let writer = writer.delayed();
        let result = engine.merge_agent_branch(&task.worktree.branch, "main");
        writer.join().unwrap();
        assert!(
            result.is_err(),
            "stale approval must fail (packed={packed})"
        );
        assert_eq!(git(&bare, &["rev-parse", "main"]), competing);
        assert_eq!(git(&bare, &["rev-parse", &task.worktree.branch]), reviewed);
        assert!(!bare.join("refs/heads/main.lock").exists());
        GitEngine::verify_git_fsck(&bare).unwrap();
        GitEngine::verify_git_fsck(&task.worktree_dir).unwrap();
    }
}

#[test]
fn explicit_commit_ref_cas_preserves_the_concurrent_tip_and_original_index() {
    let (_temp, bare, base) = bare_fixture();
    let engine = GitEngine::new(&bare);
    let task = materialize(&engine, "agent/test/commit-cas").unwrap();
    fs::write(task.worktree_dir.join("new.txt"), "requested contents\n").unwrap();
    let original_index = fs::read(task.worktree_admin_dir.join("index")).unwrap();
    let prepared = engine
        .prepare_worktree_commit(
            task.worktree.id,
            &task.worktree.branch,
            &task.worktree.base_commit,
            &["new.txt".into()],
            "prepared operator commit",
            "test@example.test",
        )
        .unwrap();
    let competing = child_commit(&bare, &base, "concurrent task writer");
    let name = format!("refs/heads/{}", task.worktree.branch);
    let writer = Writer::prepare(&bare, &name, &competing, Some(&base)).delayed();
    let result = engine.finish_worktree_commit(prepared);
    writer.join().unwrap();
    assert!(result.is_err());
    assert_eq!(git(&bare, &["rev-parse", &name]), competing);
    assert_eq!(
        fs::read(task.worktree_admin_dir.join("index")).unwrap(),
        original_index
    );
    assert!(!task.worktree_admin_dir.join("index.lock").exists());
    GitEngine::verify_git_fsck(&bare).unwrap();
}

#[test]
fn mirror_cas_conflict_does_not_publish_other_members_of_the_failed_batch() {
    let (temp, bare, base) = bare_fixture();
    let source = temp.path().join("source");
    git(
        &source,
        &["commit", "--allow-empty", "-m", "upstream advance"],
    );
    git(&source, &["branch", "new-public"]);
    let engine = GitEngine::new(&bare);
    engine
        .fetch_inbound_mirror_staging(source.to_str().unwrap())
        .unwrap();
    let competing = child_commit(&bare, &base, "independent local advance");
    let writer = Writer::prepare(&bare, "refs/heads/main", &competing, Some(&base));
    let (release, begin) = std::sync::mpsc::sync_channel(1);
    let writer = std::thread::spawn(move || {
        begin.recv_timeout(Duration::from_secs(5)).unwrap();
        std::thread::sleep(Duration::from_millis(60));
        writer.commit();
    });
    let mut gates = 0;
    let result = engine.apply_inbound_mirror_refs_with_gate(
        source.to_str().unwrap(),
        Instant::now(),
        || {
            gates += 1;
            if gates == 2 {
                // Release only after ancestry classification and batch construction.
                release.send(()).unwrap();
            }
            Ok(())
        },
    );
    writer.join().unwrap();
    assert!(
        result.is_err(),
        "a failed batch must not become a skipped success"
    );
    assert_eq!(git(&bare, &["rev-parse", "main"]), competing);
    assert_eq!(git(&bare, &["for-each-ref", "refs/heads/new-public"]), "");
    assert!(!bare.join("refs/heads/new-public.lock").exists());
    GitEngine::verify_git_fsck(&bare).unwrap();
}

#[test]
fn create_only_ref_cas_preserves_a_concurrently_created_same_tip() {
    let (_temp, bare, base) = bare_fixture();
    let engine = GitEngine::new(&bare);
    let writer = Writer::prepare(&bare, "refs/heads/agent/test/create-cas", &base, None).delayed();
    let result = materialize(&engine, "agent/test/create-cas");
    writer.join().unwrap();
    assert!(
        result.is_err(),
        "an existing equal tip is still not a new branch"
    );
    assert_eq!(git(&bare, &["rev-parse", "agent/test/create-cas"]), base);
    assert!(engine.list_agent_worktrees().unwrap().is_empty());
    GitEngine::verify_git_fsck(&bare).unwrap();
}
