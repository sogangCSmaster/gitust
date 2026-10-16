import type { AgentVerificationStep } from "../patterns/AgentSwarmView";
import type {
  RunHistoryEntry,
  RunHistoryState,
} from "../patterns/run-history-types";
import { getLiveToken } from "./live-repo-client";
export type {
  RunHistoryEntry,
  RunHistoryState,
} from "../patterns/run-history-types";

const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const oid = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i;
const invalid = (): never => {
  throw new Error("Invalid run history response from server");
};
const object = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const text = (value: unknown, cap: number): value is string =>
  typeof value === "string" &&
  value.length <= cap &&
  new TextEncoder().encode(value).length <= cap;
const duration = (value: unknown): value is number =>
  Number.isSafeInteger(value) && (value as number) >= 0;
const code = (value: unknown): value is number =>
  Number.isInteger(value) &&
  (value as number) >= -2_147_483_648 &&
  (value as number) <= 2_147_483_647;
const nullableOid = (value: unknown): value is string | null =>
  value === null || (typeof value === "string" && oid.test(value));

/** Historical command prefixes may be truncated; they are display text, never retry inputs. */
export function parseRunHistory(
  value: unknown,
  worktreeId: string,
): RunHistoryEntry[] {
  if (!uuid.test(worktreeId) || !Array.isArray(value) || value.length > 5)
    return invalid();
  const ids = new Set<string>();
  return value.map((entry): RunHistoryEntry => {
    if (
      !object(entry) ||
      typeof entry.completed_at !== "string" ||
      entry.completed_at.length > 64 ||
      !/^\d{4}-\d\d-\d\dT/.test(entry.completed_at) ||
      !Number.isFinite(Date.parse(entry.completed_at)) ||
      typeof entry.user_id !== "string" ||
      !uuid.test(entry.user_id) ||
      typeof entry.verification_requested !== "boolean" ||
      typeof entry.history_truncated !== "boolean" ||
      !nullableOid(entry.checked_commit) ||
      !nullableOid(entry.checks_blob_oid) ||
      entry.verification_requested !== (entry.checked_commit !== null) ||
      (entry.checks_blob_oid !== null && !entry.verification_requested)
    )
      return invalid();
    const result = entry.result;
    if (
      !object(result) ||
      result.worktree_id !== worktreeId ||
      typeof result.run_id !== "string" ||
      !uuid.test(result.run_id) ||
      ids.has(result.run_id) ||
      !text(result.command, 4096) ||
      !text(result.stdout, 16384) ||
      !text(result.stderr, 16384) ||
      !code(result.exit_code) ||
      !duration(result.duration_ms) ||
      typeof result.output_truncated !== "boolean" ||
      !Array.isArray(result.steps) ||
      result.steps.length < 1 ||
      result.steps.length > 9
    )
      return invalid();
    ids.add(result.run_id);
    let failed: number | undefined;
    const steps = result.steps.map((step, index): AgentVerificationStep => {
      if (
        !object(step) ||
        !text(step.command, 4096) ||
        step.required !== (index !== 0)
      )
        return invalid();
      if (failed !== undefined) {
        if (step.exit_code !== null || step.duration_ms !== null)
          return invalid();
      } else {
        if (!code(step.exit_code) || !duration(step.duration_ms))
          return invalid();
        if (step.exit_code !== 0) failed = step.exit_code;
      }
      return {
        command: step.command,
        required: step.required,
        exitCode: step.exit_code as number | null,
        durationMs: step.duration_ms as number | null,
      };
    });
    if (result.exit_code !== (failed ?? 0)) return invalid();
    return {
      runId: result.run_id,
      completedAt: entry.completed_at,
      userId: entry.user_id,
      verificationRequested: entry.verification_requested,
      checkedCommit: entry.checked_commit,
      checksBlobOid: entry.checks_blob_oid,
      historyTruncated: entry.history_truncated,
      result: {
        command: result.command,
        exitCode: result.exit_code,
        durationMs: result.duration_ms,
        stdout: result.stdout,
        stderr: result.stderr,
        outputTruncated: result.output_truncated,
        steps,
      },
    };
  });
}

export function createLiveRunHistoryClient(
  update: (state: RunHistoryState) => void,
) {
  let generation = 0;
  let controller: AbortController | undefined;
  const invalidate = () => {
    generation += 1;
    controller?.abort();
    controller = undefined;
  };
  const clear = () => {
    invalidate();
    update({ status: "idle", records: [] });
  };
  const load = async (worktreeId: string) => {
    invalidate();
    const request = generation;
    const token = getLiveToken();
    if (!token || !uuid.test(worktreeId)) {
      update({
        status: "error",
        records: [],
        error: "Sign in as a SiteAdmin to view this worktree's history.",
      });
      return;
    }
    controller = new AbortController();
    const current = () => {
      if (generation !== request) return false;
      if (token !== getLiveToken()) {
        clear();
        return false;
      }
      return true;
    };
    update({ status: "loading", records: [] });
    try {
      const response = await fetch(
        `/api/v1/swarm/worktrees/${encodeURIComponent(worktreeId)}/runs`,
        {
          headers: { Authorization: `Bearer ${token}` },
          cache: "no-store",
          signal: controller.signal,
        },
      );
      if (!current()) return;
      if (!response.ok)
        throw new Error(
          `Unable to load run history (HTTP ${response.status}). Refresh after checking your session and access.`,
        );
      const body = await response.text();
      if (!current()) return;
      if (body.length > 3 * 1024 * 1024) return invalid();
      let parsed: unknown;
      try {
        parsed = JSON.parse(body);
      } catch {
        return invalid();
      }
      const records = parseRunHistory(parsed, worktreeId);
      if (current()) update({ status: "ready", records });
    } catch (error) {
      if (current())
        update({
          status: "error",
          records: [],
          error:
            error instanceof Error
              ? error.message
              : "Unable to load run history.",
        });
    }
  };
  return { load, clear };
}
