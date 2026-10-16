import type { AgentVerificationResult } from "./AgentSwarmView";

export interface RunHistoryEntry {
  runId: string;
  completedAt: string;
  userId: string;
  verificationRequested: boolean;
  checkedCommit: string | null;
  checksBlobOid: string | null;
  historyTruncated: boolean;
  result: AgentVerificationResult;
}

export interface RunHistoryState {
  status: "idle" | "loading" | "ready" | "error";
  records: RunHistoryEntry[];
  error?: string;
}
