import type { LiveCommitSummary } from "./live-repo-context";
interface HeadResponse {
  head_sha?: string;
  commits?: LiveCommitSummary[];
  commit_sha?: string;
  entries?: Array<{ path: string; is_dir: boolean }>;
  truncated?: boolean;
}
/** Bounded HEAD reads stay separate from explicit worktree branch/SHA comparisons. */
export const readHeadSnapshot = async (
  fetchRepository: (path: string) => Promise<HeadResponse>,
  revision: string,
) => {
  if (!/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i.test(revision))
    throw new Error(
      "Repository commit is unavailable; refresh repository metadata",
    );
  const query = encodeURIComponent(revision);
  const [commits, tree] = await Promise.all([
    fetchRepository(`commits?rev=${query}&limit=8`),
    fetchRepository(`tree?rev=${query}&recursive=true`),
  ]);
  if (
    typeof commits.head_sha !== "string" ||
    commits.head_sha.toLowerCase() !== revision.toLowerCase() ||
    !Array.isArray(commits.commits) ||
    commits.commits.length === 0 ||
    typeof commits.commits[0]?.sha !== "string" ||
    commits.commits[0].sha.toLowerCase() !== revision.toLowerCase()
  )
    throw new Error("Server did not identify the repository history revision");
  if (
    typeof tree.commit_sha !== "string" ||
    tree.commit_sha.toLowerCase() !== revision.toLowerCase()
  )
    throw new Error("Server did not identify the repository tree revision");
  if (!Array.isArray(tree.entries))
    throw new Error("Server did not provide repository tree entries");
  return [
    commits,
    { ...tree, commit_sha: tree.commit_sha, entries: tree.entries },
  ] as const;
};
