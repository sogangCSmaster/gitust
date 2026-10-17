import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { parseLocalChecks } from "../src/showcase/live-local-checks";
import {
  createLocalChecksClient,
  type LocalChecksState,
} from "../src/showcase/live-local-checks-client";
import { localPage, localRecord, localRepoId } from "./local-checks-fixture";

let token: string | null;
let repository: string | undefined;
let epoch: number;
let allowed: boolean;
beforeEach(() => {
  token = "local-session";
  repository = localRepoId;
  epoch = 1;
  allowed = true;
  vi.stubGlobal("window", { localStorage: { getItem: () => token } });
});
afterEach(() => vi.unstubAllGlobals());
const client = () => {
  let state: LocalChecksState = { status: "idle", records: [] };
  const api = createLocalChecksClient(
    (next) => {
      state = next;
    },
    {
      repositoryId: () => repository,
      selectionEpoch: () => epoch,
      canRead: () => allowed,
    },
  );
  return { api, state: () => state };
};

it("preserves append order and rejects Activity-shaped or inconsistent local results", () => {
  const first = localRecord();
  const second = localRecord("cccc1111-1111-4111-8111-111111111111");
  second.completed_at = "2026-10-18T02:00:00Z";
  const page = { ...localPage(), records: [first, second] };
  expect(parseLocalChecks(page, localRepoId).map((row) => row.runId)).toEqual([
    first.result.run_id,
    second.result.run_id,
  ]);
  const bad = [
    { ...localPage(), repo_id: "00000000-0000-0000-0000-000000000000" },
    { ...localPage(), records: [{ ...first, status: "passed" }] },
    {
      ...localPage(),
      records: [
        {
          ...first,
          result: {
            ...first.result,
            steps: [{ ...first.result.steps[0], required: false }],
          },
        },
      ],
    },
    {
      ...localPage(),
      records: [
        { ...first, result: { ...first.result, stdout: "가".repeat(5462) } },
      ],
    },
    {
      ...localPage(),
      records: [
        {
          ...first,
          result: { ...first.result, worktree_id: "deleted-checkout" },
        },
      ],
    },
    { ...localPage(), records: [first, first] },
  ];
  for (const value of bad)
    expect(() => parseLocalChecks(value, localRepoId)).toThrow();
});

it("discards an old body after repository ABA or token replacement", async () => {
  for (const change of [
    () => {
      epoch += 2;
    },
    () => {
      token = "replacement";
    },
  ]) {
    let release!: (bytes: Uint8Array) => void;
    let finish!: () => void;
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        release = (bytes) => controller.enqueue(bytes);
        finish = () => controller.close();
      },
    });
    const fetcher = vi.fn().mockResolvedValue(new Response(body));
    vi.stubGlobal("fetch", fetcher);
    const loaded = client();
    const pending = loaded.api.load();
    await Promise.resolve();
    change();
    release(new TextEncoder().encode(JSON.stringify(localPage())));
    finish();
    await pending;
    expect(loaded.state()).toEqual({ status: "idle", records: [] });
    expect(loaded.api.isCurrent()).toBe(false);
  }
});

it("distinguishes an empty retained history from unavailable or oversized responses", async () => {
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(
      new Response(JSON.stringify({ ...localPage(), records: [] })),
    )
    .mockResolvedValueOnce(new Response("unavailable", { status: 503 }))
    .mockResolvedValueOnce(
      new Response("{}", {
        headers: { "content-length": String(3 * 1024 * 1024 + 1) },
      }),
    );
  vi.stubGlobal("fetch", fetcher);
  const loaded = client();
  await loaded.api.load();
  expect(loaded.state()).toEqual({ status: "ready", records: [] });
  await loaded.api.load();
  expect(loaded.state()).toMatchObject({ status: "error", records: [] });
  expect(loaded.state().error).toContain("HTTP 503");
  await loaded.api.load();
  expect(loaded.state()).toMatchObject({ status: "error", records: [] });
  expect(fetcher.mock.calls[0]?.[1]).toMatchObject({
    headers: { Authorization: "Bearer local-session" },
    cache: "no-store",
  });
  allowed = false;
  await loaded.api.load();
  expect(fetcher).toHaveBeenCalledTimes(3);
});
