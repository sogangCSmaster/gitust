import type {
  AutomaticRunPhase,
  AutomaticRunReason,
  RunActivityEntry,
} from "./run-history-types";
import type { StatusTone } from "../components/workspace/presentation";
const labels: Record<AutomaticRunPhase, string> = {
  prepared: "Preparing",
  queued: "Queued",
  running: "Running",
  completed: "Completed",
  failed: "Failed",
  timed_out: "Timed out",
  cancelled: "Cancelled",
  interrupted: "Interrupted",
  rejected: "Rejected",
  superseded: "Superseded",
  not_applied: "Not applied",
  blocked: "Blocked",
};
export function activityLabel(entry: RunActivityEntry): string {
  if (entry.job && entry.job.phase !== "completed")
    return labels[entry.job.phase];
  if (entry.history)
    return entry.history.result.exitCode === 0 ? "Passed" : "Failed";
  return "Completed";
}
export function activityTone(entry: RunActivityEntry): StatusTone {
  if (entry.job && entry.job.phase !== "completed") {
    if (["failed", "timed_out", "blocked"].includes(entry.job.phase))
      return "danger";
    if (["interrupted", "rejected"].includes(entry.job.phase)) return "warning";
    return "neutral";
  }
  return entry.history
    ? entry.history.result.exitCode === 0
      ? "success"
      : "danger"
    : "neutral";
}
const explanations: Record<AutomaticRunReason, string> = {
  authority_unavailable:
    "The account, administrator role, ownership or repository access is no longer available.",
  execution_disabled:
    "Trusted server execution is disabled. This attempt will not resume automatically.",
  state_changed: "The worktree changed state before this attempt could run.",
  target_changed: "The worktree no longer contains this exact commit.",
  target_unavailable:
    "The exact checkout could not be validated. Inspect the worktree before starting another attempt.",
  policy_changed:
    "The committed Checks policy changed or automatic Checks were disabled.",
  policy_unavailable: "The committed Checks policy could not be read safely.",
  operator_context_ended:
    "The original local operator session ended. Start a new explicit local operation to run again.",
  service_stopped:
    "The service stopped this attempt. Unconfirmed execution still requires operator recovery.",
  unconfirmed_execution:
    "Execution could not be confirmed after restart. This attempt will never replay automatically.",
  command_failed:
    "A command returned a nonzero exit code. Inspect the stored output when available.",
  execution_failed:
    "The execution did not complete normally. Inspect the worktree and execution recovery state.",
  validation_failed:
    "The checkout or Checks policy could not be confirmed after execution.",
  timed_out:
    "The execution deadline was reached. Cleanup may still require operator confirmation.",
  git_not_applied:
    "The proposed Git update was not accepted, so this attempt did not run.",
  superseded: "A newer accepted revision replaced this queued attempt.",
};
export function activityExplanation(entry: RunActivityEntry): string {
  if (entry.job?.reason) return explanations[entry.job.reason];
  if (
    entry.job &&
    entry.history &&
    ["prepared", "queued", "running"].includes(entry.job.phase)
  ) {
    return `Stored output is available. The lifecycle snapshot still says ${labels[entry.job.phase].toLowerCase()}; refresh to update its status.`;
  }
  switch (entry.job?.phase) {
    case "prepared":
      return "Waiting for the proposed Git update to be confirmed.";
    case "queued":
      return "Waiting to execute after current authority, commit and Checks policy are rechecked.";
    case "running":
      return "Execution has started. No completed output is stored yet.";
    default:
      return "Automatic Checks completed. Captured output may no longer be in the retained run history.";
  }
}
