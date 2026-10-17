// @vitest-environment jsdom
import { lazy } from "solid-js";
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { DeferredView } from "../src/components/workspace/deferred-view";
import { shortcutManager } from "../src/keyboard/shortcut-manager";
import {
  registerWorkbenchShortcuts,
  type WorkbenchShortcutCallbacks,
} from "../src/showcase/register-workbench-shortcuts";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const callbacks = () => ({
  getActiveTab: () => "swarm",
  setActiveTab: vi.fn(),
  toggleCommandPalette: vi.fn(),
  toggleShortcutsModal: vi.fn(),
  getVisibleSwarmTasks: () => [{ id: 17 }],
  getFocusedSwarmIdx: () => 0,
  setFocusedSwarmIdx: vi.fn(),
  getVisibleIssues: () => [],
  getFocusedIssueIdx: () => 0,
  setFocusedIssueIdx: vi.fn(),
  approveAndMergeSwarmTask: vi.fn(),
  discardSwarmTask: vi.fn(),
  spawnBatchAgentSwarm: vi.fn(),
  toggleIssueStatus: vi.fn(),
  createOptimisticIssue: vi.fn(),
  rerunAllActionJobs: vi.fn(),
  focusFilterInput: vi.fn(),
  showQuickFeedback: vi.fn(),
});

it.each(["resolve", "reject"] as const)(
  "blocks hidden worktree mutations while a module loads and on %s",
  async (outcome) => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    let resolve!: (value: { default: () => string }) => void;
    let reject!: (error: Error) => void;
    const Feature = lazy(
      () =>
        new Promise<{ default: () => string }>((yes, no) => {
          resolve = yes;
          reject = no;
        }),
    );
    const cb = callbacks();
    const unregister = registerWorkbenchShortcuts(
      cb as unknown as WorkbenchShortcutCallbacks,
    );
    try {
      render(() => (
        <DeferredView label="Worktrees">
          <Feature />
        </DeferredView>
      ));
      expect(screen.getByRole("status").textContent).toContain(
        "Opening Worktrees",
      );
      await fireEvent.keyDown(document.body, { key: "a" });
      shortcutManager.executeById("swarm.discard");
      expect(cb.approveAndMergeSwarmTask).not.toHaveBeenCalled();
      expect(cb.discardSwarmTask).not.toHaveBeenCalled();
      shortcutManager.executeById("nav.issues");
      expect(cb.setActiveTab).toHaveBeenCalledWith("issues");
      if (outcome === "resolve") {
        resolve({ default: () => "Worktree loaded" });
        await screen.findByText("Worktree loaded");
      } else {
        reject(new Error("missing worktree chunk"));
        await screen.findByRole("alert");
      }
      shortcutManager.executeById("swarm.approve");
      await fireEvent.keyDown(document.body, { key: "x" });
      const expected = outcome === "resolve" ? 1 : 0;
      expect(cb.approveAndMergeSwarmTask).toHaveBeenCalledTimes(expected);
      expect(cb.discardSwarmTask).toHaveBeenCalledTimes(expected);
    } finally {
      unregister.forEach((remove) => remove());
    }
  },
);
