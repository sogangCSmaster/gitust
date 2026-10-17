import type { LiveCommitSummary } from "./live-repo-context";
interface HeadResponse {
  commits?: LiveCommitSummary[];
  commit_sha?: string;
  entries?: Array<{ path: string; is_dir: boolean }>;
  truncated?: boolean;
}
/** Bounded HEAD reads stay separate from explicit worktree branch/SHA comparisons. */
export const readHeadSnapshot = async (
  fetchRepository: (path: string) => Promise<HeadResponse>,
) => {
  const [commits, tree] = await Promise.all([
    fetchRepository("commits?rev=HEAD&limit=8"),
    fetchRepository("tree?rev=HEAD&recursive=true"),
  ]);
  if (
    typeof tree.commit_sha !== "string" ||
    !/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i.test(tree.commit_sha)
  )
    throw new Error("Server did not identify the repository tree revision");
  if (!Array.isArray(tree.entries))
    throw new Error("Server did not provide repository tree entries");
  return [
    commits,
    { ...tree, commit_sha: tree.commit_sha, entries: tree.entries },
  ] as const;
};
