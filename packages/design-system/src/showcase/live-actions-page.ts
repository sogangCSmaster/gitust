import type { AutomaticRunJob } from "../patterns/run-history-types";
import {
  compareAutomaticRunTimes,
  isUuid,
  parseAutomaticRuns,
} from "./live-automatic-runs";

export interface AutomaticWorktreeGroup {
  worktreeId: string;
  branch: string;
  baseBranch: string;
  runs: AutomaticRunJob[];
}
export interface AutomaticActionsPage {
  repositoryId: string;
  scannedWorktrees: number;
  worktrees: AutomaticWorktreeGroup[];
  nextCursor: string | null;
}
const MAX_BRANCH_METADATA_BYTES = 32 * 1024;
const invalid = (): never => {
  throw new Error("Invalid automatic Actions page from server");
};
const object = (value: unknown, keys: string[]): Record<string, unknown> => {
  if (!value || typeof value !== "object" || Array.isArray(value))
    return invalid();
  const wire = value as Record<string, unknown>;
  if (
    Object.keys(wire).length !== keys.length ||
    Object.keys(wire).some((key) => !keys.includes(key))
  )
    return invalid();
  return wire;
};
const branch = (value: unknown, empty = false): value is string =>
  typeof value === "string" &&
  value.length <= MAX_BRANCH_METADATA_BYTES &&
  (empty || value.length > 0) &&
  !/[\u0000-\u001f\u007f-\u009f]/.test(value);
function cursorId(value: unknown, repositoryId: string): string {
  if (typeof value !== "string" || value.length !== 76) return invalid();
  const [version, repo, after] = value.split(".");
  if (
    version !== "v1" ||
    repo !== repositoryId.toLowerCase() ||
    !isUuid(after) ||
    after !== after.toLowerCase()
  )
    return invalid();
  return after;
}

export function parseActionsPage(
  value: unknown,
  repositoryId: string,
  cursor: string | null = null,
  limit = 10,
): AutomaticActionsPage {
  if (
    !isUuid(repositoryId) ||
    !Number.isInteger(limit) ||
    limit < 1 ||
    limit > 20
  )
    return invalid();
  const after = cursor === null ? null : cursorId(cursor, repositoryId);
  const wire = object(value, [
    "repo_id",
    "order",
    "scanned_worktrees",
    "worktrees",
    "next_cursor",
  ]);
  if (
    !isUuid(wire.repo_id) ||
    wire.repo_id.toLowerCase() !== repositoryId.toLowerCase() ||
    wire.order !== "worktree_id_asc" ||
    !Number.isInteger(wire.scanned_worktrees) ||
    (wire.scanned_worktrees as number) < 0 ||
    (wire.scanned_worktrees as number) > limit ||
    !Array.isArray(wire.worktrees) ||
    wire.worktrees.length > (wire.scanned_worktrees as number)
  )
    return invalid();
  let previousId = after;
  const groups = wire.worktrees.map((value): AutomaticWorktreeGroup => {
    const group = object(value, [
      "worktree_id",
      "branch",
      "base_branch",
      "runs",
    ]);
    if (
      !isUuid(group.worktree_id) ||
      !branch(group.branch) ||
      !branch(group.base_branch, true) ||
      group.branch.length + group.base_branch.length >
        MAX_BRANCH_METADATA_BYTES ||
      new TextEncoder().encode(group.branch + group.base_branch).length >
        MAX_BRANCH_METADATA_BYTES
    )
      return invalid();
    const id = group.worktree_id.toLowerCase();
    if (previousId !== null && id <= previousId) return invalid();
    previousId = id;
    const runs = parseAutomaticRuns(group.runs, id);
    for (let index = 1; index < runs.length; index++) {
      const left = runs[index - 1]!;
      const right = runs[index]!;
      const order = compareAutomaticRunTimes(left.createdAt, right.createdAt);
      if (order < 0 || (order === 0 && left.runId <= right.runId))
        return invalid();
    }
    return {
      worktreeId: id,
      branch: group.branch,
      baseBranch: group.base_branch,
      runs,
    };
  });
  if (wire.next_cursor !== null) {
    const next = cursorId(wire.next_cursor, repositoryId);
    if (
      wire.scanned_worktrees !== limit ||
      (after !== null && next <= after) ||
      (previousId !== null && next < previousId)
    )
      return invalid();
  }
  return {
    repositoryId: repositoryId.toLowerCase(),
    scannedWorktrees: wire.scanned_worktrees as number,
    worktrees: groups,
    nextCursor: wire.next_cursor as string | null,
  };
}
