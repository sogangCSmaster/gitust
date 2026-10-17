// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@solidjs/testing-library";
import { WorkbenchApp } from "../src/showcase/WorkbenchApp";

const row = {
  id: "11111111-1111-4111-8111-111111111111",
  repo_id: "repo",
  agent: "CodexSwarm",
  branch: "agent/real-branch",
  base_branch: "main",
  base_commit: "a".repeat(40),
  verified_commit: "b".repeat(40),
  prompt_summary: "REAL worktree",
  materialize_micros: 500,
  status: "Verified",
  last_sandbox_duration_ms: 20,
  tests_passed: 1,
  tests_total: 1,
  files_changed: 1,
  additions: 1,
  deletions: 0,
  github_synced: false,
  created_at: "2026-10-17T00:00:00Z",
};
let fetcher: ReturnType<typeof vi.fn>;
beforeEach(() => {
  localStorage.setItem("gitust_token", "session-a");
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: vi.fn(() => ({
      matches: false,
      addEventListener() {},
      removeEventListener() {},
    })),
  });
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
  fetcher = vi.fn(
    async (url: string) =>
      new Response(
        JSON.stringify(
          url.endsWith("/auth/me")
            ? { user_id: "admin", username: "Admin", role: "SiteAdmin" }
            : url.endsWith("/swarm/worktrees")
              ? [row]
              : url.includes("/repo/commits")
                ? { commits: [] }
                : url.includes("/repo/tree")
                  ? { entries: [] }
                  : url.includes("/repo/diff")
                    ? { files: [], lines: [] }
                    : [],
        ),
      ),
  );
  vi.stubGlobal("fetch", fetcher);
});
afterEach(async () => {
  cleanup();
  await new Promise((resolve) => setTimeout(resolve, 0));
  localStorage.clear();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});
const storageChanged = (value: string | null) => {
  const oldValue = localStorage.getItem("gitust_token");
  if (value === null) localStorage.removeItem("gitust_token");
  else localStorage.setItem("gitust_token", value);
  window.dispatchEvent(
    new StorageEvent("storage", {
      key: "gitust_token",
      oldValue,
      newValue: value,
    }),
  );
};

it("clears live rows on another tab's logout instead of allowing a fake live merge", async () => {
  render(() => <WorkbenchApp />);
  await screen.findByText("REAL worktree");
  await waitFor(() =>
    expect(
      (
        screen.getByRole("button", {
          name: "Approve & merge",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(false),
  );
  const oldButton = screen.getByRole("button", { name: "Approve & merge" });
  const requests = fetcher.mock.calls.length;
  storageChanged(null);
  expect(screen.getByText("Demo workspace")).toBeTruthy();
  expect(screen.queryByText("REAL worktree")).toBeNull();
  await fireEvent.click(oldButton);
  expect(
    screen.queryByText(/Fast-forward merged agent\/real-branch/),
  ).toBeNull();
  expect(fetcher.mock.calls.length).toBe(requests);
});

it("never sends a live row into demo actions while its storage event is still pending", async () => {
  render(() => <WorkbenchApp />);
  await screen.findByText("REAL worktree");
  localStorage.removeItem("gitust_token");
  await fireEvent.click(
    screen.getByRole("button", { name: "Approve & merge" }),
  );
  expect(
    screen.queryByText(/Fast-forward merged agent\/real-branch/),
  ).toBeNull();
  expect(screen.getByRole("button", { name: "Approve & merge" })).toBeTruthy();
});

it("drops the old account and rows immediately when a replacement account is still loading", async () => {
  render(() => <WorkbenchApp />);
  await screen.findByText("REAL worktree");
  fetcher.mockImplementation(() => new Promise<Response>(() => {}));
  storageChanged("session-b");
  expect(screen.getByText("Live workspace")).toBeTruthy();
  expect(screen.queryByText("REAL worktree")).toBeNull();
  expect(screen.queryByRole("button", { name: "Run history" })).toBeNull();
  expect(
    fetcher.mock.calls.some(
      (call) => call[1]?.headers?.Authorization === "Bearer session-b",
    ),
  ).toBe(true);
});

it("shows persistent partial-result notices in Changes and Repository files", async () => {
  const normalFetch = fetcher.getMockImplementation()!;
  fetcher.mockImplementation(async (url: string, options?: RequestInit) => {
    if (url.includes("/repo/tree"))
      return new Response(
        JSON.stringify({
          entries: [{ path: "partial.txt", is_dir: false }],
          truncated: true,
        }),
      );
    if (url.includes("/repo/diff"))
      return new Response(
        JSON.stringify({
          files: [{ path: "partial.txt" }],
          files_changed: 1,
          truncated: true,
          lines: [
            {
              old_line: null,
              new_line: 1,
              kind: "add",
              content: "PARTIAL LINE",
            },
          ],
        }),
      );
    return normalFetch(url, options);
  });
  render(() => <WorkbenchApp />);
  await screen.findByText("REAL worktree");
  await fireEvent.click(
    screen.getByRole("button", { name: "Diff", exact: true }),
  );
  await screen.findByText(/This diff preview is incomplete/);
  await fireEvent.click(
    screen.getByRole("button", { name: "Files", exact: true }),
  );
  expect(
    screen.getByText(/This repository path list is incomplete/),
  ).toBeTruthy();
  await fireEvent.click(
    screen.getByRole("button", { name: "Diff", exact: true }),
  );
  expect(screen.getByText(/This diff preview is incomplete/)).toBeTruthy();
});

it("changes the selected diff file without displaying the aggregate multi-file content", async () => {
  const normalFetch = fetcher.getMockImplementation()!;
  fetcher.mockImplementation(async (url: string, options?: RequestInit) => {
    if (url.includes("/repo/diff")) {
      const path = new URL(url, "https://example.invalid").searchParams.get(
        "path",
      );
      return new Response(
        JSON.stringify({
          head_sha: "a".repeat(40),
          base_sha: "b".repeat(40),
          truncated: false,
          files: path ? [{ path }] : [{ path: "one.txt" }, { path: "two.txt" }],
          files_changed: path ? 1 : 2,
          lines: [
            {
              old_line: null,
              new_line: 1,
              kind: "add",
              content:
                path === "two.txt"
                  ? "SECOND FILE"
                  : path
                    ? "FIRST FILE"
                    : "UNATTRIBUTED AGGREGATE",
            },
          ],
        }),
      );
    }
    return normalFetch(url, options);
  });
  render(() => <WorkbenchApp />);
  await screen.findByText("REAL worktree");
  await fireEvent.click(
    screen.getByRole("button", { name: "Diff", exact: true }),
  );
  await screen.findByText("FIRST FILE");
  expect(screen.queryByText("UNATTRIBUTED AGGREGATE")).toBeNull();
  await fireEvent.click(
    screen.getByRole("button", { name: "two.txt", exact: true }),
  );
  await screen.findByText("SECOND FILE");
  expect(screen.queryByText("FIRST FILE")).toBeNull();
});
