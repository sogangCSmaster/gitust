import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { AgentSandboxTask } from "../src/patterns/AgentSwarmView";
import { createLiveSwarmClient } from "../src/showcase/live-swarm-client";
const input = {
  repoId: "11111111-1111-4111-8111-111111111111",
  agent: "CodexSwarm" as const,
  branch: "agent/review/create-form",
  baseBranch: "trunk",
  promptSummary: "Build the creation form",
};
const created = {
  id: "22222222-2222-4222-8222-222222222222",
  repo_id: input.repoId,
  agent: input.agent,
  branch: input.branch,
  base_branch: input.baseBranch,
  prompt_summary: input.promptSummary,
  base_commit: "a".repeat(40),
  materialize_micros: 50,
  status: "pending",
  last_sandbox_duration_ms: 0,
  tests_passed: 0,
  tests_total: 0,
  files_changed: 0,
  additions: 0,
  deletions: 0,
  github_synced: false,
  created_at: "2026-10-17T00:00:00Z",
};
const response = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status });
const deferred = () => {
  let resolve!: (value: Response) => void;
  const promise = new Promise<Response>((done) => {
    resolve = done;
  });
  return { promise, resolve };
};
let token: string | null;
let tasks: AgentSandboxTask[];
let fetcher: ReturnType<typeof vi.fn>;
let client: ReturnType<typeof createLiveSwarmClient>;
beforeEach(() => {
  token = "create-session";
  tasks = [];
  fetcher = vi.fn();
  vi.stubGlobal("window", { localStorage: { getItem: () => token } });
  vi.stubGlobal("fetch", fetcher);
  client = createLiveSwarmClient({
    getSwarmTasks: () => tasks,
    setSwarmTasks: (update) => {
      tasks = typeof update === "function" ? update(tasks) : update;
    },
    showQuickFeedback: vi.fn(),
  });
});
afterEach(() => vi.unstubAllGlobals());
it("uses the reviewed repository, branch, base and task name and returns the created row identity", async () => {
  const request = deferred();
  fetcher.mockReturnValueOnce(request.promise);
  const pending = client.spawnLiveWorktree(input);
  expect(tasks[0]).toMatchObject({
    branch: input.branch,
    baseBranch: "trunk",
    promptSummary: input.promptSummary,
    agent: "Codex Swarm",
  });
  const id = tasks[0]!.id;
  expect(JSON.parse(fetcher.mock.calls[0][1].body)).toEqual({
    repo_id: input.repoId,
    agent: input.agent,
    branch: input.branch,
    base_branch: "trunk",
    prompt_summary: input.promptSummary,
  });
  request.resolve(response(created));
  expect(await pending).toMatchObject({
    kind: "created",
    taskId: id,
    worktreeId: created.id,
  });
  expect(tasks).toHaveLength(1);
  expect(tasks[0]?.liveId).toBe(created.id);
});
it.each([409, 404, 429])(
  "returns a visible rejection for HTTP %i and removes only the pending row",
  async (status) => {
    fetcher.mockResolvedValueOnce(response({ error: "Cannot create" }, status));
    expect(await client.spawnLiveWorktree(input)).toMatchObject({
      kind: "error",
      error: expect.stringContaining(`HTTP ${status}`),
    });
    expect(tasks).toEqual([]);
  },
);
it("does not duplicate an in-flight branch and keeps the exact branch when retrying a network failure", async () => {
  const request = deferred();
  fetcher.mockReturnValueOnce(request.promise);
  const first = client.spawnLiveWorktree(input);
  expect(await client.spawnLiveWorktree(input)).toMatchObject({
    kind: "error",
    error: expect.stringContaining("already"),
  });
  expect(fetcher).toHaveBeenCalledOnce();
  request.resolve(response({ error: "busy" }, 409));
  await first;
  fetcher.mockRejectedValueOnce(new TypeError("offline"));
  expect(await client.spawnLiveWorktree(input)).toMatchObject({
    kind: "error",
    uncertain: true,
  });
  expect(JSON.parse(fetcher.mock.calls[1][1].body).branch).toBe(input.branch);
});
it("drops a completed creation after the session epoch changes and rejects invalid input before any request", async () => {
  const request = deferred();
  fetcher.mockReturnValueOnce(request.promise);
  const first = client.spawnLiveWorktree(input);
  client.resetSwarmEpoch();
  token = "replacement";
  tasks = [];
  request.resolve(response(created));
  expect(await first).toEqual({ kind: "stale" });
  expect(tasks).toEqual([]);
  expect(
    await client.spawnLiveWorktree({ ...input, branch: "main" }),
  ).toMatchObject({ kind: "error" });
  expect(fetcher).toHaveBeenCalledOnce();
});
it("does not accept a successful response for a different creation target", async () => {
  fetcher.mockResolvedValueOnce(response({ ...created, base_branch: "main" }));
  expect(await client.spawnLiveWorktree(input)).toMatchObject({
    kind: "error",
    uncertain: true,
  });
  expect(tasks).toEqual([]);
});
it("does not confirm a creation response without a valid worktree UUID", async () => {
  fetcher.mockResolvedValueOnce(response({ ...created, id: "not-a-uuid" }));
  expect(await client.spawnLiveWorktree(input)).toMatchObject({
    kind: "error",
    uncertain: true,
  });
  expect(tasks).toEqual([]);
});

it.each(["verified", "merged", "Pending", undefined, 42])(
  "rejects a creation response with a non-pending wire status: %j",
  async (status) => {
    fetcher.mockResolvedValueOnce(response({ ...created, status }));
    expect(await client.spawnLiveWorktree(input)).toMatchObject({
      kind: "error",
      uncertain: true,
    });
    expect(tasks).toEqual([]);
  },
);
