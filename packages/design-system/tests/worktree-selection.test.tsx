// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import {
  AgentSwarmView,
  type AgentSandboxTask,
  type AgentSwarmViewProps,
} from "../src/patterns/AgentSwarmView";

beforeEach(() => {
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});
const task = (id: number): AgentSandboxTask => ({
  id,
  liveId: `worktree-${id}`,
  agent: "Codex Swarm",
  branch: `agent/task-${id}`,
  baseBranch: "main",
  forkLatencyMs: 0,
  promptSummary: `Task ${id}`,
  sandboxStatus: "pending",
  compileTimeMs: 0,
  testsPassed: 0,
  testsTotal: 0,
  filesChanged: 0,
  additions: 0,
  deletions: 0,
  updatedAt: "now",
});
const callbacks = (): Omit<AgentSwarmViewProps, "tasks" | "focusedIndex"> => ({
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
  onViewRunHistory: vi.fn(),
  onReviewWorktree: vi.fn(),
});
it("moves focus into a mobile detail and restores it when returning", async () => {
  render(() => (
    <AgentSwarmView {...callbacks()} tasks={[task(1)]} focusedIndex={0} />
  ));
  const row = within(
    screen.getByRole("list", { name: "Agent worktrees" }),
  ).getByRole("button", { name: /Task 1/ });
  const back = screen.getByRole("button", { name: "Back to worktrees" });
  vi.spyOn(back, "getClientRects").mockReturnValue([
    {},
  ] as unknown as DOMRectList);
  row.focus();
  await fireEvent.click(row);
  expect(document.activeElement).toBe(back);
  await fireEvent.click(back);
  expect(document.activeElement).toBe(row);
});
it("only changes the selected worktree on explicit selection and binds actions to it", async () => {
  const props = callbacks();
  const [index, setIndex] = createSignal(0);
  render(() => (
    <AgentSwarmView
      {...props}
      tasks={[task(1), task(2)]}
      focusedIndex={index()}
      onFocusIndexChange={setIndex}
    />
  ));
  const list = screen.getByRole("list", { name: "Agent worktrees" });
  const second = within(list).getByRole("button", { name: /Task 2/ });
  await fireEvent.mouseEnter(second);
  await fireEvent.click(
    screen.getByRole("button", { name: "Review committed changes" }),
  );
  expect(props.onReviewWorktree).toHaveBeenLastCalledWith(1);
  await fireEvent.click(second);
  await fireEvent.click(
    screen.getByRole("button", { name: "Review committed changes" }),
  );
  expect(props.onReviewWorktree).toHaveBeenLastCalledWith(2);
  await fireEvent.click(screen.getByRole("button", { name: "Run history" }));
  expect(props.onViewRunHistory).toHaveBeenLastCalledWith("worktree-2");
  await fireEvent.click(
    screen.getByRole("button", { name: "Back to worktrees" }),
  );
  expect(
    list.closest("[data-mobile-detail]")?.getAttribute("data-mobile-detail"),
  ).toBe("false");
  expect(document.activeElement).toBe(second);
});
it("preserves selected identity after refresh, then returns to the list if it disappears", async () => {
  const props = callbacks();
  const [tasks, setTasks] = createSignal([task(1), task(2)]);
  const [index, setIndex] = createSignal(0);
  render(() => (
    <AgentSwarmView
      {...props}
      tasks={tasks()}
      focusedIndex={index()}
      onFocusIndexChange={setIndex}
    />
  ));
  const list = screen.getByRole("list", { name: "Agent worktrees" });
  await fireEvent.click(within(list).getByRole("button", { name: /Task 2/ }));
  setTasks([task(2), task(1)]);
  expect(index()).toBe(0);
  await fireEvent.click(
    screen.getByRole("button", { name: "Review committed changes" }),
  );
  expect(props.onReviewWorktree).toHaveBeenLastCalledWith(2);
  setTasks([task(1)]);
  expect(
    list.closest("[data-mobile-detail]")?.getAttribute("data-mobile-detail"),
  ).toBe("false");
  await fireEvent.input(screen.getByLabelText("Search worktrees"), {
    target: { value: "missing" },
  });
  expect(
    screen.queryByRole("button", { name: "Review committed changes" }),
  ).toBeNull();
  expect(screen.getByText("No matching worktrees")).toBeTruthy();
});
it("tracks keyboard focus changes and keeps verification bound to its original live ID", async () => {
  const props = callbacks();
  const [index, setIndex] = createSignal(0);
  render(() => (
    <AgentSwarmView
      {...props}
      tasks={[task(1), task(2)]}
      focusedIndex={index()}
      onFocusIndexChange={setIndex}
    />
  ));
  setIndex(1);
  await fireEvent.click(screen.getByRole("button", { name: "Verify command" }));
  setIndex(0);
  const dialog = screen.getByRole("dialog", { name: "Worktree verification" });
  expect(within(dialog).getByText("agent/task-2")).toBeTruthy();
  await fireEvent.input(within(dialog).getByLabelText("Verification command"), {
    target: { value: "cargo check" },
  });
  await fireEvent.click(
    within(dialog).getByRole("button", { name: "Run verification" }),
  );
  expect(props.onVerifyWorktree).toHaveBeenCalledWith(2, "cargo check");
});

it("keeps refresh-required approval blocked and publication separate from a local merge", async () => {
  const props = callbacks();
  const [current, setCurrent] = createSignal<AgentSandboxTask>({
    ...task(1),
    sandboxStatus: "verified",
    verifiedCommit: "a".repeat(40),
    verificationNeedsRefresh: true,
  });
  render(() => (
    <AgentSwarmView {...props} tasks={[current()]} focusedIndex={0} />
  ));
  expect(
    screen
      .getByRole("button", { name: "Approve & merge" })
      .hasAttribute("disabled"),
  ).toBe(true);
  await fireEvent.click(
    within(screen.getByRole("region", { name: "Selected worktree" })).getByRole(
      "button",
      { name: "Refresh worktrees" },
    ),
  );
  expect(props.onRefreshWorktrees).toHaveBeenCalledOnce();
  setCurrent({
    ...task(1),
    sandboxStatus: "merged",
    publicationError: "Upstream unavailable",
  });
  expect(screen.queryByRole("button", { name: "Approve & merge" })).toBeNull();
  expect(screen.getByText("Upstream unavailable")).toBeTruthy();
  await fireEvent.click(
    screen.getByRole("button", { name: "Retry publication" }),
  );
  expect(props.onPublishGithub).toHaveBeenCalledWith(1);
  expect(props.onApproveMerge).not.toHaveBeenCalled();
});
