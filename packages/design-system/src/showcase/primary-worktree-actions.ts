import type { AgentSandboxTask } from "../patterns/AgentSwarmView";
import { getLiveToken } from "./live-repo-client";

/** Every entry point keeps the primary workbench boundary, including stale UI callbacks. */
export const createPrimaryWorktreeActions = (options: {
  canWorkbench: () => boolean;
  activeTab: () => string;
  liveMode: () => boolean;
  sessionReady: () => boolean;
  feedback: (message: string) => void;
  liveApprove: (id: number) => unknown;
  liveDiscard: (id: number) => unknown;
  livePublish: (id: number) => unknown;
  demoApprove: (id: number) => void;
  demoDiscard: (id: number) => void;
  demoSpawn: () => void;
  openNewWorktree: () => void;
  tasks: () => AgentSandboxTask[];
  openChanges: () => void;
  loadWorktreeDiff: (task: AgentSandboxTask) => unknown;
}) => {
  const available = () =>
    options.canWorkbench() && options.activeTab() === "swarm";
  const live = () => options.liveMode() || Boolean(getLiveToken());
  return {
    approveAndMergeSwarmTask: (id: number) => {
      if (available())
        void (live() ? options.liveApprove(id) : options.demoApprove(id));
    },
    publishSwarmTask: (id: number) => {
      if (available() && live()) void options.livePublish(id);
    },
    discardSwarmTask: (id: number) => {
      if (available())
        void (live() ? options.liveDiscard(id) : options.demoDiscard(id));
    },
    spawnBatchAgentSwarm: () => {
      if (!available()) return;
      if (!live()) {
        options.demoSpawn();
        return;
      }
      if (!options.sessionReady()) {
        options.feedback(
          "Your account is still loading. Try again when it is ready.",
        );
        return;
      }
      options.openNewWorktree();
    },
    reviewSwarmTask: (id: number) => {
      if (!available() || !options.liveMode() || !getLiveToken()) return;
      const task = options.tasks().find((task) => task.id === id);
      if (!task?.liveId) return;
      options.openChanges();
      void options.loadWorktreeDiff(task);
    },
  };
};
