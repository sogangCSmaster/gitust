// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@solidjs/testing-library";
import { WorkbenchApp } from "../src/showcase/WorkbenchApp";
const repo = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "trunk-repo",
  branch: "trunk",
  head_commit: "a".repeat(40),
};
const row = {
  id: "22222222-2222-4222-8222-222222222222",
  repo_id: repo.id,
  agent: "CodexSwarm",
  branch: "agent/existing",
  base_branch: "trunk",
  base_commit: repo.head_commit,
  prompt_summary: "Existing task",
  status: "pending",
  materialize_micros: 1,
  last_sandbox_duration_ms: 0,
  tests_passed: 0,
  tests_total: 0,
  files_changed: 0,
  additions: 0,
  deletions: 0,
  github_synced: false,
  created_at: "2026-10-17T00:00:00Z",
};
const response = (data: unknown) => new Response(JSON.stringify(data));
beforeEach(() => {
  localStorage.setItem("gitust_token", "developer-session");
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: () => ({
      matches: false,
      addEventListener() {},
      removeEventListener() {},
    }),
  });
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
});
afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});
it("lets a Developer create from the connected trunk branch and selects the returned worktree", async () => {
  let createdBody: Record<string, string> | undefined;
  const fetcher = vi.fn(async (url: string, options?: RequestInit) => {
    if (url.endsWith("/api/v1/repo")) return response(repo);
    if (url.endsWith("/auth/me"))
      return response({
        user_id: "developer",
        username: "Developer",
        role: "Developer",
      });
    if (url.endsWith("/swarm/worktrees") && options?.method === "POST") {
      createdBody = JSON.parse(options.body as string);
      return response({
        ...row,
        id: "33333333-3333-4333-8333-333333333333",
        ...createdBody,
      });
    }
    if (url.endsWith("/swarm/worktrees")) return response([row]);
    if (url.includes("/repo/tree"))
      return response({ commit_sha: repo.head_commit, entries: [] });
    if (url.includes("/repo/commits")) return response({ commits: [] });
    if (url.includes("/repo/diff"))
      return response({
        head_sha: repo.head_commit,
        base_sha: "b".repeat(40),
        files: [],
        lines: [],
      });
    return response([]);
  });
  vi.stubGlobal("fetch", fetcher);
  render(() => <WorkbenchApp />);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Open repository trunk-repo" }),
  );
  await fireEvent.click(
    within(
      screen.getByRole("navigation", { name: "Repository views" }),
    ).getByRole("button", { name: "Worktrees", exact: true }),
  );
  await screen.findByRole("heading", { name: "Existing task" });
  await fireEvent.click(
    screen.getByRole("button", { name: "New worktree", exact: true }),
  );
  const name = await screen.findByRole("textbox", { name: "Task name" });
  expect(createdBody).toBeUndefined();
  expect(
    (screen.getByRole("textbox", { name: "Base branch" }) as HTMLInputElement)
      .value,
  ).toBe("trunk");
  await fireEvent.input(name, { target: { value: "Developer work" } });
  const branch = screen.getByTestId("new-worktree-branch").textContent;
  await fireEvent.click(
    screen.getByRole("button", { name: "Create worktree", exact: true }),
  );
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await screen.findByRole("heading", { name: "Developer work" });
  expect(createdBody).toMatchObject({
    repo_id: repo.id,
    agent: "CodexSwarm",
    branch,
    base_branch: "trunk",
    prompt_summary: "Developer work",
  });
  const selected = within(
    screen.getByRole("list", { name: "Agent worktrees" }),
  ).getByRole("button", { name: /Developer work/ });
  expect(selected.getAttribute("aria-pressed")).toBe("true");
  expect(
    screen.queryByRole("button", { name: "Run verification", exact: true }),
  ).toBeNull();
});
