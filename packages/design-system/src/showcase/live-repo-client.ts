import type { DiffLine } from "../patterns/VirtualDiffViewer";
import type { FileTreeNode } from "../patterns/VirtualFileTree";
import type { AgentSandboxTask } from "../patterns/AgentSwarmView";

const EMPTY_TREE_SHA1 = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

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
  showQuickFeedback: (msg: string) => void;
}

/**
 * Dedicated client for live Gitust HTTP API endpoints (`/api/v1/repo/*` and `/api/v1/sync/github`),
 * keeping live backend hydration strictly separated from in-memory showcase demo actions.
 * Changes, file previews and metadata use independent request ordering within one session epoch.
 */
export function createLiveRepoClient(cb: LiveRepoClientCallbacks) {
  let latestViewReqId = 0;
  let latestBlobReqId = 0;
  let latestHydrationReqId = 0;
  let repoEpoch = 0;
  let treeRev: string | null = null;
  let commitDiff:
    | { head: string; base?: string; files: string[]; truncated: boolean; review?: LiveWorktreeReview }
    | undefined;
  const sessionCurrent = (epoch: number, token: string | null) =>
    epoch === repoEpoch && token === getLiveToken();
  const resetRepoEpoch = () => {
    repoEpoch += 1;
    latestViewReqId += 1;
    latestBlobReqId += 1;
    latestHydrationReqId += 1;
    commitDiff = undefined;
    treeRev = null;
    cb.setWorktreeReview?.(null);
    cb.setLiveDiffLines([]);
    cb.setLookupMicros(null);
    cb.setLiveDiffFiles([]);
    cb.setDiffTruncated(false);
    cb.setTreeTruncated(false);
    cb.setSelectedTreePath("");
    cb.setLiveBlobLines(statusDiffLine("[Select a file to preview.]"));
    cb.setLiveFileTreeNodes([]);
  };
  const filePaths = (data: { files?: Array<{ path: string }> }): string[] =>
    Array.isArray(data.files)
      ? data.files
          .map((file) => file.path)
          .filter((path) => typeof path === "string")
      : [];
  const showDiff = (
    data: {
      lookup_micros?: number;
      truncated?: boolean;
      files_changed?: number;
      files?: Array<{ path: string }>;
      lines?: Array<{
        old_line: number | null;
        new_line: number | null;
        kind: "context" | "add" | "del" | "hunk";
        content: string;
      }>;
    },
    path: string,
  ) => {
    cb.setSelectedFilePath(path);
    cb.setLookupMicros(Number(data.lookup_micros ?? 0));
    cb.setDiffTruncated(Boolean(data.truncated || commitDiff?.truncated));
    cb.setLiveDiffLines(
      Array.isArray(data.lines) && data.lines.length > 0
        ? data.lines.map((line, index) => ({
            index,
            oldLineNumber: line.old_line ?? null,
            newLineNumber: line.new_line ?? null,
            type: line.kind,
            content: line.content,
          }))
        : statusDiffLine(
            (data.files_changed ?? filePaths(data).length) > 0
              ? "[Changed file with no textual hunks — binary content, an empty file, or file metadata changed.]"
              : commitDiff?.review
                ? "[No committed changes between the worktree head and merge target.]"
                : `[No file changes in commit ${cb.getActiveCommitRev()}]`,
          ),
    );
  };
  const fetchDiff = async (
    head: string,
    token: string,
    path?: string,
    base?: string,
  ) => {
    const query = new URLSearchParams({ head });
    // The API resolves explicit bases as commits. A fixed root head derives its empty tree implicitly.
    if (base && base !== EMPTY_TREE_SHA1) query.set("base", base);
    if (path !== undefined) query.set("path", path);
    const response = await fetch(`/api/v1/repo/diff?${query}`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    if (!response.ok)
      throw new Error(
        `HTTP ${response.status}: ${(await response.text()).trim()}`,
      );
    return response.json();
  };
  const assertSnapshot = (data: { head_sha?: string; base_sha?: string }, head: string, base?: string) => {
    if (typeof data.head_sha !== "string" || data.head_sha.toLowerCase() !== head.toLowerCase() ||
        typeof data.base_sha !== "string" || data.base_sha.toLowerCase() !== base?.toLowerCase()) {
      throw new Error("Server returned a different diff revision; reopen the comparison");
    }
  };
  const loadDiff = async (rev: string, base?: string, reviewTarget?: LiveWorktreeReview) => {
    const reqId = ++latestViewReqId;
    const epoch = repoEpoch;
    const token = getLiveToken();
    cb.setActiveCommitRev(rev);
    cb.setLiveDiffFiles([]);
    cb.setDiffTruncated(false);
    cb.setLookupMicros(null);
    cb.setLiveDiffLines([]);
    cb.setSelectedFilePath(reviewTarget ? `worktree:${reviewTarget.branch}` : `commit:${rev}`);
    cb.setWorktreeReview?.(reviewTarget ?? null);
    commitDiff = undefined;
    const current = () =>
      reqId === latestViewReqId && sessionCurrent(epoch, token);
    try {
      if (!token) throw new Error("Sign in to review committed changes");
      if (reviewTarget) {
        if (!reviewTarget.worktreeId) throw new Error("No live worktree is available for review");
        if (!reviewTarget.baseBranch.trim()) throw new Error("Worktree merge target is unknown; restore its creation metadata before review");
        if (!reviewTarget.branch.trim()) throw new Error("Worktree branch is unknown");
        if (reviewTarget.source === "verified" && !/^[0-9a-f]{40}$/i.test(rev))
          throw new Error("Verified worktree has no immutable commit SHA; refresh worktrees");
      }
      const data = await fetchDiff(rev, token, undefined, base);
      if (!current()) return;
      const paths = filePaths(data);
      const oid = reviewTarget ? /^[0-9a-f]{40}$/i : /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i;
      if (
        (reviewTarget || paths.length > 1) &&
        (!oid.test(data.head_sha) || !oid.test(data.base_sha))
      )
        throw new Error(
          "Server did not provide immutable commit IDs for this diff",
        );
      if (reviewTarget?.source === "verified" && data.head_sha.toLowerCase() !== rev.toLowerCase())
        throw new Error("Server did not return the displayed verified commit; refresh worktrees");
      const resolvedReview: LiveWorktreeReview | undefined = reviewTarget
        ? { ...reviewTarget, state: "ready", headSha: data.head_sha, baseSha: data.base_sha }
        : undefined;
      commitDiff = {
        head: data.head_sha ?? rev,
        base: data.base_sha,
        files: paths,
        truncated: data.truncated === true,
        review: resolvedReview,
      };
      if (resolvedReview) {
        cb.setWorktreeReview?.(resolvedReview);
      }
      cb.setActiveCommitRev(commitDiff.head);
      cb.setLiveDiffFiles(paths);
      cb.setDiffTruncated(commitDiff.truncated);
      cb.setSelectedFilePath(paths[0] ?? `commit:${rev}`);
      const firstPath = paths[0];
      if (paths.length > 1 && firstPath !== undefined) {
        const snapshot = commitDiff;
        // Aggregate lines have no file boundaries. Fetch only the selected path at fixed commits.
        const first = await fetchDiff(
          snapshot.head,
          token,
          firstPath,
          snapshot.base,
        );
        if (current()) {
          assertSnapshot(first, snapshot.head, snapshot.base);
          showDiff(first, firstPath);
        }
      } else showDiff(data, paths[0] ?? `commit:${rev}`);
    } catch (err) {
      if (!current()) return;
      const msg = `Diff lookup failed for ${rev}: ${formatErr(err)}`;
      cb.setLiveDiffLines(statusDiffLine(`[${msg}]`, "del"));
      if (reviewTarget) cb.setWorktreeReview?.({ ...(commitDiff?.review ?? reviewTarget), state: "error", error: formatErr(err) });
      cb.showQuickFeedback(msg);
    }
  };
  const loadLiveCommitDiff = (rev: string) => loadDiff(rev);
  const branchRef = (branch: string) => branch.startsWith("refs/heads/") ? branch : `refs/heads/${branch}`;
  const loadLiveWorktreeDiff = (task: AgentSandboxTask) => {
    const verified = task.sandboxStatus === "verified";
    return loadDiff(verified ? task.verifiedCommit ?? "" : branchRef(task.branch), branchRef(task.baseBranch), {
      worktreeId: task.liveId ?? "", branch: task.branch, baseBranch: task.baseBranch,
      source: verified ? "verified" : "branch", headSha: verified ? task.verifiedCommit : undefined, state: "loading",
    });
  };
  const selectCommitDiffFile = async (path: string) => {
    const snapshot = commitDiff;
    const token = getLiveToken();
    if (!token || !snapshot?.files.includes(path)) return;
    const reqId = ++latestViewReqId;
    const epoch = repoEpoch;
    const current = () =>
      reqId === latestViewReqId && sessionCurrent(epoch, token);
    cb.setSelectedFilePath(path);
    cb.setLiveDiffLines([]);
    cb.setLookupMicros(null);
    cb.setDiffTruncated(snapshot.truncated);
    try {
      const data = await fetchDiff(snapshot.head, token, path, snapshot.base);
      if (current()) {
        assertSnapshot(data, snapshot.head, snapshot.base);
        if (snapshot.review) cb.setWorktreeReview?.(snapshot.review);
        showDiff(data, path);
      }
    } catch (err) {
      if (!current()) return;
      const msg = `Diff lookup failed for ${path}: ${formatErr(err)}`;
      cb.setLiveDiffLines(statusDiffLine(`[${msg}]`, "del"));
      if (snapshot.review) cb.setWorktreeReview?.({ ...snapshot.review, state: "error", error: formatErr(err) });
      cb.showQuickFeedback(msg);
    }
  };

  const selectFileFromTree = async (node: {
    path: string;
    kind: "file" | "dir";
  }) => {
    const reqId = ++latestBlobReqId;
    const epoch = repoEpoch;
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
    const rev = treeRev;
    const token = getLiveToken();
    if (!token) {
      cb.setLiveBlobLines(
        statusDiffLine("[Sign in to preview repository files]", "hunk"),
      );
      return;
    }
    if (!rev) {
      const message = "Repository tree revision is unavailable; refresh repository files before opening a file";
      cb.setLiveBlobLines(statusDiffLine(`[${message}]`, "del"));
      cb.showQuickFeedback(message);
      return;
    }
    try {
      const res = await fetch(
        `/api/v1/repo/blob?rev=${encodeURIComponent(rev)}&path=${encodeURIComponent(node.path)}`,
        { headers: { Authorization: `Bearer ${token}` } },
      );
      if (reqId !== latestBlobReqId || !sessionCurrent(epoch, token)) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (reqId !== latestBlobReqId || !sessionCurrent(epoch, token)) return;
        const msg = `Blob lookup failed for ${node.path} (HTTP ${res.status}${errText ? `: ${errText}` : ""})`;
        cb.setLiveBlobLines(statusDiffLine(`[${msg}]`, "del"));
        cb.showQuickFeedback(msg);
        return;
      }
      const blob = await res.json();
      if (reqId !== latestBlobReqId || !sessionCurrent(epoch, token)) return;
      if (typeof blob.commit_sha !== "string" || blob.commit_sha.toLowerCase() !== rev.toLowerCase())
        throw new Error("Server returned a different repository snapshot; refresh repository files");
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
      if (reqId !== latestBlobReqId || !sessionCurrent(epoch, token)) return;
      const msg = `Blob lookup failed for ${node.path}: ${formatErr(err)}`;
      cb.setLiveBlobLines(statusDiffLine(`[${msg}]`, "del"));
      cb.showQuickFeedback(msg);
    }
  };

  const hydrateFromLiveBackend = async () => {
    const token = getLiveToken();
    if (!token) return;
    const reqId = ++latestHydrationReqId;
    const diffRequestAtStart = latestViewReqId;
    const epoch = repoEpoch;
    const current = () => reqId === latestHydrationReqId && sessionCurrent(epoch, token);
    try {
      const headers = { Authorization: `Bearer ${token}` };
      const [commitsRes, treeRes] = await Promise.all([
        fetch("/api/v1/repo/commits?rev=HEAD&limit=8", { headers }),
        fetch("/api/v1/repo/tree?rev=HEAD&recursive=true", { headers }),
      ]);
      if (!current()) return;
      if (!commitsRes.ok || !treeRes.ok) {
        treeRev = null;
        cb.setTreeTruncated(false);
        cb.setLiveCommits([]);
        cb.setLiveFileTreeNodes([]);
        latestBlobReqId++;
        cb.setSelectedTreePath("");
        cb.setLiveBlobLines(statusDiffLine("[Repository files are unavailable; refresh to retry.]", "del"));
        cb.showQuickFeedback(
          `Live repository hydration failed (HTTP ${commitsRes.status}/${treeRes.status})`,
        );
        return;
      }
      const [cData, tData] = await Promise.all([
        commitsRes.json(),
        treeRes.json(),
      ]);
      if (!current()) return;
      if (typeof tData.commit_sha !== "string" || !/^[0-9a-f]{40}$/i.test(tData.commit_sha))
        throw new Error("Server did not identify the repository tree revision");
      if (!Array.isArray(tData.entries)) throw new Error("Server did not provide repository tree entries");
      if (treeRev !== tData.commit_sha) {
        latestBlobReqId++;
        cb.setSelectedTreePath("");
        cb.setLiveBlobLines(statusDiffLine("[Select a file to preview.]"));
      }
      treeRev = tData.commit_sha;
      if (Array.isArray(cData.commits)) {
        cb.setLiveCommits(cData.commits);
      }
      cb.setTreeTruncated(tData.truncated === true);
      if (Array.isArray(tData.entries)) {
        cb.setLiveFileTreeNodes(
          tData.entries.map((e: { path: string; is_dir: boolean }) => ({
            path: e.path,
            kind: e.is_dir ? ("dir" as const) : ("file" as const),
          })),
        );
      }
      if (diffRequestAtStart === latestViewReqId) await loadLiveCommitDiff("HEAD");
    } catch (err) {
      if (!current()) return;
      treeRev = null;
      cb.setTreeTruncated(false);
      cb.setLiveCommits([]);
      cb.setLiveFileTreeNodes([]);
      latestBlobReqId++;
      cb.setSelectedTreePath("");
      cb.setLiveBlobLines(statusDiffLine("[Repository files are unavailable; refresh to retry.]", "del"));
      cb.showQuickFeedback(
        `Live repository hydration failed: ${formatErr(err)}`,
      );
    }
  };

  const triggerGithubSync = () => {
    const token = getLiveToken();
    if (!token) {
      cb.showQuickFeedback(
        "Sign in to fetch from the configured GitHub mirror",
      );
      return;
    }
    const epoch = repoEpoch;
    const t0 = performance.now();
    cb.showQuickFeedback(
      "GitHub Read-Only Mirror Sync: Synchronizing upstream refs (Agent branches kept private)...",
    );
    void (async () => {
      try {
        const res = await fetch("/api/v1/sync/github", {
          method: "POST",
          headers: {
            Authorization: `Bearer ${token}`,
            "Content-Type": "application/json",
          },
          body: "{}",
        });
        if (!sessionCurrent(epoch, token)) return;
        const dt = (performance.now() - t0).toFixed(1);
        if (res.ok) {
          const report = await res.json();
          if (!sessionCurrent(epoch, token)) return;
          const sha = String(report.last_synced_commit ?? "").slice(0, 7);
          const priv = Number(report.private_agent_branches_filtered ?? 0);
          const skippedCount = Array.isArray(report.skipped_branches)
            ? report.skipped_branches.length
            : 0;
          cb.showQuickFeedback(
            `GitHub Mirror Synced @ ${sha} in ${dt}ms (${priv} private agent branches isolated, ${skippedCount} skipped)`,
          );
          await hydrateFromLiveBackend();
        } else {
          const errText = (await res.text().catch(() => "")).trim();
          if (!sessionCurrent(epoch, token)) return;
          cb.showQuickFeedback(
            `GitHub Mirror Sync rejected (HTTP ${res.status} in ${dt}ms${errText ? `: ${errText}` : ""})`,
          );
        }
      } catch (err) {
        if (!sessionCurrent(epoch, token)) return;
        cb.showQuickFeedback(`GitHub Mirror Sync failed: ${formatErr(err)}`);
      }
    })();
  };

  return {
    loadLiveCommitDiff,
    loadLiveWorktreeDiff,
    selectFileFromTree,
    selectCommitDiffFile,
    resetRepoEpoch,
    hydrateFromLiveBackend,
    triggerGithubSync,
  };
}
