// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import {
  AgentSwarmView,
  type AgentSandboxTask,
  type AgentSwarmViewProps,
} from "../src/patterns/AgentSwarmView";
import { EnterpriseAdminView } from "../src/patterns/EnterpriseAdminView";
import { VirtualFileTree } from "../src/patterns/VirtualFileTree";

beforeEach(() => {
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});
const task: AgentSandboxTask = {
  id: 1,
  liveId: "worktree-a",
  agent: "Claude Code",
  branch: "agent/test",
  baseBranch: "main",
  promptSummary: "Review this change",
  sandboxStatus: "verified",
  forkLatencyMs: 2,
  compileTimeMs: 10,
  testsPassed: 0,
  testsTotal: 0,
  filesChanged: 1,
  additions: 1,
  deletions: 0,
  updatedAt: "now",
};
function swarmProps(tasks: AgentSandboxTask[]): AgentSwarmViewProps {
  return {
    tasks,
    focusedIndex: 0,
    onFocusIndexChange: vi.fn(),
    onApproveMerge: vi.fn(),
    onDiscardBranch: vi.fn(),
    onPublishGithub: vi.fn(),
    onVerifyWorktree: vi.fn(),
    onRefreshWorktrees: vi.fn(),
    canRunVerification: true,
    liveMode: true,
    onSpawnBatchSwarm: vi.fn(),
    onTriggerGithubSync: vi.fn(),
  };
}
it("requires a confirmed verified SHA and an idle worktree before approval", async () => {
  const props = swarmProps([{ ...task }]);
  const view = render(() => <AgentSwarmView {...props} />);
  expect(
    screen
      .getByRole("button", { name: "Approve & merge" })
      .hasAttribute("disabled"),
  ).toBe(true);
  view.unmount();
  const confirmed = swarmProps([{ ...task, verifiedCommit: "a".repeat(40) }]);
  render(() => <AgentSwarmView {...confirmed} />);
  await fireEvent.click(
    screen.getByRole("button", { name: "Approve & merge" }),
  );
  expect(confirmed.onApproveMerge).toHaveBeenCalledWith(1);
});
it("never renders repository-controlled task text as executable markup", () => {
  render(() => (
    <AgentSwarmView
      {...swarmProps([
        {
          ...task,
          promptSummary: '<img src=x onerror="alert(1)">',
          branch: "agent/<script>text</script>",
        },
      ])}
    />
  ));
  expect(screen.getByText('<img src=x onerror="alert(1)">')).toBeTruthy();
  expect(document.querySelector("script, img")).toBeNull();
});
it("submits the labelled sign-in form and clears the entered password", async () => {
  const signIn = vi.fn();
  render(() => (
    <EnterpriseAdminView
      users={[]}
      onToggleRole={vi.fn()}
      onToggleStatus={vi.fn()}
      onPolicyFeedback={vi.fn()}
      onSignIn={signIn}
    />
  ));
  await fireEvent.input(screen.getByLabelText("Username or email"), {
    target: { value: "operator" },
  });
  await fireEvent.input(screen.getByLabelText("Password", { exact: true }), {
    target: { value: "local-test-password" },
  });
  await fireEvent.click(
    screen.getByRole("button", { name: "Sign in", exact: true }),
  );
  expect(signIn).toHaveBeenCalledWith("operator", "local-test-password");
  expect(
    (screen.getByLabelText("Password", { exact: true }) as HTMLInputElement)
      .value,
  ).toBe("");
});
it("keeps file rendering bounded and selects a filtered path", async () => {
  const onSelect = vi.fn();
  const files = Array.from({ length: 5000 }, (_, index) => ({
    path: `src/file-${index}.rs`,
    kind: "file" as const,
  }));
  render(() => (
    <VirtualFileTree files={files} height={320} onSelect={onSelect} />
  ));
  expect(screen.getAllByRole("button").length).toBeLessThan(50);
  await fireEvent.input(screen.getByLabelText("Search repository paths"), {
    target: { value: "file-4999.rs" },
  });
  await fireEvent.click(
    screen.getByRole("button", { name: "src/file-4999.rs" }),
  );
  expect(onSelect).toHaveBeenCalledWith(files[4999]);
});
it("blocks background approval shortcuts in the actual verification dialog", async () => {
  const { shortcutManager } = await import("../src/keyboard/shortcut-manager");
  const handler = vi.fn();
  const unregister = shortcutManager.register({
    id: "test.actual-modal-approval",
    title: "Approve",
    category: "Actions",
    keys: "a",
    handler,
  });
  try {
    render(() => <AgentSwarmView {...swarmProps([{ ...task }])} />);
    await fireEvent.click(
      screen.getByRole("button", { name: "Verify command" }),
    );
    const modal = screen.getByRole("dialog", { name: "Worktree verification" });
    await fireEvent.keyDown(modal, { key: "a" });
    expect(handler).not.toHaveBeenCalled();
  } finally {
    unregister();
  }
});
it("keeps role-selection typeahead out of global worktree shortcuts", async () => {
  const { shortcutManager } = await import("../src/keyboard/shortcut-manager");
  const spawn = vi.fn();
  const unregister = shortcutManager.register({
    id: "test.select-spawn",
    title: "Spawn",
    category: "Actions",
    keys: "s",
    handler: spawn,
  });
  try {
    render(() => (
      <EnterpriseAdminView
        users={[]}
        onToggleRole={vi.fn()}
        onToggleStatus={vi.fn()}
        onPolicyFeedback={vi.fn()}
      />
    ));
    const trigger = screen.getByLabelText("Invite role");
    await fireEvent.keyDown(trigger, { key: "s" });
    expect(spawn).not.toHaveBeenCalled();
    await fireEvent.keyDown(trigger, { key: "ArrowDown" });
    await fireEvent.keyDown(screen.getByRole("listbox"), { key: "s" });
    expect(spawn).not.toHaveBeenCalled();
  } finally {
    unregister();
  }
});
