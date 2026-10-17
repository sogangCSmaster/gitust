// @vitest-environment node
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { AgentSandboxTask } from "../src/patterns/AgentSwarmView";
import { createLiveSwarmClient, mapLiveWorktreeToTask, type LiveWorktreeRecord } from "../src/showcase/live-swarm-client";
import { parseVerificationResult } from "../src/showcase/live-verification-result";

const firstSha = "a".repeat(40);
const secondSha = "b".repeat(40);
const worktree = (sha = firstSha, status = "Verified"): LiveWorktreeRecord => ({
  id: "worktree-one", repo_id: "repo", agent: "CodexSwarm", branch: "agent/review/change",
  base_branch: "main", base_commit: status === "Merged" ? sha : "0".repeat(40),
  verified_commit: sha, prompt_summary: "Review this revision", materialize_micros: 10,
  status, last_sandbox_duration_ms: 1, tests_passed: 0, tests_total: 0,
  files_changed: 1, additions: 1, deletions: 0, github_synced: false, created_at: "now",
});
const run = (checked: unknown = firstSha) => ({
  worktree_id: "worktree-one", command: "test -f README.md", checked_commit: checked,
  exit_code: 0, duration_ms: 1, stdout: "revision A passed", stderr: "", output_truncated: false,
});
const response = (body: unknown) => new Response(JSON.stringify(body));
let tasks: AgentSandboxTask[];
let fetchMock: ReturnType<typeof vi.fn>;
let feedback: ReturnType<typeof vi.fn>;
let client: ReturnType<typeof createLiveSwarmClient>;
beforeEach(() => {
  tasks = [mapLiveWorktreeToTask(worktree(), 1)];
  fetchMock = vi.fn();
  feedback = vi.fn();
  vi.stubGlobal("window", { localStorage: { getItem: () => "fixture-session" } });
  vi.stubGlobal("fetch", fetchMock);
  client = createLiveSwarmClient({
    getSwarmTasks: () => tasks,
    setSwarmTasks: (update) => { tasks = typeof update === "function" ? update(tasks) : update; },
    showQuickFeedback: feedback,
  });
});
afterEach(() => vi.unstubAllGlobals());

it("approves exactly the SHA displayed before another client reverified the worktree", async () => {
  fetchMock.mockResolvedValueOnce(response(worktree(firstSha, "Merged")));
  await client.approveLiveWorktree(1);
  expect(fetchMock).toHaveBeenCalledWith("/api/v1/swarm/worktrees/worktree-one/approve", {
    method: "POST",
    headers: { Authorization: "Bearer fixture-session", "Content-Type": "application/json" },
    body: JSON.stringify({ expected_commit: firstSha }),
  });
  expect(tasks[0]?.sandboxStatus).toBe("merged");
});

it("does not confirm a merge response for a different commit", async () => {
  fetchMock.mockResolvedValueOnce(response(worktree(secondSha, "Merged")));
  await client.approveLiveWorktree(1);
  expect(tasks[0]).toMatchObject({ sandboxStatus: "pending", verificationNeedsRefresh: true });
  expect(tasks[0]?.verifiedCommit).toBeUndefined();
  await client.publishLiveWorktree(1);
  expect(fetchMock).toHaveBeenCalledTimes(1);
});

it.each([firstSha, secondSha])("retains saved output only while its checked SHA matches refreshed %s", async (sha) => {
  tasks[0]!.verificationResult = parseVerificationResult(run(), "worktree-one", "test -f README.md");
  fetchMock.mockResolvedValueOnce(response([worktree(sha)]));
  await client.hydrateSwarmWorktrees();
  expect(tasks[0]?.verifiedCommit).toBe(sha);
  if (sha === firstSha) expect(tasks[0]?.verificationResult?.stdout).toBe("revision A passed");
  else expect(tasks[0]?.verificationResult).toBeUndefined();
});

it("refuses to associate this client's successful command with another client's newer verification", async () => {
  fetchMock.mockResolvedValueOnce(response(run())).mockResolvedValueOnce(response([worktree(secondSha)]));
  await client.verifyLiveWorktree(1, "test -f README.md");
  expect(tasks[0]).toMatchObject({ sandboxStatus: "pending", verificationNeedsRefresh: true });
  expect(tasks[0]?.verificationResult?.stdout).toBe("revision A passed");
  await client.approveLiveWorktree(1);
  expect(fetchMock).toHaveBeenCalledTimes(2);
});

it.each([undefined, null])("keeps unbound legacy output as diagnostics without fresh approval authority: %j", async (checked) => {
  fetchMock.mockResolvedValueOnce(response({ ...run(), checked_commit: checked }))
    .mockResolvedValueOnce(response([worktree()]));
  await client.verifyLiveWorktree(1, "test -f README.md");
  expect(tasks[0]).toMatchObject({ sandboxStatus: "pending", verificationNeedsRefresh: true });
  expect(tasks[0]?.verificationResult?.stdout).toBe("revision A passed");
  await client.approveLiveWorktree(1);
  expect(fetchMock).toHaveBeenCalledTimes(2);
});

it.each(["a".repeat(39), "a".repeat(64), "g".repeat(40), 42, {}])("rejects malformed checked commit metadata: %j", (checked) => {
  expect(() => parseVerificationResult(run(checked), "worktree-one", "test -f README.md")).toThrow();
});

it("records an immutable checked commit while retaining compatibility with unbound diagnostics", () => {
  expect(parseVerificationResult(run(), "worktree-one", "test -f README.md")).toMatchObject({ checkedCommit: firstSha });
  expect(() => parseVerificationResult({ ...run(), checked_commit: undefined }, "worktree-one", "test -f README.md")).not.toThrow();
});

it("discards a checkout without claiming that preserved committed branches were deleted", async () => {
  fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }));
  await client.discardLiveWorktree(1);
  expect(tasks).toHaveLength(0);
  expect(feedback).toHaveBeenLastCalledWith("Discarded worktree agent/review/change");
});
