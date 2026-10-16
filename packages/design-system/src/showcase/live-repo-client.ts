import type { DiffLine } from "../patterns/VirtualDiffViewer";
import type { FileTreeNode } from "../patterns/VirtualFileTree";

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

  const loadLiveCommitDiff = async (rev: string) => {
    const reqId = ++latestViewReqId;
    cb.setActiveCommitRev(rev);
    const token = getLiveToken();
    if (!token) return;
    cb.setLiveDiffLines([]);
    try {
      const res = await fetch(
        `/api/v1/repo/diff?head=${encodeURIComponent(rev)}`,
        { headers: { Authorization: `Bearer ${token}` } },
      );
      if (reqId !== latestViewReqId || token !== getLiveToken()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (reqId !== latestViewReqId || token !== getLiveToken()) return;
        const msg = `Diff lookup failed for ${rev} (HTTP ${res.status}${errText ? `: ${errText}` : ""})`;
        cb.setLiveDiffLines(statusDiffLine(`[${msg}]`, "del"));
        cb.showQuickFeedback(msg);
        return;
      }
      const data = await res.json();
      if (reqId !== latestViewReqId || token !== getLiveToken()) return;
      cb.setLookupMicros(Number(data.lookup_micros ?? 0));
      if (Array.isArray(data.files) && data.files[0]?.path) {
        cb.setSelectedFilePath(String(data.files[0].path));
      } else {
        cb.setSelectedFilePath(`commit:${rev}`);
      }
      if (Array.isArray(data.lines) && data.lines.length > 0) {
        cb.setLiveDiffLines(
          data.lines.map(
            (
              ln: {
                old_line: number | null;
                new_line: number | null;
                kind: "context" | "add" | "del" | "hunk";
                content: string;
              },
              idx: number,
            ) => ({
              index: idx,
              oldLineNumber: ln.old_line ?? null,
              newLineNumber: ln.new_line ?? null,
              type: ln.kind,
              content: ln.content,
            }),
          ),
        );
      } else {
        cb.setLiveDiffLines(
          statusDiffLine(`[No file changes in commit ${rev}]`, "hunk"),
        );
      }
    } catch (err) {
      if (reqId !== latestViewReqId || token !== getLiveToken()) return;
      const msg = `Diff lookup failed for ${rev}: ${formatErr(err)}`;
      cb.setLiveDiffLines(statusDiffLine(`[${msg}]`, "del"));
      cb.showQuickFeedback(msg);
    }
  };

  const selectFileFromTree = async (node: {
    path: string;
    kind: "file" | "dir";
  }) => {
    const reqId = ++latestViewReqId;
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
        if (token !== getLiveToken()) return;
        const dt = (performance.now() - t0).toFixed(1);
        if (res.ok) {
          const report = await res.json();
          if (token !== getLiveToken()) return;
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
          if (token !== getLiveToken()) return;
          cb.showQuickFeedback(
            `GitHub Mirror Sync rejected (HTTP ${res.status} in ${dt}ms${errText ? `: ${errText}` : ""})`,
          );
        }
      } catch (err) {
        if (token !== getLiveToken()) return;
        cb.showQuickFeedback(`GitHub Mirror Sync failed: ${formatErr(err)}`);
      }
    })();
  };

  return {
    loadLiveCommitDiff,
    selectFileFromTree,
    hydrateFromLiveBackend,
    triggerGithubSync,
  };
}
