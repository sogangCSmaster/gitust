// @vitest-environment jsdom
import { createSignal, Show } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@solidjs/testing-library";
import { RunHistoryDialog } from "../src/patterns/RunHistoryDialog";
import { LiveRunHistoryDialog } from "../src/showcase/LiveRunHistoryDialog";
import {
  parseRunHistory,
  type RunHistoryState,
} from "../src/showcase/live-run-history-client";
import {
  AgentSwarmView,
  type AgentSandboxTask,
} from "../src/patterns/AgentSwarmView";
import { shortcutManager } from "../src/keyboard/shortcut-manager";
import { worktreeId, historyRecord } from "./run-history-fixture";

beforeEach(() => {
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  window.localStorage.setItem("gitust_token", "test-session");
});
afterEach(() => {
  cleanup();
  window.localStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
const ready = (): RunHistoryState => ({
  status: "ready",
  records: parseRunHistory([historyRecord()], worktreeId),
});

it("returns focus to the history opener after Escape", async () => {
  const [open, setOpen] = createSignal(false);
  render(() => (
    <>
      <button onClick={() => setOpen(true)}>Open saved runs</button>
      <Show when={open()}>
        <RunHistoryDialog
          branch="agent/test/history"
          state={ready()}
          onRefresh={vi.fn()}
          onClose={() => setOpen(false)}
        />
      </Show>
    </>
  ));
  const opener = screen.getByRole("button", { name: "Open saved runs" });
  opener.focus();
  await fireEvent.click(opener);
  await screen.findByText("checked", { exact: true });
  await fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await waitFor(() => expect(document.activeElement).toBe(opener));
});

it("shows historical SHA and output without approval actions and blocks background shortcuts", async () => {
  const approve = vi.fn();
  const unregister = shortcutManager.register({
    id: "history.approve",
    title: "Approve",
    category: "Actions",
    keys: "a",
    handler: approve,
  });
  try {
    render(() => (
      <RunHistoryDialog
        branch="agent/test/history"
        state={ready()}
        onRefresh={vi.fn()}
        onClose={vi.fn()}
      />
    ));
    const dialog = screen.getByRole("dialog", { name: "Run history" });
    dialog.dispatchEvent(
      new KeyboardEvent("keydown", { key: "a", bubbles: true }),
    );
    expect(approve).not.toHaveBeenCalled();
    expect(
      await screen.findByText(`Checked commit at execution: ${"a".repeat(40)}`),
    ).toBeTruthy();
    expect(screen.getByText("checked", { exact: true })).toBeTruthy();
    expect(screen.getByText(/Past results do not grant approval/)).toBeTruthy();
    expect(
      screen.queryByRole("button", {
        name: /Approve|Run verification|Retry verification/,
      }),
    ).toBeNull();
  } finally {
    unregister();
  }
});

it("selects older failures and renders shortened commands and markup as plain diagnostics", async () => {
  const state = ready();
  const old = {
    ...state.records[0],
    runId: "44444444-4444-4444-8444-444444444444",
    historyTruncated: true,
    result: {
      ...state.records[0].result,
      command: "<script>old command</script>",
      stdout: '<img src=x onerror="bad()">',
      exitCode: 7,
      steps: [
        { command: "exit 7", required: false, exitCode: 7, durationMs: 1 },
        {
          command: "required",
          required: true,
          exitCode: null,
          durationMs: null,
        },
      ],
    },
  };
  state.records.push(old);
  render(() => (
    <RunHistoryDialog
      branch="agent/test/history"
      state={state}
      onRefresh={vi.fn()}
      onClose={vi.fn()}
    />
  ));
  await fireEvent.click(
    await screen.findByRole("button", { name: `View run ${old.runId}` }),
  );
  expect(screen.getByText("Skipped", { exact: true })).toBeTruthy();
  expect(
    screen.getByText("Stored command or output was shortened"),
  ).toBeTruthy();
  expect(
    screen.getByText('<img src=x onerror="bad()">', { exact: true }),
  ).toBeTruthy();
  expect(document.querySelector("script, img")).toBeNull();
  expect(screen.queryByText("checked", { exact: true })).toBeNull();
});

it("clears stored details during refresh or errors and presents an explicit empty state", async () => {
  const [state, setState] = createSignal(ready());
  render(() => (
    <RunHistoryDialog
      branch="agent/test/history"
      state={state()}
      onRefresh={vi.fn()}
      onClose={vi.fn()}
    />
  ));
  await screen.findByText("checked", { exact: true });
  setState({ status: "loading", records: [] });
  expect(screen.queryByText("checked", { exact: true })).toBeNull();
  setState({
    status: "error",
    records: [],
    error: "History denied (HTTP 403)",
  });
  expect(screen.getByRole("alert").textContent).toContain("403");
  setState({ status: "ready", records: [] });
  expect(screen.getByText("No stored runs")).toBeTruthy();
});

it("binds the live refresh and closes on browser account replacement", async () => {
  const fetchMock = vi
    .fn()
    .mockResolvedValue(new Response(JSON.stringify([historyRecord()])));
  vi.stubGlobal("fetch", fetchMock);
  const close = vi.fn();
  render(() => (
    <LiveRunHistoryDialog
      worktreeId={worktreeId}
      branch="agent/test/history"
      onClose={close}
    />
  ));
  await screen.findByText("checked", { exact: true });
  fetchMock.mockResolvedValue(new Response("denied", { status: 403 }));
  await fireEvent.click(
    screen.getByRole("button", { name: "Refresh history" }),
  );
  await waitFor(() =>
    expect(screen.getByRole("alert").textContent).toContain("403"),
  );
  expect(screen.queryByText("checked", { exact: true })).toBeNull();
  window.dispatchEvent(
    new StorageEvent("storage", {
      key: "gitust_token",
      newValue: "another-account",
    }),
  );
  expect(close).toHaveBeenCalledTimes(1);
});

it("exposes live admin history even for merged rows and omits it for demo/non-admin views", async () => {
  const task: AgentSandboxTask = {
    id: 1,
    liveId: worktreeId,
    agent: "Claude Code",
    branch: "agent/test/history",
    baseBranch: "main",
    promptSummary: "history",
    sandboxStatus: "merged",
    forkLatencyMs: 0,
    compileTimeMs: 0,
    testsPassed: 0,
    testsTotal: 0,
    filesChanged: 0,
    additions: 0,
    deletions: 0,
    updatedAt: "now",
  };
  const viewHistory = vi.fn();
  const props = {
    tasks: [task],
    focusedIndex: 0,
    onFocusIndexChange: vi.fn(),
    onApproveMerge: vi.fn(),
    onDiscardBranch: vi.fn(),
    onPublishGithub: vi.fn(),
    onVerifyWorktree: vi.fn(),
    onRefreshWorktrees: vi.fn(),
    onSpawnBatchSwarm: vi.fn(),
    onTriggerGithubSync: vi.fn(),
    onViewRunHistory: viewHistory,
  };
  const view = render(() => (
    <AgentSwarmView {...props} liveMode canRunVerification />
  ));
  await fireEvent.click(
    screen.getByRole("button", { name: "Run history", exact: true }),
  );
  expect(viewHistory).toHaveBeenCalledWith(worktreeId);
  view.unmount();
  const nonAdmin = render(() => (
    <AgentSwarmView {...props} liveMode canRunVerification={false} />
  ));
  expect(screen.queryByRole("button", { name: "Run history" })).toBeNull();
  nonAdmin.unmount();
  render(() => (
    <AgentSwarmView {...props} liveMode={false} canRunVerification />
  ));
  expect(screen.queryByRole("button", { name: "Run history" })).toBeNull();
});
