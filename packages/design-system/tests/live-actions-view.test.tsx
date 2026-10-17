// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@solidjs/testing-library";
import { LiveActionsView } from "../src/showcase/LiveActionsView";
import {
  actionsPage,
  actionsRepoId,
  pageCursor,
  secondWorktreeId,
} from "./actions-page-fixture";
import { automaticJob, historyRecord, worktreeId } from "./run-history-fixture";

beforeEach(() => {
  localStorage.setItem("gitust_token", "actions-a");
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("continues from an empty retained page and refreshes from the first page", async () => {
  const first = {
    ...actionsPage(),
    scanned_worktrees: 10,
    worktrees: [{ ...actionsPage().worktrees[0]!, runs: [] }],
    next_cursor: pageCursor(),
  };
  const second = {
    ...actionsPage(),
    worktrees: [
      {
        ...actionsPage().worktrees[0]!,
        worktree_id: secondWorktreeId,
        branch: "agent/next-page",
      },
    ],
  };
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(new Response(JSON.stringify(first)))
    .mockResolvedValueOnce(new Response(JSON.stringify(second)))
    .mockResolvedValueOnce(new Response(JSON.stringify(first)));
  vi.stubGlobal("fetch", fetcher);
  render(() => (
    <LiveActionsView repositoryId={actionsRepoId} role="SiteAdmin" />
  ));
  await screen.findByText("No attempts on this page");
  await fireEvent.click(
    screen.getByRole("button", { name: "Next", exact: true }),
  );
  await screen.findByText("agent/next-page");
  expect(
    new URL(fetcher.mock.calls[1]![0], "http://localhost").searchParams.get(
      "cursor",
    ),
  ).toBe(pageCursor());
  expect(
    screen.queryByRole("button", { name: "Next", exact: true }),
  ).toBeNull();
  await fireEvent.click(
    screen.getByRole("button", { name: "Refresh", exact: true }),
  );
  await screen.findByText("No attempts on this page");
  expect(fetcher.mock.calls[2]![0]).not.toContain("cursor=");
});

it("opens the exact attempt in Activity and explicitly reports a result lost to retention", async () => {
  const requested = automaticJob();
  const retained = {
    ...historyRecord(),
    automatic: true,
    verification_requested: false,
  };
  const fetcher = vi.fn(
    async (url: string) =>
      new Response(
        JSON.stringify(
          url.includes("/repo/automatic-runs")
            ? actionsPage()
            : url.endsWith("/automatic-runs")
              ? []
              : [retained],
        ),
      ),
  );
  vi.stubGlobal("fetch", fetcher);
  render(() => (
    <LiveActionsView repositoryId={actionsRepoId} role="SiteAdmin" />
  ));
  await fireEvent.click(
    await screen.findByRole("button", {
      name: /^Queued agent\/real-actions aaaaaaa/,
    }),
  );
  const dialog = await screen.findByRole("dialog");
  await within(dialog).findByText("This run is no longer retained");
  expect(within(dialog).queryByText("checked", { exact: true })).toBeNull();
  expect(within(dialog).queryByText(`Run: ${requested.run_id}`)).toBeNull();
  await fireEvent.click(
    within(dialog).getByRole("button", { name: /^Passed aaaaaaa/ }),
  );
  expect(
    await within(dialog).findByText("checked", { exact: true }),
  ).toBeTruthy();
  expect(
    fetcher.mock.calls.filter(([url]) =>
      url.includes(`/worktrees/${worktreeId}/`),
    ),
  ).toHaveLength(2);
});

it("does not fetch for nonadmins and clears rows and Activity when authority or repository changes", async () => {
  const fetcher = vi.fn(
    async (url: string) =>
      new Response(
        JSON.stringify(
          url.includes("/repo/automatic-runs") ? actionsPage() : [],
        ),
      ),
  );
  vi.stubGlobal("fetch", fetcher);
  const [role, setRole] = createSignal("Developer");
  const [repo, setRepo] = createSignal(actionsRepoId);
  render(() => <LiveActionsView repositoryId={repo()} role={role()} />);
  expect(screen.getByText("Administrator access required")).toBeTruthy();
  expect(fetcher).not.toHaveBeenCalled();
  setRole("SiteAdmin");
  await screen.findByText("agent/real-actions");
  await fireEvent.click(
    screen.getByRole("button", { name: /^Queued agent\/real-actions aaaaaaa/ }),
  );
  expect(await screen.findByRole("dialog")).toBeTruthy();
  setRole("Developer");
  expect(screen.queryByText("agent/real-actions")).toBeNull();
  expect(screen.queryByRole("dialog")).toBeNull();
  setRepo(secondWorktreeId);
  setRole("SiteAdmin");
  await screen.findByText("Invalid automatic Actions page from server");
  expect(screen.queryByText("agent/real-actions")).toBeNull();
});

it("clears the page and dialog on logout and rejects a stale row before its storage event arrives", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(
      async (url: string) =>
        new Response(
          JSON.stringify(
            url.includes("/repo/automatic-runs") ? actionsPage() : [],
          ),
        ),
    ),
  );
  render(() => (
    <LiveActionsView repositoryId={actionsRepoId} role="SiteAdmin" />
  ));
  const row = await screen.findByRole("button", {
    name: /^Queued agent\/real-actions aaaaaaa/,
  });
  await fireEvent.click(row);
  await screen.findByRole("dialog");
  window.dispatchEvent(new StorageEvent("storage", { key: "gitust_token" }));
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(screen.queryByText("agent/real-actions")).toBeNull();
  await fireEvent.click(
    screen.getByRole("button", { name: "Refresh", exact: true }),
  );
  const stale = await screen.findByRole("button", {
    name: /^Queued agent\/real-actions aaaaaaa/,
  });
  localStorage.removeItem("gitust_token");
  await fireEvent.click(stale);
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(screen.queryByText("agent/real-actions")).toBeNull();
});
