import type {
  AgentVerificationResult,
  AgentVerificationStep,
} from "../patterns/AgentSwarmView";
import { isOid, isUtcTime, isUuid } from "./live-automatic-runs";
import { parseCommandOutput } from "./live-command-output";

export interface LocalChecksRecord {
  runId: string;
  completedAt: string;
  sourceRef: string;
  checkedCommit: string;
  checksBlobOid: string;
  status: "passed" | "failed";
  historyTruncated: boolean;
  result: AgentVerificationResult;
}

export const invalidLocalChecks = (): never => {
  throw new Error("Invalid recorded Local Checks response from server.");
};
const object = (
  value: unknown,
  keys: string[],
  optional: string[] = [],
): Record<string, unknown> => {
  if (!value || typeof value !== "object" || Array.isArray(value))
    return invalidLocalChecks();
  const row = value as Record<string, unknown>;
  if (
    keys.some((key) => !(key in row)) ||
    Object.keys(row).some(
      (key) => !keys.includes(key) && !optional.includes(key),
    )
  )
    return invalidLocalChecks();
  return row;
};
const text = (value: unknown, limit: number): value is string =>
  typeof value === "string" &&
  value.length <= limit &&
  new TextEncoder().encode(value).length <= limit;
const command = (value: unknown): value is string =>
  text(value, 4096) && value.trim().length > 0 && !value.includes("\0");
const duration = (value: unknown): value is number =>
  Number.isSafeInteger(value) && (value as number) >= 0;
const code = (value: unknown): value is number =>
  Number.isInteger(value) &&
  (value as number) >= -2147483648 &&
  (value as number) <= 2147483647;

/** Store append order is authoritative, including equal timestamps or a clock moving backwards. */
export function parseLocalChecks(
  value: unknown,
  repositoryId: string,
): LocalChecksRecord[] {
  const page = object(value, ["version", "repo_id", "records"]);
  if (
    !isUuid(repositoryId) ||
    page.version !== 1 ||
    !isUuid(page.repo_id) ||
    page.repo_id.toLowerCase() !== repositoryId.toLowerCase() ||
    !Array.isArray(page.records) ||
    page.records.length > 5
  )
    return invalidLocalChecks();
  const seen = new Set<string>();
  return page.records.map((value): LocalChecksRecord => {
    const row = object(value, [
      "completed_at",
      "source_ref",
      "checked_commit",
      "checks_blob_oid",
      "status",
      "result",
      "history_truncated",
    ]);
    if (
      !isUtcTime(row.completed_at) ||
      !text(row.source_ref, 32779) ||
      !row.source_ref.startsWith("refs/heads/") ||
      row.source_ref.length === 11 ||
      /^agent(?:\/|$)/i.test(row.source_ref.slice(11)) ||
      /[\u0000-\u001f\u007f]/.test(row.source_ref) ||
      !isOid(row.checked_commit) ||
      !isOid(row.checks_blob_oid) ||
      row.checked_commit.length !== row.checks_blob_oid.length ||
      !["passed", "failed"].includes(row.status as string) ||
      typeof row.history_truncated !== "boolean"
    )
      return invalidLocalChecks();
    const result = object(row.result, [
      "run_id",
      "worktree_id",
      "command",
      "exit_code",
      "duration_ms",
      "stdout",
      "stderr",
      "output_truncated",
      "steps",
      "backend",
      "network_isolated",
    ]);
    if (
      !isUuid(result.run_id) ||
      !isUuid(result.worktree_id) ||
      seen.has(result.run_id.toLowerCase()) ||
      !command(result.command) ||
      !code(result.exit_code) ||
      !duration(result.duration_ms) ||
      !text(result.stdout, 16384) ||
      !text(result.stderr, 16384) ||
      typeof result.output_truncated !== "boolean" ||
      !["macos_seatbelt", "linux_bubblewrap"].includes(
        result.backend as string,
      ) ||
      result.network_isolated !== true ||
      !Array.isArray(result.steps) ||
      result.steps.length < 1 ||
      result.steps.length > 8
    )
      return invalidLocalChecks();
    seen.add(result.run_id.toLowerCase());
    let failed: number | undefined;
    const steps = result.steps.map((value): AgentVerificationStep => {
      const step = object(
        value,
        ["command", "required", "exit_code", "duration_ms"],
        ["output"],
      );
      if (!command(step.command) || step.required !== true)
        return invalidLocalChecks();
      if (failed !== undefined) {
        if (
          step.exit_code !== null ||
          step.duration_ms !== null ||
          step.output !== undefined
        )
          return invalidLocalChecks();
      } else {
        if (
          !code(step.exit_code) ||
          !duration(step.duration_ms) ||
          step.output === undefined
        )
          return invalidLocalChecks();
        object(step.output, ["stdout", "stderr", "truncated"]);
        if (step.exit_code !== 0) failed = step.exit_code;
      }
      const output = parseCommandOutput(
        step.output,
        step.exit_code === null,
        1024,
      );
      return {
        command: step.command,
        required: true,
        exitCode: step.exit_code as number | null,
        durationMs: step.duration_ms as number | null,
        ...(output === undefined ? {} : { output }),
      };
    });
    if (
      result.exit_code !== (failed ?? 0) ||
      (row.status === "passed") !== (result.exit_code === 0)
    )
      return invalidLocalChecks();
    return {
      runId: result.run_id.toLowerCase(),
      completedAt: row.completed_at,
      sourceRef: row.source_ref,
      checkedCommit: row.checked_commit,
      checksBlobOid: row.checks_blob_oid,
      status: row.status as "passed" | "failed",
      historyTruncated: row.history_truncated,
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
