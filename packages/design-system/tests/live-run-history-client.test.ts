import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  createLiveRunHistoryClient,
  parseRunHistory,
  type RunHistoryState,
} from "../src/showcase/live-run-history-client";

import { worktreeId, historyRecord } from "./run-history-fixture";
let token: string | null;
let state: RunHistoryState;
let fetchMock: ReturnType<typeof vi.fn>;
const response = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status });
beforeEach(() => {
  token = "session-a";
  vi.stubGlobal("window", { localStorage: { getItem: () => token } });
  fetchMock = vi.fn().mockResolvedValue(response([historyRecord()]));
  vi.stubGlobal("fetch", fetchMock);
});
afterEach(() => vi.unstubAllGlobals());
const client = () =>
  createLiveRunHistoryClient((next) => {
    state = next;
  });

it("loads authenticated history and preserves historical diagnostics separately from authority", async () => {
  await client().load(worktreeId);
  expect(fetchMock).toHaveBeenCalledWith(
    `/api/v1/swarm/worktrees/${worktreeId}/runs`,
    expect.objectContaining({
      cache: "no-store",
      headers: { Authorization: "Bearer session-a" },
    }),
  );
  expect(state.status).toBe("ready");
  expect(state.records[0].checkedCommit).toBe("a".repeat(40));
  expect(state.records[0].result.stdout).toBe("checked");
  expect(state.records[0]).not.toHaveProperty("verifiedCommit");
});

it("drops responses after replacement, close or account changes", async () => {
  const pending: Array<(response: Response) => void> = [];
  fetchMock.mockImplementation(
    () => new Promise<Response>((resolve) => pending.push(resolve)),
  );
  const c = client();
  const first = c.load(worktreeId);
  const second = c.load(worktreeId);
  pending[1](response([]));
  await second;
  pending[0](response([historyRecord()]));
  await first;
  expect(state.records).toEqual([]);
  const third = c.load(worktreeId);
  c.clear();
  pending[2](response([historyRecord()]));
  await third;
  expect(state.status).toBe("idle");
  const fourth = c.load(worktreeId);
  token = "session-b";
  pending[3](response([historyRecord()]));
  await fourth;
  expect(state.records).toEqual([]);
});

it("clears old results on denied refresh and reports the failure", async () => {
  const c = client();
  await c.load(worktreeId);
  fetchMock.mockResolvedValue(response({}, 403));
  await c.load(worktreeId);
  expect(state.records).toEqual([]);
  expect(state.status).toBe("error");
  expect(state.error).toContain("403");
});

it("rejects mismatched identities, invalid step outcomes and excessive records", () => {
  expect(() =>
    parseRunHistory(
      [
        {
          ...historyRecord(),
          result: {
            ...historyRecord().result,
            worktree_id: "another-worktree",
          },
        },
      ],
      worktreeId,
    ),
  ).toThrow();
  expect(() =>
    parseRunHistory(Array(6).fill(historyRecord()), worktreeId),
  ).toThrow();
  const invalid = historyRecord();
  invalid.result.steps[0].exit_code = 7;
  expect(() => parseRunHistory([invalid], worktreeId)).toThrow();
});

it("accepts explicitly truncated command prefixes without turning them into retry input", () => {
  const record = historyRecord();
  record.history_truncated = true;
  record.result.command = " ".repeat(4096);
  record.result.steps[0].command = " ".repeat(4096);
  const parsed = parseRunHistory([record], worktreeId);
  expect(parsed[0].historyTruncated).toBe(true);
  expect(parsed[0].result).not.toHaveProperty("requestedCommand");
});
