// @vitest-environment jsdom
import { commitHistory } from "./fixtures/commit-history";
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
import { LiveRepositoryActionsView } from "../src/showcase/LiveRepositoryActionsView";
import { localRecord } from "./local-checks-fixture";

const primaryId = "11111111-1111-4111-8111-111111111111";
const secondaryId = "22222222-2222-4222-8222-222222222222";
const readonlyId = "33333333-3333-4333-8333-333333333333";
const mainSha = "a".repeat(40);
const releaseSha = "b".repeat(40);
const time = "2026-10-18T01:00:00.123456789Z";

const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status });

beforeEach(() => {
  localStorage.setItem("gitust_token", "ordinary-checks-session");
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
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("defaults secondary writable repositories to Local Checks and loads both history and automatic status", async () => {
  const fetcher = vi.fn(async (url: string) => {
    if (url.endsWith(`/repos/${secondaryId}/local-check-runs`)) {
      return json({
        version: 1,
        repo_id: secondaryId,
        records: [localRecord()],
      });
    }
    if (url.endsWith(`/repos/${secondaryId}/local-check-automatic-runs`)) {
      return json({
        version: 1,
        repo_id: secondaryId,
        runs: [
          {
            run_id: "44444444-4444-4444-8444-444444444444",
            source_ref: "refs/heads/release",
            expected_commit: releaseSha,
            checks_blob_oid: "f".repeat(40),
            phase: "running",
            reason: null,
            history: "not_attempted",
            created_at: time,
            updated_at: time,
          },
        ],
      });
    }
    return json({});
  });
  vi.stubGlobal("fetch", fetcher);

  render(() => (
    <LiveRepositoryActionsView
      repositoryId={secondaryId}
      role="SiteAdmin"
      isPrimary={false}
      selectionEpoch={1}
      canRunChecks={true}
      canViewLocalChecksAutomatic={true}
      defaultRef="refs/heads/main"
      defaultCommit={mainSha}
    />
  ));

  expect(screen.queryByRole("tab", { name: "Automatic" })).toBeNull();
  const panel = await screen.findByRole("region", {
    name: "Automatic Local Checks",
  });
  await within(panel).findByText("Running");
  expect(within(panel).getByText(/release/)).toBeTruthy();
  await screen.findByRole("list", { name: "Local Checks commands" });
  expect(
    fetcher.mock.calls.some(([url]) =>
      url.endsWith(`/repos/${secondaryId}/local-check-runs`),
    ),
  ).toBe(true);
  expect(
    fetcher.mock.calls.some(([url]) =>
      url.endsWith(`/repos/${secondaryId}/local-check-automatic-runs`),
    ),
  ).toBe(true);
});

it("opens Actions on a secondary writable repository, switches ordinary branches, and posts the selected branch ref and commit", async () => {
  const releaseRecord = {
    ...localRecord("cccc3333-3333-4333-8333-333333333333"),
    source_ref: "refs/heads/release",
    checked_commit: releaseSha,
  };
  let saved = false;
  const fetcher = vi.fn(async (url: string, init?: RequestInit) => {
    if (url.endsWith("/auth/me")) {
      return json({ user_id: "admin", username: "Admin", role: "SiteAdmin" });
    }
    if (url.endsWith("/api/v1/repos")) {
      return json({
        repositories: [
          {
            id: primaryId,
            name: "primary-workbench",
            branch: "main",
            head_commit: mainSha,
            capability: "workbench",
            is_default: true,
            can_run_local_checks: true,
            can_view_local_checks_automatic: true,
          },
          {
            id: secondaryId,
            name: "secondary-writable",
            branch: "main",
            head_commit: mainSha,
            capability: "browse",
            is_default: false,
            can_push: true,
            can_run_local_checks: true,
            can_view_local_checks_automatic: true,
          },
          {
            id: readonlyId,
            name: "secondary-readonly",
            branch: "main",
            head_commit: mainSha,
            capability: "browse",
            is_default: false,
          },
        ],
      });
    }
    if (url.includes(`/repos/${secondaryId}/branches`)) {
      return json({
        branches: [
          {
            name: "main",
            ref_name: "refs/heads/main",
            commit_sha: mainSha,
          },
          {
            name: "release",
            ref_name: "refs/heads/release",
            commit_sha: releaseSha,
          },
        ],
        next_cursor: null,
      });
    }
    if (
      url.endsWith(`/repos/${secondaryId}/local-check-runs`) &&
      init?.method === "POST"
    ) {
      saved = true;
      return json({
        version: 1,
        repo_id: secondaryId,
        status: "failed",
        reason: null,
        cleanup: "confirmed",
        history: { status: "saved", reason: null },
        record: releaseRecord,
      });
    }
    if (url.endsWith(`/repos/${secondaryId}/local-check-runs`)) {
      return json({
        version: 1,
        repo_id: secondaryId,
        records: saved ? [releaseRecord] : [],
      });
    }
    if (url.endsWith(`/repos/${secondaryId}/local-check-automatic-runs`)) {
      return json({
        version: 1,
        repo_id: secondaryId,
        runs: [],
      });
    }
    if (url.includes("/commits?")) return json(commitHistory(mainSha));
    if (url.includes("/tree?")) return json({ commit_sha: mainSha, entries: [] });
    return json([]);
  });
  vi.stubGlobal("fetch", fetcher);

  render(() => <WorkbenchApp />);

  // Read-only secondary repository without checks capabilities hides Actions
  await fireEvent.click(
    await screen.findByRole("button", {
      name: "Open repository secondary-readonly",
    }),
  );
  const nav = screen.getByRole("navigation", { name: "Repository views" });
  expect(within(nav).queryByRole("button", { name: "Actions" })).toBeNull();
  await fireEvent.click(
    screen.getByRole("button", { name: "Back to repositories" }),
  );

  // Secondary writable repository with Local Checks capabilities exposes Actions
  await fireEvent.click(
    await screen.findByRole("button", {
      name: "Open repository secondary-writable",
    }),
  );
  await fireEvent.click(
    within(
      screen.getByRole("navigation", { name: "Repository views" }),
    ).getByRole("button", { name: "Actions", exact: true }),
  );

  await screen.findByText("No recorded Local Checks");
  expect(
    screen.getByText(`Default: main · ${mainSha.slice(0, 7)}`),
  ).toBeTruthy();

  // Select the 'release' ordinary branch
  const branchTrigger = await screen.findByLabelText("Select branch");
  await screen.findByText(
    `Snapshot ${mainSha.slice(0, 8)}. Select a branch to change it.`,
  );
  await fireEvent.keyDown(branchTrigger, { key: "ArrowDown" });
  await fireEvent.click(
    await screen.findByRole("option", {
      name: `release · ${releaseSha.slice(0, 8)}`,
    }),
  );

  await waitFor(() =>
    expect(
      screen.getByText(`Branch: release · ${releaseSha.slice(0, 7)}`),
    ).toBeTruthy(),
  );

  // Run Checks on the selected 'release' branch
  await fireEvent.click(
    screen.getByRole("button", { name: "Run Checks", exact: true }),
  );

  await screen.findByText("Checks failed. Result saved.");
  const postCalls = fetcher.mock.calls.filter(
    ([, init]) => init?.method === "POST",
  );
  expect(postCalls).toHaveLength(1);
  expect(postCalls[0]?.[0]).toBe(
    `/api/v1/repos/${secondaryId}/local-check-runs`,
  );
  expect(JSON.parse(postCalls[0]?.[1]?.body as string)).toEqual({
    expected_source_ref: "refs/heads/release",
    expected_commit: releaseSha,
  });
});
