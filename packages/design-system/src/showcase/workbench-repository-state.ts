import { createMemo, createSignal } from "solid-js";
import type { DiffLine } from "../patterns/VirtualDiffViewer";
import type { FileTreeNode } from "../patterns/VirtualFileTree";
import {
  statusDiffLine,
  type LiveCommitSummary,
  type LiveWorktreeReview,
} from "./live-repo-context";

export const createWorkbenchRepositoryState = (liveMode: () => boolean) => {
  const [liveDiffLines, setLiveDiffLines] = createSignal<DiffLine[] | null>(
    null,
  );
  const [liveFileTreeNodes, setLiveFileTreeNodes] = createSignal<
    FileTreeNode[] | null
  >(null);
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
  const [treeRevision, setTreeRevision] = createSignal<string | null>(null);
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
    treeRevision,
    setTreeRevision,
    setWorktreeReview,
    liveCommits,
    worktreeReview,
    lookupMicros,
    selectedFilePath,
    diffLines: liveDiffLines,
    liveDiffFiles,
    diffTruncated,
    treeTruncated,
    fileTreeNodes: liveFileTreeNodes,
    selectedTreePath,
    setSelectedTreePath,
    setLiveBlobLines,
    blobLines,
  };
};
