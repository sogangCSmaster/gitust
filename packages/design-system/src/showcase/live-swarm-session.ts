import type {
  AgentProvider,
  AgentSandboxTask,
} from "../patterns/AgentSwarmView";
import { formatErr, getLiveToken } from "./live-repo-context";
import {
  validateNewWorktreeInput,
  type NewWorktreeInput,
  type NewWorktreeResult,
} from "./new-worktree-input";

export interface LiveWorktreeRecord {
  id: string;
  repo_id: string;
  agent: string;
  branch: string;
  base_branch: string;
  base_commit: string;
  verified_commit?: string | null;
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
  getRepositoryId?: () => string | null | undefined;
  getSwarmTasks: () => AgentSandboxTask[];
  setSwarmTasks: (
    updater:
      | AgentSandboxTask[]
      | ((prev: AgentSandboxTask[]) => AgentSandboxTask[]),
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

const mapSandboxStatus = (raw: string): AgentSandboxTask["sandboxStatus"] => {
  switch (raw.toLowerCase()) {
    case "verified":
      return "verified";
    case "failed":
      return "failed";
    case "merged":
      return "merged";
    case "discarded":
      return "discarded";
    case "compiling":
      return "compiling";
    case "pending":
    default:
      return "pending";
  }
};

export const mapLiveWorktreeToTask = (
  wt: LiveWorktreeRecord,
  numericId: number,
): AgentSandboxTask => ({
  id: numericId,
  liveId: wt.id,
  agent: mapAgentProvider(wt.agent),
  branch: wt.branch,
  baseBranch: wt.base_branch ?? "",
  forkLatencyMs: Number(((wt.materialize_micros ?? 0) / 1000).toFixed(2)),
  promptSummary: wt.prompt_summary,
  sandboxStatus:
    wt.status.toLowerCase() === "verified" && !wt.verified_commit
      ? "pending"
      : mapSandboxStatus(wt.status),
  verifiedCommit: wt.verified_commit || undefined,
  verificationError:
    wt.status.toLowerCase() === "verified" && !wt.verified_commit
      ? "Server did not provide a verified commit. Run verification again."
      : undefined,
  compileTimeMs: Number(wt.last_sandbox_duration_ms ?? 0),
  testsPassed: Number(wt.tests_passed ?? 0),
  testsTotal: Number(wt.tests_total ?? 0),
  filesChanged: Number(wt.files_changed ?? 0),
  additions: Number(wt.additions ?? 0),
  deletions: Number(wt.deletions ?? 0),
  githubSynced: Boolean(wt.github_synced),
  updatedAt: wt.created_at || "live",
});

/** Live worktree hydration, creation, and approval/discard/publish state machine. */
export function createLiveSwarmSession(cb: LiveSwarmClientCallbacks) {
  let latestSwarmReqId = 0;
  let swarmEpoch = 0;
  const inFlightWorktrees = new Map<string, symbol>();
  const spawningIds = new Set<number>();
  const spawningBranches = new Set<string>();
  const isCurrent = (epoch: number, token: string) =>
    epoch === swarmEpoch && token === getLiveToken();
  const nextId = () =>
    cb.getSwarmTasks().reduce((max, task) => Math.max(max, task.id), 0) + 1;

  const hydrateSwarmWorktrees = async () => {
    const token = getLiveToken();
    if (!token) return;
    const epoch = swarmEpoch;
    const reqId = ++latestSwarmReqId;
    const selectedRepoId = cb.getRepositoryId?.();
    const current = () =>
      isCurrent(epoch, token) &&
      reqId === latestSwarmReqId &&
      cb.getRepositoryId?.() === selectedRepoId;
    try {
      const res = await fetch("/api/v1/swarm/worktrees", {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (!current()) return;
      if (!res.ok) {
        throw new Error(`HTTP ${res.status}: ${(await res.text()).trim()}`);
      }
      const data = (await res.json()) as LiveWorktreeRecord[];
      if (!current()) return;
      if (!Array.isArray(data))
        throw new Error("Invalid worktree list response");
      const scoped = selectedRepoId
        ? data.filter(
            (wt) =>
              !wt.repo_id ||
              wt.repo_id === "repo" ||
              wt.repo_id === selectedRepoId,
          )
        : data;
      let newId = nextId();
      cb.setSwarmTasks((previous) => {
        const byLiveId = new Map(
          previous
            .filter((task) => task.liveId)
            .map((task) => [task.liveId, task]),
        );
        const incomingIds = new Set(scoped.map((wt) => wt.id));
        const tasks = scoped.map((wt) => {
          const old = byLiveId.get(wt.id);
          if (old && inFlightWorktrees.has(wt.id)) return old;
          const mapped = mapLiveWorktreeToTask(wt, old?.id ?? newId++);
          if (!mapped.githubSynced)
            mapped.publicationError = old?.publicationError;
          const resultCommit =
            old?.verificationResult?.checkedCommit ?? old?.verifiedCommit;
          const sameVerification = mapped.verifiedCommit
            ? mapped.verifiedCommit === resultCommit
            : !old?.verifiedCommit;
          mapped.verificationResult = sameVerification
            ? old?.verificationResult
            : undefined;
          return mapped;
        });
        return [
          ...tasks,
          ...previous.filter(
            (task) =>
              spawningIds.has(task.id) ||
              (task.liveId &&
                !incomingIds.has(task.liveId) &&
                inFlightWorktrees.has(task.liveId)),
          ),
        ];
      });
    } catch (err) {
      if (current())
        cb.showQuickFeedback(`Agent Swarm hydration failed: ${formatErr(err)}`);
    }
  };

  type WorktreeAction = "approve" | "discard" | "publish";
  const mutateWorktree = async (id: number, action: WorktreeAction) => {
    const token = getLiveToken();
    if (!token) return;
    const target = cb.getSwarmTasks().find((task) => task.id === id);
    if (!target?.liveId) {
      cb.showQuickFeedback(
        `Cannot ${action} worktree: no live worktree ID on server`,
      );
      return;
    }
    const liveId = target.liveId;
    if (inFlightWorktrees.has(liveId)) {
      cb.showQuickFeedback("Worktree operation already in progress...");
      return;
    }
    if (
      action === "approve" &&
      (target.sandboxStatus !== "verified" ||
        !target.verifiedCommit ||
        target.verificationNeedsRefresh)
    ) {
      cb.showQuickFeedback(
        "Run verification and confirm the worktree state before approval.",
      );
      return;
    }
    if (action === "publish") {
      if (target.sandboxStatus !== "merged" || target.githubSynced) return;
    } else if (
      target.sandboxStatus === "merged" ||
      target.sandboxStatus === "discarded"
    ) {
      return;
    }
    const epoch = swarmEpoch;
    const operationId = Symbol(action);
    inFlightWorktrees.set(liveId, operationId);
    ++latestSwarmReqId;
    const previous: AgentSandboxTask = { ...target };
    const operation =
      action === "publish"
        ? "publishing"
        : action === "approve"
          ? "approving"
          : "discarding";
    const updateTarget = (
      update: (task: AgentSandboxTask) => AgentSandboxTask,
    ) => {
      cb.setSwarmTasks((tasks) =>
        tasks.map((task) => (task.liveId === liveId ? update(task) : task)),
      );
    };
    updateTarget((task) => ({
      ...task,
      operation,
      publicationError: undefined,
      sandboxStatus:
        action === "approve"
          ? "merged"
          : action === "discard"
            ? "discarded"
            : task.sandboxStatus,
      githubSynced: action === "approve" ? false : task.githubSynced,
      updatedAt: "just now",
    }));
    cb.showQuickFeedback(
      action === "publish"
        ? `Publishing approved ${target.branch} to the configured upstream...`
        : action === "approve"
          ? `Approving & fast-forward merging ${target.branch} into ${target.baseBranch}...`
          : `Discarding worktree ${target.branch} on server...`,
    );

    try {
      const endpoint = `/api/v1/swarm/worktrees/${encodeURIComponent(liveId)}`;
      const res = await fetch(
        action === "discard" ? endpoint : `${endpoint}/${action}`,
        {
          method: action === "discard" ? "DELETE" : "POST",
          headers: {
            Authorization: `Bearer ${token}`,
            ...(action === "approve"
              ? { "Content-Type": "application/json" }
              : {}),
          },
          ...(action === "approve"
            ? {
                body: JSON.stringify({
                  expected_commit: target.verifiedCommit,
                }),
              }
            : {}),
        },
      );
      if (!isCurrent(epoch, token)) return;
      if (!res.ok)
        throw new Error(`HTTP ${res.status}: ${(await res.text()).trim()}`);
      if (action === "discard") {
        cb.setSwarmTasks((tasks) =>
          tasks.filter((task) => task.liveId !== liveId),
        );
        cb.showQuickFeedback(`Discarded worktree ${target.branch}`);
        return;
      }
      const result = (await res.json()) as LiveWorktreeRecord;
      if (!isCurrent(epoch, token)) return;
      if (
        result.id !== liveId ||
        result.status.toLowerCase() !== "merged" ||
        (action === "approve" &&
          result.base_commit !== target.verifiedCommit) ||
        (action === "publish" && result.github_synced !== true)
      ) {
        throw new Error(
          "Server did not confirm the requested worktree operation",
        );
      }
      updateTarget((task) => ({
        ...mapLiveWorktreeToTask(result, task.id),
        verificationResult: task.verificationResult,
      }));
      cb.showQuickFeedback(
        action === "publish"
          ? `Published approved ${result.branch} to upstream ${result.base_branch}`
          : `Fast-forward merged ${result.branch} into ${result.base_branch} on-premise`,
      );
      cb.onRepoMutated?.();
    } catch (err) {
      if (!isCurrent(epoch, token)) return;
      const error = formatErr(err);
      updateTarget((task) => ({
        ...previous,
        id: task.id,
        operation: undefined,
        publicationError:
          action === "publish" ? error : previous.publicationError,
        ...(action === "approve"
          ? {
              sandboxStatus: "pending",
              verifiedCommit: undefined,
              verificationNeedsRefresh: true,
              verificationError: `${error}. Refresh worktrees to confirm the server state before retry.`,
            }
          : {}),
      }));
      cb.showQuickFeedback(
        action === "publish"
          ? `Publication failed for ${target.branch}: ${error}. Local merge preserved; the upstream may already have received the commit. Retry to confirm.`
          : `${action === "approve" ? "Merge" : "Discard"} failed for ${target.branch}: ${error} — UI restored; check repository state before retry`,
      );
    } finally {
      if (inFlightWorktrees.get(liveId) === operationId)
        inFlightWorktrees.delete(liveId);
      if (isCurrent(epoch, token)) ++latestSwarmReqId;
    }
  };

  const spawnLiveWorktree = async (
    input: NewWorktreeInput,
  ): Promise<NewWorktreeResult> => {
    const targetRepoId = input.repoId || cb.getRepositoryId?.() || "";
    const validation = validateNewWorktreeInput({
      ...input,
      repoId: targetRepoId,
    });
    if (validation) return { kind: "error", error: validation };
    const token = getLiveToken();
    if (!token)
      return { kind: "error", error: "Sign in before creating a worktree." };
    if (spawningBranches.has(input.branch))
      return {
        kind: "error",
        error: "This branch is already being created.",
      };
    const epoch = swarmEpoch;
    ++latestSwarmReqId;
    const newId = nextId();
    spawningIds.add(newId);
    spawningBranches.add(input.branch);
    const optimisticTask: AgentSandboxTask = {
      id: newId,
      agent: mapAgentProvider(input.agent),
      branch: input.branch,
      baseBranch: input.baseBranch,
      forkLatencyMs: 0,
      promptSummary: input.promptSummary,
      sandboxStatus: "pending",
      operation: "creating",
      compileTimeMs: 0,
      testsPassed: 0,
      testsTotal: 0,
      filesChanged: 0,
      additions: 0,
      deletions: 0,
      githubSynced: false,
      updatedAt: "creating...",
    };
    cb.setSwarmTasks((tasks) => [optimisticTask, ...tasks]);
    let uncertain = true;
    try {
      const res = await fetch("/api/v1/swarm/worktrees", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          repo_id: targetRepoId,
          agent: input.agent,
          branch: input.branch,
          base_branch: input.baseBranch,
          prompt_summary: input.promptSummary,
        }),
      });
      if (!isCurrent(epoch, token)) return { kind: "stale" };
      if (!res.ok) {
        uncertain = res.status >= 500;
        const message = (await res.text()).trim();
        if (!isCurrent(epoch, token)) return { kind: "stale" };
        throw new Error(`HTTP ${res.status}: ${message}`);
      }
      const created = (await res.json()) as LiveWorktreeRecord;
      if (!isCurrent(epoch, token)) return { kind: "stale" };
      if (
        !created ||
        typeof created.id !== "string" ||
        !/^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i.test(created.id) ||
        created.repo_id !== targetRepoId ||
        created.branch !== input.branch ||
        created.base_branch !== input.baseBranch ||
        created.agent !== input.agent ||
        created.prompt_summary !== input.promptSummary ||
        created.status !== "pending" ||
        !/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i.test(created.base_commit)
      )
        throw new Error(
          "The server returned a different worktree. Refresh worktrees before retrying.",
        );
      const mapped = mapLiveWorktreeToTask(created, newId);
      cb.setSwarmTasks((tasks) =>
        tasks
          .filter((task) => task.liveId !== created.id)
          .map((task) => (task.id === newId ? mapped : task)),
      );
      cb.showQuickFeedback(`Created worktree ${created.branch}`);
      return { kind: "created", taskId: newId, worktreeId: created.id };
    } catch (err) {
      if (!isCurrent(epoch, token)) return { kind: "stale" };
      cb.setSwarmTasks((tasks) => tasks.filter((task) => task.id !== newId));
      return { kind: "error", error: formatErr(err), uncertain };
    } finally {
      if (isCurrent(epoch, token)) {
        spawningIds.delete(newId);
        spawningBranches.delete(input.branch);
        ++latestSwarmReqId;
      }
    }
  };

  const resetSwarmEpoch = () => {
    ++swarmEpoch;
    ++latestSwarmReqId;
    inFlightWorktrees.clear();
    spawningIds.clear();
    spawningBranches.clear();
  };

  return {
    cb,
    inFlightWorktrees,
    getEpoch: () => swarmEpoch,
    bumpReqId: () => ++latestSwarmReqId,
    isCurrent,
    hydrateSwarmWorktrees,
    approveLiveWorktree: (id: number) => mutateWorktree(id, "approve"),
    discardLiveWorktree: (id: number) => mutateWorktree(id, "discard"),
    publishLiveWorktree: (id: number) => mutateWorktree(id, "publish"),
    spawnLiveWorktree,
    resetSwarmEpoch,
  };
}

export type LiveSwarmSessionController = ReturnType<typeof createLiveSwarmSession>;
