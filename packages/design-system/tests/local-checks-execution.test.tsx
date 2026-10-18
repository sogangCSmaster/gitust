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
import { LiveRepositoryActionsView } from "../src/showcase/LiveRepositoryActionsView";
import { actionsPage } from "./actions-page-fixture";
import { localRecord, localRepoId } from "./local-checks-fixture";

beforeEach(() => localStorage.setItem("gitust_token", "local-session"));
afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.unstubAllGlobals();
});

it("runs only after an explicit click, preserves listed results while pending and shows the saved Check failure", async () => {
  const previous = localRecord();
  const completed = {
    ...localRecord("cccc1111-1111-4111-8111-111111111111"),
    source_ref: "refs/heads/main",
  };
  let finish!: (response: Response) => void;
  let saved = false;
  const fetcher = vi.fn(async (url: string, init?: RequestInit) => {
    if (init?.method === "POST")
      return new Promise<Response>((resolve) => {
        finish = resolve;
      });
    return new Response(
      JSON.stringify(
        url.endsWith("/local-check-runs")
          ? {
              version: 1,
              repo_id: localRepoId,
              records: saved ? [completed, previous] : [previous],
            }
          : { ...actionsPage(), repo_id: localRepoId },
      ),
    );
  });
  vi.stubGlobal("fetch", fetcher);
  const props = {
    repositoryId: localRepoId,
    role: "SiteAdmin",
    isPrimary: true,
    selectionEpoch: 1,
    canRunChecks: true,
    defaultRef: "refs/heads/main",
    defaultCommit: "e".repeat(40),
  };
  render(() => <LiveRepositoryActionsView {...props} />);
  await fireEvent.click(
    await screen.findByRole("tab", { name: "Local Checks" }),
  );
  await screen.findByRole("list", { name: "Local Checks commands" });
  expect(
    fetcher.mock.calls.filter(([, init]) => init?.method === "POST"),
  ).toHaveLength(0);
  await fireEvent.click(
    screen.getByRole("button", { name: "Run Checks", exact: true }),
  );
  await waitFor(() =>
    expect(
      fetcher.mock.calls.filter(([, init]) => init?.method === "POST"),
    ).toHaveLength(1),
  );
  const [, request] = fetcher.mock.calls.find(
    ([, init]) => init?.method === "POST",
  )!;
  expect(JSON.parse(request!.body as string)).toEqual({
    expected_source_ref: "refs/heads/main",
    expected_commit: "e".repeat(40),
  });
  expect(
    screen.getByRole("list", { name: "Local Checks commands" }),
  ).toBeTruthy();
  expect(
    screen.getByRole("button", { name: "Refresh", exact: true }),
  ).toHaveProperty("disabled", true);
  saved = true;
  finish(
    new Response(
      JSON.stringify({
        version: 1,
        repo_id: localRepoId,
        status: "failed",
        reason: null,
        cleanup: "confirmed",
        history: { status: "saved", reason: null },
        record: completed,
      }),
    ),
  );
  await screen.findByText("Checks failed. Result saved.");
  expect(screen.getAllByText("Required check")).toHaveLength(2);
  expect(screen.getAllByText("Skipped")).toHaveLength(1);
  expect(
    fetcher.mock.calls.filter(([, init]) => init?.method === "POST"),
  ).toHaveLength(1);
});

it("keeps old-server viewing and refreshes a stale default before another explicit run, preserving access to the current execution result", async () => {
  const { createSignal } = await import("solid-js");
  const [hint, setHint] = createSignal(false);
  const [head, setHead] = createSignal("e".repeat(40));
  let posts = 0;
  const fetcher = vi.fn(async (url: string, init?: RequestInit) => {
    if (init?.method === "POST") {
      posts++;
      if (posts === 1)
        return new Response(
          JSON.stringify({ code: "local_checks_target_changed" }),
          { status: 409 },
        );
      return new Response(
        JSON.stringify({
          version: 1,
          repo_id: localRepoId,
          status: "failed",
          reason: "history_unconfirmed",
          cleanup: "confirmed",
          history: { status: "failed", reason: "storage_unavailable" },
          record: {
            ...localRecord("cccc2222-2222-4222-8222-222222222222"),
            source_ref: "refs/heads/main",
            checked_commit: head(),
          },
        }),
        { status: 503 },
      );
    }
    return new Response(
      JSON.stringify(
        url.endsWith("/local-check-runs")
          ? {
              version: 1,
              repo_id: localRepoId,
              records: head().startsWith("d") ? [localRecord()] : [],
            }
          : { ...actionsPage(), repo_id: localRepoId },
      ),
    );
  });
  vi.stubGlobal("fetch", fetcher);
  const refresh = vi.fn(async () => {
    setHead("d".repeat(40));
    return true;
  });
  render(() => (
    <LiveRepositoryActionsView
      repositoryId={localRepoId}
      role="SiteAdmin"
      isPrimary
      selectionEpoch={1}
      canRunChecks={hint()}
      defaultRef="refs/heads/main"
      defaultCommit={head()}
      refreshRepository={refresh}
    />
  ));
  await fireEvent.click(
    await screen.findByRole("tab", { name: "Local Checks" }),
  );
  await screen.findByText("No recorded Local Checks");
  expect(
    screen.queryByRole("button", { name: "Run Checks", exact: true }),
  ).toBeNull();
  setHint(true);
  await fireEvent.click(
    screen.getByRole("button", { name: "Run Checks", exact: true }),
  );
  await screen.findByText(
    "The default branch changed. Refresh before running again.",
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Refresh", exact: true }),
  );
  await waitFor(() => expect(refresh).toHaveBeenCalledTimes(1));
  expect(posts).toBe(1);
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Run Checks", exact: true }),
    ).toHaveProperty("disabled", false),
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Run Checks", exact: true }),
  );
  await screen.findByText(/Saving this result could not be confirmed/);
  await screen.findByRole("list", { name: "Local Checks commands" });
  expect(screen.queryByText("No recorded Local Checks")).toBeNull();
  await fireEvent.click(
    within(screen.getByRole("list", { name: "Recorded runs" })).getByRole(
      "button",
    ),
  );
  expect(
    within(
      screen.getByRole("region", { name: "Local Checks result" }),
    ).getByText("topic/local-checks"),
  ).toBeTruthy();
  await fireEvent.click(
    screen.getByRole("button", { name: "View execution result", exact: true }),
  );
  expect(
    within(
      screen.getByRole("region", { name: "Local Checks result" }),
    ).getByText("main"),
  ).toBeTruthy();
  expect(posts).toBe(2);
  expect(
    JSON.parse(
      fetcher.mock.calls.filter(([, init]) => init?.method === "POST")[1]![1]!
        .body as string,
    ).expected_commit,
  ).toBe("d".repeat(40));
});
