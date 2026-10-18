import { createLiveRepositorySync } from "./live-repo-sync";
import {
  formatErr,
  getLiveToken,
  statusDiffLine,
  type LiveRepoClientCallbacks,
} from "./live-repo-context";
import {
  createLiveRepoSession,
  type LiveRepoSessionController,
} from "./live-repo-session";

export { formatErr, getLiveToken, statusDiffLine } from "./live-repo-context";
export type {
  LiveCommitSummary,
  LiveWorktreeReview,
  LiveRepoClientCallbacks,
} from "./live-repo-context";
export { createLiveRepoSession, type LiveRepoSessionController };

export function extendLiveRepoClient(session: LiveRepoSessionController) {
  const {
    cb,
    getEpoch,
    bumpViewReqId,
    getViewReqId,
    bumpBlobReqId,
    getBlobReqId,
    getCommitDiff,
    getTreeSnapshot,
    sessionCurrent,
    fetchRepository,
    fetchDiff,
    assertSnapshot,
    showDiff,
    hydrateFromLiveBackend,
  } = session;

  const selectCommitDiffFile = async (path: string) => {
    const snapshot = getCommitDiff();
    const token = getLiveToken();
    if (
      !token ||
      !snapshot?.files.includes(path) ||
      snapshot.repositoryId !== cb.getRepositoryId()
    )
      return;
    const reqId = bumpViewReqId();
    const epoch = getEpoch();
    const repositoryId = cb.getRepositoryId();
    const current = () =>
      reqId === getViewReqId() && sessionCurrent(epoch, token, repositoryId);
    cb.setSelectedFilePath(path);
    cb.setLiveDiffLines([]);
    cb.setLookupMicros(null);
    cb.setDiffTruncated(snapshot.truncated);
    try {
      const data = await fetchDiff(
        snapshot.repositoryId,
        snapshot.head,
        token,
        current,
        path,
        snapshot.base,
      );
      if (current()) {
        assertSnapshot(data, snapshot.head, snapshot.base);
        if (snapshot.review) cb.setWorktreeReview?.(snapshot.review);
        showDiff(data, path);
      }
    } catch (err) {
      if (!current()) return;
      const msg = `Diff lookup failed for ${path}: ${formatErr(err)}`;
      cb.setLiveDiffLines(statusDiffLine(`[${msg}]`, "del"));
      if (snapshot.review)
        cb.setWorktreeReview?.({
          ...snapshot.review,
          state: "error",
          error: formatErr(err),
        });
      cb.showQuickFeedback(msg);
    }
  };

  const selectFileFromTree = async (node: {
    path: string;
    kind: "file" | "dir";
  }) => {
    const reqId = bumpBlobReqId();
    const epoch = getEpoch();
    const repositoryId = cb.getRepositoryId();
    cb.setSelectedTreePath(node.path);
    cb.setLiveBlobLines([]);
    if (node.kind !== "file") {
      cb.setLiveBlobLines(
        statusDiffLine(
          `[Directory: ${node.path} — select a file to preview]`,
          "hunk",
        ),
      );
      return;
    }
    const { treeRev, treeRepository } = getTreeSnapshot();
    const token = getLiveToken();
    if (!token) {
      cb.setLiveBlobLines(
        statusDiffLine("[Sign in to preview repository files]", "hunk"),
      );
      return;
    }
    if (!treeRev || treeRepository !== repositoryId) {
      const message =
        "Repository tree revision is unavailable; refresh repository files before opening a file";
      cb.setLiveBlobLines(statusDiffLine(`[${message}]`, "del"));
      cb.showQuickFeedback(message);
      return;
    }
    try {
      if (!repositoryId) return;
      const blob = await fetchRepository(
        `blob?rev=${encodeURIComponent(treeRev)}&path=${encodeURIComponent(node.path)}`,
        token,
        repositoryId,
        () =>
          reqId === getBlobReqId() &&
          sessionCurrent(epoch, token, repositoryId),
      );
      if (
        reqId !== getBlobReqId() ||
        !sessionCurrent(epoch, token, repositoryId)
      )
        return;
      if (
        typeof blob.commit_sha !== "string" ||
        blob.commit_sha.toLowerCase() !== treeRev.toLowerCase()
      )
        throw new Error(
          "Server returned a different repository snapshot; refresh repository files",
        );
      cb.setSelectedTreePath(node.path);
      if (blob.is_binary) {
        const msg = `Binary file: ${node.path} (${blob.size ?? 0} bytes) — raw text preview disabled`;
        cb.setLiveBlobLines(statusDiffLine(`[${msg}]`, "hunk"));
        cb.showQuickFeedback(msg);
      } else if (blob.truncated) {
        const msg = `File exceeds 1 MiB viewer limit: ${node.path} (${blob.size ?? 0} bytes) — content truncated`;
        cb.setLiveBlobLines(statusDiffLine(`[${msg}]`, "hunk"));
        cb.showQuickFeedback(msg);
      } else if (typeof blob.content === "string") {
        if (blob.content.length === 0) {
          cb.setLiveBlobLines(
            statusDiffLine(`[Empty file: ${node.path} (0 bytes)]`, "hunk"),
          );
        } else {
          const rawLines = blob.content.split("\n");
          cb.setLiveBlobLines(
            rawLines.map((line: string, idx: number) => ({
              index: idx,
              oldLineNumber: idx + 1,
              newLineNumber: idx + 1,
              type: "context" as const,
              content: line,
            })),
          );
        }
      } else {
        cb.setLiveBlobLines(
          statusDiffLine(`[No text content returned for ${node.path}]`, "hunk"),
        );
      }
    } catch (err) {
      if (
        reqId !== getBlobReqId() ||
        !sessionCurrent(epoch, token, repositoryId)
      )
        return;
      const msg = `Blob lookup failed for ${node.path}: ${formatErr(err)}`;
      cb.setLiveBlobLines(statusDiffLine(`[${msg}]`, "del"));
      cb.showQuickFeedback(msg);
    }
  };

  const triggerGithubSync = createLiveRepositorySync(
    cb,
    getEpoch,
    sessionCurrent,
    hydrateFromLiveBackend,
  );

  return {
    branches: session.branches,
    loadLiveCommitDiff: session.loadLiveCommitDiff,
    loadLiveWorktreeDiff: session.loadLiveWorktreeDiff,
    selectFileFromTree,
    selectCommitDiffFile,
    resetRepoEpoch: session.resetRepoEpoch,
    hydrateFromLiveBackend,
    triggerGithubSync,
  };
}

export function createLiveRepoClient(cb: LiveRepoClientCallbacks) {
  return extendLiveRepoClient(createLiveRepoSession(cb));
}
