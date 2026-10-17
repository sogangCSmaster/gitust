import { automaticJob, worktreeId } from "./run-history-fixture";
export const actionsRepoId = "99999999-9999-4999-8999-999999999999";
export const secondWorktreeId = "77777777-7777-4777-8777-777777777777";
export const pageCursor = (id = worktreeId) => `v1.${actionsRepoId}.${id}`;
export const actionsPage = () => ({
  repo_id: actionsRepoId,
  order: "worktree_id_asc",
  scanned_worktrees: 1,
  worktrees: [
    {
      worktree_id: worktreeId,
      branch: "agent/real-actions",
      base_branch: "main",
      runs: [automaticJob()],
    },
  ],
  next_cursor: null as string | null,
});
