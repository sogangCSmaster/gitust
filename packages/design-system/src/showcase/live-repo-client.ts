import type { DiffLine } from "../patterns/VirtualDiffViewer";
import type { FileTreeNode } from "../patterns/VirtualFileTree";

const EMPTY_TREE_SHA1 = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

export interface LiveCommitSummary {
  sha: string;
  short_sha: string;
  summary: string;
  author_name: string;
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
  setLiveFileTreeNodes: (nodes: FileTreeNode[] | null) => void;
  setLiveCommits: (commits: LiveCommitSummary[]) => void;
  setLiveDiffFiles: (paths: string[]) => void;
  setDiffTruncated: (value: boolean) => void;
  setTreeTruncated: (value: boolean) => void;
  getActiveCommitRev: () => string;
  showQuickFeedback: (msg: string) => void;
}

/**
 * Dedicated client for live Gitust HTTP API endpoints (`/api/v1/repo/*` and `/api/v1/sync/github`),
 * keeping live backend hydration strictly separated from in-memory showcase demo actions.
 * Uses a monotonically increasing `latestViewReqId` to drop out-of-order responses.
 */
export function createLiveRepoClient(cb: LiveRepoClientCallbacks) {
  let latestViewReqId = 0;
  let repoEpoch = 0;
  let commitDiff:
    | { head: string; base?: string; files: string[]; truncated: boolean }
    | undefined;
  const sessionCurrent = (epoch: number, token: string) =>
    epoch === repoEpoch && token === getLiveToken();
  const resetRepoEpoch = () => {
    repoEpoch += 1;
    latestViewReqId += 1;
    commitDiff = undefined;
    cb.setLiveDiffFiles([]);
    cb.setDiffTruncated(false);
    cb.setTreeTruncated(false);
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
  const loadLiveCommitDiff = async (rev: string) => {
    const reqId = ++latestViewReqId;
    const epoch = repoEpoch;
    const token = getLiveToken();
    cb.setActiveCommitRev(rev);
    cb.setLiveDiffFiles([]);
    cb.setDiffTruncated(false);
    cb.setLookupMicros(null);
    commitDiff = undefined;
    if (!token) return;
    const current = () =>
      reqId === latestViewReqId && sessionCurrent(epoch, token);
    cb.setLiveDiffLines([]);
    try {
      const data = await fetchDiff(rev, token);
      if (!current()) return;
      const paths = filePaths(data);
      const oid = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i;
      if (
        paths.length > 1 &&
        (!oid.test(data.head_sha) || !oid.test(data.base_sha))
      )
        throw new Error(
          "Server did not provide immutable commit IDs for this multi-file diff",
        );
      commitDiff = {
        head: data.head_sha ?? rev,
        base: data.base_sha,
        files: paths,
        truncated: data.truncated === true,
      };
      cb.setLiveDiffFiles(paths);
      cb.setDiffTruncated(commitDiff.truncated);
      cb.setSelectedFilePath(paths[0] ?? `commit:${rev}`);
      const firstPath = paths[0];
      if (paths.length > 1 && firstPath !== undefined) {
        // Aggregate lines have no file boundaries. Fetch only the selected path at fixed commits.
        const first = await fetchDiff(
          commitDiff.head,
          token,
          firstPath,
          commitDiff.base,
        );
        if (current()) showDiff(first, firstPath);
      } else showDiff(data, paths[0] ?? `commit:${rev}`);
    } catch (err) {
      if (!current()) return;
      const msg = `Diff lookup failed for ${rev}: ${formatErr(err)}`;
      cb.setLiveDiffLines(statusDiffLine(`[${msg}]`, "del"));
      cb.showQuickFeedback(msg);
    }
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
      if (current()) showDiff(data, path);
    } catch (err) {
      if (!current()) return;
      const msg = `Diff lookup failed for ${path}: ${formatErr(err)}`;
      cb.setLiveDiffLines(statusDiffLine(`[${msg}]`, "del"));
      cb.showQuickFeedback(msg);
    }
  };

  const selectFileFromTree = async (node: {
    path: string;
    kind: "file" | "dir";
  }) => {
    const reqId = ++latestViewReqId;
    commitDiff = undefined;
    cb.setLiveDiffFiles([]);
    cb.setDiffTruncated(false);
    cb.setSelectedFilePath(node.path);
    cb.setLookupMicros(null);
    cb.setLiveDiffLines([]);
    if (node.kind !== "file") {
      cb.setLiveDiffLines(
        statusDiffLine(
          `[Directory: ${node.path} — select a file to preview]`,
          "hunk",
        ),
      );
      return;
    }
    const rev = cb.getActiveCommitRev();
    const token = getLiveToken();
    if (!token) {
      cb.setLiveDiffLines(
        statusDiffLine("[Sign in to preview repository files]", "hunk"),
      );
      return;
    }
    try {
      const res = await fetch(
        `/api/v1/repo/blob?rev=${encodeURIComponent(rev)}&path=${encodeURIComponent(node.path)}`,
        { headers: { Authorization: `Bearer ${token}` } },
      );
      if (reqId !== latestViewReqId || token !== getLiveToken()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (reqId !== latestViewReqId || token !== getLiveToken()) return;
        const msg = `Blob lookup failed for ${node.path} (HTTP ${res.status}${errText ? `: ${errText}` : ""})`;
        cb.setLiveDiffLines(statusDiffLine(`[${msg}]`, "del"));
        cb.showQuickFeedback(msg);
        return;
      }
      const blob = await res.json();
      if (reqId !== latestViewReqId || token !== getLiveToken()) return;
      cb.setSelectedFilePath(node.path);
      cb.setLookupMicros(Number(blob.lookup_micros ?? 0));
      if (blob.is_binary) {
        const msg = `Binary file: ${node.path} (${blob.size ?? 0} bytes) — raw text preview disabled`;
        cb.setLiveDiffLines(statusDiffLine(`[${msg}]`, "hunk"));
        cb.showQuickFeedback(msg);
      } else if (blob.truncated) {
        const msg = `File exceeds 1 MiB viewer limit: ${node.path} (${blob.size ?? 0} bytes) — content truncated`;
        cb.setLiveDiffLines(statusDiffLine(`[${msg}]`, "hunk"));
        cb.showQuickFeedback(msg);
      } else if (typeof blob.content === "string") {
        if (blob.content.length === 0) {
          cb.setLiveDiffLines(
            statusDiffLine(`[Empty file: ${node.path} (0 bytes)]`, "hunk"),
          );
        } else {
          const rawLines = blob.content.split("\n");
          cb.setLiveDiffLines(
            rawLines.map((line: string, idx: number) => ({
              index: idx,
              oldLineNumber: idx + 1,
              newLineNumber: idx + 1,
              type: "context" as const,
              content: line,
            })),
          );
        }
        cb.showQuickFeedback(
          `Loaded ${node.path} via in-process gix (${blob.lookup_micros ?? 0}µs)`,
        );
      } else {
        cb.setLiveDiffLines(
          statusDiffLine(`[No text content returned for ${node.path}]`, "hunk"),
        );
      }
    } catch (err) {
      if (reqId !== latestViewReqId || token !== getLiveToken()) return;
      const msg = `Blob lookup failed for ${node.path}: ${formatErr(err)}`;
      cb.setLiveDiffLines(statusDiffLine(`[${msg}]`, "del"));
      cb.showQuickFeedback(msg);
    }
  };

  const hydrateFromLiveBackend = async () => {
    const token = getLiveToken();
    if (!token) return;
    const reqId = ++latestViewReqId;
    try {
      const headers = { Authorization: `Bearer ${token}` };
      const [commitsRes, treeRes] = await Promise.all([
        fetch("/api/v1/repo/commits?rev=HEAD&limit=8", { headers }),
        fetch("/api/v1/repo/tree?rev=HEAD&recursive=true", { headers }),
      ]);
      if (reqId !== latestViewReqId || token !== getLiveToken()) return;
      if (!commitsRes.ok || !treeRes.ok) {
        cb.setTreeTruncated(false);
        cb.setLiveCommits([]);
        cb.setLiveFileTreeNodes([]);
        cb.setLiveDiffLines([]);
        cb.showQuickFeedback(
          `Live repository hydration failed (HTTP ${commitsRes.status}/${treeRes.status})`,
        );
        return;
      }
      const [cData, tData] = await Promise.all([
        commitsRes.json(),
        treeRes.json(),
      ]);
      if (reqId !== latestViewReqId || token !== getLiveToken()) return;
      if (Array.isArray(cData.commits)) {
        cb.setLiveCommits(cData.commits);
        cb.setLookupMicros(Number(cData.lookup_micros ?? 0));
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
      await loadLiveCommitDiff("HEAD");
    } catch (err) {
      if (reqId !== latestViewReqId || token !== getLiveToken()) return;
      cb.setTreeTruncated(false);
      cb.setLiveCommits([]);
      cb.setLiveFileTreeNodes([]);
      cb.setLiveDiffLines([]);
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
    selectFileFromTree,
    selectCommitDiffFile,
    resetRepoEpoch,
    hydrateFromLiveBackend,
    triggerGithubSync,
  };
}
