import type { AgentSandboxTask } from "./AgentSwarmView";

export function filterAgentSwarmTasks(
  tasks: AgentSandboxTask[],
  rawQuery: string,
): AgentSandboxTask[] {
  const q = rawQuery.trim().toLowerCase();
  if (!q) return tasks;
  return tasks.filter(
    (t) =>
      t.branch.toLowerCase().includes(q) ||
      t.agent.toLowerCase().includes(q) ||
      t.promptSummary.toLowerCase().includes(q) ||
      t.sandboxStatus.toLowerCase().includes(q),
  );
}
