//! Explicit local diagnostics; recorded results never grant verification, approval or dispatch authority.
use crate::history_cmd::{save_completed, HistoryWrite};
use crate::sandbox_cmd::{run_prepared_sandbox, RunCommands};
use gitust_core::CommitSha;
use gitust_git::GitEngine;
use gitust_sandbox::SandboxExecutionResult;
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
pub(crate) struct ChecksReport {
    version: u32,
    status: &'static str,
    reason: Option<&'static str>,
    source_ref: Option<String>,
    checked_commit: Option<CommitSha>,
    checks_blob_oid: Option<String>,
    result: Option<SandboxExecutionResult>,
    cleanup: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    history: Option<HistoryWrite>,
}
impl ChecksReport {
    fn rejected(reason: &'static str, record: bool) -> Self {
        Self {
            version: if record { 2 } else { 1 },
            status: "rejected",
            reason: Some(reason),
            source_ref: None,
            checked_commit: None,
            checks_blob_oid: None,
            result: None,
            cleanup: "not_created",
            history: record.then(HistoryWrite::not_attempted),
        }
    }
    pub(crate) fn exit_code(&self) -> i32 {
        match self.status {
            "rejected" => 2,
            "interrupted" => 130,
            "passed" | "failed" => self.result.as_ref().map_or(1, |result| {
                if (0..=255).contains(&result.exit_code) {
                    if result.exit_code == 0
                        && self.history.as_ref().is_some_and(HistoryWrite::failed)
                    {
                        1
                    } else {
                        result.exit_code
                    }
                } else {
                    1
                }
            }),
            _ => 1,
        }
    }
}

pub(crate) async fn run_cli_checks(
    repo: &Path,
    expected: &str,
    timeout: u64,
    record: bool,
) -> ChecksReport {
    if !matches!(expected.len(), 40 | 64)
        || !expected.bytes().all(|b| b.is_ascii_hexdigit())
        || !expected.bytes().any(|b| b != b'0')
    {
        return ChecksReport::rejected("invalid_expected_commit", record);
    }
    if !(1..=600).contains(&timeout) {
        return ChecksReport::rejected("invalid_timeout", record);
    }
    let expected = CommitSha(expected.to_ascii_lowercase());
    let path = repo.to_owned();
    let prepared = tokio::task::spawn_blocking(move || {
        let engine = GitEngine::new(path);
        let owner = engine
            .lock_worktree_service()
            .map_err(|_| "repository_unavailable")?;
        engine
            .ensure_config_has_no_credentials()
            .map_err(|_| "repository_unavailable")?;
        let base = engine
            .committed_default_branch()
            .map_err(|_| "unsupported_head")?;
        let head = engine
            .inspect_head_identity()
            .map_err(|_| "repository_unavailable")?;
        if head.head_sha != expected {
            return Err("revision_mismatch");
        }
        let checks = engine
            .required_checks_at_commit(&expected)
            .map_err(|_| "checks_invalid")?
            .ok_or("checks_missing")?;
        Ok((engine, owner, base, expected, checks))
    })
    .await;
    let (engine, owner, base, expected, checks) = match prepared {
        Ok(Ok(value)) => value,
        Ok(Err(reason)) => return ChecksReport::rejected(reason, record),
        // fail-open-ok: failed setup returns rejected/exit2 before any checkout or execution.
        Err(_) => return ChecksReport::rejected("repository_unavailable", record),
    };
    let mut report = ChecksReport {
        version: if record { 2 } else { 1 },
        status: "unconfirmed",
        reason: Some("run_unconfirmed"),
        source_ref: Some(format!("refs/heads/{base}")),
        checked_commit: Some(expected.clone()),
        checks_blob_oid: Some(checks.blob_oid),
        result: None,
        cleanup: "unconfirmed",
        history: record.then(HistoryWrite::not_attempted),
    };
    let outcome = run_prepared_sandbox(
        engine.clone(),
        owner.clone(),
        base,
        RunCommands::Checks {
            expected,
            commands: checks.commands,
        },
        false,
        timeout,
    )
    .await;
    match outcome {
        Ok(outcome) => {
            report.cleanup = if outcome.cleanup_confirmed {
                "confirmed"
            } else {
                "unconfirmed"
            };
            if outcome.interrupted {
                report.status = "interrupted";
                report.reason = Some("interrupted");
            } else if outcome.cleanup_confirmed && outcome.fence_confirmed {
                if let Some(result) = &outcome.result {
                    report.status = if result.exit_code == 0 {
                        "passed"
                    } else {
                        "failed"
                    };
                    report.reason = None;
                }
            }
            report.result = outcome.result;
        }
        Err(_) => {
            report.status = "unconfirmed";
            report.reason = Some("run_unconfirmed");
        }
    }
    if record && matches!(report.status, "passed" | "failed") && report.cleanup == "confirmed" {
        if let (Some(source_ref), Some(commit), Some(blob), Some(result)) = (
            &report.source_ref,
            &report.checked_commit,
            &report.checks_blob_oid,
            &report.result,
        ) {
            report.history = Some(
                save_completed(
                    engine,
                    owner,
                    source_ref.clone(),
                    commit.clone(),
                    blob.clone(),
                    result.clone(),
                )
                .await,
            );
        }
    }
    report
}
