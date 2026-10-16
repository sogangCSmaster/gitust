// @vitest-environment node
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createLiveSwarmClient, mapLiveWorktreeToTask } from "../src/showcase/live-swarm-client";
import type { LiveWorktreeRecord } from "../src/showcase/live-swarm-client";
import type { AgentSandboxTask } from "../src/patterns/AgentSwarmView";

const record = (id = "worktree-one", overrides: Partial<LiveWorktreeRecord> = {}): LiveWorktreeRecord => ({
  id, repo_id: "repository", agent: "CodexSwarm", branch: `agent/${id}`,
  base_branch: "main", base_commit: "approved-sha", prompt_summary: "Test change",
  materialize_micros: 50, status: "Merged", last_sandbox_duration_ms: 0,
  tests_passed: 1, tests_total: 1, files_changed: 1, additions: 1, deletions: 0,
  verified_commit: overrides.status === "Verified" ? "a".repeat(40) : null,
  github_synced: false, created_at: "2026-10-16T00:00:00Z", ...overrides,
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

const response = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
let token: string | null;
let tasks: AgentSandboxTask[];
let feedback: ReturnType<typeof vi.fn>;
let fetchMock: ReturnType<typeof vi.fn>;
let client: ReturnType<typeof createLiveSwarmClient>;

beforeEach(() => {
  token = "first-session";
  tasks = [mapLiveWorktreeToTask(record(), 1)];
  feedback = vi.fn();
  fetchMock = vi.fn();
  vi.stubGlobal("window", { localStorage: { getItem: () => token } });
  vi.stubGlobal("fetch", fetchMock);
  client = createLiveSwarmClient({
    getSwarmTasks: () => tasks,
    setSwarmTasks: (update) => { tasks = typeof update === "function" ? update(tasks) : update; },
    showQuickFeedback: feedback,
  });
});
afterEach(() => vi.unstubAllGlobals());

describe("explicit approved worktree publication", () => {
  it("shows immediate publishing feedback and confirms sync only after the server succeeds", async () => {
    const request = deferred<Response>();
    fetchMock.mockReturnValueOnce(request.promise);
    const pending = client.publishLiveWorktree(1);
    expect(tasks[0]).toMatchObject({ sandboxStatus: "merged", githubSynced: false, operation: "publishing" });
    expect(fetchMock).toHaveBeenCalledWith("/api/v1/swarm/worktrees/worktree-one/publish", {
      method: "POST", headers: { Authorization: "Bearer first-session" },
    });
    await client.publishLiveWorktree(1);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    request.resolve(response(record("worktree-one", { github_synced: true })));
    await pending;
    expect(tasks[0]).toMatchObject({ sandboxStatus: "merged", githubSynced: true });
    expect(tasks[0]?.operation).toBeUndefined();
    expect(tasks[0]?.publicationError).toBeUndefined();
  });

  it.each(["http", "network", "invalid-success"])("keeps the local merge and supports retry after %s failure", async (failure) => {
    if (failure === "network") fetchMock.mockRejectedValueOnce(new Error("connection lost"));
    else fetchMock.mockResolvedValueOnce(response(record(), failure === "http" ? 502 : 200));
    await client.publishLiveWorktree(1);
    expect(tasks[0]).toMatchObject({ sandboxStatus: "merged", githubSynced: false });
    expect(tasks[0]?.operation).toBeUndefined();
    expect(tasks[0]?.publicationError).toBeTruthy();
    expect(feedback).toHaveBeenLastCalledWith(expect.stringContaining("GitHub may already have received the commit"));
    fetchMock.mockResolvedValueOnce(response(record("worktree-one", { github_synced: true })));
    await client.publishLiveWorktree(1);
    expect(tasks[0]?.githubSynced).toBe(true);
    expect(tasks[0]?.publicationError).toBeUndefined();
  });

  it("does not publish unapproved, already synced, or offline rows", async () => {
    tasks = [mapLiveWorktreeToTask(record("worktree-one", { status: "Verified" }), 1)];
    await client.publishLiveWorktree(1);
    tasks[0] = mapLiveWorktreeToTask(record("worktree-one", { github_synced: true }), 1);
    await client.publishLiveWorktree(1);
    token = null;
    await client.publishLiveWorktree(1);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("does not publish while optimistic approval is still pending", async () => {
    tasks = [mapLiveWorktreeToTask(record("worktree-one", { status: "Verified" }), 1)];
    const request = deferred<Response>();
    fetchMock.mockReturnValueOnce(request.promise);
    const pending = client.approveLiveWorktree(1);
    await client.publishLiveWorktree(1);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    request.resolve(response(record()));
    await pending;
    expect(tasks[0]?.operation).toBeUndefined();
  });

  it("drops list responses started before publication", async () => {
    const list = deferred<Response>();
    fetchMock.mockReturnValueOnce(list.promise).mockResolvedValueOnce(response(record("worktree-one", { github_synced: true })));
    const hydration = client.hydrateSwarmWorktrees();
    await client.publishLiveWorktree(1);
    list.resolve(response([record()]));
    await hydration;
    expect(tasks[0]?.githubSynced).toBe(true);
  });

  it("preserves pending state and stable row IDs when a list response changes order", async () => {
    tasks.push(mapLiveWorktreeToTask(record("worktree-two"), 2));
    const publish = deferred<Response>();
    fetchMock.mockReturnValueOnce(publish.promise).mockResolvedValueOnce(response([record("worktree-two"), record()]));
    const pending = client.publishLiveWorktree(1);
    await client.hydrateSwarmWorktrees();
    expect(tasks.find((task) => task.liveId === "worktree-one")).toMatchObject({ id: 1, operation: "publishing" });
    await client.publishLiveWorktree(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    publish.resolve(response(record("worktree-one", { github_synced: true })));
    await pending;
    expect(tasks.find((task) => task.id === 1)?.githubSynced).toBe(true);
    expect(tasks.find((task) => task.id === 2)?.githubSynced).toBe(false);
  });

  it("drops a list snapshot captured during publication but delivered after success", async () => {
    const publish = deferred<Response>();
    const list = deferred<Response>();
    fetchMock.mockReturnValueOnce(publish.promise).mockReturnValueOnce(list.promise);
    const pending = client.publishLiveWorktree(1);
    const hydration = client.hydrateSwarmWorktrees();
    publish.resolve(response(record("worktree-one", { github_synced: true })));
    await pending;
    list.resolve(response([record()]));
    await hydration;
    expect(tasks[0]?.githubSynced).toBe(true);
  });

  it("ignores an earlier list response when two hydrations overlap", async () => {
    const first = deferred<Response>();
    fetchMock.mockReturnValueOnce(first.promise).mockResolvedValueOnce(response([record("worktree-one", { github_synced: true })]));
    const pending = client.hydrateSwarmWorktrees();
    await client.hydrateSwarmWorktrees();
    first.resolve(response([record()]));
    await pending;
    expect(tasks[0]?.githubSynced).toBe(true);
  });

  it("does not let an old session response release the replacement session's operation", async () => {
    const old = deferred<Response>();
    const current = deferred<Response>();
    fetchMock.mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise);
    const oldPending = client.publishLiveWorktree(1);
    client.resetSwarmEpoch();
    token = "second-session";
    tasks = [mapLiveWorktreeToTask(record(), 1)];
    const currentPending = client.publishLiveWorktree(1);
    old.resolve(response(record("worktree-one", { github_synced: true })));
    await oldPending;
    expect(tasks[0]).toMatchObject({ githubSynced: false, operation: "publishing" });
    await client.publishLiveWorktree(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    current.resolve(response(record("worktree-one", { github_synced: true })));
    await currentPending;
    expect(tasks[0]?.githubSynced).toBe(true);
  });

  it("ignores a response from a changed token even without an explicit epoch reset", async () => {
    const request = deferred<Response>();
    fetchMock.mockReturnValueOnce(request.promise);
    const pending = client.publishLiveWorktree(1);
    token = "second-session";
    tasks = [mapLiveWorktreeToTask(record("different-worktree"), 1)];
    feedback.mockClear();
    request.resolve(response(record("worktree-one", { github_synced: true })));
    await pending;
    expect(tasks[0]).toMatchObject({ liveId: "different-worktree", githubSynced: false });
    expect(feedback).not.toHaveBeenCalled();
  });
});

describe("shared worktree mutation ordering", () => {
  it("updates separate worktrees independently when publication replies arrive out of order", async () => {
    tasks.push(mapLiveWorktreeToTask(record("worktree-two"), 2));
    const first = deferred<Response>();
    fetchMock.mockReturnValueOnce(first.promise).mockResolvedValueOnce(response(record("worktree-two", { github_synced: true })));
    const pending = client.publishLiveWorktree(1);
    await client.publishLiveWorktree(2);
    first.resolve(response(record("worktree-one", { github_synced: true })));
    await pending;
    expect(tasks.map((task) => task.githubSynced)).toEqual([true, true]);
  });

  it.each(["approveLiveWorktree", "discardLiveWorktree"] as const)("rolls back a rejected %s without losing row identity", async (action) => {
    tasks = [mapLiveWorktreeToTask(record("worktree-one", { status: "Verified" }), 1)];
    const previous = { ...tasks[0] };
    fetchMock.mockResolvedValueOnce(response("conflict", 409));
    await client[action](1);
    if (action === "approveLiveWorktree") {
      expect(tasks[0]).toMatchObject({ id: 1, liveId: previous.liveId, sandboxStatus: "pending", verificationNeedsRefresh: true });
    } else {
      expect(tasks[0]).toEqual(previous);
    }
    expect(tasks[0]?.operation).toBeUndefined();
    expect(feedback).toHaveBeenLastCalledWith(expect.stringContaining("UI restored; check repository state before retry"));
  });

  it("removes a discarded row without overwriting another publication", async () => {
    tasks.push(mapLiveWorktreeToTask(record("worktree-two", { status: "Verified" }), 2));
    const publish = deferred<Response>();
    fetchMock.mockReturnValueOnce(publish.promise).mockResolvedValueOnce(new Response(null, { status: 204 }));
    const pending = client.publishLiveWorktree(1);
    await client.discardLiveWorktree(2);
    expect(tasks).toHaveLength(1);
    expect(tasks[0]?.operation).toBe("publishing");
    publish.resolve(response(record("worktree-one", { github_synced: true })));
    await pending;
    expect(tasks[0]?.githubSynced).toBe(true);
  });

  it("ignores failed response bodies delivered after the account changes", async () => {
    const body = deferred<string>();
    fetchMock.mockResolvedValueOnce({ ok: false, status: 502, text: () => body.promise });
    const pending = client.publishLiveWorktree(1);
    await Promise.resolve();
    client.resetSwarmEpoch();
    token = "second-session";
    tasks = [mapLiveWorktreeToTask(record("different-worktree"), 1)];
    feedback.mockClear();
    body.resolve("old account failure");
    await pending;
    expect(tasks[0]?.liveId).toBe("different-worktree");
    expect(tasks[0]?.publicationError).toBeUndefined();
    expect(feedback).not.toHaveBeenCalled();
  });

  it("retains a pending spawn through hydration and deduplicates its returned server row", async () => {
    const spawn = deferred<Response>();
    fetchMock.mockReturnValueOnce(spawn.promise).mockResolvedValueOnce(response([record(), record("spawned")]));
    const pending = client.spawnLiveWorktree();
    const placeholderId = tasks.find((task) => !task.liveId)?.id;
    await client.hydrateSwarmWorktrees();
    expect(tasks.find((task) => task.id === placeholderId)?.liveId).toBeUndefined();
    spawn.resolve(response(record("spawned", { status: "Pending" })));
    await pending;
    expect(tasks.filter((task) => task.liveId === "spawned")).toHaveLength(1);
    expect(tasks.find((task) => task.liveId === "spawned")?.id).toBe(placeholderId);
  });
});
