import {
  AgentSandboxTask,
  IssueItem,
  shortcutManager,
  toggleTheme,
} from "../index";
import type { WorkbenchTab } from "./WorkbenchApp";

export interface WorkbenchShortcutCallbacks {
  getActiveTab: () => WorkbenchTab;
  setActiveTab: (tab: WorkbenchTab) => void;
  toggleCommandPalette: () => void;
  toggleShortcutsModal: () => void;
  getVisibleSwarmTasks: () => AgentSandboxTask[];
  getFocusedSwarmIdx: () => number;
  setFocusedSwarmIdx: (updater: (i: number) => number) => void;
  getVisibleIssues: () => IssueItem[];
  getFocusedIssueIdx: () => number;
  setFocusedIssueIdx: (updater: (i: number) => number) => void;
  approveAndMergeSwarmTask: (id: number) => void;
  discardSwarmTask: (id: number) => void;
  spawnBatchAgentSwarm: () => void;
  toggleIssueStatus: (id: number) => void;
  createOptimisticIssue: () => void;
  rerunAllActionJobs: () => void;
  focusFilterInput: () => void;
  showQuickFeedback: (msg: string) => void;
}

export function registerWorkbenchShortcuts(
  cb: WorkbenchShortcutCallbacks
): Array<() => void> {
  return [
    shortcutManager.register({
      id: "cmd.palette",
      title: "Open Command Palette",
      category: "Actions",
      keys: "mod+k",
      description: "Search all commands, agent branches, and actions",
      allowInInput: true,
      allowWhenModalOpen: true,
      handler: () => cb.toggleCommandPalette(),
    }),
    shortcutManager.register({
      id: "nav.swarm",
      title: "Go to Agent Swarm & Sandbox Fleet",
      category: "Navigation",
      keys: "g w",
      description:
        "Orchestrate Claude Code / Cursor parallel branches & sandboxes",
      handler: () => cb.setActiveTab("swarm"),
    }),
    shortcutManager.register({
      id: "nav.issues",
      title: "Go to Issues & Pull Requests",
      category: "Navigation",
      keys: "g i",
      description: "Superhuman list view with j/k navigation",
      handler: () => cb.setActiveTab("issues"),
    }),
    shortcutManager.register({
      id: "nav.diff",
      title: "Go to 10,000-Line Virtualized Diff Viewer",
      category: "Navigation",
      keys: "g d",
      description: "120fps O(1) DOM sliced PR diff engine",
      handler: () => cb.setActiveTab("diff"),
    }),
    shortcutManager.register({
      id: "nav.actions",
      title: "Go to Gitust Actions CI/CD & Live Log Stream",
      category: "Navigation",
      keys: "g a",
      description: "GitHub/Gitea Actions workflow pipeline & 10k-line console",
      handler: () => cb.setActiveTab("actions"),
    }),
    shortcutManager.register({
      id: "nav.admin",
      title: "Go to Enterprise Auth, SSO & Site Admin Console",
      category: "Navigation",
      keys: "g s",
      description: "Self-hosted on-premise IAM, domain lock, and user roles",
      handler: () => cb.setActiveTab("admin"),
    }),
    shortcutManager.register({
      id: "nav.tree",
      title: "Go to 5,000-File Monorepo Tree",
      category: "Navigation",
      keys: "g t",
      description: "Instant file finder and virtualized tree",
      handler: () => cb.setActiveTab("tree"),
    }),
    shortcutManager.register({
      id: "nav.primitives",
      title: "Go to Design Tokens & UI Primitives",
      category: "Navigation",
      keys: "g p",
      description: "Inspect tokens, buttons, badges, icons, and typography",
      handler: () => cb.setActiveTab("primitives"),
    }),
    shortcutManager.register({
      id: "list.down",
      title: "Move Focus Down",
      category: "Issues & PRs",
      keys: "j",
      description: "Select next row in active list",
      allowRepeat: true,
      handler: () => {
        if (cb.getActiveTab() === "swarm") {
          const maxIdx = Math.max(0, cb.getVisibleSwarmTasks().length - 1);
          cb.setFocusedSwarmIdx((i) => Math.min(maxIdx, i + 1));
        } else if (cb.getActiveTab() === "issues") {
          const maxIdx = Math.max(0, cb.getVisibleIssues().length - 1);
          cb.setFocusedIssueIdx((i) => Math.min(maxIdx, i + 1));
        }
      },
    }),
    shortcutManager.register({
      id: "list.up",
      title: "Move Focus Up",
      category: "Issues & PRs",
      keys: "k",
      description: "Select previous row in active list",
      allowRepeat: true,
      handler: () => {
        if (cb.getActiveTab() === "swarm") {
          const maxIdx = Math.max(0, cb.getVisibleSwarmTasks().length - 1);
          cb.setFocusedSwarmIdx((i) => Math.min(maxIdx, Math.max(0, i - 1)));
        } else if (cb.getActiveTab() === "issues") {
          const maxIdx = Math.max(0, cb.getVisibleIssues().length - 1);
          cb.setFocusedIssueIdx((i) => Math.min(maxIdx, Math.max(0, i - 1)));
        }
      },
    }),
    shortcutManager.register({
      id: "swarm.approve",
      title: "Approve & Fast-Forward Merge Focused Agent Branch",
      category: "Actions",
      keys: "a",
      description: "Approve and fast-forward merge locally; GitHub publication is separate",
      handler: () => {
        if (cb.getActiveTab() === "swarm") {
          const list = cb.getVisibleSwarmTasks();
          const clampedIdx = Math.min(
            cb.getFocusedSwarmIdx(),
            Math.max(0, list.length - 1)
          );
          const task = list[clampedIdx];
          if (task) cb.approveAndMergeSwarmTask(task.id);
        }
      },
    }),
    shortcutManager.register({
      id: "swarm.discard",
      title: "Discard Focused Agent Branch & Sandbox",
      category: "Actions",
      keys: "x",
      description: "Drop CoW worktree snapshot immediately",
      handler: () => {
        if (cb.getActiveTab() === "swarm") {
          const list = cb.getVisibleSwarmTasks();
          const clampedIdx = Math.min(
            cb.getFocusedSwarmIdx(),
            Math.max(0, list.length - 1)
          );
          const task = list[clampedIdx];
          if (task) cb.discardSwarmTask(task.id);
        }
      },
    }),
    shortcutManager.register({
      id: "swarm.spawn",
      title: "Spawn +10 Parallel Agent Sandbox Branches",
      category: "Actions",
      keys: "s",
      description: "Benchmark <1ms CoW worktree fork for coding agents",
      handler: () => cb.spawnBatchAgentSwarm(),
    }),
    shortcutManager.register({
      id: "issue.toggle",
      title: "Toggle Focused Issue Open/Closed (Optimistic)",
      category: "Issues & PRs",
      keys: "e",
      description: "Instant 0ms state mutation without spinner",
      handler: () => {
        const list = cb.getVisibleIssues();
        const clampedIdx = Math.min(
          cb.getFocusedIssueIdx(),
          Math.max(0, list.length - 1)
        );
        const item = list[clampedIdx];
        if (item) cb.toggleIssueStatus(item.id);
      },
    }),
    shortcutManager.register({
      id: "issue.create",
      title: "Create Instant Issue",
      category: "Issues & PRs",
      keys: "c",
      description: "Insert a new issue with 0ms optimistic feedback",
      handler: () => cb.createOptimisticIssue(),
    }),
    shortcutManager.register({
      id: "actions.rerun",
      title: "Re-run CI/CD Workflow Jobs",
      category: "Actions",
      keys: "r",
      description: "Trigger instant re-run on self-hosted runner",
      handler: () => cb.rerunAllActionJobs(),
    }),
    shortcutManager.register({
      id: "filter.focus",
      title: "Focus Instant Filter Bar",
      category: "Actions",
      keys: "/",
      description: "Filter issues and PRs as you type",
      handler: () => {
        cb.setActiveTab("issues");
        cb.focusFilterInput();
      },
    }),
    shortcutManager.register({
      id: "view.theme",
      title: "Toggle Dark / Light High-Contrast Theme",
      category: "View & Theme",
      keys: "shift+t",
      description: "Zero-runtime CSS Custom Property theme switch",
      handler: () => {
        const next = toggleTheme();
        cb.showQuickFeedback(`Theme switched to ${next.toUpperCase()} (0ms)`);
      },
    }),
    shortcutManager.register({
      id: "view.help",
      title: "Show Keyboard Shortcuts Reference",
      category: "View & Theme",
      keys: "?",
      description: "Inspect all registered single & chord shortcuts",
      allowWhenModalOpen: true,
      handler: () => cb.toggleShortcutsModal(),
    }),
  ];
}
