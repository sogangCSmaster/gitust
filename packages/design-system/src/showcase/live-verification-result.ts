import type { AgentVerificationResult, AgentVerificationStep } from "../patterns/AgentSwarmView";

const invalid = (): never => { throw new Error("Invalid verification result from server"); };
const record = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const exitCode = (value: unknown): value is number =>
  Number.isSafeInteger(value) && (value as number) >= -2_147_483_648 && (value as number) <= 2_147_483_647;
const duration = (value: unknown): value is number =>
  Number.isSafeInteger(value) && (value as number) >= 0;

function parseSteps(value: unknown, requestedCommand: string, overallExitCode: number): AgentVerificationStep[] | undefined {
  // Older servers return only the aggregate command result.
  if (value === undefined) return undefined;
  if (!Array.isArray(value) || value.length > 9) return invalid();
  const steps: AgentVerificationStep[] = [];
  let failedExitCode: number | undefined;
  for (const [index, step] of value.entries()) {
    const maxCommandBytes = index === 0 ? 65_536 : 4096;
    if (!record(step) || typeof step.command !== "string" || !step.command.trim() || step.command.includes("\0") ||
        step.command.length > maxCommandBytes || new TextEncoder().encode(step.command).length > maxCommandBytes ||
        step.required !== (index !== 0) || (index === 0 && step.command !== requestedCommand)) return invalid();
    if (failedExitCode !== undefined) {
      if (step.exit_code !== null || step.duration_ms !== null) return invalid();
    } else {
      if (!exitCode(step.exit_code) || !duration(step.duration_ms)) return invalid();
      if (step.exit_code !== 0) failedExitCode = step.exit_code;
    }
    steps.push({ command: step.command, required: step.required, exitCode: step.exit_code as number | null,
      durationMs: step.duration_ms as number | null });
  }
  if (steps.length && overallExitCode !== (failedExitCode ?? 0)) return invalid();
  return steps;
}

export function parseVerificationResult(value: unknown, worktreeId: string, requestedCommand: string): AgentVerificationResult {
  if (!record(value) || value.worktree_id !== worktreeId || typeof value.command !== "string" ||
      !exitCode(value.exit_code) || !duration(value.duration_ms) ||
      typeof value.stdout !== "string" || typeof value.stderr !== "string") return invalid();
  return {
    requestedCommand, command: value.command, exitCode: value.exit_code, durationMs: value.duration_ms,
    stdout: value.stdout, stderr: value.stderr, outputTruncated: value.output_truncated === true,
    steps: parseSteps(value.steps, requestedCommand, value.exit_code),
  };
}
