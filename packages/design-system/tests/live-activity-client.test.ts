import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createLiveRunHistoryClient } from "../src/showcase/live-run-history-client";
import { historyRecord, worktreeId } from "./run-history-fixture";
const job = () => ({
  run_id: "55555555-5555-4555-8555-555555555555",
  user_id: historyRecord().user_id,
  origin: "mcp",
  purpose: "automatic_checks",
  expected: "a".repeat(40),
  previous: "c".repeat(40),
  checks_oid: "b".repeat(40),
  phase: "queued",
  reason: null,
  created_at: "2026-10-17T02:00:00Z",
  changed_at: "2026-10-17T02:00:00Z",
});
afterEach(() => vi.unstubAllGlobals());
beforeEach(() =>
  vi.stubGlobal("window", { localStorage: { getItem: () => "session-a" } }),
);
it("loads lifecycle and completed history together instead of omitting queued attempts", async () => {
  const fetch = vi.fn(
    async (url: string) =>
      new Response(
        JSON.stringify(
          url.endsWith("/automatic-runs") ? [job()] : [historyRecord()],
        ),
      ),
  );
  vi.stubGlobal("fetch", fetch);
  let state: unknown;
  await createLiveRunHistoryClient((next) => {
    state = next;
  }).load(worktreeId);
  expect(fetch).toHaveBeenCalledTimes(2);
  expect(state).toMatchObject({ status: "ready", jobs: [{ phase: "queued" }] });
});

const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
};
it("keeps both resources empty until both bodies are valid and rejects either endpoint's denied or malformed refresh", async () => {
  for (const endpoint of ["/runs", "/automatic-runs"]) {
    for (const status of [401, 403, 404, 200]) {
      let state: import("../src/patterns/run-history-types").RunHistoryState = {
        status: "idle",
        records: [],
      };
      vi.stubGlobal(
        "fetch",
        vi.fn(
          async (url: string) =>
            new Response(
              JSON.stringify(
                url.endsWith("/automatic-runs") ? [job()] : [historyRecord()],
              ),
            ),
        ),
      );
      const client = createLiveRunHistoryClient((next) => {
        state = next;
      });
      await client.load(worktreeId);
      expect(state.status).toBe("ready");
      vi.stubGlobal(
        "fetch",
        vi.fn(async (url: string) =>
          url.endsWith(endpoint)
            ? new Response(status === 200 ? "not-json" : "denied", { status })
            : new Response(
                JSON.stringify(
                  url.endsWith("/automatic-runs") ? [job()] : [historyRecord()],
                ),
              ),
        ),
      );
      await client.load(worktreeId);
      expect(state.status).toBe("error");
      expect(state.records).toEqual([]);
      expect(state.jobs).toEqual([]);
      if (status !== 200) expect(state.error).toContain(String(status));
    }
  }
});
it("aborts both old requests and ignores reversed worktree responses", async () => {
  const requests: Array<{
    response: ReturnType<typeof deferred<Response>>;
    signal: AbortSignal;
  }> = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((_url: string, options: RequestInit) => {
      const response = deferred<Response>();
      requests.push({ response, signal: options.signal as AbortSignal });
      return response.promise;
    }),
  );
  let state: import("../src/patterns/run-history-types").RunHistoryState = {
    status: "idle",
    records: [],
  };
  const client = createLiveRunHistoryClient((next) => {
    state = next;
  });
  const first = client.load(worktreeId);
  const other = "66666666-6666-4666-8666-666666666666";
  const second = client.load(other);
  expect(requests[0]!.signal.aborted).toBe(true);
  expect(requests[1]!.signal.aborted).toBe(true);
  requests[2]!.response.resolve(new Response("[]"));
  requests[3]!.response.resolve(new Response(JSON.stringify([job()])));
  await second;
  expect(state.jobs?.[0]?.worktreeId).toBe(other);
  requests[0]!.response.resolve(
    new Response(JSON.stringify([historyRecord()])),
  );
  requests[1]!.response.resolve(new Response(JSON.stringify([job()])));
  await first;
  expect(state.jobs?.[0]?.worktreeId).toBe(other);
  expect(state.records).toEqual([]);
});
it("rechecks account and generation after each body, including close and replacements during JSON delivery", async () => {
  for (const action of ["account", "close", "replacement"]) {
    let token = "session-a";
    vi.stubGlobal("window", { localStorage: { getItem: () => token } });
    const bodies = [deferred<string>(), deferred<string>()];
    let request = 0;
    const fetch = vi.fn(async () => ({
      ok: true,
      text: () => bodies[request++]!.promise,
    }));
    vi.stubGlobal("fetch", fetch);
    let state: import("../src/patterns/run-history-types").RunHistoryState = {
      status: "idle",
      records: [],
    };
    const client = createLiveRunHistoryClient((next) => {
      state = next;
    });
    const pending = client.load(worktreeId);
    await vi.waitFor(() => expect(request).toBe(2));
    bodies[0]!.resolve(JSON.stringify([historyRecord()]));
    await Promise.resolve();
    expect(state.status).toBe("loading");
    expect(state.records).toEqual([]);
    let replacement: Promise<void> | undefined;
    if (action === "account") token = "session-b";
    else if (action === "close") client.clear();
    else {
      vi.stubGlobal(
        "fetch",
        vi.fn(
          async (url: string) =>
            new Response(
              url.endsWith("/automatic-runs") ? JSON.stringify([job()]) : "[]",
            ),
        ),
      );
      replacement = client.load(worktreeId);
    }
    bodies[1]!.resolve(JSON.stringify([job()]));
    await pending;
    await replacement;
    if (action === "replacement") {
      expect(state.status).toBe("ready");
      expect(state.records).toEqual([]);
      expect(state.jobs).toHaveLength(1);
    } else {
      expect(state.status).toBe("idle");
      expect(state.records).toEqual([]);
      expect(state.jobs).toEqual([]);
    }
  }
});
it("clears both resources when the same RunId carries output for a different actor", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(
      async (url: string) =>
        new Response(
          JSON.stringify(
            url.endsWith("/automatic-runs")
              ? [
                  {
                    ...job(),
                    run_id: historyRecord().result.run_id,
                    user_id: "77777777-7777-4777-8777-777777777777",
                  },
                ]
              : [
                  {
                    ...historyRecord(),
                    automatic: true,
                    verification_requested: false,
                  },
                ],
          ),
        ),
    ),
  );
  let state: unknown;
  await createLiveRunHistoryClient((next) => {
    state = next;
  }).load(worktreeId);
  expect(state).toMatchObject({ status: "error", records: [], jobs: [] });
});
