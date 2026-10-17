// @vitest-environment jsdom
import { commitHistory } from "./fixtures/commit-history";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@solidjs/testing-library";
import { WorkbenchApp } from "../src/showcase/WorkbenchApp";
import { automaticJob, worktreeId } from "./run-history-fixture";
import { shortcutManager } from "../src/keyboard/shortcut-manager";

const repoId = "99999999-9999-4999-8999-999999999999";
let fetcher: ReturnType<typeof vi.fn>;
beforeEach(() => {
  localStorage.setItem("gitust_token", "actions-session");
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
                      id: repoId,
                      name: "real-actions-repo",
                      branch: "main",
                      head_commit: "c".repeat(40),
                      capability: "workbench",
                      is_default: true,
                    },
                  ],
                }
              : url.includes("/repo/automatic-runs")
                ? {
                    repo_id: repoId,
                    order: "worktree_id_asc",
                    scanned_worktrees: 1,
                    worktrees: [
                      {
                        worktree_id: worktreeId,
                        branch: "agent/real-actions",
                        base_branch: "main",
                        runs: [automaticJob()],
                      },
                    ],
                    next_cursor: null,
                  }
                : url.includes("/commits?")
                  ? commitHistory("c".repeat(40))
                  : url.includes("/tree?")
                    ? { commit_sha: "c".repeat(40), entries: [] }
                    : [],
        ),
      ),
  );
  vi.stubGlobal("fetch", fetcher);
});
afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("opens real Actions within the selected repository without sample pipeline data", async () => {
  render(() => <WorkbenchApp />);
  await fireEvent.click(
    await screen.findByRole("button", {
      name: "Open repository real-actions-repo",
    }),
  );
  await fireEvent.click(
    within(
      screen.getByRole("navigation", { name: "Repository views" }),
    ).getByRole("button", { name: "Actions", exact: true }),
  );
  await screen.findByText("agent/real-actions");
  expect(screen.queryByText("ci-enterprise-release.yml")).toBeNull();
  expect(screen.queryByRole("button", { name: /Replay|Re-run/ })).toBeNull();
  expect(
    fetcher.mock.calls.filter(([url]) => url.includes("/repo/automatic-runs")),
  ).toHaveLength(1);
  const requests = fetcher.mock.calls.length;
  shortcutManager.executeById("actions.rerun");
  expect(fetcher.mock.calls).toHaveLength(requests);
  expect(screen.queryByText(/Demo pipeline reset/)).toBeNull();
  await fireEvent.click(
    screen.getByRole("button", { name: "Back to repositories" }),
  );
  expect(screen.queryByText("agent/real-actions")).toBeNull();
});
