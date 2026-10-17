import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  createRepositoryBranches,
  parseBranchPage,
} from "../src/showcase/live-branches";
const id = "11111111-1111-4111-8111-111111111111";
const sha = "a".repeat(40);
const branch = (name = "topic", commit = sha) => ({
  name,
  ref_name: `refs/heads/${name}`,
  commit_sha: commit,
});
const page = (rows = [branch()], cursor: string | null = null) => ({
  branches: rows,
  next_cursor: cursor,
});
const response = (value: unknown) => new Response(JSON.stringify(value));
let token: string | null, repository: string;
let selected: ReturnType<typeof vi.fn>,
  unavailable: ReturnType<typeof vi.fn>,
  fetcher: ReturnType<typeof vi.fn>;
let client: ReturnType<typeof createRepositoryBranches>;
const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
};
beforeEach(() => {
  token = "session-a";
  repository = id;
  selected = vi.fn();
  unavailable = vi.fn();
  fetcher = vi.fn();
  vi.stubGlobal("window", { localStorage: { getItem: () => token } });
  vi.stubGlobal("fetch", fetcher);
  client = createRepositoryBranches({
    getRepositoryId: () => repository,
    onSelect: selected,
    onUnavailable: unavailable,
  });
});
afterEach(() => vi.unstubAllGlobals());

it("keeps only one keyset page and a pinned selection, including a newer tip of the same branch", async () => {
  fetcher.mockResolvedValueOnce(
    response(page([branch()], "v1.cmVmcy9oZWFkcy90b3BpYw")),
  );
  await client.refresh();
  client.select(client.state().branches[0]!);
  expect(selected).toHaveBeenCalledOnce();
  fetcher.mockResolvedValueOnce(response(page([branch("zulu")])));
  await client.next();
  expect(
    new URL(
      fetcher.mock.calls[1][0],
      "https://example.invalid",
    ).searchParams.get("cursor"),
  ).toBe("v1.cmVmcy9oZWFkcy90b3BpYw");
  expect(client.state().branches.map((row) => row.name)).toEqual(["zulu"]);
  expect(client.state().selected?.name).toBe("topic");
  fetcher.mockResolvedValueOnce(
    response(page([branch("topic", "b".repeat(40))])),
  );
  await client.refresh();
  expect(client.state().selected?.commit_sha).toBe(sha);
  expect(selected).toHaveBeenCalledOnce();
  client.select(client.state().branches[0]!);
  expect(client.state().selected?.commit_sha).toBe("b".repeat(40));
  expect(selected).toHaveBeenCalledTimes(2);
});

it("rejects old fetch and streamed body results after repository ABA or credential replacement", async () => {
  const oldFetch = deferred<Response>();
  fetcher.mockReturnValueOnce(oldFetch.promise);
  const first = client.refresh();
  repository = "22222222-2222-4222-8222-222222222222";
  client.reset();
  repository = id;
  client.reset();
  fetcher.mockResolvedValueOnce(response(page([branch("current")])));
  await client.refresh();
  oldFetch.resolve(response(page([branch("old")])));
  await first;
  expect(client.state().branches[0]?.name).toBe("current");
  let stream!: ReadableStreamDefaultController<Uint8Array>;
  fetcher.mockResolvedValueOnce(
    new Response(
      new ReadableStream({
        start(controller) {
          stream = controller;
        },
      }),
    ),
  );
  const pending = client.refresh();
  await vi.waitFor(() => expect(fetcher).toHaveBeenCalledTimes(3));
  token = "session-b";
  client.reset();
  stream.enqueue(
    new TextEncoder().encode(JSON.stringify(page([branch("secret")]))),
  );
  stream.close();
  await pending;
  expect(client.state().branches).toEqual([]);
  client.select(branch());
  expect(selected).not.toHaveBeenCalled();
});

it("shows current failure instead of an empty success, clears denied pages, and cancels oversized streams", async () => {
  fetcher.mockResolvedValueOnce(response(page()));
  await client.refresh();
  fetcher.mockResolvedValueOnce(
    new Response("private host diagnostic", { status: 403 }),
  );
  await client.refresh();
  expect(unavailable).toHaveBeenCalledExactlyOnceWith(id);
  expect(client.state().error).toMatch(/unavailable/);
  expect(client.state().branches).toEqual([]);
  let cancelled = false;
  fetcher.mockResolvedValueOnce(
    new Response(
      new ReadableStream({
        start(controller) {
          controller.enqueue(new Uint8Array(4 * 1024 * 1024 + 1));
        },
        cancel() {
          cancelled = true;
        },
      }),
    ),
  );
  await client.refresh();
  expect(cancelled).toBe(true);
  expect(client.state().error).toMatch(/unavailable/);
  fetcher.mockResolvedValueOnce(response(page([], null)));
  await client.refresh();
  expect(client.state().error).toBeNull();
  expect(client.state().branches).toEqual([]);
});

it("accepts existing Git branch spelling and rejects invalid, unordered or over-budget DTO identities", () => {
  const names = [
    "HEAD",
    "refs/heads/topic",
    "topic\u00a0name",
    "é".repeat(16384),
  ];
  for (const name of names)
    expect(parseBranchPage(page([branch(name)])).branches[0]?.name).toBe(name);
  for (const name of [
    "a..b",
    ".hidden",
    "topic.lock",
    "a/b.lock/c",
    "a//b",
    "x\ny",
    "é".repeat(16385),
  ])
    expect(() => parseBranchPage(page([branch(name)]))).toThrow();
  expect(() => parseBranchPage(page([branch("z"), branch("a")]))).toThrow();
  expect(() => parseBranchPage(page([branch(), branch()]))).toThrow();
  expect(() =>
    parseBranchPage(page([{ ...branch(), ref_name: "refs/tags/topic" }])),
  ).toThrow();
  expect(() =>
    parseBranchPage(page([{ ...branch(), commit_sha: "0".repeat(40) }])),
  ).toThrow();
  expect(() => parseBranchPage(page([], "v1.YQ"))).toThrow();
});
