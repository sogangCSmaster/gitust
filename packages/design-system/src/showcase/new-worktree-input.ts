export const WORKTREE_AGENTS = [
  { value: "CodexSwarm", label: "Codex Swarm" },
  { value: "ClaudeCode", label: "Claude Code" },
  { value: "CursorAgent", label: "Cursor Agent" },
  { value: "InternalBot", label: "Internal Bot" },
] as const;
export type WorktreeAgentKind = (typeof WORKTREE_AGENTS)[number]["value"];
export interface NewWorktreeInput {
  repoId: string;
  promptSummary: string;
  baseBranch: string;
  branch: string;
  agent: WorktreeAgentKind;
}
export type NewWorktreeResult =
  | { kind: "created"; taskId: number; worktreeId: string }
  | { kind: "error"; error: string; uncertain?: boolean }
  | { kind: "stale" };

const validBranch = (branch: string) =>
  branch.length > 0 &&
  branch.length <= 1024 &&
  branch !== "@" &&
  branch !== "HEAD" &&
  !branch.startsWith("-") &&
  !branch.startsWith("refs/") &&
  !branch.includes("..") &&
  !branch.includes("//") &&
  !branch.includes("@{") &&
  !branch.endsWith(".") &&
  !/[\s\p{Cc}~^:?*\[\\]/u.test(branch) &&
  branch
    .split("/")
    .every((part) => part && !part.startsWith(".") && !part.endsWith(".lock"));
export const validateNewWorktreeInput = (
  input: NewWorktreeInput,
): string | null => {
  if (!input.repoId)
    return "Select a connected repository before creating a worktree.";
  if (!input.promptSummary.trim()) return "Enter a task name.";
  if (new TextEncoder().encode(input.promptSummary).byteLength > 4096)
    return "Task name must be 4,096 UTF-8 bytes or fewer.";
  if (!validBranch(input.branch) || !input.branch.startsWith("agent/"))
    return "Use a valid branch inside agent/, such as agent/my-task.";
  if (
    !validBranch(input.baseBranch) ||
    /^agent(?:\/|$)/i.test(input.baseBranch)
  )
    return "Enter an existing repository branch outside agent/.";
  if (!WORKTREE_AGENTS.some((agent) => agent.value === input.agent))
    return "Choose a supported agent.";
  return null;
};
export const createWorktreeBranch = () => {
  const suffix = Array.from(crypto.getRandomValues(new Uint8Array(8)), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
  return `agent/task-${suffix}`;
};
