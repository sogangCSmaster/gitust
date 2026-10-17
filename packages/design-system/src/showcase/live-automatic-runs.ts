import type {
  AutomaticRunJob,
  AutomaticRunPhase,
  AutomaticRunReason,
} from "../patterns/run-history-types";
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
export const isUuid = (value: unknown): value is string =>
  typeof value === "string" &&
  uuid.test(value) &&
  value.replaceAll("-", "") !== "0".repeat(32);
export const isOid = (value: unknown): value is string =>
  typeof value === "string" &&
  /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i.test(value) &&
  !/^0+$/.test(value);
/** UTC nanoseconds preserve Chrono's ordering without Date's fractional truncation. */
function instant(value: unknown): bigint | undefined {
  if (typeof value !== "string" || value.length > 64) return;
  const match = /^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d{1,9}))?Z$/.exec(
    value,
  );
  if (!match?.[1]) return;
  const millis = Date.parse(`${match[1]}Z`);
  if (
    !Number.isFinite(millis) ||
    new Date(millis).toISOString().slice(0, 19) !== match[1]
  )
    return;
  return BigInt(millis) * 1_000_000n + BigInt((match[2] ?? "").padEnd(9, "0"));
}
export const isUtcTime = (value: unknown): value is string =>
  instant(value) !== undefined;
export function compareAutomaticRunTimes(left: string, right: string): number {
  const first = instant(left);
  const second = instant(right);
  if (first === undefined || second === undefined) return invalidActivity();
  return first < second ? -1 : first > second ? 1 : 0;
}
const phases: readonly AutomaticRunPhase[] = [
  "prepared",
  "queued",
  "running",
  "completed",
  "failed",
  "timed_out",
  "cancelled",
  "interrupted",
  "rejected",
  "superseded",
  "not_applied",
  "blocked",
];
const reasons: readonly AutomaticRunReason[] = [
  "authority_unavailable",
  "execution_disabled",
  "state_changed",
  "target_changed",
  "target_unavailable",
  "policy_changed",
  "policy_unavailable",
  "operator_context_ended",
  "service_stopped",
  "unconfirmed_execution",
  "command_failed",
  "execution_failed",
  "validation_failed",
  "timed_out",
  "git_not_applied",
  "superseded",
];
const keys = [
  "run_id",
  "user_id",
  "origin",
  "purpose",
  "expected",
  "previous",
  "checks_oid",
  "phase",
  "reason",
  "created_at",
  "changed_at",
];
export const invalidActivity = (): never => {
  throw new Error("Invalid automatic activity response from server");
};
export function parseAutomaticRuns(
  value: unknown,
  worktreeId: string,
): AutomaticRunJob[] {
  if (!isUuid(worktreeId) || !Array.isArray(value) || value.length > 8)
    return invalidActivity();
  const seen = new Set<string>();
  const active = new Set<string>();
  let terminal = 0;
  return value.map((item): AutomaticRunJob => {
    if (typeof item !== "object" || item === null || Array.isArray(item))
      return invalidActivity();
    const wire = item as Record<string, unknown>;
    const created = instant(wire.created_at);
    const changed = instant(wire.changed_at);
    if (
      Object.keys(wire).length !== keys.length ||
      Object.keys(wire).some((key) => !keys.includes(key)) ||
      !isUuid(wire.run_id) ||
      !isUuid(wire.user_id) ||
      !isOid(wire.expected) ||
      !(wire.previous === null || isOid(wire.previous)) ||
      !isOid(wire.checks_oid) ||
      !["mcp", "git_http", "local_operator"].includes(wire.origin as string) ||
      wire.purpose !== "automatic_checks" ||
      !phases.includes(wire.phase as AutomaticRunPhase) ||
      created === undefined ||
      changed === undefined ||
      changed < created
    )
      return invalidActivity();
    const phase = wire.phase as AutomaticRunPhase;
    const live =
      phase === "prepared" || phase === "queued" || phase === "running";
    if (
      live || phase === "completed"
        ? wire.reason !== null
        : !reasons.includes(wire.reason as AutomaticRunReason)
    )
      return invalidActivity();
    const runId = wire.run_id.toLowerCase();
    if (seen.has(runId)) return invalidActivity();
    seen.add(runId);
    if (live) {
      if (active.has(phase)) return invalidActivity();
      active.add(phase);
    } else if (++terminal > 5) return invalidActivity();
    return {
      worktreeId: worktreeId.toLowerCase(),
      runId,
      userId: wire.user_id.toLowerCase(),
      origin: wire.origin as AutomaticRunJob["origin"],
      purpose: "automatic_checks",
      expected: wire.expected.toLowerCase(),
      previous:
        wire.previous === null ? null : (wire.previous as string).toLowerCase(),
      checksOid: wire.checks_oid.toLowerCase(),
      phase,
      reason: wire.reason as AutomaticRunReason | null,
      createdAt: wire.created_at as string,
      changedAt: wire.changed_at as string,
    };
  });
}
