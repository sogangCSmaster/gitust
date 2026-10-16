import type { AgentProvider, AgentSandboxTask } from "../patterns/AgentSwarmView";
import { formatErr, getLiveToken } from "./live-repo-client";

export interface LiveWorktreeRecord {
  id: string;
  repo_id: string;
  agent: string;
  branch: string;
  base_branch: string;
  base_commit: string;
  prompt_summary: string;
  materialize_micros: number;
  status: string;
  last_sandbox_duration_ms: number;
  tests_passed: number;
  tests_total: number;
  files_changed: number;
  additions: number;
  deletions: number;
  github_synced: boolean;
  created_at: string;
}

export interface LiveSwarmClientCallbacks {
  getSwarmTasks: () => AgentSandboxTask[];
  setSwarmTasks: (
    updater:
      | AgentSandboxTask[]
      | ((prev: AgentSandboxTask[]) => AgentSandboxTask[])
  ) => void;
  onRepoMutated?: () => void;
  showQuickFeedback: (msg: string) => void;
}

const mapAgentProvider = (raw: string): AgentProvider => {
  switch (raw) {
    case "ClaudeCode":
    case "Claude Code":
      return "Claude Code";
    case "CursorAgent":
    case "Cursor Agent":
      return "Cursor Agent";
    case "CodexSwarm":
    case "Codex Swarm":
      return "Codex Swarm";
    case "InternalBot":
    case "Internal Bot":
      return "Internal Bot";
    default:
      return "Claude Code";
  }
};

const mapSandboxStatus = (
  raw: string
): AgentSandboxTask["sandboxStatus"] => {
  switch (raw.toLowerCase()) {
    case "verified":
      return "verified";
    case "failed":
      return "failed";
    case "merged":
      return "merged";
    case "discarded":
      return "discarded";
    case "pending":
    case "compiling":
    default:
      return "compiling";
  }
};

export const mapLiveWorktreeToTask = (
  wt: LiveWorktreeRecord,
  numericId: number
): AgentSandboxTask => ({
  id: numericId,
  liveId: wt.id,
  agent: mapAgentProvider(wt.agent),
  branch: wt.branch,
  baseBranch: wt.base_branch || "main",
  forkLatencyMs: Number(((wt.materialize_micros ?? 0) / 1000).toFixed(2)),
  promptSummary: wt.prompt_summary,
  sandboxStatus: mapSandboxStatus(wt.status),
  compileTimeMs: Number(wt.last_sandbox_duration_ms ?? 0),
  testsPassed: Number(wt.tests_passed ?? 0),
  testsTotal: Number(wt.tests_total ?? 0),
  filesChanged: Number(wt.files_changed ?? 0),
  additions: Number(wt.additions ?? 0),
  deletions: Number(wt.deletions ?? 0),
  githubSynced: Boolean(wt.github_synced),
  updatedAt: wt.created_at || "live",
});

/**
 * Dedicated client for live Agent Swarm & Worktree API endpoints (`/api/v1/swarm/worktrees/*`).
 * Enforces:
 * - Monotonically increasing `latestSwarmReqId` so stale hydrations never overwrite mutations
 * - 0ms Optimistic UI transitions with deterministic state rollback on HTTP error
 * - Honest telemetry (never fabricates GitHub sync status on local fast-forward merge)
 */
export function createLiveSwarmClient(cb: LiveSwarmClientCallbacks) {
  let latestSwarmReqId = 0;
  let swarmEpoch = 0;
  const inFlightWorktreeIds = new Set<number>();

  const hydrateSwarmWorktrees = async () => {
    const token = getLiveToken();
    if (!token) return;
    const epoch = swarmEpoch;
    const reqId = ++latestSwarmReqId;
    try {
      const res = await fetch("/api/v1/swarm/worktrees", {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (epoch !== swarmEpoch || reqId !== latestSwarmReqId || !getLiveToken()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== swarmEpoch || reqId !== latestSwarmReqId || !getLiveToken()) return;
        cb.showQuickFeedback(
          `Agent Swarm hydration failed (HTTP ${res.status}${errText ? `: ${errText}` : ""})`
        );
        return;
      }
      const data = (await res.json()) as LiveWorktreeRecord[];
      if (epoch !== swarmEpoch || reqId !== latestSwarmReqId || !getLiveToken()) return;
      if (Array.isArray(data)) {
        cb.setSwarmTasks(
          data.map((wt, idx) => mapLiveWorktreeToTask(wt, idx + 1))
        );
      }
    } catch (err) {
      if (epoch !== swarmEpoch || reqId !== latestSwarmReqId || !getLiveToken()) return;
      cb.showQuickFeedback(`Agent Swarm hydration failed: ${formatErr(err)}`);
    }
  };

  const approveLiveWorktree = async (id: number) => {
    const token = getLiveToken();
    if (!token) return;
    if (inFlightWorktreeIds.has(id)) {
      cb.showQuickFeedback("Worktree operation already in progress...");
      return;
    }
    const target = cb.getSwarmTasks().find((t) => t.id === id);
    if (!target || !target.liveId) {
      cb.showQuickFeedback(
        "Cannot approve worktree: no live worktree ID on server"
      );
      return;
    }
    if (
      target.sandboxStatus === "merged" ||
      target.sandboxStatus === "discarded"
    ) {
      return;
    }

    inFlightWorktreeIds.add(id);
    const epoch = swarmEpoch;
    ++latestSwarmReqId;
    const prevSnapshot: AgentSandboxTask = { ...target };
    const t0 = performance.now();

    // 0ms Optimistic UI update (honest local merge status; githubSynced stays false until synced)
    cb.setSwarmTasks((prev) =>
      prev.map((t) =>
        t.id === id
          ? {
              ...t,
              sandboxStatus: "merged",
              githubSynced: false,
              updatedAt: "just now",
            }
          : t
      )
    );
    const optimisticMs = (performance.now() - t0).toFixed(2);
    cb.showQuickFeedback(
      `Approving & fast-forward merging ${target.branch} into ${target.baseBranch} (${optimisticMs}ms optimistic)...`
    );

    try {
      const res = await fetch(
        `/api/v1/swarm/worktrees/${encodeURIComponent(target.liveId)}/approve`,
        {
          method: "POST",
          headers: { Authorization: `Bearer ${token}` },
        }
      );
      if (epoch !== swarmEpoch || !getLiveToken()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== swarmEpoch || !getLiveToken()) return;
        cb.setSwarmTasks((prev) =>
          prev.map((t) => (t.id === id ? prevSnapshot : t))
        );
        cb.showQuickFeedback(
          `Merge rejected for ${target.branch} (HTTP ${res.status}${errText ? `: ${errText}` : ""}) — rolled back`
        );
        return;
      }
      const merged = (await res.json()) as LiveWorktreeRecord;
      if (epoch !== swarmEpoch || !getLiveToken()) return;
      cb.setSwarmTasks((prev) =>
        prev.map((t) => (t.id === id ? mapLiveWorktreeToTask(merged, id) : t))
      );
      cb.showQuickFeedback(
        `Fast-forward merged ${merged.branch} into ${merged.base_branch} on-premise`
      );
      cb.onRepoMutated?.();
    } catch (err) {
      if (epoch !== swarmEpoch || !getLiveToken()) return;
      cb.setSwarmTasks((prev) =>
        prev.map((t) => (t.id === id ? prevSnapshot : t))
      );
      cb.showQuickFeedback(
        `Merge failed for ${target.branch}: ${formatErr(err)} — rolled back`
      );
    } finally {
      inFlightWorktreeIds.delete(id);
    }
  };

  const discardLiveWorktree = async (id: number) => {
    const token = getLiveToken();
    if (!token) return;
    if (inFlightWorktreeIds.has(id)) {
      cb.showQuickFeedback("Worktree operation already in progress...");
      return;
    }
    const target = cb.getSwarmTasks().find((t) => t.id === id);
    if (!target || !target.liveId) {
      cb.showQuickFeedback(
        "Cannot discard worktree: no live worktree ID on server"
      );
      return;
    }
    if (
      target.sandboxStatus === "merged" ||
      target.sandboxStatus === "discarded"
    ) {
      return;
    }

    inFlightWorktreeIds.add(id);
    const epoch = swarmEpoch;
    ++latestSwarmReqId;
    const prevSnapshot: AgentSandboxTask = { ...target };

    // 0ms Optimistic UI update
    cb.setSwarmTasks((prev) =>
      prev.map((t) =>
        t.id === id
          ? { ...t, sandboxStatus: "discarded", updatedAt: "just now" }
          : t
      )
    );
    cb.showQuickFeedback(
      `Discarding CoW Worktree ${target.branch} on server...`
    );

    try {
      const res = await fetch(
        `/api/v1/swarm/worktrees/${encodeURIComponent(target.liveId)}`,
        {
          method: "DELETE",
          headers: { Authorization: `Bearer ${token}` },
        }
      );
      if (epoch !== swarmEpoch || !getLiveToken()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== swarmEpoch || !getLiveToken()) return;
        cb.setSwarmTasks((prev) =>
          prev.map((t) => (t.id === id ? prevSnapshot : t))
        );
        cb.showQuickFeedback(
          `Discard rejected for ${target.branch} (HTTP ${res.status}${errText ? `: ${errText}` : ""}) — rolled back`
        );
        return;
      }
      cb.setSwarmTasks((prev) => prev.filter((t) => t.id !== id));
      cb.showQuickFeedback(
        `Discarded CoW Worktree ${target.branch} & removed branch`
      );
    } catch (err) {
      if (epoch !== swarmEpoch || !getLiveToken()) return;
      cb.setSwarmTasks((prev) =>
        prev.map((t) => (t.id === id ? prevSnapshot : t))
      );
      cb.showQuickFeedback(
        `Discard failed for ${target.branch}: ${formatErr(err)} — rolled back`
      );
    } finally {
      inFlightWorktreeIds.delete(id);
    }
  };

  const spawnLiveWorktree = async () => {
    const token = getLiveToken();
    if (!token) return;
    const epoch = swarmEpoch;
    ++latestSwarmReqId;

    const current = cb.getSwarmTasks();
    const newId =
      current.reduce((maxId, t) => (t.id > maxId ? t.id : maxId), 0) + 1;
    const suffix = `${Date.now().toString(36)}-${newId}`;
    const branch = `agent/claude/task-${suffix}`;
    const promptSummary = `Live CoW worktree #${newId} materialized via Agent Swarm UI`;

    const optimisticTask: AgentSandboxTask = {
      id: newId,
      agent: "Claude Code",
      branch,
      baseBranch: "main",
      forkLatencyMs: 0,
      promptSummary,
      sandboxStatus: "compiling",
      compileTimeMs: 0,
      testsPassed: 0,
      testsTotal: 0,
      filesChanged: 0,
      additions: 0,
      deletions: 0,
      githubSynced: false,
      updatedAt: "spawning...",
    };

    cb.setSwarmTasks((prev) => [optimisticTask, ...prev]);

    try {
      const res = await fetch("/api/v1/swarm/worktrees", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          agent: "ClaudeCode",
          branch,
          base_branch: "main",
          prompt_summary: promptSummary,
        }),
      });
      if (epoch !== swarmEpoch || !getLiveToken()) return;
      if (!res.ok) {
        const errText = (await res.text().catch(() => "")).trim();
        if (epoch !== swarmEpoch || !getLiveToken()) return;
        cb.setSwarmTasks((prev) => prev.filter((t) => t.id !== newId));
        cb.showQuickFeedback(
          `Worktree spawn failed (HTTP ${res.status}${errText ? `: ${errText}` : ""}) — rolled back`
        );
        return;
      }
      const created = (await res.json()) as LiveWorktreeRecord;
      if (epoch !== swarmEpoch || !getLiveToken()) return;
      const mapped = mapLiveWorktreeToTask(created, newId);
      cb.setSwarmTasks((prev) =>
        prev.map((t) => (t.id === newId ? mapped : t))
      );
      cb.showQuickFeedback(
        `Materialized isolated worktree ${created.branch} in ${mapped.forkLatencyMs}ms`
      );
    } catch (err) {
      if (epoch !== swarmEpoch || !getLiveToken()) return;
      cb.setSwarmTasks((prev) => prev.filter((t) => t.id !== newId));
      cb.showQuickFeedback(
        `Worktree spawn failed: ${formatErr(err)} — rolled back`
      );
    }
  };

  const resetSwarmEpoch = () => {
    ++swarmEpoch;
    ++latestSwarmReqId;
    inFlightWorktreeIds.clear();
  };

  return {
    hydrateSwarmWorktrees,
    approveLiveWorktree,
    discardLiveWorktree,
    spawnLiveWorktree,
    resetSwarmEpoch,
  };
}
