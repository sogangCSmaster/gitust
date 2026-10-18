import type {
  ActionJobStep,
  AgentProvider,
  AgentSandboxTask,
  IssueItem,
} from "../index";
import type { WorkbenchTab } from "./WorkbenchApp";

export interface ShowcaseDemoCallbacks {
  getSwarmTasks: () => AgentSandboxTask[];
  setSwarmTasks: (
    updater:
      | AgentSandboxTask[]
      | ((prev: AgentSandboxTask[]) => AgentSandboxTask[])
  ) => void;
  getIssues: () => IssueItem[];
  setIssues: (
    updater: IssueItem[] | ((prev: IssueItem[]) => IssueItem[])
  ) => void;
  setActiveTab: (tab: WorkbenchTab) => void;
  setFocusedIssueIdx: (idx: number) => void;
  setActionJobs: (
    updater: ActionJobStep[] | ((prev: ActionJobStep[]) => ActionJobStep[])
  ) => void;
  showQuickFeedback: (msg: string) => void;
}

/**
 * Pure in-memory showcase demo actions used when no live backend session (`gitust_token`) is active.
 */
export function createShowcaseDemoActions(cb: ShowcaseDemoCallbacks) {
  const approveDemoSwarmTask = (id: number) => {
    cb.setSwarmTasks((prev) =>
      prev.map((t) => {
        if (
          t.id !== id || t.liveId ||
          t.sandboxStatus === "merged" ||
          t.sandboxStatus === "discarded"
        ) {
          return t;
        }
        cb.showQuickFeedback(
          `Demo worktree marked merged: ${t.branch}.`
        );
        return {
          ...t,
          sandboxStatus: "merged",
          githubSynced: false,
          updatedAt: "just now",
        };
      })
    );
  };

  const discardDemoSwarmTask = (id: number) => {
    cb.setSwarmTasks((prev) =>
      prev.map((t) => {
        if (
          t.id !== id || t.liveId ||
          t.sandboxStatus === "merged" ||
          t.sandboxStatus === "discarded"
        ) {
          return t;
        }
        cb.showQuickFeedback(
          `Demo worktree marked discarded: ${t.branch}.`
        );
        return { ...t, sandboxStatus: "discarded", updatedAt: "just now" };
      })
    );
  };

  const spawnDemoBatchSwarm = () => {
    const providers: AgentProvider[] = [
      "Claude Code",
      "Cursor Agent",
      "Codex Swarm",
    ];
    const current = cb.getSwarmTasks();
    const baseId =
      current.reduce((maxId, t) => (t.id > maxId ? t.id : maxId), 0) + 1;
    const batch: AgentSandboxTask[] = Array.from({ length: 10 }, (_, idx) => {
      const id = baseId + idx;
      const agent = providers[idx % providers.length]!;
      return {
        id,
        agent,
        branch: `agent/${agent.toLowerCase().split(" ")[0]}/parallel-task-${id}`,
        baseBranch: "main",
        forkLatencyMs: Number((0.25 + (idx % 4) * 0.04).toFixed(2)),
        promptSummary: `Sample agent task #${id}: update code and review example results`,
        sandboxStatus: "verified",
        compileTimeMs: 68 + ((idx * 7) % 35),
        testsPassed: 24,
        testsTotal: 24,
        filesChanged: 2 + (idx % 3),
        additions: 48 + idx * 9,
        deletions: 12 + idx * 2,
        updatedAt: "just now",
      };
    });

    cb.setSwarmTasks((prev) => [...batch, ...prev]);
    cb.showQuickFeedback(
      "Added 10 demo worktrees."
    );
  };

  const toggleIssueStatus = (id: number) => {
    cb.setIssues((prev) =>
      prev.map((item) => {
        if (item.id !== id) return item;
        const nextStatus = item.status === "open" ? "closed" : "open";
        cb.showQuickFeedback(
          `Demo issue #${item.id} marked ${nextStatus}.`
        );
        return { ...item, status: nextStatus, updatedAt: "just now" };
      })
    );
  };

  const createOptimisticIssue = () => {
    const current = cb.getIssues();
    const newId =
      current.reduce((maxId, i) => (i.id > maxId ? i.id : maxId), 0) + 1;
    const newItem: IssueItem = {
      id: newId,
      kind: "issue",
      title: `feat: instant optimistic issue #${newId} created via 'c' shortcut`,
      status: "open",
      author: "yuseungjae",
      labels: [{ name: "optimistic-ui", variant: "accent" }],
      commentsCount: 0,
      updatedAt: "just now",
    };
    cb.setIssues((prev) => [newItem, ...prev]);
    cb.setActiveTab("issues");
    cb.setFocusedIssueIdx(0);
    cb.showQuickFeedback(`Added demo issue #${newId}.`);
  };

  const rerunAllActionJobs = () => {
    cb.setActionJobs((prev) =>
      prev.map((j) => ({ ...j, status: "success", duration: "12s (cached)" }))
    );
    cb.showQuickFeedback(
      "Demo pipeline reset. No server workflow was started."
    );
  };

  return {
    approveDemoSwarmTask,
    discardDemoSwarmTask,
    spawnDemoBatchSwarm,
    toggleIssueStatus,
    createOptimisticIssue,
    rerunAllActionJobs,
  };
}
