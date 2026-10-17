import { createMemo, createSignal } from "solid-js";
import type { DiffLine } from "../patterns/VirtualDiffViewer";
import {
  statusDiffLine,
  type LiveCommitSummary,
  type LiveWorktreeReview,
} from "./live-repo-client";
import { generateLargeDiff, generateMonorepoFileTree } from "./mock-data";

export const createWorkbenchRepositoryState = (
  liveMode: () => boolean,
  activeTab: () => string,
) => {
  // Demo data is only available in the explicitly labelled showcase mode.
  const fallbackDiffLines = createMemo(() =>
    !liveMode() && activeTab() === "diff" ? generateLargeDiff(10000) : [],
  );
  const fallbackFileTreeNodes = createMemo(() =>
    !liveMode() && activeTab() === "tree" ? generateMonorepoFileTree(5000) : [],
  );
  const [liveDiffLines, setLiveDiffLines] = createSignal<ReturnType<
    typeof generateLargeDiff
  > | null>(null);
  const [liveFileTreeNodes, setLiveFileTreeNodes] = createSignal<ReturnType<
    typeof generateMonorepoFileTree
  > | null>(null);
  const [liveCommits, setLiveCommits] = createSignal<LiveCommitSummary[]>(
    liveMode()
      ? []
      : [
          {
            sha: "554fb6c",
            short_sha: "554fb6c",
            summary:
              "feat(git,server): Step 2-4 gix commit/tree/blob/diff & GitHub mirror sync",
            author_name: "yuseungjae",
          },
          {
            sha: "5edd70a",
            short_sha: "5edd70a",
            summary:
              "feat(mcp,oauth): Step 2-3 OAuth 2.1 PKCE S256 & MCP gateway",
            author_name: "yuseungjae",
          },
        ],
  );
  const [activeCommitRev, setActiveCommitRev] = createSignal("HEAD");
  const [worktreeReview, setWorktreeReview] =
    createSignal<LiveWorktreeReview | null>(null);
  const [lookupMicros, setLookupMicros] = createSignal<number | null>(null);
  const [liveDiffFiles, setLiveDiffFiles] = createSignal<string[]>([]);
  const [diffTruncated, setDiffTruncated] = createSignal(false);
  const [treeTruncated, setTreeTruncated] = createSignal(false);
  const [selectedFilePath, setSelectedFilePath] = createSignal(
    liveMode() ? "Repository preview" : "crates/gitust-git/src/browse.rs",
  );

  const [selectedTreePath, setSelectedTreePath] = createSignal("");
  const [liveBlobLines, setLiveBlobLines] = createSignal<DiffLine[] | null>(
    statusDiffLine("[Select a file to preview.]"),
  );
  const blobLines = createMemo(
    () => liveBlobLines() ?? statusDiffLine("[Select a file to preview.]"),
  );
  const diffLines = createMemo(() => liveDiffLines() ?? fallbackDiffLines());
  const fileTreeNodes = createMemo(
    () => liveFileTreeNodes() ?? fallbackFileTreeNodes(),
  );

  return {
    setActiveCommitRev,
    setLookupMicros,
    setSelectedFilePath,
    setLiveDiffLines,
    setLiveFileTreeNodes,
    setLiveCommits,
    activeCommitRev,
    setLiveDiffFiles,
    setDiffTruncated,
    setTreeTruncated,
    setWorktreeReview,
    liveCommits,
    worktreeReview,
    lookupMicros,
    selectedFilePath,
    diffLines,
    liveDiffFiles,
    diffTruncated,
    treeTruncated,
    fileTreeNodes,
    selectedTreePath,
    setSelectedTreePath,
    setLiveBlobLines,
    blobLines,
  };
};
