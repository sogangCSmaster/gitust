//! GitHub Actions `uses:` step compatibility (`actions/checkout`, `actions/cache`,
//! `actions/upload-artifact`, `actions/download-artifact`, and repository-local composite actions).

use std::collections::BTreeMap;

use gix::prelude::HeaderExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::GitEngineError;

use super::{
    invalid, parse_safe_workflow_yaml, tree_err, validate_workflow_relative_path,
    MAX_WORKFLOW_BYTES,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowArtifactDownloadSpec {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawStep {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub uses: Option<String>,
    #[serde(default)]
    pub with: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawStrategy {
    pub matrix: BTreeMap<String, Vec<String>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum RawNeeds {
    Single(String),
    Multiple(Vec<String>),
}
impl Default for RawNeeds {
    fn default() -> Self {
        Self::Multiple(Vec::new())
    }
}
impl RawNeeds {
    pub fn into_vec(self) -> Vec<String> {
        match self {
            Self::Single(s) => vec![s],
            Self::Multiple(v) => v,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawJob {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub needs: RawNeeds,
    #[serde(default, alias = "runs-on")]
    pub runs_on: Option<String>,
    #[serde(default, alias = "isolation-tier")]
    pub isolation_tier: Option<gitust_core::IsolationTier>,
    #[serde(default)]
    pub strategy: Option<RawStrategy>,
    #[serde(default)]
    pub matrix: Option<BTreeMap<String, Vec<String>>>,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub cache: Option<super::WorkflowCacheSpec>,
    #[serde(default)]
    pub artifacts: Option<super::WorkflowArtifactSpec>,
    pub steps: Vec<RawStep>,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum RawJobsContainer {
    List(Vec<RawJob>),
    Map(BTreeMap<String, RawJob>),
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum RawOnTrigger {
    Single(String),
    Multiple(Vec<String>),
    Map(BTreeMap<String, Value>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawWorkflow {
    #[serde(default)]
    pub version: Option<u32>,
    pub name: String,
    #[serde(default)]
    pub on: Option<RawOnTrigger>,
    pub jobs: RawJobsContainer,
}

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub(super) enum RawWorkflowFile {
    Envelope {
        #[serde(default)]
        version: Option<u32>,
        workflows: Vec<RawWorkflow>,
    },
    Single(RawWorkflow),
}

#[derive(Deserialize)]
struct RawCompositeRuns {
    using: String,
    steps: Vec<RawStep>,
}

#[derive(Deserialize)]
struct RawCompositeAction {
    #[allow(dead_code)]
    #[serde(default)]
    name: Option<String>,
    runs: RawCompositeRuns,
}

pub(super) struct ResolvedJobUses {
    pub steps: Vec<super::WorkflowStepDefinition>,
    pub cache: Option<super::WorkflowCacheSpec>,
    pub artifacts: Option<super::WorkflowArtifactSpec>,
    pub download_artifacts: Vec<WorkflowArtifactDownloadSpec>,
}

pub(super) fn resolve_job_uses(
    repo: &gix::Repository,
    root_tree: &gix::Tree<'_>,
    raw_steps: Vec<RawStep>,
    existing_cache: Option<super::WorkflowCacheSpec>,
    existing_artifacts: Option<super::WorkflowArtifactSpec>,
) -> Result<ResolvedJobUses, GitEngineError> {
    let mut out_steps = Vec::new();
    let mut cache = existing_cache;
    let mut artifacts = existing_artifacts;
    let mut download_artifacts = Vec::new();

    for raw in raw_steps {
        match (raw.run, raw.uses) {
            (Some(run), None) => {
                if !raw.with.is_empty() {
                    return Err(invalid("'with' is only valid on 'uses' steps"));
                }
                out_steps.push(super::WorkflowStepDefinition {
                    name: raw.name,
                    run,
                });
            }
            (None, Some(uses_raw)) => {
                let uses = uses_raw.trim();
                if matches!(uses, "actions/checkout@v3" | "actions/checkout@v4") {
                    let label = if raw.name.trim().is_empty() {
                        "Checkout repository".to_owned()
                    } else {
                        raw.name
                    };
                    out_steps.push(super::WorkflowStepDefinition {
                        name: label,
                        run: ": # actions/checkout".to_owned(),
                    });
                } else if matches!(uses, "actions/cache@v3" | "actions/cache@v4") {
                    let key = raw
                        .with
                        .get("key")
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| invalid("actions/cache requires 'with.key'"))?;
                    let raw_path = raw
                        .with
                        .get("path")
                        .ok_or_else(|| invalid("actions/cache requires 'with.path'"))?;
                    let paths = split_multiline_paths(raw_path)?;
                    cache = Some(super::WorkflowCacheSpec { key, paths });
                    let label = if raw.name.trim().is_empty() {
                        "Cache build dependencies".to_owned()
                    } else {
                        raw.name
                    };
                    out_steps.push(super::WorkflowStepDefinition {
                        name: label,
                        run: ": # actions/cache".to_owned(),
                    });
                } else if matches!(
                    uses,
                    "actions/upload-artifact@v3" | "actions/upload-artifact@v4"
                ) {
                    let name = raw
                        .with
                        .get("name")
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| invalid("actions/upload-artifact requires 'with.name'"))?;
                    let raw_path = raw
                        .with
                        .get("path")
                        .ok_or_else(|| invalid("actions/upload-artifact requires 'with.path'"))?;
                    let paths = split_multiline_paths(raw_path)?;
                    artifacts = Some(super::WorkflowArtifactSpec { name, paths });
                    let label = if raw.name.trim().is_empty() {
                        "Upload artifact".to_owned()
                    } else {
                        raw.name
                    };
                    out_steps.push(super::WorkflowStepDefinition {
                        name: label,
                        run: ": # actions/upload-artifact".to_owned(),
                    });
                } else if matches!(
                    uses,
                    "actions/download-artifact@v3" | "actions/download-artifact@v4"
                ) {
                    let name = raw
                        .with
                        .get("name")
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| invalid("actions/download-artifact requires 'with.name'"))?;
                    let dest = match raw
                        .with
                        .get("path")
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                    {
                        Some(".") | None => ".".to_owned(),
                        Some(p) => validate_workflow_relative_path(p)?,
                    };
                    download_artifacts.push(WorkflowArtifactDownloadSpec { name, path: dest });
                    let label = if raw.name.trim().is_empty() {
                        "Download artifact".to_owned()
                    } else {
                        raw.name
                    };
                    out_steps.push(super::WorkflowStepDefinition {
                        name: label,
                        run: ": # actions/download-artifact".to_owned(),
                    });
                } else if let Some(local_rel) = uses.strip_prefix("./") {
                    let clean_dir = validate_workflow_relative_path(local_rel)?;
                    if !clean_dir.starts_with(".github/actions/") {
                        return Err(invalid(format!(
                            "local action '{uses}' must reside under ./.github/actions/"
                        )));
                    }
                    let composite_steps =
                        load_local_composite_action(repo, root_tree, &clean_dir, &raw.with)?;
                    out_steps.extend(composite_steps);
                } else {
                    return Err(invalid(format!(
                        "unsupported or untrusted workflow action 'uses: {uses}'"
                    )));
                }
            }
            _ => {
                return Err(invalid(
                    "each workflow step must specify exactly one of 'run' or 'uses'",
                ));
            }
        }
    }

    Ok(ResolvedJobUses {
        steps: out_steps,
        cache,
        artifacts,
        download_artifacts,
    })
}

fn split_multiline_paths(raw: &str) -> Result<Vec<String>, GitEngineError> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            out.push(validate_workflow_relative_path(trimmed)?);
        }
    }
    if out.is_empty() {
        return Err(invalid("action path list must not be empty"));
    }
    Ok(out)
}

fn load_local_composite_action(
    repo: &gix::Repository,
    root_tree: &gix::Tree<'_>,
    clean_dir: &str,
    inputs: &BTreeMap<String, String>,
) -> Result<Vec<super::WorkflowStepDefinition>, GitEngineError> {
    for val in inputs.values() {
        if val.len() > 512 || val.contains('\0') || val.contains('\n') || val.contains('\r') {
            return Err(invalid("invalid composite action input value"));
        }
    }
    let candidates = [
        format!("{clean_dir}/action.yml"),
        format!("{clean_dir}/action.yaml"),
        format!("{clean_dir}/action.json"),
    ];
    let mut found_bytes = None;
    let mut is_json = false;
    for path in &candidates {
        if let Some(entry) = root_tree.lookup_entry_by_path(path).map_err(tree_err)? {
            if !entry.mode().is_blob() || entry.mode().is_link() {
                return Err(invalid(
                    "composite action definition must be a regular blob",
                ));
            }
            let oid: gix::ObjectId = entry.oid().into();
            let hdr = repo
                .objects
                .header(oid)
                .map_err(|e| invalid(e.to_string()))?;
            if hdr.size() > MAX_WORKFLOW_BYTES {
                return Err(invalid("composite action file exceeds 64 KiB"));
            }
            let obj = repo.find_object(oid).map_err(|e| invalid(e.to_string()))?;
            found_bytes = Some(obj.data.clone());
            is_json = path.ends_with(".json");
            break;
        }
    }
    let bytes = found_bytes.ok_or_else(|| {
        invalid(format!(
            "local composite action not found at './{clean_dir}'"
        ))
    })?;
    let val: Value = if is_json {
        serde_json::from_slice(&bytes).map_err(|_| invalid("invalid composite action JSON"))?
    } else {
        let text =
            std::str::from_utf8(&bytes).map_err(|_| invalid("composite action must be UTF-8"))?;
        parse_safe_workflow_yaml(text)?
    };
    let parsed: RawCompositeAction =
        serde_json::from_value(val).map_err(|_| invalid("invalid composite action structure"))?;
    if parsed.runs.using.trim() != "composite" {
        return Err(invalid("local action 'runs.using' must be 'composite'"));
    }
    if parsed.runs.steps.is_empty() || parsed.runs.steps.len() > 8 {
        return Err(invalid("composite action must contain 1..=8 steps"));
    }
    let mut expanded = Vec::with_capacity(parsed.runs.steps.len());
    for step in parsed.runs.steps {
        if step.uses.is_some() {
            return Err(invalid(
                "nested 'uses' inside composite action is unsupported",
            ));
        }
        let raw_run = step
            .run
            .ok_or_else(|| invalid("composite action step missing 'run'"))?;
        let run = substitute_action_inputs(&raw_run, inputs)?;
        let name = substitute_action_inputs(&step.name, inputs)?;
        expanded.push(super::WorkflowStepDefinition { name, run });
    }
    Ok(expanded)
}

fn substitute_action_inputs(
    template: &str,
    inputs: &BTreeMap<String, String>,
) -> Result<String, GitEngineError> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("${{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        let end = after
            .find("}}")
            .ok_or_else(|| invalid("unclosed '${{' in composite action"))?;
        let expr = after[..end].trim();
        if let Some(inp_key) = expr.strip_prefix("inputs.").map(str::trim) {
            let val = inputs.get(inp_key).ok_or_else(|| {
                invalid(format!(
                    "missing required composite action input '{inp_key}'"
                ))
            })?;
            out.push_str(val);
        } else {
            out.push_str(&rest[start..start + 3 + end + 2]);
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}
