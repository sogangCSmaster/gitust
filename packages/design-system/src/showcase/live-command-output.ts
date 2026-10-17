import type { AgentVerificationStep } from "../patterns/AgentSwarmView";

/** Optional only for older servers/history. A skipped command cannot contain captured output. */
export function parseCommandOutput(
  value: unknown,
  skipped: boolean,
  cap: number,
): AgentVerificationStep["output"] {
  if (value === undefined) return undefined;
  const invalid = (): never => {
    throw new Error("Invalid command output from server");
  };
  if (
    skipped ||
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value)
  )
    return invalid();
  const output = value as Record<string, unknown>;
  const bounded = (text: unknown): text is string =>
    typeof text === "string" &&
    text.length <= cap &&
    new TextEncoder().encode(text).length <= cap;
  if (
    !bounded(output.stdout) ||
    !bounded(output.stderr) ||
    typeof output.truncated !== "boolean"
  )
    return invalid();
  return {
    stdout: output.stdout,
    stderr: output.stderr,
    truncated: output.truncated,
  };
}
