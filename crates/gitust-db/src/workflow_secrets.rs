use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use gitust_core::{AuthError, AuthenticatedPrincipal, EnterpriseRole, RepoAccess};
use serde::{Deserialize, Serialize};

use crate::pull_requests::authorize_repo_locked;
use crate::AuthStore;

const MAX_WORKFLOW_SECRETS_PER_REPO: usize = 64;
const MAX_SECRET_VALUE_BYTES: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowSecretMetadata {
    pub name: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredWorkflowSecret {
    pub(crate) name: String,
    #[serde(flatten)]
    pub(crate) secret: crate::secret_storage::StoredSecret,
    pub(crate) updated_at: DateTime<Utc>,
}

pub fn validate_workflow_secret_name(raw: &str) -> Result<String, AuthError> {
    let trimmed = raw.trim();
    let bytes = trimmed.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 64
        || !bytes[0].is_ascii_uppercase()
        || !bytes[1..]
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
    {
        return Err(AuthError::InvalidInput(
            "workflow secret name must match ^[A-Z][A-Z0-9_]{0,63}$".into(),
        ));
    }
    Ok(trimmed.to_owned())
}

fn validate_workflow_secret_value(value: &str) -> Result<(), AuthError> {
    if value.is_empty() || value.len() > MAX_SECRET_VALUE_BYTES || value.contains('\0') {
        return Err(AuthError::InvalidInput(
            "workflow secret value must be 1..=8192 bytes without NUL".into(),
        ));
    }
    Ok(())
}

impl AuthStore {
    pub fn set_workflow_secret(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
        name: &str,
        value: &str,
    ) -> Result<WorkflowSecretMetadata, AuthError> {
        let clean_name = validate_workflow_secret_name(name)?;
        validate_workflow_secret_value(value)?;
        let now = Utc::now();
        let encrypted = StoredWorkflowSecret {
            name: clean_name.clone(),
            secret: self.secret_key.seal(
                &format!("workflow:{}:{clean_name}", expected.repo_id.0),
                value.as_bytes(),
            )?,
            updated_at: now,
        };

        let (meta, snapshot) = {
            let mut guard = self.write_state()?;
            let active = authorize_repo_locked(&guard, caller, expected, true)?;
            if active.role != EnterpriseRole::SiteAdmin {
                return Err(AuthError::AdminPrivilegeRequired);
            }
            let list = guard.workflow_secrets.entry(expected.repo_id).or_default();
            if let Some(existing) = list.iter_mut().find(|s| s.name == clean_name) {
                *existing = encrypted;
            } else {
                if list.len() >= MAX_WORKFLOW_SECRETS_PER_REPO {
                    return Err(AuthError::InvalidInput(
                        "workflow secrets limit exceeded for repository".into(),
                    ));
                }
                list.push(encrypted);
                list.sort_by(|a, b| a.name.cmp(&b.name));
            }
            (
                WorkflowSecretMetadata {
                    name: clean_name,
                    updated_at: now,
                },
                self.maybe_snapshot(guard),
            )
        };
        self.persist_snapshot(snapshot)?;
        Ok(meta)
    }

    pub fn list_workflow_secrets(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
    ) -> Result<Vec<WorkflowSecretMetadata>, AuthError> {
        let guard = self.read_state()?;
        let active = authorize_repo_locked(&guard, caller, expected, true)?;
        if active.role != EnterpriseRole::SiteAdmin {
            return Err(AuthError::AdminPrivilegeRequired);
        }
        let mut out: Vec<WorkflowSecretMetadata> = guard
            .workflow_secrets
            .get(&expected.repo_id)
            .map(|list| {
                list.iter()
                    .map(|s| WorkflowSecretMetadata {
                        name: s.name.clone(),
                        updated_at: s.updated_at,
                    })
                    .collect()
            })
            // fail-open-ok: repositories without configured workflow secrets return an empty list.
            .unwrap_or_default();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn delete_workflow_secret(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
        name: &str,
    ) -> Result<(), AuthError> {
        let clean_name = validate_workflow_secret_name(name)?;
        let snapshot = {
            let mut guard = self.write_state()?;
            let active = authorize_repo_locked(&guard, caller, expected, true)?;
            if active.role != EnterpriseRole::SiteAdmin {
                return Err(AuthError::AdminPrivilegeRequired);
            }
            let Some(list) = guard.workflow_secrets.get_mut(&expected.repo_id) else {
                return Err(AuthError::InvalidInput("workflow secret not found".into()));
            };
            let before = list.len();
            list.retain(|s| s.name != clean_name);
            if list.len() == before {
                return Err(AuthError::InvalidInput("workflow secret not found".into()));
            }
            self.maybe_snapshot(guard)
        };
        self.persist_snapshot(snapshot)
    }

    pub fn resolve_workflow_secrets(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
    ) -> Result<BTreeMap<String, String>, AuthError> {
        let guard = self.read_state()?;
        let active = authorize_repo_locked(&guard, caller, expected, true)?;
        if active.role != EnterpriseRole::SiteAdmin {
            return Err(AuthError::AdminPrivilegeRequired);
        }
        let mut resolved = BTreeMap::new();
        if let Some(list) = guard.workflow_secrets.get(&expected.repo_id) {
            for item in list {
                let bytes = self.secret_key.open(
                    &format!("workflow:{}:{}", expected.repo_id.0, item.name),
                    &item.secret,
                )?;
                let plain = String::from_utf8(bytes).map_err(|_| {
                    AuthError::StorageError("invalid workflow secret encoding".into())
                })?;
                validate_workflow_secret_value(&plain)?;
                resolved.insert(item.name.clone(), plain);
            }
        }
        Ok(resolved)
    }
}
