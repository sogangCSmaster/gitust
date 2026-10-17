//! Durable diagnostics are execution intents, never restored user credentials or approval.
use chrono::{DateTime, Utc};
use gitust_core::{CommitSha, RepoId, SandboxRunId, UserId, WorktreeId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Origin {
    Mcp,
    GitHttp,
    LocalOperator,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Purpose {
    AutomaticChecks,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Prepared,
    Queued,
    Running,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
    Interrupted,
    Rejected,
    Superseded,
    NotApplied,
    Blocked,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reason {
    AuthorityUnavailable,
    ExecutionDisabled,
    StateChanged,
    TargetChanged,
    TargetUnavailable,
    PolicyChanged,
    PolicyUnavailable,
    OperatorContextEnded,
    ServiceStopped,
    UnconfirmedExecution,
    CommandFailed,
    ExecutionFailed,
    ValidationFailed,
    TimedOut,
    GitNotApplied,
    Superseded,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Job {
    pub run_id: SandboxRunId,
    pub user_id: UserId,
    pub origin: Origin,
    pub purpose: Purpose,
    pub expected: CommitSha,
    pub previous: Option<CommitSha>,
    pub checks_oid: String,
    pub phase: Phase,
    pub reason: Option<Reason>,
    pub created_at: DateTime<Utc>,
    pub changed_at: DateTime<Utc>,
}
impl Job {
    pub fn new(
        user_id: UserId,
        origin: Origin,
        expected: CommitSha,
        previous: Option<CommitSha>,
        checks_oid: String,
    ) -> Self {
        let now = Utc::now();
        Self {
            run_id: SandboxRunId::new(),
            user_id,
            origin,
            purpose: Purpose::AutomaticChecks,
            expected,
            previous,
            checks_oid,
            phase: Phase::Prepared,
            reason: None,
            created_at: now,
            changed_at: now,
        }
    }
    pub fn transition(&mut self, phase: Phase) {
        self.phase = phase;
        self.reason = match phase {
            Phase::Prepared | Phase::Queued | Phase::Running | Phase::Completed => None,
            Phase::Failed => Some(Reason::ExecutionFailed),
            Phase::TimedOut => Some(Reason::TimedOut),
            Phase::Cancelled => Some(Reason::ServiceStopped),
            Phase::Interrupted => Some(Reason::UnconfirmedExecution),
            Phase::Rejected => Some(Reason::AuthorityUnavailable),
            Phase::Superseded => Some(Reason::Superseded),
            Phase::NotApplied => Some(Reason::GitNotApplied),
            Phase::Blocked => Some(Reason::TargetUnavailable),
        };
        self.changed_at = Utc::now().max(self.changed_at);
    }
    fn valid(&self) -> bool {
        !self.run_id.0.is_nil()
            && !self.user_id.0.is_nil()
            && oid(&self.expected.0)
            && self.previous.as_ref().is_none_or(|s| oid(&s.0))
            && oid(&self.checks_oid)
            && self.changed_at >= self.created_at
            && match self.phase {
                Phase::Prepared | Phase::Queued | Phase::Running | Phase::Completed => {
                    self.reason.is_none()
                }
                _ => self.reason.is_some(),
            }
    }
}
fn oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value.bytes().all(|b| b.is_ascii_hexdigit())
        && value.bytes().any(|b| b != b'0')
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Document {
    pub version: u32,
    pub repo_id: RepoId,
    pub worktree_id: WorktreeId,
    pub branch: String,
    pub base_branch: String,
    pub prepared: Option<Job>,
    pub queued: Option<Job>,
    pub running: Option<Job>,
    pub recent: Vec<Job>,
}
impl Document {
    pub fn new(repo_id: RepoId, id: WorktreeId, branch: &str, base: &str) -> Self {
        Self {
            version: 1,
            repo_id,
            worktree_id: id,
            branch: branch.into(),
            base_branch: base.into(),
            prepared: None,
            queued: None,
            running: None,
            recent: Vec::new(),
        }
    }
    pub fn valid(&self, repo: RepoId, id: WorktreeId) -> bool {
        self.version == 1
            && self.repo_id == repo
            && !repo.0.is_nil()
            && self.worktree_id == id
            && !id.0.is_nil()
            && matches!(gitust_git::GitEngine::validate_agent_branch_name(&self.branch), Ok(value) if value == self.branch)
            && matches!(gitust_git::GitEngine::validate_outbound_branch(&self.base_branch), Ok(value) if value == self.base_branch)
            && self.branch.starts_with("agent/")
            && self.branch.len() <= 256
            && !self.branch.chars().any(char::is_control)
            && self.base_branch.len() <= 256
            && self.recent.len() <= 5
            && {
                let mut ids = std::collections::HashSet::new();
                self.prepared
                    .iter()
                    .chain(self.queued.iter())
                    .chain(self.running.iter())
                    .chain(self.recent.iter())
                    .all(|j| ids.insert(j.run_id))
            }
            && self
                .prepared
                .as_ref()
                .is_none_or(|j| j.valid() && j.phase == Phase::Prepared)
            && self
                .queued
                .as_ref()
                .is_none_or(|j| j.valid() && j.phase == Phase::Queued)
            && self
                .running
                .as_ref()
                .is_none_or(|j| j.valid() && j.phase == Phase::Running)
            && self.recent.iter().all(|j| {
                j.valid() && !matches!(j.phase, Phase::Prepared | Phase::Queued | Phase::Running)
            })
    }
    pub fn finish(&mut self, mut job: Job, phase: Phase) {
        job.transition(phase);
        self.recent.insert(0, job);
        self.recent.truncate(5);
    }
    pub fn finish_reason(&mut self, mut job: Job, phase: Phase, reason: Reason) {
        job.transition(phase);
        job.reason = Some(reason);
        self.recent.insert(0, job);
        self.recent.truncate(5);
    }
    pub fn queue_prepared(&mut self) {
        if let Some(mut next) = self.prepared.take() {
            if let Some(old) = self.queued.take() {
                self.finish(old, Phase::Superseded);
            }
            next.transition(Phase::Queued);
            self.queued = Some(next);
        }
    }
    pub fn records(&self) -> Vec<Job> {
        self.prepared
            .iter()
            .chain(self.queued.iter())
            .chain(self.running.iter())
            .chain(self.recent.iter())
            .cloned()
            .collect()
    }
}
