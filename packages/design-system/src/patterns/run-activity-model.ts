import type {
  AutomaticRunJob,
  RunActivityEntry,
  RunHistoryEntry,
} from "./run-history-types";
export function mergeRunActivity(
  records: RunHistoryEntry[],
  jobs: AutomaticRunJob[] = [],
): RunActivityEntry[] {
  const entries = new Map<string, RunActivityEntry>();
  for (const history of records)
    entries.set(history.runId.toLowerCase(), { runId: history.runId, history });
  for (const job of jobs) {
    const entry = entries.get(job.runId);
    const history = entry?.history;
    if (
      history &&
      (!history.automatic ||
        history.userId.toLowerCase() !== job.userId ||
        history.checkedCommit?.toLowerCase() !== job.expected ||
        history.checksBlobOid?.toLowerCase() !== job.checksOid)
    ) {
      throw new Error(
        "Stored output does not match its automatic activity identity",
      );
    }
    entries.set(job.runId, { ...(entry ?? {}), runId: job.runId, job });
  }
  if (entries.size > 13)
    throw new Error("Activity response exceeds the retained record limit");
  return [...entries.values()].sort(
    (a, b) => Date.parse(activityTime(b)) - Date.parse(activityTime(a)),
  );
}
export const activityTime = (entry: RunActivityEntry): string =>
  entry.job?.changedAt ?? entry.history!.completedAt;
export const activityCommit = (entry: RunActivityEntry): string | null =>
  entry.job?.expected ?? entry.history?.checkedCommit ?? null;
