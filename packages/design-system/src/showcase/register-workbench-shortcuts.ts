import { shortcutManager } from "../keyboard/shortcut-manager";
import { toggleTheme } from "../tokens/theme";
import type { AgentSandboxTask, IssueItem } from "../index";
import type { ShortcutDefinition } from "../keyboard/shortcut-manager";
import type { WorkbenchTab } from "./WorkbenchApp";

export interface WorkbenchShortcutCallbacks {
  isBrowseOnly?: () => boolean;
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
  cb: WorkbenchShortcutCallbacks,
): Array<() => void> {
  const whenViewReady = (action: () => void) => () => {
    if (
      typeof document !== "undefined" &&
      document.querySelector("[data-workspace-unavailable]")
    ) {
      cb.showQuickFeedback(
        "This view is not ready. Wait for it to load, or reload after an error.",
      );
      return;
    }
    action();
  };
  const definitions: ShortcutDefinition[] = [
    {
      id: "cmd.palette",
      title: "Open Command Palette",
      category: "Actions",
      keys: "mod+k",
      description: "Search all commands, agent branches, and actions",
      allowInInput: true,
      allowWhenModalOpen: true,
      handler: () => cb.toggleCommandPalette(),
    },
    {
      id: "nav.swarm",
      title: "Go to worktrees",
      category: "Navigation",
      keys: "g w",
      description: "Review isolated changes and verification results.",
      handler: () => cb.setActiveTab("swarm"),
    },
    {
      id: "nav.issues",
      title: "Go to issues & pull requests",
      category: "Navigation",
      keys: "g i",
      description: "Explore sample issues with keyboard navigation.",
      handler: () => cb.setActiveTab("issues"),
    },
    {
      id: "nav.diff",
      title: "Go to changes",
      category: "Navigation",
      keys: "g d",
      description: "Browse repository commits and diffs.",
      handler: () => cb.setActiveTab("diff"),
    },
    {
      id: "nav.actions",
      title: "Go to Actions",
      category: "Navigation",
      keys: "g a",
      description:
        "Browse automatic Checks; demo mode shows a sample pipeline.",
      handler: () => cb.setActiveTab("actions"),
    },
    {
      id: "nav.admin",
      title: "Go to access & accounts",
      category: "Navigation",
      keys: "g s",
      description: "Manage your account, invitations and Git credentials.",
      handler: () => cb.setActiveTab("admin"),
    },
    {
      id: "nav.tree",
      title: "Go to repository files",
      category: "Navigation",
      keys: "g t",
      description: "Search paths and preview repository files.",
      handler: () => cb.setActiveTab("tree"),
    },
    {
      id: "nav.primitives",
      title: "Go to components",
      category: "Navigation",
      keys: "g p",
      description: "Explore the solid-ui component reference.",
      handler: () => cb.setActiveTab("primitives"),
    },
    {
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
    },
    {
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
    },
    {
      id: "swarm.approve",
      title: "Approve & merge selected worktree",
      category: "Actions",
      keys: "a",
      description:
        "Approve and fast-forward merge locally; GitHub publication is separate",
      handler: whenViewReady(() => {
        if (cb.getActiveTab() === "swarm") {
          const list = cb.getVisibleSwarmTasks();
          const clampedIdx = Math.min(
            cb.getFocusedSwarmIdx(),
            Math.max(0, list.length - 1),
          );
          const task = list[clampedIdx];
          if (task) cb.approveAndMergeSwarmTask(task.id);
        }
      }),
    },
    {
      id: "swarm.discard",
      title: "Discard selected worktree",
      category: "Actions",
      keys: "x",
      description: "Remove the unmerged local branch.",
      handler: whenViewReady(() => {
        if (cb.getActiveTab() === "swarm") {
          const list = cb.getVisibleSwarmTasks();
          const clampedIdx = Math.min(
            cb.getFocusedSwarmIdx(),
            Math.max(0, list.length - 1),
          );
          const task = list[clampedIdx];
          if (task) cb.discardSwarmTask(task.id);
        }
      }),
    },
    {
      id: "swarm.spawn",
      title: "Create worktree",
      category: "Actions",
      keys: "s",
      description:
        "Create an isolated workspace. Demo mode uses sample worktrees.",
      handler: whenViewReady(() => cb.spawnBatchAgentSwarm()),
    },
    {
      id: "issue.toggle",
      title: "Toggle selected demo issue status",
      category: "Issues & PRs",
      keys: "e",
      description: "Toggle a sample issue between open and closed.",
      handler: whenViewReady(() => {
        const list = cb.getVisibleIssues();
        const clampedIdx = Math.min(
          cb.getFocusedIssueIdx(),
          Math.max(0, list.length - 1),
        );
        const item = list[clampedIdx];
        if (item) cb.toggleIssueStatus(item.id);
      }),
    },
    {
      id: "issue.create",
      title: "Create demo issue",
      category: "Issues & PRs",
      keys: "c",
      description: "Add an issue to the sample list.",
      handler: whenViewReady(() => cb.createOptimisticIssue()),
    },
    {
      id: "actions.rerun",
      title: "Replay Actions demo",
      category: "Actions",
      keys: "r",
      description: "Reset sample jobs and logs.",
      handler: whenViewReady(() => cb.rerunAllActionJobs()),
    },
    {
      id: "filter.focus",
      title: "Search issues",
      category: "Actions",
      keys: "/",
      description: "Filter issues and PRs as you type",
      handler: () => {
        cb.setActiveTab("issues");
        cb.focusFilterInput();
      },
    },
    {
      id: "view.theme",
      title: "Toggle light / dark theme",
      category: "View & Theme",
      keys: "shift+t",
      description: "Switch the workspace appearance.",
      handler: () => {
        const next = toggleTheme();
        cb.showQuickFeedback(`Theme switched to ${next.toUpperCase()}`);
      },
    },
    {
      id: "view.help",
      title: "Show keyboard shortcuts",
      category: "View & Theme",
      keys: "?",
      description: "View navigation and action shortcuts.",
      allowWhenModalOpen: true,
      handler: () => cb.toggleShortcutsModal(),
    },
  ];
  const primaryCommands = new Set([
    "nav.swarm",
    "nav.actions",
    "swarm.approve",
    "swarm.discard",
    "swarm.spawn",
    "actions.rerun",
  ]);
  return definitions.map((definition) =>
    shortcutManager.register({
      ...definition,
      enabled: primaryCommands.has(definition.id)
        ? () => !cb.isBrowseOnly?.()
        : undefined,
    }),
  );
}
