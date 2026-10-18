import { getLiveToken } from "./live-repo-context";
import {
  isOid,
  isUuid,
  isUtcTime,
  compareAutomaticRunTimes,
} from "./live-automatic-runs";
import { readLocalChecksJson } from "./live-local-checks-client";
import type { LocalChecksRecord } from "./live-local-checks";

type Phase =
  | "queued"
  | "running"
  | "completed"
  | "failed"
  | "rejected"
  | "superseded"
  | "interrupted";
export interface LocalChecksAutomaticRun {
  runId: string;
  sourceRef: string;
  expectedCommit: string;
  checksBlobOid: string | null;
  phase: Phase;
  reason: string | null;
  history: "saved" | "failed" | "not_attempted";
  createdAt: string;
  updatedAt: string;
}
export interface LocalChecksAutomaticState {
  status: "idle" | "loading" | "ready" | "error";
  runs: LocalChecksAutomaticRun[];
  error?: string;
}
const invalid = (): never => {
  throw new Error("Invalid automatic Local Checks response");
};
const exact = (value: unknown, keys: string[]): Record<string, unknown> => {
  if (
    !value ||
    typeof value !== "object" ||
    Array.isArray(value) ||
    Object.keys(value).length !== keys.length ||
    !keys.every((key) => Object.hasOwn(value, key))
  )
    return invalid();
  return value as Record<string, unknown>;
};
const terminalReasons: Record<string, string[]> = {
  rejected: [
    "execution_disabled",
    "authority_unavailable",
    "repository_unavailable",
    "policy_disabled",
    "policy_missing",
    "policy_invalid",
    "push_not_confirmed",
  ],
  superseded: ["latest_queued_replaced", "default_changed", "target_changed"],
  interrupted: [
    "acceptance_unconfirmed",
    "service_stopped",
    "unconfirmed_execution",
  ],
};
const ordinaryRef = (value: unknown): value is string =>
  typeof value === "string" &&
  value.length <= 32779 &&
  value.startsWith("refs/heads/") &&
  value.length > 11 &&
  new TextEncoder().encode(value).length <= 32779 &&
  !/^agent(?:\/|$)/i.test(value.slice(11)) &&
  !/[\x00-\x20\x7f~^:?*\[\\]/u.test(value) &&
  !value.includes("..") &&
  !value.includes("@{") &&
  !value.endsWith(".") &&
  value
    .split("/")
    .every((part) => part && !part.startsWith(".") && !part.endsWith(".lock"));
export function parseLocalChecksAutomatic(
  value: unknown,
  repositoryId: string,
): LocalChecksAutomaticRun[] {
  const body = exact(value, ["version", "repo_id", "runs"]);
  if (
    body.version !== 1 ||
    !isUuid(repositoryId) ||
    !isUuid(body.repo_id) ||
    body.repo_id.toLowerCase() !== repositoryId.toLowerCase() ||
    !Array.isArray(body.runs) ||
    body.runs.length > 7
  )
    return invalid();
  const seen = new Set<string>();
  let previous = -1,
    running = 0,
    queued = 0,
    recent = 0;
  return body.runs.map((value) => {
    const row = exact(value, [
      "run_id",
      "source_ref",
      "expected_commit",
      "checks_blob_oid",
      "phase",
      "reason",
      "history",
      "created_at",
      "updated_at",
    ]);
    if (
      typeof row.phase !== "string" ||
      !isUuid(row.run_id) ||
      !ordinaryRef(row.source_ref) ||
      !isOid(row.expected_commit) ||
      !(
        row.checks_blob_oid === null ||
        (isOid(row.checks_blob_oid) &&
          row.checks_blob_oid.length === row.expected_commit.length)
      ) ||
      !isUtcTime(row.created_at) ||
      !isUtcTime(row.updated_at) ||
      compareAutomaticRunTimes(row.updated_at, row.created_at) < 0
    )
      return invalid();
    const phase = row.phase as Phase;
    const live = phase === "running" || phase === "queued";
    if (live) {
      if (
        row.reason !== null ||
        row.history !== "not_attempted" ||
        row.checks_blob_oid === null
      )
        return invalid();
    } else if (phase === "completed" || phase === "failed") {
      if (
        row.checks_blob_oid === null ||
        !(
          (row.history === "saved" &&
            row.reason === (phase === "completed" ? null : "command_failed")) ||
          (row.history === "failed" && row.reason === "history_unconfirmed")
        )
      )
        return invalid();
    } else if (
      !Object.hasOwn(terminalReasons, phase) ||
      row.history !== "not_attempted" ||
      typeof row.reason !== "string" ||
      !terminalReasons[phase]!.includes(row.reason)
    )
      return invalid();
    const order = phase === "running" ? 0 : phase === "queued" ? 1 : 2;
    const id = row.run_id.toLowerCase();
    if (
      order < previous ||
      seen.has(id) ||
      (phase === "running"
        ? ++running > 1
        : phase === "queued"
          ? ++queued > 1
          : ++recent > 5)
    )
      return invalid();
    previous = order;
    seen.add(id);
    return {
      runId: id,
      sourceRef: row.source_ref,
      expectedCommit: row.expected_commit.toLowerCase(),
      checksBlobOid:
        row.checks_blob_oid === null ? null : row.checks_blob_oid.toLowerCase(),
      phase,
      reason: row.reason as string | null,
      history: row.history as LocalChecksAutomaticRun["history"],
      createdAt: row.created_at,
      updatedAt: row.updated_at,
    };
  });
}
export const matchingLocalChecksRecord = (
  run: LocalChecksAutomaticRun,
  records: LocalChecksRecord[],
) =>
  run.checksBlobOid === null
    ? undefined
    : records.find(
        (record) =>
          record.runId === run.runId &&
          record.sourceRef === run.sourceRef &&
          record.checkedCommit === run.expectedCommit &&
          record.checksBlobOid === run.checksBlobOid &&
          (run.phase !== "completed" || record.status === "passed") &&
          (run.phase !== "failed" || record.status === "failed"),
      );

/** Lifecycle reads never retry execution or share the history/POST request generation. */
export function createLocalChecksAutomaticClient(
  publish: (state: LocalChecksAutomaticState) => void,
  context: {
    repositoryId: () => string | undefined;
    selectionEpoch: () => number;
    canRead: () => boolean;
    onDenied: () => void;
  },
) {
  let generation = 0;
  let controller: AbortController | undefined;
  let isCurrent = () => false;
  const clear = () => {
    generation++;
    controller?.abort();
    controller = undefined;
    isCurrent = () => false;
    publish({ status: "idle", runs: [] });
  };
  const load = async () => {
    clear();
    const request = generation,
      token = getLiveToken(),
      id = context.repositoryId(),
      epoch = context.selectionEpoch();
    if (!token || !isUuid(id) || !context.canRead()) return;
    const pending = new AbortController();
    controller = pending;
    const current = () => {
      if (request !== generation) return false;
      if (
        token !== getLiveToken() ||
        id !== context.repositoryId() ||
        epoch !== context.selectionEpoch() ||
        !context.canRead()
      ) {
        clear();
        return false;
      }
      return true;
    };
    isCurrent = current;
    publish({ status: "loading", runs: [] });
    try {
      const response = await fetch(
        `/api/v1/repos/${encodeURIComponent(id)}/local-check-automatic-runs`,
        {
          headers: { Authorization: `Bearer ${token}` },
          cache: "no-store",
          signal: pending.signal,
        },
      );
      if (!current()) return;
      if (response.status === 401 || response.status === 403) {
        pending.abort();
        publish({
          status: "error",
          runs: [],
          error:
            "Automatic Local Checks access was denied. Check your session and repository access.",
        });
        context.onDenied();
        return;
      }
      if (!response.ok) throw new Error("Automatic status unavailable");
      const raw = await readLocalChecksJson(response);
      if (!current()) return;
      const runs = parseLocalChecksAutomatic(raw, id);
      if (current()) publish({ status: "ready", runs });
    } catch {
      if (current()) {
        pending.abort();
        publish({
          status: "error",
          runs: [],
          error: "Unable to load automatic Local Checks. Refresh to try again.",
        });
      }
    }
  };
  return { load, clear, isCurrent: () => isCurrent() };
}
