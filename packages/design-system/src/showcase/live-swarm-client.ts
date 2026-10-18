import type { AgentSandboxTask } from "../patterns/AgentSwarmView";
import { formatErr, getLiveToken } from "./live-repo-context";
import { parseVerificationResult } from "./live-verification-result";
import {
  createLiveSwarmSession,
  mapLiveWorktreeToTask,
  type LiveSwarmClientCallbacks,
  type LiveSwarmSessionController,
  type LiveWorktreeRecord,
} from "./live-swarm-session";

export {
  createLiveSwarmSession,
  mapLiveWorktreeToTask,
  type LiveSwarmClientCallbacks,
  type LiveSwarmSessionController,
  type LiveWorktreeRecord,
};

export function extendLiveSwarmClient(session: LiveSwarmSessionController) {
  const { cb, inFlightWorktrees, getEpoch, bumpReqId, isCurrent } = session;

  const verifyLiveWorktree = async (id: number, command: string) => {
    const token = getLiveToken();
    if (!token) return;
    if (!command.trim() || new TextEncoder().encode(command).length > 65_536) {
      cb.showQuickFeedback(
        "Enter a verification command of at most 65,536 bytes.",
      );
      return;
    }
    const target = cb.getSwarmTasks().find((task) => task.id === id);
    if (
      !target?.liveId ||
      target.sandboxStatus === "merged" ||
      target.sandboxStatus === "discarded"
    )
      return;
    const liveId = target.liveId;
    if (
      inFlightWorktrees.has(liveId) ||
      target.sandboxStatus === "compiling" ||
      target.verificationNeedsRefresh
    ) {
      cb.showQuickFeedback(
        "Worktree is busy or its state needs a refresh before verification.",
      );
      return;
    }
    const epoch = getEpoch();
    const operationId = Symbol("verify");
    inFlightWorktrees.set(liveId, operationId);
    bumpReqId();
    const updateTarget = (
      update: (task: AgentSandboxTask) => AgentSandboxTask,
    ) => {
      cb.setSwarmTasks((tasks) =>
        tasks.map((task) => (task.liveId === liveId ? update(task) : task)),
      );
    };
    updateTarget((task) => ({
      ...task,
      sandboxStatus: "compiling",
      operation: "verifying",
      verifiedCommit: undefined,
      verificationError: undefined,
      verificationResult: undefined,
      verificationNeedsRefresh: undefined,
    }));
    cb.showQuickFeedback(`Running verification for ${target.branch}...`);
    try {
      const res = await fetch(
        `/api/v1/swarm/worktrees/${encodeURIComponent(liveId)}/sandbox`,
        {
          method: "POST",
          headers: {
            Authorization: `Bearer ${token}`,
            "Content-Type": "application/json",
          },
          body: JSON.stringify({ command, verify: true }),
        },
      );
      if (!isCurrent(epoch, token)) return;
      if (!res.ok)
        throw new Error(`HTTP ${res.status}: ${(await res.text()).trim()}`);
      const rawResult: unknown = await res.json();
      if (!isCurrent(epoch, token)) return;
      const result = parseVerificationResult(rawResult, liveId, command);
      updateTarget((task) => ({ ...task, verificationResult: result }));
      const statusRes = await fetch("/api/v1/swarm/worktrees", {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (!isCurrent(epoch, token)) return;
      if (!statusRes.ok)
        throw new Error(`Cannot confirm server state (HTTP ${statusRes.status})`);
      const records = (await statusRes.json()) as LiveWorktreeRecord[];
      if (!isCurrent(epoch, token)) return;
      if (!Array.isArray(records))
        throw new Error("Invalid worktree list response");
      const authoritative = records.find((record) => record.id === liveId);
      if (!authoritative)
        throw new Error("Worktree missing from server state; refresh worktrees");
      const mapped = mapLiveWorktreeToTask(authoritative, id);
      if (mapped.sandboxStatus === "verified" && result.exitCode !== 0) {
        throw new Error(
          "Failed command conflicts with server verification state; refresh worktrees",
        );
      }
      if (!result.checkedCommit) {
        throw new Error(
          "Server did not identify the checked commit; refresh worktrees before approval",
        );
      }
      if (
        result.exitCode === 0 &&
        mapped.verifiedCommit !== result.checkedCommit
      ) {
        throw new Error(
          "Server verification changed since this command completed; refresh worktrees before approval",
        );
      }
      const verified =
        mapped.sandboxStatus === "verified" && result.exitCode === 0;
      updateTarget((task) => ({
        ...mapped,
        id: task.id,
        verificationResult: task.verificationResult,
        verificationError: verified
          ? undefined
          : mapped.verificationError ||
            `Command exited ${result.exitCode}; server worktree status is ${mapped.sandboxStatus}.`,
      }));
      cb.showQuickFeedback(
        verified
          ? `Verified ${target.branch} at ${mapped.verifiedCommit?.slice(0, 12)}`
          : `Verification finished; server worktree status is ${mapped.sandboxStatus}.`,
      );
    } catch (err) {
      if (!isCurrent(epoch, token)) return;
      const error = formatErr(err);
      updateTarget((task) => ({
        ...task,
        sandboxStatus: "pending",
        operation: undefined,
        verifiedCommit: undefined,
        verificationNeedsRefresh: true,
        verificationError: `${error}. Refresh worktrees to confirm the server state.`,
      }));
      cb.showQuickFeedback(`Verification could not be confirmed: ${error}`);
    } finally {
      if (inFlightWorktrees.get(liveId) === operationId)
        inFlightWorktrees.delete(liveId);
      if (isCurrent(epoch, token)) bumpReqId();
    }
  };

  return {
    hydrateSwarmWorktrees: session.hydrateSwarmWorktrees,
    verifyLiveWorktree,
    approveLiveWorktree: session.approveLiveWorktree,
    discardLiveWorktree: session.discardLiveWorktree,
    publishLiveWorktree: session.publishLiveWorktree,
    spawnLiveWorktree: session.spawnLiveWorktree,
    resetSwarmEpoch: session.resetSwarmEpoch,
  };
}

export function createLiveSwarmClient(cb: LiveSwarmClientCallbacks) {
  return extendLiveSwarmClient(createLiveSwarmSession(cb));
}
