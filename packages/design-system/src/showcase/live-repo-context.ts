import type { DiffLine } from "../patterns/VirtualDiffViewer";
import type { FileTreeNode } from "../patterns/VirtualFileTree";

export interface LiveCommitSummary {
  sha: string;
  short_sha: string;
  summary: string;
  author_name: string;
}

export interface LiveWorktreeReview {
  worktreeId: string;
  branch: string;
  baseBranch: string;
  source: "verified" | "branch";
  headSha?: string;
  baseSha?: string;
  state: "loading" | "ready" | "error";
  error?: string;
}

export const formatErr = (err: unknown): string =>
  err instanceof Error ? err.message : String(err);

export const getLiveToken = (): string | null => {
  try {
    return typeof window !== "undefined"
      ? window.localStorage.getItem("gitust_token")
      : null;
  } catch (err) {
    console.warn(
      "Unable to read gitust_token from localStorage:",
      formatErr(err),
    );
    return null;
  }
};

export const statusDiffLine = (
  content: string,
  type: "context" | "add" | "del" | "hunk" = "hunk",
): DiffLine[] => [
  {
    index: 0,
    oldLineNumber: null,
    newLineNumber: null,
    type,
    content,
  },
];

export interface LiveRepoClientCallbacks {
  setActiveCommitRev: (rev: string) => void;
  setLookupMicros: (micros: number | null) => void;
  setSelectedFilePath: (path: string) => void;
  setLiveDiffLines: (lines: DiffLine[] | null) => void;
  setSelectedTreePath: (path: string) => void;
  setLiveBlobLines: (lines: DiffLine[] | null) => void;
  setLiveFileTreeNodes: (nodes: FileTreeNode[] | null) => void;
  setLiveCommits: (commits: LiveCommitSummary[]) => void;
  setLiveDiffFiles: (paths: string[]) => void;
  setDiffTruncated: (value: boolean) => void;
  setTreeTruncated: (value: boolean) => void;
  setWorktreeReview?: (review: LiveWorktreeReview | null) => void;
  getActiveCommitRev: () => string;
  getRepositoryId: () => string | null | undefined;
  canWorkbench: () => boolean;
  onRepositoryUnavailable?: (id: string) => void;
  showQuickFeedback: (msg: string) => void;
}
