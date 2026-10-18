//! GitHub Actions-compatible workflow definitions (`.gitust-workflows.json` and `.github/workflows/*.json`),
//! DAG topological validation, matrix expansion, and path hygiene.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use gitust_core::CommitSha;
use gix::prelude::HeaderExt;
use serde::{Deserialize, Serialize};

use crate::{GitEngine, GitEngineError};

const ROOT_WORKFLOWS_PATH: &str = ".gitust-workflows.json";
const GITHUB_WORKFLOWS_DIR: &str = ".github/workflows";
const MAX_WORKFLOW_BYTES: u64 = 65_536;
const MAX_WORKFLOW_FILES: usize = 16;
const MAX_JOBS_PER_WORKFLOW: usize = 16;
const MAX_STEPS_PER_JOB: usize = 16;
const MAX_MATRIX_COMBINATIONS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStepDefinition {
    #[serde(default)]
    pub name: String,
    pub run: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCacheSpec {
    pub key: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowArtifactSpec {
    pub name: String,
    pub paths: Vec<String>,
}

fn default_runs_on() -> String {
    "local".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowJobDefinition {
    pub id: String,
    pub name: String,
    pub needs: Vec<String>,
    #[serde(default = "default_runs_on")]
    pub runs_on: String,
    #[serde(default)]
    pub isolation_tier: gitust_core::IsolationTier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix: Option<BTreeMap<String, Vec<String>>>,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<WorkflowCacheSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<WorkflowArtifactSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_name: Option<String>,
    pub steps_count: usize,
    pub steps: Vec<WorkflowStepDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    pub name: String,
    pub on: Vec<String>,
    pub jobs: Vec<WorkflowJobDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedWorkflowJob {
    pub job_id: String,
    pub display_name: String,
    pub needs: Vec<String>,
    pub runs_on: String,
    pub isolation_tier: gitust_core::IsolationTier,
    pub matrix_values: BTreeMap<String, String>,
    pub secrets: Vec<String>,
    pub cache: Option<WorkflowCacheSpec>,
    pub artifacts: Option<WorkflowArtifactSpec>,
    pub steps: Vec<WorkflowStepDefinition>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStrategy {
    matrix: BTreeMap<String, Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJob {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    needs: Vec<String>,
    #[serde(default, alias = "runs-on")]
    runs_on: Option<String>,
    #[serde(default, alias = "isolation-tier")]
    isolation_tier: Option<gitust_core::IsolationTier>,
    #[serde(default)]
    strategy: Option<RawStrategy>,
    #[serde(default)]
    matrix: Option<BTreeMap<String, Vec<String>>>,
    #[serde(default)]
    secrets: Vec<String>,
    #[serde(default)]
    cache: Option<WorkflowCacheSpec>,
    #[serde(default)]
    artifacts: Option<WorkflowArtifactSpec>,
    steps: Vec<WorkflowStepDefinition>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawJobsContainer {
    List(Vec<RawJob>),
    Map(BTreeMap<String, RawJob>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawOnTrigger {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWorkflow {
    #[serde(default)]
    version: Option<u32>,
    name: String,
    #[serde(default)]
    on: Option<RawOnTrigger>,
    jobs: RawJobsContainer,
}

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum RawWorkflowFile {
    Envelope {
        #[serde(default)]
        version: Option<u32>,
        workflows: Vec<RawWorkflow>,
    },
    Single(RawWorkflow),
}

fn invalid(msg: impl Into<String>) -> GitEngineError {
    GitEngineError::VerificationInvalid(msg.into())
}

fn tree_err(e: impl std::fmt::Display) -> GitEngineError {
    GitEngineError::TreeTraversal(e.to_string())
}

fn is_valid_ident(s: &str, max_len: usize) -> bool {
    !s.is_empty()
        && s.len() <= max_len
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn is_valid_matrix_value(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.contains("..")
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'))
}

pub fn validate_secret_name_syntax(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_uppercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
}

pub fn validate_workflow_relative_path(path: &str) -> Result<String, GitEngineError> {
    if path.is_empty() || path.starts_with('/') || path.ends_with('/') || path.trim() != path {
        return Err(invalid(format!("invalid workflow relative path '{path}'")));
    }
    let clean = GitEngine::validate_repo_relative_path(path)
        .map_err(|_| invalid(format!("invalid workflow relative path '{path}'")))?;
    if clean.is_empty() || clean != path {
        return Err(invalid(format!("workflow path '{path}' must be canonical")));
    }
    Ok(clean)
}

#[path = "workflows_expand.rs"]
mod expand;

fn clean_opt_label(opt: Option<&str>, fallback: String) -> Result<String, GitEngineError> {
    // fail-open-ok: optional workflow/step display name falls back to validated job ID or default step label.
    let label = opt
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or(fallback, str::to_owned);
    if label.len() > 128 || label.chars().any(char::is_control) {
        return Err(invalid("workflow display name invalid"));
    }
    Ok(label)
}

fn validate_named_paths(
    raw_label: &str,
    raw_paths: &[String],
    max_paths: usize,
    matrix_keys: &BTreeSet<String>,
    secrets_set: &mut BTreeSet<String>,
) -> Result<(String, Vec<String>), GitEngineError> {
    let label = clean_opt_label(Some(raw_label), String::new())?;
    if label.is_empty() || raw_paths.is_empty() || raw_paths.len() > max_paths {
        return Err(invalid("invalid workflow cache or artifact specification"));
    }
    expand::parse_and_validate_template_tokens(&label, matrix_keys, secrets_set, false)?;
    let mut clean_paths = Vec::with_capacity(raw_paths.len());
    for p in raw_paths {
        clean_paths.push(validate_workflow_relative_path(p)?);
    }
    Ok((label, clean_paths))
}

fn normalize_job(job_id: String, rj: RawJob) -> Result<WorkflowJobDefinition, GitEngineError> {
    if !is_valid_ident(&job_id, 64) {
        return Err(invalid(format!("invalid job id '{job_id}'")));
    }
    let job_name = clean_opt_label(rj.name.as_deref(), job_id.clone())?;
    let mut seen_needs = BTreeSet::new();
    if rj.needs.len() > MAX_JOBS_PER_WORKFLOW
        || rj
            .needs
            .iter()
            .any(|d| d == &job_id || !seen_needs.insert(d.clone()))
    {
        return Err(invalid(format!("invalid 'needs' in job '{job_id}'")));
    }
    let matrix = rj.strategy.map(|s| s.matrix).or(rj.matrix);
    let mut matrix_keys = BTreeSet::new();
    if let Some(ref m) = matrix {
        if m.is_empty() || m.len() > 4 {
            return Err(invalid("matrix must have 1..=4 keys"));
        }
        let mut combos = 1usize;
        for (k, vals) in m {
            if !is_valid_ident(k, 64)
                || vals.is_empty()
                || vals.len() > MAX_MATRIX_COMBINATIONS
                || !vals.iter().all(|v| is_valid_matrix_value(v))
            {
                return Err(invalid(format!("invalid matrix entry '{k}' in '{job_id}'")));
            }
            combos = combos.saturating_mul(vals.len());
            if combos > MAX_MATRIX_COMBINATIONS {
                return Err(invalid(format!("matrix exceeds limit in '{job_id}'")));
            }
            matrix_keys.insert(k.clone());
        }
    }
    let mut secrets_set = BTreeSet::new();
    for sec in &rj.secrets {
        if !validate_secret_name_syntax(sec) {
            return Err(invalid(format!("invalid secret name '{sec}'")));
        }
        secrets_set.insert(sec.clone());
    }
    let runs_on = match rj.runs_on {
        Some(s) => {
            let trimmed = s.trim().to_owned();
            if trimmed.is_empty() || trimmed.len() > 128 {
                return Err(invalid("runs_on must not be empty"));
            }
            trimmed
        }
        None => default_runs_on(),
    };
    expand::parse_and_validate_template_tokens(&runs_on, &matrix_keys, &mut secrets_set, false)?;
    if !runs_on.contains("${{") && !is_valid_ident(&runs_on, 64) {
        return Err(invalid(format!("invalid runs_on label '{runs_on}'")));
    }
    // fail-open-ok: optional workflow job isolation_tier defaults to HostOs when omitted.
    let isolation_tier = rj.isolation_tier.unwrap_or_default();
    let mut cache = None;
    if let Some(c) = rj.cache {
        let (key, paths) =
            validate_named_paths(&c.key, &c.paths, 8, &matrix_keys, &mut secrets_set)?;
        cache = Some(WorkflowCacheSpec { key, paths });
    }
    let mut artifacts = None;
    if let Some(a) = rj.artifacts {
        let (name, paths) =
            validate_named_paths(&a.name, &a.paths, 16, &matrix_keys, &mut secrets_set)?;
        artifacts = Some(WorkflowArtifactSpec { name, paths });
    }
    if rj.steps.is_empty() || rj.steps.len() > MAX_STEPS_PER_JOB {
        return Err(invalid("job must contain 1..=16 steps"));
    }
    let mut steps = Vec::with_capacity(rj.steps.len());
    for (idx, step) in rj.steps.into_iter().enumerate() {
        if step.run.trim().is_empty() || step.run.len() > 4096 || step.run.contains('\0') {
            return Err(invalid("step has invalid command"));
        }
        expand::parse_and_validate_template_tokens(
            &step.run,
            &matrix_keys,
            &mut secrets_set,
            true,
        )?;
        let name = clean_opt_label(Some(&step.name), format!("Step {}", idx + 1))?;
        expand::parse_and_validate_template_tokens(&name, &matrix_keys, &mut secrets_set, false)?;
        steps.push(WorkflowStepDefinition {
            name,
            run: step.run,
        });
    }
    let cache_key = cache.as_ref().map(|c| c.key.clone());
    let artifact_name = artifacts.as_ref().map(|a| a.name.clone());
    let steps_count = steps.len();
    Ok(WorkflowJobDefinition {
        id: job_id,
        name: job_name,
        needs: rj.needs,
        runs_on,
        isolation_tier,
        matrix,
        secrets: secrets_set.into_iter().collect(),
        cache,
        artifacts,
        cache_key,
        artifact_name,
        steps_count,
        steps,
    })
}

fn validate_and_normalize_workflow(raw: RawWorkflow) -> Result<WorkflowDefinition, GitEngineError> {
    if raw.version.is_some_and(|v| v != 1) {
        return Err(invalid("unsupported workflow version"));
    }
    let wf_name = raw.name.trim();
    if wf_name.is_empty() || wf_name.len() > 128 || wf_name.chars().any(char::is_control) {
        return Err(invalid("workflow name must be 1..=128 printable bytes"));
    }
    let on = match raw.on {
        Some(RawOnTrigger::Single(s)) => vec![s],
        Some(RawOnTrigger::Multiple(v)) => v,
        None => vec!["push".to_owned()],
    };
    if on.is_empty() || on.len() > 8 || !on.iter().all(|t| is_valid_ident(t, 64)) {
        return Err(invalid("invalid workflow 'on' triggers"));
    }
    let mut raw_jobs = Vec::new();
    match raw.jobs {
        RawJobsContainer::List(list) => {
            for j in list {
                let id = j.id.clone().ok_or_else(|| invalid("missing job id"))?;
                raw_jobs.push((id, j));
            }
        }
        RawJobsContainer::Map(map) => raw_jobs.extend(map),
    }
    if raw_jobs.is_empty() || raw_jobs.len() > MAX_JOBS_PER_WORKFLOW {
        return Err(invalid("workflow must contain 1..=16 jobs"));
    }
    let mut by_id = BTreeMap::new();
    let mut in_degree = BTreeMap::new();
    let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut decl_order = Vec::with_capacity(raw_jobs.len());
    for (job_id, rj) in raw_jobs {
        if by_id.contains_key(&job_id) {
            return Err(invalid(format!("duplicate job id '{job_id}'")));
        }
        in_degree.insert(job_id.clone(), 0usize);
        dependents.insert(job_id.clone(), Vec::new());
        decl_order.push(job_id.clone());
        by_id.insert(job_id.clone(), normalize_job(job_id, rj)?);
    }
    for (id, job) in &by_id {
        for dep in &job.needs {
            let Some(list) = dependents.get_mut(dep) else {
                return Err(invalid(format!("unknown dependency '{dep}' in '{id}'")));
            };
            list.push(id.clone());
            if let Some(deg) = in_degree.get_mut(id) {
                *deg += 1;
            }
        }
    }
    let mut queue: VecDeque<String> = decl_order
        .iter()
        .filter(|id| in_degree.get(*id).copied() == Some(0))
        .cloned()
        .collect();
    let mut sorted_jobs = Vec::with_capacity(by_id.len());
    while let Some(curr) = queue.pop_front() {
        if let Some(job) = by_id.get(&curr) {
            sorted_jobs.push(job.clone());
        }
        for child in dependents.get(&curr).into_iter().flatten() {
            if let Some(deg) = in_degree.get_mut(child) {
                *deg = deg.saturating_sub(1);
                if *deg == 0 {
                    queue.push_back(child.clone());
                }
            }
        }
    }
    if sorted_jobs.len() != by_id.len() {
        return Err(invalid("workflow jobs contain a cycle in 'needs'"));
    }
    let name = wf_name.to_owned();
    Ok(WorkflowDefinition {
        name,
        on,
        jobs: sorted_jobs,
    })
}

fn parse_workflow_blob_bytes(bytes: &[u8]) -> Result<Vec<WorkflowDefinition>, GitEngineError> {
    if bytes.len() as u64 > MAX_WORKFLOW_BYTES {
        return Err(invalid("workflow file exceeds 64 KiB limit"));
    }
    let parsed: RawWorkflowFile =
        serde_json::from_slice(bytes).map_err(|_| invalid("invalid workflow JSON"))?;
    match parsed {
        RawWorkflowFile::Envelope { version, workflows } => {
            if version.is_some_and(|v| v != 1)
                || workflows.is_empty()
                || workflows.len() > MAX_WORKFLOW_FILES
            {
                return Err(invalid("invalid workflow envelope"));
            }
            let mut out = Vec::with_capacity(workflows.len());
            for wf in workflows {
                out.push(validate_and_normalize_workflow(wf)?);
            }
            Ok(out)
        }
        RawWorkflowFile::Single(wf) => Ok(vec![validate_and_normalize_workflow(wf)?]),
    }
}

fn load_blob_workflows(
    repo: &gix::Repository,
    mode: gix::object::tree::EntryMode,
    oid: gix::ObjectId,
) -> Result<Vec<WorkflowDefinition>, GitEngineError> {
    if !mode.is_blob() || mode.is_link() {
        return Err(invalid("workflow definition must be a regular blob"));
    }
    let hdr = repo
        .objects
        .header(oid)
        .map_err(|e| invalid(e.to_string()))?;
    if hdr.size() > MAX_WORKFLOW_BYTES {
        return Err(invalid("workflow file exceeds 64 KiB"));
    }
    let obj = repo.find_object(oid).map_err(|e| invalid(e.to_string()))?;
    parse_workflow_blob_bytes(&obj.data)
}

impl GitEngine {
    pub fn workflows_at_commit(
        &self,
        commit: &CommitSha,
    ) -> Result<Vec<WorkflowDefinition>, GitEngineError> {
        let repo = self.acquire_repo()?;
        let not_found = || GitEngineError::RevisionNotFound(commit.0.clone());
        let oid = gix::ObjectId::from_hex(commit.0.as_bytes()).map_err(|_| not_found())?;
        let obj = repo.find_object(oid).map_err(|_| not_found())?;
        let tree = obj
            .try_into_commit()
            .map_err(|_| not_found())?
            .tree()
            .map_err(tree_err)?;
        let mut workflows = Vec::new();
        let root = ROOT_WORKFLOWS_PATH;
        if let Some(e) = tree.lookup_entry_by_path(root).map_err(tree_err)? {
            workflows.extend(load_blob_workflows(&repo, e.mode(), e.oid().into())?);
        }
        let gh_dir = GITHUB_WORKFLOWS_DIR;
        if let Some(dir) = tree.lookup_entry_by_path(gh_dir).map_err(tree_err)? {
            if dir.mode().is_tree() {
                let obj = dir.object().map_err(tree_err)?;
                let wf_tree = obj.try_into_tree().map_err(tree_err)?;
                for item_res in wf_tree.iter() {
                    let item = item_res.map_err(tree_err)?;
                    if !String::from_utf8_lossy(item.filename()).ends_with(".json") {
                        continue;
                    }
                    workflows.extend(load_blob_workflows(&repo, item.mode(), item.oid().into())?);
                    if workflows.len() > MAX_WORKFLOW_FILES {
                        return Err(invalid("too many workflow definitions in repository"));
                    }
                }
            }
        }
        Ok(workflows)
    }

    pub fn workflows_for_branch(
        &self,
        branch: &str,
    ) -> Result<(CommitSha, Vec<WorkflowDefinition>), GitEngineError> {
        let trimmed = branch.trim();
        // fail-open-ok: normalizes optional refs/heads/ prefix before strict outbound branch validation.
        let short = trimmed.strip_prefix("refs/heads/").unwrap_or(trimmed);
        let clean = Self::validate_outbound_branch(short)?;
        let sha = self.resolve_checks_ref(&format!("refs/heads/{clean}"))?;
        let workflows = self.workflows_at_commit(&sha)?;
        Ok((sha, workflows))
    }
}
