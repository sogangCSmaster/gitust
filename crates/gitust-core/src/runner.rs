//! Execution isolation tiers, multi-node runner fleet domain records, and shared build cache entries.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum IsolationTier {
    #[default]
    HostOs,
    ContainerVm,
}

impl IsolationTier {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HostOs => "host_os",
            Self::ContainerVm => "container_vm",
        }
    }
}

impl fmt::Display for IsolationTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RunnerStatus {
    #[default]
    Online,
    Draining,
    Offline,
}

impl RunnerStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Draining => "draining",
            Self::Offline => "offline",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RunnerNodeId(pub Uuid);

impl RunnerNodeId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RunnerNodeId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RunnerNodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.hyphenated())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerNodeRecord {
    pub id: RunnerNodeId,
    pub name: String,
    pub labels: Vec<String>,
    pub supported_tiers: Vec<IsolationTier>,
    pub max_concurrency: u32,
    pub active_leases: u32,
    pub status: RunnerStatus,
    pub registered_at: String,
    pub last_heartbeat_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedCacheEntry {
    pub key: String,
    pub digest_sha256: String,
    pub size_bytes: u64,
    pub files_count: usize,
    pub created_at_epoch_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_job_id: Option<String>,
}
