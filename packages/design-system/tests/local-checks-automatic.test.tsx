// @vitest-environment jsdom
import { createSignal } from "solid-js";
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
import { LiveLocalChecksView } from "../src/showcase/LiveLocalChecksView";
import { actionsPage } from "./actions-page-fixture";
import { localPage, localRecord, localRepoId } from "./local-checks-fixture";

const time = "2026-10-18T01:00:00.123456789Z";
const automatic = (runId: string, phase = "queued") => ({
  run_id: runId,
  source_ref: "refs/heads/main",
  expected_commit: "d".repeat(40),
  checks_blob_oid: "f".repeat(40),
  phase,
  reason: null as string | null,
  history: "not_attempted",
  created_at: time,
  updated_at: time,
});
const page = (runs: unknown[]) => ({ version: 1, repo_id: localRepoId, runs });
const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status });
beforeEach(() => localStorage.setItem("gitust_token", "local-session"));
afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.unstubAllGlobals();
});

it("keeps old-server history, then shows compact automatic status and opens only its exact retained result", async () => {
  const [hint, setHint] = createSignal(false);
  const [context, setContext] = createSignal({
    id: localRepoId,
    role: "SiteAdmin",
    epoch: 1,
  });
  const done = {
    ...localRecord("cccc1111-1111-4111-8111-111111111111"),
    source_ref: "refs/heads/main",
    checked_commit: "d".repeat(40),
  };
  const runs = [
    automatic("11111111-1111-4111-8111-111111111111", "running"),
    automatic("22222222-2222-4222-8222-222222222222"),
    {
      ...automatic(done.result.run_id, "failed"),
      history: "saved",
      reason: "command_failed",
    },
    {
      ...automatic(localRecord().result.run_id, "failed"),
      source_ref: localRecord().source_ref,
      expected_commit: localRecord().checked_commit,
      checks_blob_oid: "a".repeat(40),
      history: "failed",
      reason: "history_unconfirmed",
    },
  ];
  let statusRequests = 0;
  const fetcher = vi.fn(async (url: string, _init?: RequestInit) => {
    if (url.endsWith("/local-check-automatic-runs")) {
      statusRequests++;
      if (statusRequests === 2)
        return json(
          page([
            {
              ...runs[0],
              phase: ["rejected"],
              reason: "policy_missing",
              checks_blob_oid: null,
            },
          ]),
        );
      if (statusRequests === 3)
        return json({ code: "local_checks_automatic_forbidden" }, 403);
      return json(page(runs));
    }
    return json(
      url.endsWith("/local-check-runs")
        ? { ...localPage(), records: [localRecord(), done] }
        : { ...actionsPage(), repo_id: localRepoId },
    );
  });
  vi.stubGlobal("fetch", fetcher);
  const props = {
    get repositoryId() {
      return context().id;
    },
    get role() {
      return context().role;
    },
    isPrimary: true,
    get selectionEpoch() {
      return context().epoch;
    },
    get canViewLocalChecksAutomatic() {
      return hint();
    },
    refreshRepository: async () => {
      setHint(true);
      return true;
    },
  };
  render(() => <LiveRepositoryActionsView {...props} />);
  expect(statusRequests).toBe(0);
  await fireEvent.click(
    await screen.findByRole("tab", { name: "Local Checks" }),
  );
  await screen.findByRole("list", { name: "Local Checks commands" });
  expect(statusRequests).toBe(0);
  await fireEvent.click(
    screen.getByRole("button", { name: "Refresh", exact: true }),
  );
  const panel = await screen.findByRole("region", {
    name: "Automatic Local Checks",
  });
  await within(panel).findByText("Running");
  expect(statusRequests).toBe(1);
  expect(within(panel).getByText("Queued")).toBeTruthy();
  expect(within(panel).queryByText("Saving not confirmed")).toBeNull();
  expect(
    within(
      screen.getByRole("region", { name: "Local Checks result" }),
    ).getByText("topic/local-checks"),
  ).toBeTruthy();
  await fireEvent.click(
    await within(panel).findByRole("button", {
      name: "View result",
      exact: true,
    }),
  );
  expect(
    within(
      screen.getByRole("region", { name: "Local Checks result" }),
    ).getByText("main"),
  ).toBeTruthy();
  await fireEvent.click(
    within(panel).getByRole("button", { name: "Earlier attempts (1)" }),
  );
  await within(panel).findByText("Saving not confirmed");
  expect(
    within(panel).getByText("Output is not in loaded history."),
  ).toBeTruthy();
  expect(
    within(panel).getAllByRole("button", { name: "View result", exact: true }),
  ).toHaveLength(1);
  setContext({ ...context() });
  await vi.dynamicImportSettled();
  expect(statusRequests).toBe(1);
  await fireEvent.click(
    screen.getByRole("button", { name: "Refresh", exact: true }),
  );
  await within(panel).findByText(
    "Unable to load automatic Local Checks. Refresh to try again.",
  );
  expect(
    screen.getByRole("list", { name: "Local Checks commands" }),
  ).toBeTruthy();
  expect(
    within(
      screen.getByRole("region", { name: "Local Checks result" }),
    ).getByText("main"),
  ).toBeTruthy();
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Refresh", exact: true }),
    ).toHaveProperty("disabled", false),
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Refresh", exact: true }),
  );
  await within(panel).findByText(
    "Automatic Local Checks access was denied. Check your session and repository access.",
  );
  expect(
    screen.queryByRole("list", { name: "Local Checks commands" }),
  ).toBeNull();
  expect(
    fetcher.mock.calls.every(
      ([, init]) =>
        !(init as RequestInit | undefined)?.method ||
        (init as RequestInit).method === "GET",
    ),
  ).toBe(true);
});

it("ignores an old automatic body across repository selection ABA and clears diagnostics on a session change", async () => {
  const [epoch, setEpoch] = createSignal(1);
  let release!: (value: ReadableStreamReadResult<Uint8Array>) => void;
  let statusRequests = 0;
  const reader = {
    read: vi
      .fn()
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            release = resolve;
          }),
      )
      .mockResolvedValue({ done: true }),
    cancel: vi.fn().mockResolvedValue(undefined),
  };
  const fetcher = vi.fn(async (url: string, _init?: RequestInit) => {
    if (url.endsWith("/local-check-automatic-runs")) {
      statusRequests++;
      if (statusRequests === 1)
        return {
          ok: true,
          status: 200,
          headers: new Headers(),
          body: { getReader: () => reader },
        } as unknown as Response;
      return json(page([]));
    }
    return json(localPage());
  });
  vi.stubGlobal("fetch", fetcher);
  const props = {
    repositoryId: localRepoId,
    role: "SiteAdmin",
    isPrimary: true,
    canViewLocalChecksAutomatic: true,
    get selectionEpoch() {
      return epoch();
    },
  };
  render(() => <LiveLocalChecksView {...props} />);
  const panel = await screen.findByRole("region", {
    name: "Automatic Local Checks",
  });
  await waitFor(() => expect(reader.read).toHaveBeenCalledOnce());
  setEpoch(2);
  setEpoch(3);
  await within(panel).findByText("No automatic attempts recorded.");
  release({
    done: false,
    value: new TextEncoder().encode(
      JSON.stringify(
        page([automatic("44444444-4444-4444-8444-444444444444", "running")]),
      ),
    ),
  });
  await waitFor(() => expect(reader.cancel).toHaveBeenCalled());
  expect(within(panel).queryByText("Running")).toBeNull();
  await screen.findByRole("list", { name: "Local Checks commands" });
  const requests = fetcher.mock.calls.length;
  localStorage.setItem("gitust_token", "replacement-session");
  window.dispatchEvent(new StorageEvent("storage", { key: "gitust_token" }));
  expect(
    screen.queryByRole("list", { name: "Local Checks commands" }),
  ).toBeNull();
  expect(
    within(panel).getByText("Refresh to load automatic status."),
  ).toBeTruthy();
  expect(fetcher).toHaveBeenCalledTimes(requests);
});
