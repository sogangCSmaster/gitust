//! Template token validation (`${{ matrix.<key> }}` and `${{ secrets.<NAME> }}`),
//! matrix substitution, and deterministic Cartesian product job expansion.

use std::collections::{BTreeMap, BTreeSet};

use crate::GitEngineError;

use super::{
    invalid, validate_secret_name_syntax, ExpandedWorkflowJob, WorkflowArtifactDownloadSpec,
    WorkflowDefinition, WorkflowStepDefinition,
};

pub(super) fn parse_and_validate_template_tokens(
    input: &str,
    matrix_keys: &BTreeSet<String>,
    referenced_secrets: &mut BTreeSet<String>,
    allow_secrets: bool,
) -> Result<(), GitEngineError> {
    let mut rest = input;
    while let Some(start) = rest.find("${{") {
        let after_start = &rest[start + 3..];
        let end = after_start
            .find("}}")
            .ok_or_else(|| invalid("unclosed '${{' template expression in workflow"))?;
        let expr = after_start[..end].trim();
        if let Some(m_key) = expr.strip_prefix("matrix.").map(str::trim) {
            if !matrix_keys.contains(m_key) {
                return Err(invalid(format!("unknown matrix key '{m_key}'")));
            }
        } else if let Some(s_name) = expr.strip_prefix("secrets.").map(str::trim) {
            if !allow_secrets {
                return Err(invalid(
                    "secret expressions are not allowed in cache keys or artifact names",
                ));
            }
            if !validate_secret_name_syntax(s_name) {
                return Err(invalid(format!("invalid secret name '{s_name}'")));
            }
            referenced_secrets.insert(s_name.to_owned());
        } else {
            return Err(invalid(format!("unsupported expression '{expr}'")));
        }
        rest = &after_start[end + 2..];
    }
    Ok(())
}

pub(super) fn substitute_matrix_tokens(
    input: &str,
    matrix_values: &BTreeMap<String, String>,
) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${{") {
        out.push_str(&rest[..start]);
        let after_start = &rest[start + 3..];
        if let Some(end) = after_start.find("}}") {
            let expr = after_start[..end].trim();
            if let Some(k) = expr.strip_prefix("matrix.").map(str::trim) {
                if let Some(val) = matrix_values.get(k) {
                    out.push_str(val);
                    rest = &after_start[end + 2..];
                    continue;
                }
            }
            out.push_str(&rest[start..start + 3 + end + 2]);
            rest = &after_start[end + 2..];
        } else {
            out.push_str(&rest[start..]);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

impl WorkflowDefinition {
    pub fn expand_jobs(&self) -> Result<Vec<ExpandedWorkflowJob>, GitEngineError> {
        let mut expanded = Vec::new();
        for job in &self.jobs {
            let mut combinations: Vec<BTreeMap<String, String>> = vec![BTreeMap::new()];
            if let Some(ref matrix) = job.matrix {
                for (key, values) in matrix {
                    let mut next = Vec::with_capacity(combinations.len() * values.len());
                    for combo in &combinations {
                        for val in values {
                            let mut c = combo.clone();
                            c.insert(key.clone(), val.clone());
                            next.push(c);
                        }
                    }
                    combinations = next;
                }
            }
            for combo in combinations {
                let display_name = if combo.is_empty() {
                    job.name.clone()
                } else {
                    let parts: Vec<_> = combo.iter().map(|(k, v)| format!("{k}={v}")).collect();
                    format!("{} ({})", job.name, parts.join(", "))
                };
                let sub = |s: &str| substitute_matrix_tokens(s, &combo);
                let runs_on = sub(&job.runs_on);
                if !super::is_valid_ident(&runs_on, 64) {
                    return Err(invalid("expanded runs_on label is invalid"));
                }
                let mut steps = Vec::with_capacity(job.steps.len());
                for s in &job.steps {
                    let (name, run) = (sub(&s.name), sub(&s.run));
                    steps.push(WorkflowStepDefinition { name, run });
                }
                let mut cache = job.cache.clone();
                if let Some(ref mut c) = cache {
                    c.key = sub(&c.key);
                }
                let mut artifacts = job.artifacts.clone();
                if let Some(ref mut a) = artifacts {
                    a.name = sub(&a.name);
                }
                let download_artifacts = job
                    .download_artifacts
                    .iter()
                    .map(|d| WorkflowArtifactDownloadSpec {
                        name: sub(&d.name),
                        path: d.path.clone(),
                    })
                    .collect();
                expanded.push(ExpandedWorkflowJob {
                    job_id: job.id.clone(),
                    display_name,
                    needs: job.needs.clone(),
                    runs_on,
                    isolation_tier: job.isolation_tier,
                    matrix_values: combo,
                    secrets: job.secrets.clone(),
                    cache,
                    artifacts,
                    download_artifacts,
                    steps,
                });
            }
        }
        Ok(expanded)
    }
}
