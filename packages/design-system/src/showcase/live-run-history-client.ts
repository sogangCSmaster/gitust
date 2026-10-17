import type { AgentVerificationStep } from "../patterns/AgentSwarmView";
import type {
  RunHistoryEntry,
  RunHistoryState,
} from "../patterns/run-history-types";
import {
  parseAutomaticRuns,
  isUuid,
  isOid,
  isUtcTime,
} from "./live-automatic-runs";
import { mergeRunActivity } from "../patterns/run-activity-model";
import { getLiveToken } from "./live-repo-client";
import { parseCommandOutput } from "./live-command-output";
export type {
  RunHistoryEntry,
  RunHistoryState,
} from "../patterns/run-history-types";

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
  value === null || isOid(value);

/** Historical command prefixes may be truncated; they are display text, never retry inputs. */
export function parseRunHistory(
  value: unknown,
  worktreeId: string,
): RunHistoryEntry[] {
  if (!isUuid(worktreeId) || !Array.isArray(value) || value.length > 5)
    return invalid();
  const ids = new Set<string>();
  return value.map((entry): RunHistoryEntry => {
    if (
      !object(entry) ||
      !isUtcTime(entry.completed_at) ||
      typeof entry.user_id !== "string" ||
      !isUuid(entry.user_id) ||
      typeof entry.verification_requested !== "boolean" ||
      (entry.automatic !== undefined && typeof entry.automatic !== "boolean") ||
      typeof entry.history_truncated !== "boolean" ||
      !nullableOid(entry.checked_commit) ||
      !nullableOid(entry.checks_blob_oid) ||
      (entry.automatic === true && entry.verification_requested) ||
      (entry.verification_requested || entry.automatic === true) !==
        (entry.checked_commit !== null) ||
      (entry.automatic === true && entry.checks_blob_oid === null) ||
      (entry.checks_blob_oid !== null &&
        !entry.verification_requested &&
        entry.automatic !== true)
    )
      return invalid();
    const result = entry.result;
    if (
      !object(result) ||
      result.worktree_id !== worktreeId ||
      typeof result.run_id !== "string" ||
      !isUuid(result.run_id) ||
      ids.has(result.run_id.toLowerCase()) ||
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
    ids.add(result.run_id.toLowerCase());
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
      const output = parseCommandOutput(
        step.output,
        step.exit_code === null,
        1024,
      );
      return {
        command: step.command,
        required: step.required,
        exitCode: step.exit_code as number | null,
        durationMs: step.duration_ms as number | null,
        ...(output === undefined ? {} : { output }),
      };
    });
    if (result.exit_code !== (failed ?? 0)) return invalid();
    return {
      runId: result.run_id.toLowerCase(),
      completedAt: entry.completed_at,
      userId: entry.user_id,
      verificationRequested: entry.verification_requested,
      automatic: entry.automatic === true,
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
    update({ status: "idle", records: [], jobs: [] });
  };
  const load = async (worktreeId: string) => {
    invalidate();
    const request = generation;
    const token = getLiveToken();
    if (!token || !isUuid(worktreeId)) {
      update({
        status: "error",
        records: [],
        jobs: [],
        error: "Sign in as a SiteAdmin to view this worktree's history.",
      });
      return;
    }
    const pending = new AbortController();
    controller = pending;
    const current = () => {
      if (generation !== request) return false;
      if (token !== getLiveToken()) {
        clear();
        return false;
      }
      return true;
    };
    update({ status: "loading", records: [], jobs: [] });
    try {
      const read = async (path: string, limit: number): Promise<unknown> => {
        const response = await fetch(
          `/api/v1/swarm/worktrees/${encodeURIComponent(worktreeId)}/${path}`,
          {
            headers: { Authorization: `Bearer ${token}` },
            cache: "no-store",
            signal: pending.signal,
          },
        );
        if (!current()) return;
        if (!response.ok)
          throw new Error(
            `Unable to load activity (HTTP ${response.status}). Check your session and access.`,
          );
        const body = await response.text();
        if (!current()) return;
        if (new TextEncoder().encode(body).length > limit) return invalid();
        let parsed: unknown;
        try {
          parsed = JSON.parse(body);
        } catch {
          return invalid();
        }
        return current() ? parsed : undefined;
      };
      const [history, automatic] = await Promise.all([
        read("runs", 3 * 1024 * 1024),
        read("automatic-runs", 64 * 1024),
      ]);
      if (!current()) return;
      const records = parseRunHistory(history, worktreeId);
      const jobs = parseAutomaticRuns(automatic, worktreeId);
      mergeRunActivity(records, jobs);
      if (current()) update({ status: "ready", records, jobs });
    } catch (error) {
      if (current()) {
        pending.abort();
        update({
          status: "error",
          records: [],
          jobs: [],
          error:
            error instanceof Error
              ? error.message
              : "Unable to load run history.",
        });
      }
    }
  };
  return { load, clear };
}
