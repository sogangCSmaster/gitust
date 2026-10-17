import { getLiveToken } from "./live-repo-context";
import type { RepositoryBranch } from "./live-branches";

// Management targets must also work as stored worktree/outbound targets; read-only
// branch browsing deliberately accepts a wider set of existing Git references.
export const isDefaultBranchCandidate = (branch: RepositoryBranch) =>
  branch.name !== "@" &&
  branch.name !== "HEAD" &&
  !branch.name.startsWith("-") &&
  !branch.name.startsWith("refs/") &&
  !/^agent(?:\/|$)/i.test(branch.name) &&
  !/[\p{White_Space}\p{Cc}]/u.test(branch.name);

export interface DefaultBranchIntent {
  repoId: string;
  token: string;
  epoch: number;
  key: number;
  currentRef: string;
}
export type DefaultBranchResult =
  { kind: "saved" } | { kind: "stale" } | { kind: "error"; error: string };
export interface DefaultBranchActions {
  begin: () => DefaultBranchIntent | null;
  isCurrent: (intent: DefaultBranchIntent) => boolean;
  invalidateReads: () => void;
  confirmed: (
    intent: DefaultBranchIntent,
    branch: string,
    head: string,
  ) => void;
  cancel: (intent: DefaultBranchIntent) => void;
  refresh: (intent: DefaultBranchIntent) => Promise<void>;
  unavailable: (intent: DefaultBranchIntent) => void;
}

export const createDefaultBranchMutation = (options: {
  isCurrent: DefaultBranchActions["isCurrent"];
  invalidateReads: () => void;
  confirmed: (
    intent: DefaultBranchIntent,
    branch: string,
    head: string,
  ) => void;
  unavailable: DefaultBranchActions["unavailable"];
}) => {
  let pending: { key: number; controller: AbortController } | undefined;
  const cancel = (key?: number) => {
    if (!pending || (key !== undefined && pending.key !== key)) return;
    pending.controller.abort();
    pending = undefined;
  };
  const save = async (
    intent: DefaultBranchIntent,
    target: RepositoryBranch,
  ): Promise<DefaultBranchResult> => {
    if (!options.isCurrent(intent) || intent.token !== getLiveToken())
      return { kind: "stale" };
    if (pending)
      return {
        kind: "error",
        error: "A default change is already in progress.",
      };
    if (
      target.ref_name === intent.currentRef ||
      !isDefaultBranchCandidate(target)
    )
      return { kind: "error", error: "Choose another committed branch." };
    options.invalidateReads();
    const own = { key: intent.key, controller: new AbortController() };
    pending = own;
    const current = () =>
      pending === own &&
      options.isCurrent(intent) &&
      intent.token === getLiveToken();
    try {
      const response = await fetch(
        `/api/v1/repos/${encodeURIComponent(intent.repoId)}/default-branch`,
        {
          method: "PATCH",
          headers: {
            Authorization: `Bearer ${intent.token}`,
            "Content-Type": "application/json",
          },
          body: JSON.stringify({
            target_ref: target.ref_name,
            expected_current_ref: intent.currentRef,
            expected_target_commit: target.commit_sha,
          }),
          signal: own.controller.signal,
        },
      );
      if (!current()) return { kind: "stale" };
      if ([401, 403, 404].includes(response.status)) {
        options.unavailable(intent);
        return { kind: "stale" };
      }
      if (response.status === 409)
        return {
          kind: "error",
          error:
            "The default or selected branch changed, or this branch is unsupported. Refresh before retrying.",
        };
      if (response.status !== 200) throw new Error("unconfirmed");
      const text = await response.text();
      if (!current()) return { kind: "stale" };
      if (
        text.length > 256 * 1024 ||
        new TextEncoder().encode(text).length > 256 * 1024
      )
        throw new Error("invalid response");
      const raw: unknown = JSON.parse(text);
      if (!raw || typeof raw !== "object" || Array.isArray(raw))
        throw new Error("invalid response");
      const row = raw as Record<string, unknown>;
      if (
        Object.keys(row).sort().join() !== "branch,head_commit,id" ||
        typeof row.id !== "string" ||
        row.id.toLowerCase() !== intent.repoId ||
        row.branch !== target.name ||
        typeof row.head_commit !== "string" ||
        row.head_commit.toLowerCase() !== target.commit_sha
      )
        throw new Error("invalid response");
      if (!current()) return { kind: "stale" };
      pending = undefined;
      options.confirmed(intent, target.name, target.commit_sha);
      return { kind: "saved" };
    } catch {
      if (!current()) return { kind: "stale" };
      return {
        kind: "error",
        error: "Could not confirm the default change. Refresh before retrying.",
      };
    } finally {
      if (pending === own) cancel(intent.key);
    }
  };
  return { save, cancel };
};
