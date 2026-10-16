// @vitest-environment node
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createLiveSwarmClient, mapLiveWorktreeToTask } from "../src/showcase/live-swarm-client";
import type { LiveWorktreeRecord } from "../src/showcase/live-swarm-client";
import type { AgentSandboxTask } from "../src/patterns/AgentSwarmView";

const worktree = (status = "Pending"): LiveWorktreeRecord => ({
  id: "verification-worktree", repo_id: "repository", agent: "CodexSwarm",
  branch: "agent/test/verification", base_branch: "main", base_commit: "baseline-sha",
  prompt_summary: "Manual verification", materialize_micros: 50, status,
  verified_commit: status === "Verified" ? "a".repeat(40) : null,
  last_sandbox_duration_ms: 0, tests_passed: 0, tests_total: 0,
  files_changed: 1, additions: 1, deletions: 0, github_synced: false,
  created_at: "2026-10-16T00:00:00Z",
});
const runResult = (exitCode = 0) => ({
  run_id: "run-one", worktree_id: "verification-worktree", command: "cargo test",
  exit_code: exitCode, duration_ms: 27, stdout: "fixture stdout", stderr: exitCode ? "fixture stderr" : "",
  output_truncated: false, backend: "OsSandbox", network_isolated: true,
});
const response = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => { resolve = resolvePromise; });
  return { promise, resolve };
}
let token: string | null;
let tasks: AgentSandboxTask[];
let fetchMock: ReturnType<typeof vi.fn>;
let feedback: ReturnType<typeof vi.fn>;
let client: ReturnType<typeof createLiveSwarmClient>;
beforeEach(() => {
  token = "verification-session";
  tasks = [mapLiveWorktreeToTask(worktree(), 1)];
  fetchMock = vi.fn();
  feedback = vi.fn();
  vi.stubGlobal("window", { localStorage: { getItem: () => token } });
  vi.stubGlobal("fetch", fetchMock);
  client = createLiveSwarmClient({
    getSwarmTasks: () => tasks,
    setSwarmTasks: (update) => { tasks = typeof update === "function" ? update(tasks) : update; },
    showQuickFeedback: feedback,
  });
});
afterEach(() => vi.unstubAllGlobals());

describe("manual verification before approval", () => {
  it("preserves the requested command separately from expanded required checks for retries", async () => {
    const expanded = "/bin/sh -c 'cargo test' && /bin/sh -c 'cargo fmt --check'";
    fetchMock.mockResolvedValueOnce(response({ ...runResult(), command: expanded }))
      .mockResolvedValueOnce(response([worktree("Verified")]));
    await client.verifyLiveWorktree(1, "cargo test");
    expect(tasks[0]?.verificationResult).toMatchObject({
      requestedCommand: "cargo test", command: expanded,
    });
    fetchMock.mockResolvedValueOnce(response([worktree("Verified")]));
    await client.hydrateSwarmWorktrees();
    expect(tasks[0]?.verificationResult?.requestedCommand).toBe("cargo test");
  });
  it("distinguishes idle Pending worktrees from actively Compiling worktrees", () => {
    expect(mapLiveWorktreeToTask(worktree(), 1).sandboxStatus).toBe("pending");
    expect(mapLiveWorktreeToTask(worktree("Compiling"), 1).sandboxStatus).toBe("compiling");
    expect(mapLiveWorktreeToTask(worktree("Unknown"), 1).sandboxStatus).toBe("pending");
    expect(mapLiveWorktreeToTask({ ...worktree("Verified"), verified_commit: null }, 1).sandboxStatus).toBe("pending");
  });

  it("requires an explicit nonempty command and rejects oversized input without dispatch", async () => {
    await client.verifyLiveWorktree(1, "   ");
    await client.verifyLiveWorktree(1, "x".repeat(65_537));
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("submits the user's command, blocks conflicting actions, and waits for authoritative state", async () => {
    const execution = deferred<Response>();
    const state = deferred<Response>();
    fetchMock.mockReturnValueOnce(execution.promise).mockReturnValueOnce(state.promise);
    const pending = client.verifyLiveWorktree(1, "cargo test");
    expect(tasks[0]).toMatchObject({ sandboxStatus: "compiling", operation: "verifying" });
    expect(fetchMock).toHaveBeenCalledWith("/api/v1/swarm/worktrees/verification-worktree/sandbox", {
      method: "POST", headers: { Authorization: "Bearer verification-session", "Content-Type": "application/json" },
      body: JSON.stringify({ command: "cargo test", verify: true }),
    });
    await client.verifyLiveWorktree(1, "cargo test");
    await client.approveLiveWorktree(1);
    await client.discardLiveWorktree(1);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    execution.resolve(response(runResult()));
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
    expect(tasks[0]?.sandboxStatus).not.toBe("verified");
    await client.approveLiveWorktree(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    state.resolve(response([worktree("Verified")]));
    await pending;
    expect(tasks[0]).toMatchObject({ sandboxStatus: "verified", verificationResult: {
      command: "cargo test", exitCode: 0, durationMs: 27, stdout: "fixture stdout", stderr: "", outputTruncated: false,
    } });
    expect(tasks[0]?.operation).toBeUndefined();
    fetchMock.mockResolvedValueOnce(response(worktree("Merged")));
    await client.approveLiveWorktree(1);
    expect(tasks[0]?.sandboxStatus).toBe("merged");
  });

  it("keeps failed command output and never allows approval", async () => {
    fetchMock.mockResolvedValueOnce(response(runResult(1))).mockResolvedValueOnce(response([worktree("Failed")]));
    await client.verifyLiveWorktree(1, "cargo test");
    expect(tasks[0]).toMatchObject({ sandboxStatus: "failed", verificationResult: { exitCode: 1, stderr: "fixture stderr" } });
    await client.approveLiveWorktree(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("requires a refresh when failed output conflicts with a Verified server snapshot", async () => {
    fetchMock.mockResolvedValueOnce(response(runResult(1))).mockResolvedValueOnce(response([worktree("Verified")]));
    await client.verifyLiveWorktree(1, "cargo test");
    expect(tasks[0]).toMatchObject({ sandboxStatus: "pending", verificationNeedsRefresh: true });
    await client.approveLiveWorktree(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("does not infer Verified from exit zero if the server invalidated the worktree", async () => {
    fetchMock.mockResolvedValueOnce(response(runResult())).mockResolvedValueOnce(response([worktree("Pending")]));
    await client.verifyLiveWorktree(1, "cargo test");
    expect(tasks[0]?.sandboxStatus).toBe("pending");
    expect(tasks[0]?.verificationError).toBeTruthy();
    await client.approveLiveWorktree(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it.each([403, 409, 500])("surfaces HTTP %s and requires refresh instead of restoring an older Verified result", async (status) => {
    tasks = [mapLiveWorktreeToTask(worktree("Verified"), 1)];
    fetchMock.mockResolvedValueOnce(response("verification unavailable", status));
    await client.verifyLiveWorktree(1, "cargo test");
    expect(tasks[0]).toMatchObject({ sandboxStatus: "pending", verificationNeedsRefresh: true });
    expect(tasks[0]?.verificationError).toContain(String(status));
    await client.approveLiveWorktree(1);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("preserves output but blocks approval if post-run status cannot be loaded, then permits explicit refresh", async () => {
    fetchMock.mockResolvedValueOnce(response(runResult())).mockRejectedValueOnce(new Error("status offline"));
    await client.verifyLiveWorktree(1, "cargo test");
    expect(tasks[0]).toMatchObject({ sandboxStatus: "pending", verificationNeedsRefresh: true, verificationResult: { exitCode: 0 } });
    await client.approveLiveWorktree(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    fetchMock.mockResolvedValueOnce(response([worktree("Verified")]));
    await client.hydrateSwarmWorktrees();
    expect(tasks[0]?.sandboxStatus).toBe("verified");
    expect(tasks[0]?.verificationNeedsRefresh).toBeUndefined();
    expect(tasks[0]?.verificationResult?.stdout).toBe("fixture stdout");
  });

  it("ignores a stale hydration delivered after verification completed", async () => {
    const list = deferred<Response>();
    fetchMock.mockReturnValueOnce(list.promise).mockResolvedValueOnce(response(runResult())).mockResolvedValueOnce(response([worktree("Verified")]));
    const hydration = client.hydrateSwarmWorktrees();
    await client.verifyLiveWorktree(1, "cargo test");
    list.resolve(response([worktree()]));
    await hydration;
    expect(tasks[0]?.sandboxStatus).toBe("verified");
  });

  it("does not let an old account response overwrite or unlock the new account's verification", async () => {
    const old = deferred<Response>();
    const current = deferred<Response>();
    fetchMock.mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise).mockResolvedValueOnce(response([worktree("Verified")]));
    const oldPending = client.verifyLiveWorktree(1, "cargo test");
    client.resetSwarmEpoch();
    token = "new-session";
    tasks = [mapLiveWorktreeToTask(worktree(), 1)];
    const currentPending = client.verifyLiveWorktree(1, "cargo test");
    old.resolve(response(runResult()));
    await oldPending;
    expect(tasks[0]).toMatchObject({ sandboxStatus: "compiling", operation: "verifying" });
    expect(tasks[0]?.verificationResult).toBeUndefined();
    await client.verifyLiveWorktree(1, "cargo test");
    expect(fetchMock).toHaveBeenCalledTimes(2);
    current.resolve(response(runResult()));
    await currentPending;
    expect(tasks[0]?.sandboxStatus).toBe("verified");
  });

  it("ignores post-run status delivered after a token change", async () => {
    const status = deferred<Response>();
    fetchMock.mockResolvedValueOnce(response(runResult())).mockReturnValueOnce(status.promise);
    const pending = client.verifyLiveWorktree(1, "cargo test");
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
    token = "new-session";
    tasks = [mapLiveWorktreeToTask({ ...worktree(), id: "other-worktree" }, 1)];
    status.resolve(response([worktree("Verified")]));
    await pending;
    expect(tasks[0]?.liveId).toBe("other-worktree");
    expect(tasks[0]?.sandboxStatus).toBe("pending");
  });

  it("invalidates displayed approval eligibility after the server rejects approval", async () => {
    fetchMock.mockResolvedValueOnce(response(runResult())).mockResolvedValueOnce(response([worktree("Verified")])).mockResolvedValueOnce(response("commit changed; verify again", 409));
    await client.verifyLiveWorktree(1, "cargo test");
    await client.approveLiveWorktree(1);
    expect(tasks[0]).toMatchObject({ sandboxStatus: "pending", verificationNeedsRefresh: true });
    expect(tasks[0]?.verificationResult?.exitCode).toBe(0);
    await client.approveLiveWorktree(1);
    expect(fetchMock).toHaveBeenCalledTimes(3);
  });
});
