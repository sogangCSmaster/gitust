//! Slice 6 (Phase 4.1): Source-of-Truth Mode with Upstream Backup Mirror and Verified Rollback.

#[path = "source_of_truth_rollback.rs"]
mod rollback;

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use chrono::Utc;
use gitust_core::{CommitSha, RepoId};
use gitust_sync::{
    BackupMirrorPushReport, BackupMirrorSnapshot, MirrorRefPin, SyncTopologyMode, SyncTopologyState,
};
use uuid::Uuid;

use crate::ancestry::is_agent_branch_short;
use crate::process::{git_command, run_capped, run_capped_with_input, sanitize_git_environment};
use crate::{io_err, run_git, GitEngine, GitEngineError};

const MAX_SYNC_SNAPSHOTS: usize = 32;
const MAX_TOPOLOGY_BYTES: u64 = 1024 * 1024;
const SYNC_DIR_NAME: &str = "gitust-sync";
const TOPOLOGY_FILE_NAME: &str = "topology.json";

pub struct PreparedBackupMirrorPush {
    pub repo_id: RepoId,
    pub display_url: String,
    pub clean_url: String,
    pub snapshot: BackupMirrorSnapshot,
    pub pushed_branches: u32,
    pub pushed_tags: u32,
    pub filtered_agent_refs: u32,
    staging: tempfile::TempDir,
}

pub(super) fn sync_state_err(msg: impl Into<String>) -> GitEngineError {
    GitEngineError::ApprovalState(msg.into())
}

fn validate_hex_sha(sha: &str) -> Result<(), GitEngineError> {
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(sync_state_err(format!("invalid object SHA '{sha}'")));
    }
    Ok(())
}

pub fn validate_mirror_ref_pin(pin: &MirrorRefPin) -> Result<(), GitEngineError> {
    validate_hex_sha(&pin.target_sha.0)?;
    if let Some(branch) = pin.ref_name.strip_prefix("refs/heads/") {
        let clean = GitEngine::validate_outbound_branch(branch)
            .map_err(|_| sync_state_err(format!("invalid branch ref pin '{}'", pin.ref_name)))?;
        if clean != branch {
            return Err(sync_state_err(format!(
                "non-canonical branch ref '{}'",
                pin.ref_name
            )));
        }
        return Ok(());
    }
    if let Some(tag) = pin.ref_name.strip_prefix("refs/tags/") {
        let clean = GitEngine::validate_branch_name(tag)
            .map_err(|_| sync_state_err(format!("invalid tag ref pin '{}'", pin.ref_name)))?;
        if clean != tag {
            return Err(sync_state_err(format!(
                "non-canonical tag ref '{}'",
                pin.ref_name
            )));
        }
        return Ok(());
    }
    Err(sync_state_err(format!(
        "unsupported mirror ref pin '{}'",
        pin.ref_name
    )))
}

pub(super) fn mirror_transport_command(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    sanitize_git_environment(&mut cmd);
    cmd.arg("-C").arg(dir).args([
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "protocol.allow=never",
        "-c",
        "protocol.https.allow=always",
        "-c",
        "protocol.ssh.allow=always",
        "-c",
        "protocol.file.allow=always",
        "-c",
        "http.followRedirects=false",
        "-c",
        "http.lowSpeedLimit=1000",
        "-c",
        "http.lowSpeedTime=30",
        "-c",
        "credential.helper=",
        "-c",
        "core.sshCommand=ssh -o BatchMode=yes -o ConnectTimeout=15 -o ServerAliveInterval=15 -o ServerAliveCountMax=2",
    ]);
    cmd
}

impl GitEngine {
    pub fn load_sync_topology(
        &self,
        repo_id: RepoId,
        default_remote_url: &str,
    ) -> Result<SyncTopologyState, GitEngineError> {
        let _service = self.lock_worktree_service()?;
        let common = self.repository_common_directory()?;
        let dir = common.join(SYNC_DIR_NAME);
        match std::fs::symlink_metadata(&dir) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.file_type().is_dir() => {
                return Err(sync_state_err(
                    "sync topology directory is not a safe directory",
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(SyncTopologyState {
                    repo_id,
                    mode: SyncTopologyMode::InboundMirror,
                    backup_remote_url: default_remote_url.trim().to_owned(),
                    snapshots: Vec::new(),
                    last_push_report: None,
                });
            }
            Err(e) => return Err(io_err(&dir, &e)),
        }
        let path = dir.join(TOPOLOGY_FILE_NAME);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) => {
                if meta.file_type().is_symlink()
                    || !meta.file_type().is_file()
                    || meta.len() > MAX_TOPOLOGY_BYTES
                {
                    return Err(sync_state_err(
                        "sync topology file is a symlink, non-regular file, or exceeds size limit",
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(SyncTopologyState {
                    repo_id,
                    mode: SyncTopologyMode::InboundMirror,
                    backup_remote_url: default_remote_url.trim().to_owned(),
                    snapshots: Vec::new(),
                    last_push_report: None,
                });
            }
            Err(e) => return Err(io_err(&path, &e)),
        }
        let file = std::fs::File::open(&path).map_err(|e| io_err(&path, &e))?;
        let mut bytes = Vec::new();
        file.take(MAX_TOPOLOGY_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|e| io_err(&path, &e))?;
        if (bytes.len() as u64) > MAX_TOPOLOGY_BYTES {
            return Err(sync_state_err("sync topology file exceeds size limit"));
        }
        let mut state: SyncTopologyState = serde_json::from_slice(&bytes)
            .map_err(|e| sync_state_err(format!("corrupt sync topology state: {e}")))?;
        if state.snapshots.len() > MAX_SYNC_SNAPSHOTS {
            return Err(sync_state_err("sync topology snapshot count exceeds limit"));
        }
        for snap in &state.snapshots {
            for pin in &snap.pinned_refs {
                validate_mirror_ref_pin(pin)?;
            }
        }
        state.repo_id = repo_id;
        if state.backup_remote_url.trim().is_empty() {
            state.backup_remote_url = default_remote_url.trim().to_owned();
        }
        Ok(state)
    }

    fn persist_sync_topology(
        &self,
        common: &Path,
        state: &SyncTopologyState,
    ) -> Result<(), GitEngineError> {
        let dir = common.join(SYNC_DIR_NAME);
        match std::fs::symlink_metadata(&dir) {
            Ok(meta) => {
                if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
                    return Err(sync_state_err(
                        "sync topology directory is not a safe directory",
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(&dir).map_err(|err| io_err(&dir, &err))?;
            }
            Err(e) => return Err(io_err(&dir, &e)),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| io_err(&dir, &e))?;
        }
        let tmp = dir.join("topology.json.tmp");
        let final_path = dir.join(TOPOLOGY_FILE_NAME);
        match std::fs::symlink_metadata(&final_path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(sync_state_err("sync topology file cannot be a symlink"));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err(&final_path, &e)),
        }
        let bytes = serde_json::to_vec(state)
            .map_err(|e| sync_state_err(format!("failed to serialize sync topology: {e}")))?;
        std::fs::write(&tmp, &bytes).map_err(|e| io_err(&tmp, &e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| io_err(&tmp, &e))?;
        }
        std::fs::rename(&tmp, &final_path).map_err(|e| io_err(&final_path, &e))?;
        Ok(())
    }

    fn capture_ordinary_ref_snapshot(
        &self,
        repo_id: RepoId,
        backup_remote_url: &str,
    ) -> Result<(BackupMirrorSnapshot, u32, u32, u32), GitEngineError> {
        let out = run_git(
            &self.repo_path,
            [
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads/",
                "refs/tags/",
            ],
        )?;
        let text = String::from_utf8(out)
            .map_err(|e| sync_state_err(format!("non-UTF-8 ref listing: {e}")))?;

        let mut pinned_refs = Vec::new();
        let mut ordinary_branches: u32 = 0;
        let mut ordinary_tags: u32 = 0;
        let mut filtered_agent_refs: u32 = 0;

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some((ref_name, sha)) = trimmed.split_once(' ') else {
                return Err(sync_state_err("malformed for-each-ref line"));
            };
            if let Some(short) = ref_name.strip_prefix("refs/heads/") {
                if is_agent_branch_short(short) {
                    filtered_agent_refs = filtered_agent_refs.saturating_add(1);
                    continue;
                }
                let pin = MirrorRefPin {
                    ref_name: ref_name.to_owned(),
                    target_sha: CommitSha(sha.to_owned()),
                };
                validate_mirror_ref_pin(&pin)?;
                ordinary_branches = ordinary_branches.saturating_add(1);
                pinned_refs.push(pin);
            } else if ref_name.starts_with("refs/tags/") {
                let pin = MirrorRefPin {
                    ref_name: ref_name.to_owned(),
                    target_sha: CommitSha(sha.to_owned()),
                };
                validate_mirror_ref_pin(&pin)?;
                ordinary_tags = ordinary_tags.saturating_add(1);
                pinned_refs.push(pin);
            }
        }

        let snapshot = BackupMirrorSnapshot {
            snapshot_id: Uuid::new_v4(),
            repo_id,
            backup_remote_url: backup_remote_url.to_owned(),
            pinned_refs,
            captured_at: Utc::now(),
        };
        Ok((
            snapshot,
            ordinary_branches,
            ordinary_tags,
            filtered_agent_refs,
        ))
    }

    pub fn configure_sync_topology_with_gate(
        &self,
        repo_id: RepoId,
        mode: SyncTopologyMode,
        backup_remote_url: &str,
        gate: impl FnOnce() -> Result<(), GitEngineError>,
    ) -> Result<SyncTopologyState, GitEngineError> {
        let _clean_url = Self::validate_mirror_remote_url(backup_remote_url)?;
        let display_url = backup_remote_url.trim().to_owned();
        let _service = self.lock_worktree_service()?;
        let common = self.repository_common_directory()?;
        let mut state = self.load_sync_topology(repo_id, &display_url)?;
        let (snapshot, _, _, _) = self.capture_ordinary_ref_snapshot(repo_id, &display_url)?;
        gate()?;

        state.repo_id = repo_id;
        state.mode = mode;
        state.backup_remote_url = display_url;
        state.snapshots.insert(0, snapshot);
        state.snapshots.truncate(MAX_SYNC_SNAPSHOTS);
        self.persist_sync_topology(&common, &state)?;
        Ok(state)
    }

    pub fn prepare_backup_mirror_push(
        &self,
        repo_id: RepoId,
        default_remote_url: &str,
    ) -> Result<PreparedBackupMirrorPush, GitEngineError> {
        let _service = self.lock_worktree_service()?;
        let common = self.repository_common_directory()?;
        let topology = self.load_sync_topology(repo_id, default_remote_url)?;
        if topology.mode != SyncTopologyMode::SourceOfTruth {
            return Err(sync_state_err(
                "repository is not in source_of_truth topology mode",
            ));
        }
        let clean_url = Self::validate_mirror_remote_url(&topology.backup_remote_url)?;
        let display_url = topology.backup_remote_url.trim().to_owned();
        let (snapshot, pushed_branches, pushed_tags, filtered_agent_refs) =
            self.capture_ordinary_ref_snapshot(repo_id, &display_url)?;

        let staging = tempfile::tempdir().map_err(|e| io_err(&self.repo_path, &e))?;
        let mut init = mirror_transport_command(staging.path());
        init.args(["init", "--bare", "--template=", "-q"]);
        run_capped(init, self.fetch_timeout, 64 * 1024)?;

        let objects =
            std::fs::canonicalize(common.join("objects")).map_err(|e| io_err(&common, &e))?;
        let objects_str = objects
            .to_str()
            .filter(|p| !p.contains(['\n', '\r']))
            .ok_or_else(|| GitEngineError::InvalidRepoPath("invalid object directory".into()))?;
        let alternates = staging.path().join("objects/info/alternates");
        std::fs::write(&alternates, format!("{objects_str}\n"))
            .map_err(|e| io_err(&alternates, &e))?;

        if !snapshot.pinned_refs.is_empty() {
            let mut seed_script = String::from("start\n");
            for pin in &snapshot.pinned_refs {
                validate_mirror_ref_pin(pin)?;
                seed_script.push_str(&format!("create {} {}\n", pin.ref_name, pin.target_sha.0));
            }
            seed_script.push_str("prepare\ncommit\n");
            let mut seed_cmd = git_command(staging.path());
            seed_cmd.args(["update-ref", "--stdin"]);
            run_capped_with_input(
                seed_cmd,
                Duration::from_secs(60),
                64 * 1024,
                Some(seed_script.into_bytes()),
            )?;
        }

        Ok(PreparedBackupMirrorPush {
            repo_id,
            display_url,
            clean_url,
            snapshot,
            pushed_branches,
            pushed_tags,
            filtered_agent_refs,
            staging,
        })
    }

    pub fn execute_backup_mirror_push(
        &self,
        prepared: &PreparedBackupMirrorPush,
    ) -> Result<(), GitEngineError> {
        let mut push = mirror_transport_command(prepared.staging.path());
        push.args([
            "push",
            "--porcelain",
            "--prune",
            "--no-follow-tags",
            "--recurse-submodules=no",
            "--",
            &prepared.clean_url,
            "+refs/heads/*:refs/heads/*",
            "+refs/tags/*:refs/tags/*",
        ]);
        run_capped(push, self.fetch_timeout, 1024 * 1024)?;

        let mut ls_remote = mirror_transport_command(prepared.staging.path());
        ls_remote.args(["ls-remote", "--refs", "--", &prepared.clean_url]);
        let (ls_bytes, truncated) = run_capped(ls_remote, self.fetch_timeout, 8 * 1024 * 1024)?;
        if truncated {
            return Err(sync_state_err("backup mirror ls-remote output truncated"));
        }
        let ls_text = String::from_utf8(ls_bytes)
            .map_err(|e| sync_state_err(format!("non-UTF-8 ls-remote output: {e}")))?;

        let mut remote_refs = BTreeMap::new();
        for line in ls_text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some((sha, rname)) = trimmed.split_once('\t') else {
                continue;
            };
            if let Some(short) = rname.strip_prefix("refs/heads/") {
                if is_agent_branch_short(short) {
                    return Err(sync_state_err(
                        "private agent branch detected on backup mirror",
                    ));
                }
                remote_refs.insert(rname.to_owned(), sha.to_owned());
            } else if rname.starts_with("refs/tags/") {
                remote_refs.insert(rname.to_owned(), sha.to_owned());
            }
        }

        if remote_refs.len() != prepared.snapshot.pinned_refs.len() {
            return Err(sync_state_err(format!(
                "backup mirror ref count mismatch: expected {}, got {}",
                prepared.snapshot.pinned_refs.len(),
                remote_refs.len()
            )));
        }
        for pin in &prepared.snapshot.pinned_refs {
            let Some(remote_sha) = remote_refs.get(&pin.ref_name) else {
                return Err(sync_state_err(format!(
                    "backup mirror is missing pushed ref '{}'",
                    pin.ref_name
                )));
            };
            if remote_sha != &pin.target_sha.0 {
                return Err(sync_state_err(format!(
                    "backup mirror ref '{}' mismatch: expected {}, got {}",
                    pin.ref_name, pin.target_sha.0, remote_sha
                )));
            }
        }
        Ok(())
    }

    pub fn finalize_backup_mirror_push_with_gate(
        &self,
        prepared: PreparedBackupMirrorPush,
        default_remote_url: &str,
        gate: impl FnOnce() -> Result<(), GitEngineError>,
    ) -> Result<BackupMirrorPushReport, GitEngineError> {
        let _service = self.lock_worktree_service()?;
        let common = self.repository_common_directory()?;
        let mut topology = self.load_sync_topology(prepared.repo_id, default_remote_url)?;
        if topology.mode != SyncTopologyMode::SourceOfTruth {
            return Err(sync_state_err(
                "repository is not in source_of_truth topology mode",
            ));
        }
        gate()?;

        let report = BackupMirrorPushReport {
            repo_id: prepared.repo_id,
            backup_remote_url: prepared.display_url,
            snapshot_id: prepared.snapshot.snapshot_id,
            pushed_branches: prepared.pushed_branches,
            pushed_tags: prepared.pushed_tags,
            filtered_agent_refs: prepared.filtered_agent_refs,
            verified_remote_match: true,
            pushed_at: Utc::now(),
        };
        topology.snapshots.insert(0, prepared.snapshot);
        topology.snapshots.truncate(MAX_SYNC_SNAPSHOTS);
        topology.last_push_report = Some(report.clone());
        self.persist_sync_topology(&common, &topology)?;
        Ok(report)
    }
}
