import type { AgentVerificationResult } from "./AgentSwarmView";

export interface RunHistoryEntry {
  runId: string;
  completedAt: string;
  userId: string;
  verificationRequested: boolean;
  automatic: boolean;
  checkedCommit: string | null;
  checksBlobOid: string | null;
  historyTruncated: boolean;
  result: AgentVerificationResult;
}

export interface RunHistoryState {
  status: "idle" | "loading" | "ready" | "error";
  records: RunHistoryEntry[];
  jobs?: AutomaticRunJob[];
  error?: string;
}

export type AutomaticRunPhase =
  | "prepared"
  | "queued"
  | "running"
  | "completed"
  | "failed"
  | "timed_out"
  | "cancelled"
  | "interrupted"
  | "rejected"
  | "superseded"
  | "not_applied"
  | "blocked";
export type AutomaticRunReason =
  | "authority_unavailable"
  | "execution_disabled"
  | "state_changed"
  | "target_changed"
  | "target_unavailable"
  | "policy_changed"
  | "policy_unavailable"
  | "operator_context_ended"
  | "service_stopped"
  | "unconfirmed_execution"
  | "command_failed"
  | "execution_failed"
  | "validation_failed"
  | "timed_out"
  | "git_not_applied"
  | "superseded";
export interface AutomaticRunJob {
  /** Bound by the requested, authorized route; the wire Job has no worktree field. */
  worktreeId: string;
  runId: string;
  userId: string;
  origin: "mcp" | "git_http" | "local_operator";
  purpose: "automatic_checks";
  expected: string;
  previous: string | null;
  checksOid: string;
  phase: AutomaticRunPhase;
  reason: AutomaticRunReason | null;
  createdAt: string;
  changedAt: string;
}
export interface RunActivityEntry {
  runId: string;
  history?: RunHistoryEntry;
  job?: AutomaticRunJob;
}
