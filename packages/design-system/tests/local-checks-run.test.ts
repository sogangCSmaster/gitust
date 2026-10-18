import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  createLocalChecksRunClient,
  type LocalChecksRunState,
} from "../src/showcase/live-local-checks-run";
import { localRecord, localRepoId } from "./local-checks-fixture";

let token: string | null, epoch: number, allowed: boolean, commit: string;
beforeEach(() => {
  token = "original-session";
  epoch = 1;
  allowed = true;
  commit = "e".repeat(40);
  vi.stubGlobal("window", { localStorage: { getItem: () => token } });
});
afterEach(() => vi.unstubAllGlobals());
const record = () => ({ ...localRecord(), source_ref: "refs/heads/main" });
const response = () => ({
  version: 1,
  repo_id: localRepoId,
  status: "failed",
  reason: "history_unconfirmed",
  cleanup: "confirmed",
  history: { status: "failed", reason: "storage_unavailable" },
  record: record(),
});
const client = () => {
  let state: LocalChecksRunState = { status: "idle" };
  const api = createLocalChecksRunClient(
    (next) => {
      state = next;
    },
    {
      repositoryId: () => localRepoId,
      selectionEpoch: () => epoch,
      canRun: () => allowed,
      sourceRef: () => "refs/heads/main",
      commit: () => commit,
    },
  );
  return { api, state: () => state };
};

it("retains confirmed results on a storage 503, without retrying or accepting a different snapshot", async () => {
  const fetcher = vi
    .fn()
    .mockResolvedValue(
      new Response(JSON.stringify(response()), { status: 503 }),
    );
  vi.stubGlobal("fetch", fetcher);
  const current = client();
  await current.api.run();
  expect(current.state()).toMatchObject({
    status: "complete",
    report: {
      status: "failed",
      history: { status: "failed" },
      record: { checkedCommit: commit },
    },
  });
  expect(fetcher).toHaveBeenCalledTimes(1);
  for (const wrong of [
    { ...response(), repo_id: "77777777-7777-4777-8777-777777777777" },
    { ...response(), record: { ...record(), checked_commit: "d".repeat(40) } },
  ]) {
    fetcher.mockResolvedValueOnce(
      new Response(JSON.stringify(wrong), { status: 503 }),
    );
    await current.api.run();
    expect(current.state()).toMatchObject({ status: "error" });
    expect(current.state().report).toBeUndefined();
  }
});

it("drops late bodies after context changes and does not reuse a mounted form under a replacement session", async () => {
  for (const change of [
    () => {
      epoch += 2;
    },
    () => {
      token = "replacement";
    },
    () => {
      allowed = false;
    },
    () => {
      commit = "d".repeat(40);
    },
  ]) {
    token = "original-session";
    epoch = 1;
    allowed = true;
    commit = "e".repeat(40);
    let release!: () => void;
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        release = () => {
          controller.enqueue(
            new TextEncoder().encode(JSON.stringify(response())),
          );
          controller.close();
        };
      },
    });
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(new Response(body, { status: 503 })),
    );
    const current = client();
    const pending = current.api.run();
    await Promise.resolve();
    change();
    release();
    await pending;
    expect(current.state()).toEqual({ status: "idle" });
  }
  token = "original-session";
  allowed = true;
  const current = client(),
    fetcher = vi.fn();
  vi.stubGlobal("fetch", fetcher);
  token = "replacement";
  await current.api.run();
  expect(fetcher).not.toHaveBeenCalled();
  expect(current.state().error).toContain("session changed");
});
