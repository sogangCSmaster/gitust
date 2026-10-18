// @vitest-environment jsdom
import { commitHistory } from "./fixtures/commit-history";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  configure,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@solidjs/testing-library";
import { WorkbenchApp } from "../src/showcase/WorkbenchApp";

configure({ asyncUtilTimeout: 5000 });

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
            : url.endsWith("/api/v1/repos")
              ? {
                  repositories: [
                    {
                      id: "22222222-2222-4222-8222-222222222222",
                      name: "real-repo",
                      branch: "main",
                      head_commit: "c".repeat(40),
                      capability: "workbench",
                      is_default: true,
                    },
                  ],
                }
              : url.endsWith("/swarm/worktrees")
                ? [row]
                : url.includes("/commits?")
                  ? commitHistory("c".repeat(40))
                  : url.includes("/tree?")
                    ? { commit_sha: "c".repeat(40), entries: [] }
                    : url.includes("/diff?")
                      ? {
                          head_sha: "c".repeat(40),
                          base_sha: "a".repeat(40),
                          files: [],
                          lines: [],
                        }
                      : [],
        ),
      ),
  );
  vi.stubGlobal("fetch", fetcher);
});
afterEach(async () => {
  cleanup();
  window.history.replaceState({}, "", "/");
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

const renderConnectedWorkbench = async () => {
  render(() => <WorkbenchApp />);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Open repository real-repo" }),
  );
  await fireEvent.click(
    within(
      screen.getByRole("navigation", { name: "Repository views" }),
    ).getByRole("button", { name: "Worktrees", exact: true }),
  );
};

it("starts with repository selection and returns to the list without exposing stale repository views", async () => {
  render(() => <WorkbenchApp />);
  const repository = await screen.findByRole("button", {
    name: "Open repository real-repo",
  });
  expect(screen.queryAllByText("REAL worktree")).toHaveLength(0);
  expect(
    fetcher.mock.calls.some(([url]) => url.endsWith("/swarm/worktrees")),
  ).toBe(false);
  await fireEvent.click(repository);
  await screen.findByRole("heading", { name: "real-repo" });
  await fireEvent.click(
    within(
      screen.getByRole("navigation", { name: "Repository views" }),
    ).getByRole("button", { name: "Worktrees", exact: true }),
  );
  await screen.findByRole("heading", { name: "REAL worktree", exact: true });
  await fireEvent.click(
    screen.getByRole("button", { name: "Back to repositories" }),
  );
  await screen.findByRole("heading", { name: "Repositories" });
  expect(
    screen.queryByRole("navigation", { name: "Repository views" }),
  ).toBeNull();
  expect(screen.queryAllByText("REAL worktree")).toHaveLength(0);
});

it("clears live rows on another tab's logout instead of allowing a fake live merge", async () => {
  await renderConnectedWorkbench();
  await screen.findByRole("heading", { name: "REAL worktree", exact: true });
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
  const mutations = () =>
    fetcher.mock.calls.filter((call) =>
      ["POST", "DELETE", "PUT", "PATCH"].includes(call[1]?.method),
    );
  const requests = mutations().length;
  storageChanged(null);
  expect(screen.getByText("Demo workspace")).toBeTruthy();
  expect(screen.queryAllByText("REAL worktree")).toHaveLength(0);
  await fireEvent.click(oldButton);
  expect(
    screen.queryByText(/Demo worktree marked merged: agent\/real-branch/),
  ).toBeNull();
  expect(mutations()).toHaveLength(requests);
});

it("never sends a live row into demo actions while its storage event is still pending", async () => {
  await renderConnectedWorkbench();
  await screen.findByRole("heading", { name: "REAL worktree", exact: true });
  localStorage.removeItem("gitust_token");
  await fireEvent.click(
    screen.getByRole("button", { name: "Approve & merge" }),
  );
  expect(
    screen.queryByText(/Demo worktree marked merged: agent\/real-branch/),
  ).toBeNull();
  expect(screen.getByRole("button", { name: "Approve & merge" })).toBeTruthy();
});

it("drops the old account and rows immediately when a replacement account is still loading", async () => {
  await renderConnectedWorkbench();
  await screen.findByRole("heading", { name: "REAL worktree", exact: true });
  fetcher.mockImplementation(() => new Promise<Response>(() => {}));
  storageChanged("session-b");
  expect(screen.getByText("Live workspace")).toBeTruthy();
  expect(screen.queryAllByText("REAL worktree")).toHaveLength(0);
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
    if (url.includes("/tree?"))
      return new Response(
        JSON.stringify({
          entries: [{ path: "partial.txt", is_dir: false }],
          commit_sha: "c".repeat(40),
          truncated: true,
        }),
      );
    if (url.includes("/diff?"))
      return new Response(
        JSON.stringify({
          head_sha: "c".repeat(40),
          base_sha: "a".repeat(40),
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
  await renderConnectedWorkbench();
  await screen.findByRole("heading", { name: "REAL worktree", exact: true });
  await fireEvent.click(
    screen.getByRole("button", { name: "Changes", exact: true }),
  );
  await screen.findByText(/This diff preview is incomplete/);
  await fireEvent.click(
    screen.getByRole("button", { name: "Files", exact: true }),
  );
  expect(
    screen.getByText(/This repository path list is incomplete/),
  ).toBeTruthy();
  await fireEvent.click(
    screen.getByRole("button", { name: "Changes", exact: true }),
  );
  expect(
    await screen.findByText(/This diff preview is incomplete/),
  ).toBeTruthy();
});

it("changes the selected diff file without displaying the aggregate multi-file content", async () => {
  const normalFetch = fetcher.getMockImplementation()!;
  fetcher.mockImplementation(async (url: string, options?: RequestInit) => {
    if (url.includes("/diff?")) {
      const path = new URL(url, "https://example.invalid").searchParams.get(
        "path",
      );
      return new Response(
        JSON.stringify({
          head_sha: "c".repeat(40),
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
  await renderConnectedWorkbench();
  await screen.findByRole("heading", { name: "REAL worktree", exact: true });
  await fireEvent.click(
    screen.getByRole("button", { name: "Changes", exact: true }),
  );
  await screen.findByText("FIRST FILE");
  expect(screen.queryByText("UNATTRIBUTED AGGREGATE")).toBeNull();
  await fireEvent.click(
    screen.getByRole("button", { name: "two.txt", exact: true }),
  );
  await screen.findByText("SECOND FILE");
  expect(screen.queryByText("FIRST FILE")).toBeNull();
});

it("opens committed worktree changes at the displayed verified SHA and merge target", async () => {
  const normalFetch = fetcher.getMockImplementation()!;
  const base = "c".repeat(40);
  fetcher.mockImplementation(async (url: string, options?: RequestInit) => {
    if (url.includes("/diff?"))
      return new Response(
        JSON.stringify({
          head_sha: row.verified_commit,
          base_sha: base,
          files: [{ path: "reviewed.txt" }],
          files_changed: 1,
          lines: [
            {
              old_line: null,
              new_line: 1,
              kind: "add",
              content: "COMMITTED WORKTREE CONTENT",
            },
          ],
        }),
      );
    return normalFetch(url, options);
  });
  await renderConnectedWorkbench();
  await screen.findByRole("heading", { name: "REAL worktree", exact: true });
  await fireEvent.click(
    screen.getByRole("button", {
      name: "Review committed changes",
      exact: true,
    }),
  );
  await screen.findByRole("heading", { name: "Changes", exact: true });
  await screen.findByText("COMMITTED WORKTREE CONTENT");
  await fireEvent.click(
    screen.getByRole("button", { name: "Comparison details", exact: true }),
  );
  expect(screen.getByText(/Uncommitted files are not included/)).toBeTruthy();
  expect(screen.getByText(`Head ${row.branch}`)).toBeTruthy();
  expect(
    screen.getByText(row.verified_commit, { selector: "code" }),
  ).toBeTruthy();
  expect(screen.getByText("Base main")).toBeTruthy();
  expect(screen.getByText(base, { selector: "code" })).toBeTruthy();
  const requests = fetcher.mock.calls.filter(([url]) => url.includes("/diff?"));
  const query = new URL(requests.at(-1)![0], "https://example.invalid")
    .searchParams;
  expect(query.get("head")).toBe(row.verified_commit);
  expect(query.get("base")).toBe("refs/heads/main");
});

it("preserves an unknown worktree target and reports it instead of reviewing main", async () => {
  const normalFetch = fetcher.getMockImplementation()!;
  fetcher.mockImplementation(async (url: string, options?: RequestInit) =>
    url.endsWith("/swarm/worktrees")
      ? new Response(JSON.stringify([{ ...row, base_branch: "" }]))
      : normalFetch(url, options),
  );
  await renderConnectedWorkbench();
  await screen.findByRole("heading", { name: "REAL worktree", exact: true });
  const before = fetcher.mock.calls.filter(([url]) =>
    url.includes("/diff?"),
  ).length;
  await fireEvent.click(
    screen.getByRole("button", {
      name: "Review committed changes",
      exact: true,
    }),
  );
  await screen.findByText(/Worktree merge target is unknown/, {
    selector: "p",
  });
  expect(
    fetcher.mock.calls.filter(([url]) => url.includes("/diff?")),
  ).toHaveLength(before);
});

it("does not expose live review in demo mode or dispatch a detached live opener after logout", async () => {
  await renderConnectedWorkbench();
  await screen.findByRole("heading", { name: "REAL worktree", exact: true });
  const button = screen.getByRole("button", {
    name: "Review committed changes",
    exact: true,
  });
  storageChanged(null);
  expect(
    screen.queryByRole("button", {
      name: "Review committed changes",
      exact: true,
    }),
  ).toBeNull();
  const requests = fetcher.mock.calls.filter(([url]) =>
    url.includes("/diff?"),
  ).length;
  await fireEvent.click(button);
  expect(
    fetcher.mock.calls.filter(([url]) => url.includes("/diff?")),
  ).toHaveLength(requests);
});

it("preserves password-reset access links when an old stored session is rejected", async () => {
  window.history.replaceState(
    {},
    "",
    "/auth/reset-password?token=gitust_rst_sample-link",
  );
  const normalFetch = fetcher.getMockImplementation()!;
  fetcher.mockImplementation((url: string, options?: RequestInit) =>
    url.endsWith("/auth/me")
      ? Promise.resolve(new Response("Expired", { status: 401 }))
      : normalFetch(url, options),
  );
  render(() => <WorkbenchApp />);
  const input = await screen.findByRole("textbox", {
    name: "Single-use token",
  });
  expect((input as HTMLInputElement).value).toBe("gitust_rst_sample-link");
  expect(window.location.search).toBe("");
  expect(
    screen
      .getByRole("tab", { name: "Reset password" })
      .getAttribute("aria-selected"),
  ).toBe("true");
  expect(screen.queryByRole("heading", { name: "Repositories" })).toBeNull();
});

it("keeps file previews separate from a selected worktree comparison across navigation", async () => {
  const normalFetch = fetcher.getMockImplementation()!;
  fetcher.mockImplementation(async (url: string, options?: RequestInit) => {
    if (url.includes("/tree?"))
      return new Response(
        JSON.stringify({
          commit_sha: "c".repeat(40),
          entries: [{ path: "snapshot.txt", is_dir: false }],
        }),
      );
    if (url.includes("/blob?"))
      return new Response(
        JSON.stringify({
          commit_sha: "c".repeat(40),
          content: "FILE SNAPSHOT CONTENT",
        }),
      );
    if (url.includes("/diff?"))
      return new Response(
        JSON.stringify({
          head_sha: row.verified_commit,
          base_sha: row.base_commit,
          files: [{ path: "reviewed.txt" }],
          files_changed: 1,
          lines: [
            {
              old_line: null,
              new_line: 1,
              kind: "add",
              content: "REVIEW DIFF CONTENT",
            },
          ],
        }),
      );
    return normalFetch(url, options);
  });
  await renderConnectedWorkbench();
  await screen.findByRole("heading", { name: "REAL worktree", exact: true });
  await fireEvent.click(
    screen.getByRole("button", {
      name: "Review committed changes",
      exact: true,
    }),
  );
  await screen.findByText("REVIEW DIFF CONTENT");
  await fireEvent.click(
    screen.getByRole("button", { name: "Files", exact: true }),
  );
  await screen.findByRole("heading", { name: "Repository files", exact: true });
  expect(screen.queryByText("REVIEW DIFF CONTENT")).toBeNull();
  await fireEvent.click(
    await screen.findByRole("button", { name: "snapshot.txt", exact: true }),
  );
  await screen.findByText("FILE SNAPSHOT CONTENT");
  await fireEvent.click(
    screen.getByRole("button", { name: "Changes", exact: true }),
  );
  await screen.findByText("REVIEW DIFF CONTENT");
  expect(screen.queryByText("FILE SNAPSHOT CONTENT")).toBeNull();
  await fireEvent.click(
    screen.getByRole("button", { name: "Comparison details", exact: true }),
  );
  expect(
    screen.getByText(row.verified_commit, { selector: "code" }),
  ).toBeTruthy();
  expect(screen.getByText(row.base_commit, { selector: "code" })).toBeTruthy();
  expect(screen.getByText(`Head ${row.branch}`)).toBeTruthy();
  await fireEvent.click(
    screen.getByRole("button", { name: "Files", exact: true }),
  );
  await screen.findByText("FILE SNAPSHOT CONTENT");
  expect(screen.queryByText("REVIEW DIFF CONTENT")).toBeNull();
});
