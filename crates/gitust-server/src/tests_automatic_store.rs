use crate::automatic_run_model::{Document, Job, Origin, Phase};
use crate::automatic_run_store::{Store, DIRECTORY};
use axum::http::StatusCode;
use gitust_core::{CommitSha, RepoId, UserId, WorktreeId};
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Arc;

const BRANCH: &str = "agent/durable-store-test";
const BASE: &str = "main";

struct Fixture {
    temp: tempfile::TempDir,
    repo: RepoId,
    id: WorktreeId,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let repo = RepoId::new();
        let id = WorktreeId::new();
        let store = Store::open(temp.path(), repo).unwrap();
        store
            .update(id, BRANCH, BASE, |doc| {
                doc.prepared = Some(job('a'));
                doc.queue_prepared();
                Ok(())
            })
            .unwrap();
        Self {
            temp,
            repo,
            id,
            store,
        }
    }

    fn document_path(&self) -> PathBuf {
        self.store.path().join(format!("{}.json", self.id.0))
    }

    fn reopen(&self) -> Result<Store, (StatusCode, String)> {
        Store::open(self.temp.path(), self.repo)
    }

    fn document(&self) -> Document {
        let mut documents = self.store.documents().unwrap();
        assert_eq!(documents.len(), 1);
        documents.remove(0)
    }
}

fn job(digit: char) -> Job {
    Job::new(
        UserId::new(),
        Origin::GitHttp,
        CommitSha(digit.to_string().repeat(40)),
        Some(CommitSha("1".repeat(40))),
        "2".repeat(40),
    )
}

#[test]
fn reopening_preserves_exact_bindings_and_never_requeues_a_running_attempt() {
    let fixture = Fixture::new();
    let queued = fixture.document().queued.unwrap();
    fixture
        .store
        .update(fixture.id, BRANCH, BASE, |doc| {
            let mut running = doc.queued.take().unwrap();
            running.transition(Phase::Running);
            doc.running = Some(running);
            doc.prepared = Some(job('b'));
            doc.queue_prepared();
            Ok(())
        })
        .unwrap();
    let original = fs::read(fixture.document_path()).unwrap();
    let reopened = fixture.reopen().unwrap();
    let document = reopened.documents().unwrap().remove(0);
    let running = document.running.unwrap();
    assert_eq!(running.run_id, queued.run_id);
    assert_eq!(running.phase, Phase::Running);
    assert_eq!(running.expected, queued.expected);
    assert_eq!(document.queued.unwrap().expected.0, "b".repeat(40));
    assert_eq!(document.repo_id, fixture.repo);
    assert_eq!(document.worktree_id, fixture.id);
    assert_eq!(document.branch, BRANCH);
    assert_eq!(document.base_branch, BASE);
    assert!(Store::open(fixture.temp.path(), RepoId::new()).is_err());
    for (branch, base) in [("agent/other", BASE), (BRANCH, "release")] {
        assert!(reopened
            .update(fixture.id, branch, base, |_| Ok(()))
            .is_err());
        assert_eq!(fs::read(fixture.document_path()).unwrap(), original);
    }
}

#[test]
fn malformed_or_misbound_documents_fail_closed_without_replacing_evidence() {
    let fixture = Fixture::new();
    let original: Value =
        serde_json::from_slice(&fs::read(fixture.document_path()).unwrap()).unwrap();
    let mut invalid = Vec::new();
    for (field, value) in [
        ("version", json!(2)),
        ("repo_id", json!(RepoId::new())),
        ("repo_id", json!(uuid::Uuid::nil())),
        ("worktree_id", json!(WorktreeId::new())),
        ("worktree_id", json!(uuid::Uuid::nil())),
        ("branch", json!("main")),
        ("branch", json!("agent/bad\nbranch")),
        ("unexpected", json!(true)),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        invalid.push(serde_json::to_vec(&changed).unwrap());
    }
    for (field, value) in [
        ("run_id", json!(uuid::Uuid::nil())),
        ("user_id", json!(uuid::Uuid::nil())),
        ("phase", json!("running")),
        ("purpose", json!("login")),
        ("expected", json!("not-a-commit")),
        ("checks_oid", json!("g".repeat(40))),
        ("changed_at", json!("2000-01-01T00:00:00Z")),
    ] {
        let mut changed = original.clone();
        changed["queued"][field] = value;
        invalid.push(serde_json::to_vec(&changed).unwrap());
    }
    let mut duplicate = original.clone();
    duplicate["running"] = duplicate["queued"].clone();
    duplicate["running"]["phase"] = json!("running");
    invalid.push(serde_json::to_vec(&duplicate).unwrap());
    invalid.push(b"{\"version\":1,".to_vec());
    for bytes in invalid {
        fs::write(fixture.document_path(), &bytes).unwrap();
        assert!(fixture.reopen().is_err());
        assert!(fixture.store.documents().is_err());
        assert!(fixture
            .store
            .update(fixture.id, BRANCH, BASE, |_| Ok(()))
            .is_err());
        assert_eq!(fs::read(fixture.document_path()).unwrap(), bytes);
    }
}

#[test]
fn private_directory_header_and_document_modes_are_enforced_on_reopen_and_use() {
    for leaf in [None, Some("store.json"), Some("document")] {
        let fixture = Fixture::new();
        let path = match leaf {
            None => fixture.store.path().to_owned(),
            Some("document") => fixture.document_path(),
            Some(name) => fixture.store.path().join(name),
        };
        let initial = fs::metadata(&path).unwrap().mode() & 0o7777;
        assert_eq!(initial, if leaf.is_none() { 0o700 } else { 0o600 });
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(fixture.reopen().is_err());
        if leaf != Some("store.json") {
            assert!(fixture.store.documents().is_err());
            assert!(fixture
                .store
                .update(fixture.id, BRANCH, BASE, |_| Ok(()))
                .is_err());
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(initial)).unwrap();
        fixture.reopen().unwrap();
    }
}

#[test]
fn symlink_and_hardlink_metadata_aliases_are_rejected_without_following_them() {
    for header in [false, true] {
        for hardlink in [false, true] {
            let fixture = Fixture::new();
            let path = if header {
                fixture.store.path().join("store.json")
            } else {
                fixture.document_path()
            };
            let original = fs::read(&path).unwrap();
            let outside = fixture.temp.path().join("outside-metadata");
            fs::rename(&path, &outside).unwrap();
            if hardlink {
                fs::hard_link(&outside, &path).unwrap();
            } else {
                symlink(&outside, &path).unwrap();
            }
            assert!(fixture.reopen().is_err());
            if !header {
                assert!(fixture.store.documents().is_err());
                assert!(fixture
                    .store
                    .update(fixture.id, BRANCH, BASE, |_| Ok(()))
                    .is_err());
            }
            assert_eq!(fs::read(&outside).unwrap(), original);
        }
    }
    let fixture = Fixture::new();
    let alias = tempfile::tempdir().unwrap();
    symlink(fixture.store.path(), alias.path().join(DIRECTORY)).unwrap();
    assert!(Store::open(alias.path(), fixture.repo).is_err());
}

#[test]
fn oversized_files_unsafe_staging_and_disappearing_known_documents_fail_closed() {
    let fixture = Fixture::new();
    let path = fixture.document_path();
    let original = fs::read(&path).unwrap();
    let oversized = vec![b' '; 64 * 1024 + 1];
    fs::write(&path, &oversized).unwrap();
    assert!(fixture.reopen().is_err());
    assert!(fixture.store.documents().is_err());
    assert!(fixture
        .store
        .update(fixture.id, BRANCH, BASE, |_| Ok(()))
        .is_err());
    assert_eq!(fs::read(&path).unwrap(), oversized);
    fs::write(&path, &original).unwrap();
    let staging = fixture
        .store
        .path()
        .join(format!("{}.json.pending", fixture.id.0));
    symlink(&path, &staging).unwrap();
    assert!(fixture.reopen().is_err());
    assert!(fixture
        .store
        .update(fixture.id, BRANCH, BASE, |_| Ok(()))
        .is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    fs::remove_file(staging).unwrap();
    fs::remove_file(&path).unwrap();
    assert!(fixture.store.documents().is_err());
    assert!(fixture
        .store
        .update(fixture.id, BRANCH, BASE, |_| Ok(()))
        .is_err());
    assert!(!path.exists());
}

#[test]
fn rejecting_a_candidate_preserves_queued_work_and_only_acceptance_coalesces_it() {
    let fixture = Fixture::new();
    let original = fixture.document().queued.unwrap();
    let candidate = job('b');
    let candidate_id = candidate.run_id;
    fixture
        .store
        .update(fixture.id, BRANCH, BASE, |doc| {
            doc.prepared = Some(candidate);
            Ok(())
        })
        .unwrap();
    fixture
        .store
        .update(fixture.id, BRANCH, BASE, |doc| {
            let rejected = doc.prepared.take().unwrap();
            doc.finish(rejected, Phase::NotApplied);
            Ok(())
        })
        .unwrap();
    let document = fixture.reopen().unwrap().documents().unwrap().remove(0);
    assert_eq!(document.queued.unwrap().run_id, original.run_id);
    assert!(document
        .recent
        .iter()
        .any(|j| j.run_id == candidate_id && j.phase == Phase::NotApplied));
    fixture
        .store
        .update(fixture.id, BRANCH, BASE, |doc| {
            doc.prepared = Some(job('c'));
            doc.queue_prepared();
            Ok(())
        })
        .unwrap();
    let document = fixture.document();
    assert_eq!(document.queued.unwrap().expected.0, "c".repeat(40));
    assert!(document
        .recent
        .iter()
        .any(|j| j.run_id == original.run_id && j.phase == Phase::Superseded));
    assert!(document.prepared.is_none());
}

#[test]
fn rejected_changes_leave_disk_unchanged_and_concurrent_updates_preserve_live_slots() {
    let fixture = Fixture::new();
    let original = fs::read(fixture.document_path()).unwrap();
    let queued = fixture.document().queued.unwrap().run_id;
    assert!(fixture
        .store
        .update(fixture.id, BRANCH, BASE, |doc| {
            doc.queued = None;
            Err::<(), _>((
                StatusCode::CONFLICT,
                "injected prepublication failure".into(),
            ))
        })
        .is_err());
    assert_eq!(fs::read(fixture.document_path()).unwrap(), original);
    assert!(fixture
        .store
        .update(fixture.id, BRANCH, BASE, |doc| {
            doc.running = doc.queued.clone();
            doc.running.as_mut().unwrap().transition(Phase::Running);
            Ok(())
        })
        .is_err());
    assert_eq!(fs::read(fixture.document_path()).unwrap(), original);
    let id = fixture.id;
    let store = Arc::new(fixture.store);
    let mut writers = Vec::new();
    for _ in 0..12 {
        let store = store.clone();
        writers.push(std::thread::spawn(move || {
            store
                .update(id, BRANCH, BASE, |doc| {
                    doc.finish(job('d'), Phase::Rejected);
                    Ok(())
                })
                .unwrap();
        }));
    }
    for writer in writers {
        writer.join().unwrap();
    }
    let document = Store::open(fixture.temp.path(), fixture.repo)
        .unwrap()
        .documents()
        .unwrap()
        .remove(0);
    assert_eq!(document.queued.unwrap().run_id, queued);
    assert_eq!(document.recent.len(), 5);
    assert!(document.recent.iter().all(|j| j.phase == Phase::Rejected));
}

#[test]
fn repository_job_capacity_rejects_a_new_worktree_without_dropping_existing_intents() {
    let root = tempfile::tempdir().unwrap();
    let repo = gitust_core::RepoId::new();
    let store = crate::automatic_run_store::Store::open(root.path(), repo).unwrap();
    for _ in 0..crate::state::MAX_TRACKED_WORKTREES {
        let id = gitust_core::WorktreeId::new();
        let doc = crate::automatic_run_model::Document::new(repo, id, "agent/capacity", "main");
        let path = store.path().join(format!("{}.json", id.0));
        std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .unwrap();
    }
    let store = crate::automatic_run_store::Store::open(root.path(), repo).unwrap();
    assert_eq!(
        store.documents().unwrap().len(),
        crate::state::MAX_TRACKED_WORKTREES
    );
    let extra = gitust_core::WorktreeId::new();
    assert!(store
        .update(extra, "agent/overflow", "main", |_| Ok(()))
        .is_err());
    assert!(!store.path().join(format!("{}.json", extra.0)).exists());
    assert_eq!(
        store.documents().unwrap().len(),
        crate::state::MAX_TRACKED_WORKTREES
    );
}

#[test]
fn wall_clock_rollback_does_not_destroy_a_valid_running_transition() {
    let root = tempfile::tempdir().unwrap();
    let repo = gitust_core::RepoId::new();
    let store = crate::automatic_run_store::Store::open(root.path(), repo).unwrap();
    let id = gitust_core::WorktreeId::new();
    let future = chrono::Utc::now() + chrono::Duration::days(30);
    let mut job = crate::automatic_run_model::Job::new(
        gitust_core::UserId::new(),
        crate::automatic_run_model::Origin::Mcp,
        gitust_core::CommitSha("a".repeat(40)),
        None,
        "b".repeat(40),
    );
    job.created_at = future;
    job.changed_at = future;
    job.phase = crate::automatic_run_model::Phase::Queued;
    store
        .update(id, "agent/clock", "main", |doc| {
            doc.queued = Some(job);
            Ok(())
        })
        .unwrap();
    store
        .update(id, "agent/clock", "main", |doc| {
            let mut job = doc.queued.take().unwrap();
            job.transition(crate::automatic_run_model::Phase::Running);
            doc.running = Some(job);
            Ok(())
        })
        .unwrap();
    let reopened = crate::automatic_run_store::Store::open(root.path(), repo).unwrap();
    assert_eq!(
        reopened
            .document(id, "agent/clock", "main")
            .unwrap()
            .unwrap()
            .running
            .unwrap()
            .changed_at,
        future
    );
}
