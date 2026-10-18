import { getLiveToken } from "./live-repo-context";
import { isOid, isUuid } from "./live-automatic-runs";
import { readLocalChecksJson } from "./live-local-checks-client";
import {
  invalidLocalChecks,
  parseLocalChecks,
  type LocalChecksRecord,
} from "./live-local-checks";

export interface LocalChecksRunReport {
  status: "passed" | "failed" | "unconfirmed";
  cleanup: "confirmed" | "unconfirmed" | "not_created";
  history: {
    status: "saved" | "failed" | "not_attempted";
    reason: string | null;
  };
  record: LocalChecksRecord | null;
}
export interface LocalChecksRunState {
  status: "idle" | "running" | "complete" | "error";
  report?: LocalChecksRunReport;
  error?: string;
}
interface RunContext {
  repositoryId: () => string | undefined;
  selectionEpoch: () => number;
  canRun: () => boolean;
  sourceRef: () => string | undefined;
  commit: () => string | null | undefined;
}
const exact = (
  value: unknown,
  keys: string[],
): value is Record<string, unknown> =>
  !!value &&
  typeof value === "object" &&
  !Array.isArray(value) &&
  Object.keys(value).length === keys.length &&
  keys.every((key) => Object.hasOwn(value, key));
const reasons = new Set([
  "invalid_record",
  "storage_unavailable",
  "invalid_storage",
  "conflicting_run",
  "repository_unavailable",
]);

function reportFrom(
  value: unknown,
  id: string,
  source: string,
  commit: string,
  http: number,
): LocalChecksRunReport {
  if (
    !exact(value, [
      "version",
      "repo_id",
      "status",
      "reason",
      "cleanup",
      "history",
      "record",
    ]) ||
    value.version !== 1 ||
    !isUuid(value.repo_id) ||
    value.repo_id.toLowerCase() !== id.toLowerCase() ||
    !exact(value.history, ["status", "reason"])
  )
    return invalidLocalChecks();
  const history = value.history;
  if (value.status === "unconfirmed") {
    if (
      http !== 503 ||
      value.reason !== "run_unconfirmed" ||
      value.record !== null ||
      !["unconfirmed", "not_created"].includes(value.cleanup as string) ||
      history.status !== "not_attempted" ||
      history.reason !== "execution_not_confirmed"
    )
      return invalidLocalChecks();
    return {
      status: "unconfirmed",
      cleanup: value.cleanup as "unconfirmed" | "not_created",
      history: { status: "not_attempted", reason: "execution_not_confirmed" },
      record: null,
    };
  }
  if (
    !["passed", "failed"].includes(value.status as string) ||
    value.cleanup !== "confirmed"
  )
    return invalidLocalChecks();
  const [record] = parseLocalChecks(
    { version: 1, repo_id: value.repo_id, records: [value.record] },
    id,
  );
  if (
    !record ||
    record.sourceRef !== source ||
    record.checkedCommit.toLowerCase() !== commit ||
    record.status !== value.status
  )
    return invalidLocalChecks();
  if (history.status === "saved") {
    if (http !== 200 || value.reason !== null || history.reason !== null)
      return invalidLocalChecks();
  } else if (history.status === "failed") {
    if (
      http !== 503 ||
      value.reason !== "history_unconfirmed" ||
      typeof history.reason !== "string" ||
      !reasons.has(history.reason)
    )
      return invalidLocalChecks();
  } else return invalidLocalChecks();
  return {
    status: record.status,
    cleanup: "confirmed",
    history: {
      status: history.status as "saved" | "failed",
      reason: history.reason as string | null,
    },
    record,
  };
}
const admissionMessage = (code: unknown, status: number) => {
  switch (code) {
    case "local_checks_target_changed":
      return "The default branch changed. Refresh before running again.";
    case "local_checks_busy":
      return "Local Checks are already running. Refresh stored results before another run.";
    case "local_checks_disabled":
      return "Checks execution is disabled. Recorded results remain available.";
    case "local_checks_policy_missing":
      return "The default commit has no Checks policy.";
    case "local_checks_policy_invalid":
      return "The default commit has an invalid Checks policy.";
    default:
      return `Unable to start Local Checks (HTTP ${status}). Check your session and repository access.`;
  }
};

/** Explicit execution has its own generation; a history GET must never become a POST retry. */
export function createLocalChecksRunClient(
  publish: (state: LocalChecksRunState) => void,
  context: RunContext,
) {
  const originalSession = getLiveToken();
  let generation = 0;
  let pending: AbortController | undefined;
  let active = false;
  let isCurrent = () => false;
  const clear = () => {
    generation++;
    pending?.abort();
    pending = undefined;
    active = false;
    isCurrent = () => false;
    publish({ status: "idle" });
  };
  const run = async (): Promise<LocalChecksRunReport | undefined> => {
    if (active) return;
    clear();
    const request = generation;
    const token = getLiveToken();
    const id = context.repositoryId();
    const epoch = context.selectionEpoch();
    const source = context.sourceRef();
    const rawCommit = context.commit();
    if (!token || token !== originalSession) {
      publish({
        status: "error",
        error: "Your session changed. Reopen Local Checks before running.",
      });
      return;
    }
    if (
      !isUuid(id) ||
      !context.canRun() ||
      !source?.startsWith("refs/heads/") ||
      !isOid(rawCommit)
    )
      return;
    const commit = rawCommit.toLowerCase();
    const controller = new AbortController();
    pending = controller;
    const current = () => {
      if (request !== generation) return false;
      if (
        token !== getLiveToken() ||
        id !== context.repositoryId() ||
        epoch !== context.selectionEpoch() ||
        source !== context.sourceRef() ||
        commit !== context.commit()?.toLowerCase() ||
        !context.canRun()
      ) {
        clear();
        return false;
      }
      return true;
    };
    isCurrent = current;
    active = true;
    publish({ status: "running" });
    try {
      const response = await fetch(
        `/api/v1/repos/${encodeURIComponent(id)}/local-check-runs`,
        {
          method: "POST",
          headers: {
            Authorization: `Bearer ${token}`,
            "Content-Type": "application/json",
          },
          body: JSON.stringify({
            expected_source_ref: source,
            expected_commit: commit,
          }),
          cache: "no-store",
          signal: controller.signal,
        },
      );
      if (!current()) return;
      const value = await readLocalChecksJson(response);
      if (!current()) return;
      if (exact(value, ["code"])) {
        if (response.ok) return invalidLocalChecks();
        publish({
          status: "error",
          error: admissionMessage(value.code, response.status),
        });
        return;
      }
      const report = reportFrom(value, id, source, commit, response.status);
      if (current()) {
        publish({ status: "complete", report });
        return report;
      }
    } catch {
      if (current())
        publish({
          status: "error",
          error:
            "Could not confirm the Checks response. Refresh stored results before another run. A new run may repeat commands.",
        });
    } finally {
      if (request === generation) {
        active = false;
        pending = undefined;
      }
    }
  };
  return { run, clear, isCurrent: () => isCurrent() };
}
