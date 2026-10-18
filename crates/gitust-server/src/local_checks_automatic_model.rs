//! Ordinary default-branch policy jobs are independent of managed-worktree authority.
use chrono::{DateTime, Utc};
use gitust_core::{CommitSha, RepoId, SandboxRunId, UserId};
use gitust_git::{GitEngine, MAX_BRANCH_NAME_BYTES};
use serde::{Deserialize, Serialize};

pub(crate) const DIRECTORY: &str = "gitust-local-check-automatic-runs";
pub(crate) const MAX_BYTES: u64 = 3 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Prepared,
    Queued,
    Running,
    Completed,
    Failed,
    Rejected,
    Superseded,
    Interrupted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reason {
    HistoryUnconfirmed,
    CommandFailed,
    ExecutionDisabled,
    AuthorityUnavailable,
    RepositoryUnavailable,
    PolicyDisabled,
    PolicyMissing,
    PolicyInvalid,
    PushNotConfirmed,
    LatestQueuedReplaced,
    DefaultChanged,
    TargetChanged,
    AcceptanceUnconfirmed,
    ServiceStopped,
    UnconfirmedExecution,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum History {
    Saved,
    Failed,
    NotAttempted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Purpose {
    LocalDefaultChecks,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Job {
    pub run_id: SandboxRunId,
    pub actor_id: UserId,
    purpose: Purpose,
    pub source_ref: String,
    pub previous_commit: CommitSha,
    pub expected_commit: CommitSha,
    pub consent_commit: CommitSha,
    pub consent_blob_oid: String,
    pub checks_blob_oid: Option<String>,
    pub phase: Phase,
    pub reason: Option<Reason>,
    pub history: History,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
impl Job {
    pub fn new(
        actor_id: UserId,
        source_ref: String,
        previous: CommitSha,
        expected: CommitSha,
        consent_blob: String,
    ) -> Self {
        let now = Utc::now();
        Self {
            run_id: SandboxRunId::new(),
            actor_id,
            purpose: Purpose::LocalDefaultChecks,
            source_ref,
            consent_commit: previous.clone(),
            previous_commit: previous,
            expected_commit: expected,
            consent_blob_oid: consent_blob,
            checks_blob_oid: None,
            phase: Phase::Prepared,
            reason: None,
            history: History::NotAttempted,
            created_at: now,
            updated_at: now,
        }
    }
    pub fn transition(&mut self, phase: Phase, reason: Option<Reason>, history: History) {
        self.phase = phase;
        self.reason = reason;
        self.history = history;
        self.updated_at = Utc::now().max(self.updated_at);
    }
    pub fn valid(&self) -> bool {
        let Some(short) = self.source_ref.strip_prefix("refs/heads/") else {
            return false;
        };
        let ordinary = !short.eq_ignore_ascii_case("agent")
            && !short
                .get(..6)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("agent/"));
        !self.run_id.0.is_nil()
            && !self.actor_id.0.is_nil()
            && short.len() <= MAX_BRANCH_NAME_BYTES
            && ordinary
            && GitEngine::validate_branch_name(short).is_ok_and(|name| name == short)
            && oid(&self.previous_commit.0)
            && oid(&self.expected_commit.0)
            && self.previous_commit != self.expected_commit
            && self.consent_commit == self.previous_commit
            && oid(&self.consent_blob_oid)
            && self.consent_blob_oid.len() == self.expected_commit.0.len()
            && self.previous_commit.0.len() == self.expected_commit.0.len()
            && self
                .checks_blob_oid
                .as_ref()
                .is_none_or(|value| oid(value) && value.len() == self.expected_commit.0.len())
            && self.updated_at >= self.created_at
            && self.valid_outcome()
    }
    fn valid_outcome(&self) -> bool {
        use Phase::*;
        use Reason::*;
        match self.phase {
            Prepared => {
                self.checks_blob_oid.is_none()
                    && self.reason.is_none()
                    && self.history == History::NotAttempted
            }
            Queued | Running => {
                self.checks_blob_oid.is_some()
                    && self.reason.is_none()
                    && self.history == History::NotAttempted
            }
            Completed | Failed => {
                self.checks_blob_oid.is_some()
                    && match self.history {
                        History::Saved => {
                            self.reason
                                == if self.phase == Completed {
                                    None
                                } else {
                                    Some(CommandFailed)
                                }
                        }
                        History::Failed => self.reason == Some(HistoryUnconfirmed),
                        History::NotAttempted => false,
                    }
            }
            Rejected => {
                self.history == History::NotAttempted
                    && matches!(
                        self.reason,
                        Some(
                            ExecutionDisabled
                                | AuthorityUnavailable
                                | RepositoryUnavailable
                                | PolicyDisabled
                                | PolicyMissing
                                | PolicyInvalid
                                | PushNotConfirmed
                        )
                    )
            }
            Superseded => {
                self.history == History::NotAttempted
                    && matches!(
                        self.reason,
                        Some(LatestQueuedReplaced | DefaultChanged | TargetChanged)
                    )
            }
            Interrupted => {
                self.history == History::NotAttempted
                    && matches!(
                        self.reason,
                        Some(AcceptanceUnconfirmed | ServiceStopped | UnconfirmedExecution)
                    )
            }
        }
    }
    pub fn terminal(&self) -> bool {
        !matches!(self.phase, Phase::Prepared | Phase::Queued | Phase::Running)
    }
}
fn oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && value.bytes().any(|b| b != b'0')
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Document {
    pub version: u32,
    pub repo_id: RepoId,
    pub prepared: Option<Job>,
    pub queued: Option<Job>,
    pub running: Option<Job>,
    pub recent: Vec<Job>,
}
impl Document {
    pub fn new(repo_id: RepoId) -> Self {
        Self {
            version: 1,
            repo_id,
            prepared: None,
            queued: None,
            running: None,
            recent: Vec::new(),
        }
    }
    pub fn valid(&self, id: RepoId) -> bool {
        let mut ids = std::collections::HashSet::new();
        self.version == 1
            && self.repo_id == id
            && !id.0.is_nil()
            && self.recent.len() <= 5
            && self
                .prepared
                .as_ref()
                .is_none_or(|j| j.phase == Phase::Prepared)
            && self
                .queued
                .as_ref()
                .is_none_or(|j| j.phase == Phase::Queued)
            && self
                .running
                .as_ref()
                .is_none_or(|j| j.phase == Phase::Running)
            && self.recent.iter().all(Job::terminal)
            && self
                .prepared
                .iter()
                .chain(self.queued.iter())
                .chain(self.running.iter())
                .chain(self.recent.iter())
                .all(|j| j.valid() && ids.insert(j.run_id))
    }
    pub fn finish(&mut self, mut job: Job, phase: Phase, reason: Option<Reason>, history: History) {
        job.transition(phase, reason, history);
        self.recent.insert(0, job);
        self.recent.truncate(5);
    }
    pub fn queue(&mut self, mut job: Job) {
        if let Some(old) = self.queued.take() {
            self.finish(
                old,
                Phase::Superseded,
                Some(Reason::LatestQueuedReplaced),
                History::NotAttempted,
            );
        }
        job.transition(Phase::Queued, None, History::NotAttempted);
        self.queued = Some(job);
    }
    pub fn public(&self) -> Vec<View> {
        self.running
            .iter()
            .chain(self.queued.iter())
            .chain(self.recent.iter())
            .map(View::from)
            .collect()
    }
}
#[derive(Serialize)]
pub(crate) struct View {
    run_id: SandboxRunId,
    source_ref: String,
    expected_commit: CommitSha,
    checks_blob_oid: Option<String>,
    phase: Phase,
    reason: Option<Reason>,
    history: History,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}
impl From<&Job> for View {
    fn from(j: &Job) -> Self {
        Self {
            run_id: j.run_id,
            source_ref: j.source_ref.clone(),
            expected_commit: j.expected_commit.clone(),
            checks_blob_oid: j.checks_blob_oid.clone(),
            phase: j.phase,
            reason: j.reason,
            history: j.history,
            created_at: j.created_at,
            updated_at: j.updated_at,
        }
    }
}
