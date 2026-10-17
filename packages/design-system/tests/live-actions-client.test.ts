import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { parseActionsPage } from "../src/showcase/live-actions-page";
import {
  createLiveActionsClient,
  type LiveActionsState,
} from "../src/showcase/live-actions-client";
import {
  actionsPage,
  actionsRepoId,
  pageCursor,
  secondWorktreeId,
} from "./actions-page-fixture";
import { automaticJob, worktreeId } from "./run-history-fixture";

let token: string | null;
let repo: string | undefined;
let allowed: boolean;
beforeEach(() => {
  token = "actions-a";
  repo = actionsRepoId;
  allowed = true;
  vi.stubGlobal("window", { localStorage: { getItem: () => token } });
});
afterEach(() => vi.unstubAllGlobals());
const client = () => {
  let state: LiveActionsState = { status: "idle", page: null };
  return {
    api: createLiveActionsClient(
      (next) => {
        state = next;
      },
      { repositoryId: () => repo, canRead: () => allowed },
    ),
    state: () => state,
  };
};
const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
};

it("preserves the long packed-ref branch recovered by the real server", () => {
  const branch = `agent/${Array.from({ length: 24 }, (_, index) => `segment${String(index).padStart(3, "0")}${"x".repeat(40)}`).join("/")}`;
  expect(new TextEncoder().encode(branch)).toHaveLength(1229);
  const page = {
    ...actionsPage(),
    worktrees: [{ ...actionsPage().worktrees[0]!, branch, runs: [] }],
  };
  expect(parseActionsPage(page, actionsRepoId).worktrees[0]?.branch).toBe(
    branch,
  );
});

it("bounds the combined branch and base metadata by UTF-8 bytes without truncation", () => {
  const group = actionsPage().worktrees[0]!;
  const branch = `agent/${"가".repeat(10920)}`;
  const page = (base_branch: string) => ({
    ...actionsPage(),
    worktrees: [{ ...group, branch, base_branch }],
  });
  expect(new TextEncoder().encode(branch + "ab")).toHaveLength(32 * 1024);
  expect(parseActionsPage(page("ab"), actionsRepoId).worktrees[0]?.branch).toBe(
    branch,
  );
  expect(() => parseActionsPage(page("abc"), actionsRepoId)).toThrow();
  const individuallyValid = {
    ...group,
    branch: `agent/${"가".repeat(6000)}`,
    base_branch: "가".repeat(6000),
  };
  expect(() =>
    parseActionsPage(
      { ...actionsPage(), worktrees: [individuallyValid] },
      actionsRepoId,
    ),
  ).toThrow();
});

it("loads valid pages above 256 KiB while rejecting bodies above 2 MiB", async () => {
  const branch =
    `agent/${Array.from({ length: 650 }, () => "s".repeat(50)).join("/")}`.slice(
      0,
      32 * 1024 - 4,
    );
  const page = {
    ...actionsPage(),
    scanned_worktrees: 10,
    worktrees: Array.from({ length: 10 }, (_, index) => ({
      ...actionsPage().worktrees[0]!,
      worktree_id: `${String(index + 1).padStart(8, "0")}-1111-4111-8111-111111111111`,
      branch,
      runs: [],
    })),
  };
  const body = JSON.stringify(page);
  expect(new TextEncoder().encode(body).length).toBeGreaterThan(256 * 1024);
  expect(new TextEncoder().encode(body).length).toBeLessThan(2 * 1024 * 1024);
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockResolvedValueOnce(new Response(body))
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify(actionsPage()).padEnd(2 * 1024 * 1024 + 1, " "),
        ),
      ),
  );
  const loaded = client();
  await loaded.api.load(actionsRepoId);
  expect(loaded.state().page?.worktrees).toHaveLength(10);
  expect(
    loaded.state().page?.worktrees.every((group) => group.branch === branch),
  ).toBe(true);
  await loaded.api.load(actionsRepoId);
  expect(loaded.state()).toMatchObject({
    status: "error",
    page: null,
    error: "Automatic Actions page exceeds the response limit",
  });
});

it("validates bounded repository-bound pages, WT ordering, scan count and forward cursors", () => {
  expect(
    parseActionsPage(actionsPage(), actionsRepoId).worktrees[0]?.runs[0]
      ?.worktreeId,
  ).toBe(worktreeId);
  const empty = {
    ...actionsPage(),
    worktrees: [],
    scanned_worktrees: 10,
    next_cursor: pageCursor(),
  };
  expect(parseActionsPage(empty, actionsRepoId).nextCursor).toBe(pageCursor());
  const group = actionsPage().worktrees[0]!;
  const invalid = [
    { ...actionsPage(), repo_id: secondWorktreeId },
    { ...actionsPage(), unknown: true },
    { ...actionsPage(), order: "latest" },
    { ...actionsPage(), scanned_worktrees: 0 },
    { ...actionsPage(), scanned_worktrees: 21 },
    { ...actionsPage(), worktrees: [group, group], scanned_worktrees: 2 },
    {
      ...actionsPage(),
      worktrees: [{ ...group, worktree_id: secondWorktreeId }, group],
      scanned_worktrees: 2,
    },
    { ...actionsPage(), worktrees: [{ ...group, branch: "bad\nbranch" }] },
    { ...actionsPage(), next_cursor: pageCursor() },
    { ...empty, next_cursor: `v1.${secondWorktreeId}.${worktreeId}` },
  ];
  for (const page of invalid)
    expect(() => parseActionsPage(page, actionsRepoId)).toThrow();
  expect(() => parseActionsPage(empty, actionsRepoId, pageCursor())).toThrow();
  expect(
    parseActionsPage(
      {
        ...actionsPage(),
        worktrees: [{ ...group, base_branch: "", runs: [] }],
      },
      actionsRepoId,
    ).worktrees[0]?.baseBranch,
  ).toBe("");
});

it("preserves exact per-WT job validation and nanosecond created-at sorting with RunId ties", () => {
  const first = automaticJob("abcdefab-cdef-4abc-8def-abcdefabcdef");
  const second = {
    ...automaticJob(),
    phase: "completed",
    created_at: "2026-10-17T02:00:00.000000001Z",
    changed_at: "2026-10-17T02:00:00.000000001Z",
  };
  const group = actionsPage().worktrees[0]!;
  const page = {
    ...actionsPage(),
    worktrees: [{ ...group, runs: [second, first] }],
  };
  expect(parseActionsPage(page, actionsRepoId).worktrees[0]?.runs).toHaveLength(
    2,
  );
  expect(() =>
    parseActionsPage(
      { ...page, worktrees: [{ ...group, runs: [first, second] }] },
      actionsRepoId,
    ),
  ).toThrow();
  expect(() =>
    parseActionsPage(
      {
        ...page,
        worktrees: [{ ...group, runs: [{ ...first, reason: "invented" }] }],
      },
      actionsRepoId,
    ),
  ).toThrow();
  const maximum = Array.from({ length: 20 }, (_, index) => ({
    ...group,
    worktree_id: `${(index + 1).toString(16).padStart(8, "0")}-1111-4111-8111-111111111111`,
    runs: [],
  }));
  expect(
    parseActionsPage(
      { ...actionsPage(), scanned_worktrees: 20, worktrees: maximum },
      actionsRepoId,
      null,
      20,
    ).worktrees,
  ).toHaveLength(20);
  const phases = [
    "prepared",
    "queued",
    "running",
    "completed",
    "failed",
    "timed_out",
    "cancelled",
    "superseded",
  ];
  const jobs = phases.map((phase, index) => ({
    ...automaticJob(
      `${(8 - index).toString(16).padStart(8, "0")}-1111-4111-8111-111111111111`,
    ),
    phase,
    reason: index <= 3 ? null : "service_stopped",
  }));
  const full = maximum.map((entry) => ({ ...entry, runs: jobs }));
  expect(
    parseActionsPage(
      { ...actionsPage(), scanned_worktrees: 20, worktrees: full },
      actionsRepoId,
      null,
      20,
    ).worktrees.flatMap((entry) => entry.runs),
  ).toHaveLength(160);
  const tied = [
    {
      ...automaticJob("abcdefab-cdef-4abc-8def-abcdefabcdef"),
      phase: "completed",
    },
    { ...automaticJob(), phase: "completed" },
  ];
  expect(
    parseActionsPage(
      { ...actionsPage(), worktrees: [{ ...group, runs: tied }] },
      actionsRepoId,
    ).worktrees[0]?.runs,
  ).toHaveLength(2);
  expect(() =>
    parseActionsPage(
      {
        ...actionsPage(),
        worktrees: [{ ...group, runs: [...tied].reverse() }],
      },
      actionsRepoId,
    ),
  ).toThrow();
  expect(() =>
    parseActionsPage(
      {
        ...actionsPage(),
        scanned_worktrees: 20,
        worktrees: [...maximum, group],
      },
      actionsRepoId,
      null,
      20,
    ),
  ).toThrow();
});

it("clears old pages on denied, malformed and oversized responses without demo data", async () => {
  const loaded = client();
  for (const response of [
    new Response("{}", { status: 401 }),
    new Response("{}", { status: 403 }),
    new Response("{}", { status: 404 }),
    new Response("bad"),
    new Response(" ".repeat(2 * 1024 * 1024 + 1)),
    new Response(
      JSON.stringify({ ...actionsPage(), repo_id: secondWorktreeId }),
    ),
  ]) {
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValueOnce(new Response(JSON.stringify(actionsPage())))
        .mockResolvedValueOnce(response),
    );
    await loaded.api.load(actionsRepoId);
    expect(loaded.state().page?.worktrees).toHaveLength(1);
    await loaded.api.load(actionsRepoId);
    expect(loaded.state()).toMatchObject({ status: "error", page: null });
  }
});

it("aborts replaced pages and ignores stale fetch and body delivery after repo, token or close changes", async () => {
  const old = deferred<Response>();
  const fetcher = vi
    .fn()
    .mockReturnValueOnce(old.promise)
    .mockResolvedValueOnce(new Response(JSON.stringify(actionsPage())));
  vi.stubGlobal("fetch", fetcher);
  const loaded = client();
  const first = loaded.api.load(actionsRepoId);
  const next = loaded.api.load(actionsRepoId);
  expect(fetcher.mock.calls[0]?.[1].signal.aborted).toBe(true);
  await next;
  old.resolve(new Response("invalid"));
  await first;
  expect(loaded.state().status).toBe("ready");
  for (const change of [
    () => {
      token = "actions-b";
    },
    () => {
      repo = secondWorktreeId;
    },
    () => {
      allowed = false;
    },
    () => {
      loaded.api.clear();
    },
  ]) {
    token = "actions-a";
    repo = actionsRepoId;
    allowed = true;
    const body = deferred<string>();
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({ ok: true, text: () => body.promise }),
    );
    const pending = loaded.api.load(actionsRepoId);
    await Promise.resolve();
    await Promise.resolve();
    change();
    body.resolve(JSON.stringify(actionsPage()));
    await pending;
    expect(loaded.state()).toMatchObject({ status: "idle", page: null });
  }
});
