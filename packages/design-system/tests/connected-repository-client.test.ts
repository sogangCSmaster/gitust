import { afterEach, expect, it, vi } from "vitest";
import {
  createConnectedRepositoryClient,
  type ConnectedRepositoryState,
} from "../src/showcase/connected-repository-client";
let token: string | null = "session-a";
vi.mock("../src/showcase/live-repo-client", () => ({
  getLiveToken: () => token,
  formatErr: (error: unknown) =>
    error instanceof Error ? error.message : String(error),
}));
const repo = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "real-repo",
  branch: "main",
  head_commit: "a".repeat(40),
  capability: "workbench",
  is_default: true,
};
const secondary = {
  ...repo,
  id: "22222222-2222-4222-8222-222222222222",
  name: "reference",
  capability: "browse",
  is_default: false,
};
const response = (repositories: unknown[]) =>
  new Response(JSON.stringify({ repositories }));
const setup = () => {
  const states: ConnectedRepositoryState[] = [];
  const client = createConnectedRepositoryClient((state) => states.push(state));
  return { states, client };
};
afterEach(() => {
  vi.unstubAllGlobals();
  token = "session-a";
});

it("uses the server rename hint and defaults old catalogs to read-only metadata", async () => {
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockResolvedValueOnce(response([{ ...repo, can_rename: true }]))
      .mockResolvedValueOnce(response([repo]))
      .mockResolvedValueOnce(response([{ ...repo, can_rename: "true" }])),
  );
  const { states, client } = setup();
  await client.refresh();
  expect(states.at(-1)?.repositories[0]).toMatchObject({
    id: repo.id,
    canRename: true,
  });
  await client.refresh();
  expect(states.at(-1)?.repositories[0]).toMatchObject({
    id: repo.id,
    canRename: false,
  });
  await client.refresh();
  expect(states.at(-1)?.repositories).toEqual([]);
});

it("rejects a rename response for another UUID and clears cached targets when access is denied", async () => {
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(response([{ ...repo, can_rename: true }, secondary]))
    .mockResolvedValueOnce(
      new Response(JSON.stringify({ id: secondary.id, name: "wrong-target" })),
    )
    .mockResolvedValueOnce(
      new Response("private denial detail", { status: 403 }),
    );
  vi.stubGlobal("fetch", fetcher);
  const { states, client } = setup();
  await client.refresh();
  const intent = {
    repoId: repo.id,
    name: repo.name,
    token: "session-a",
    epoch: 0,
    key: 1,
  };
  expect(await client.rename(intent, "requested-name", () => true)).toEqual({
    kind: "error",
    error: "Could not confirm the name change. Try again.",
  });
  expect(states.at(-1)?.repositories.map((row) => row.name)).toEqual([
    repo.name,
    secondary.name,
  ]);
  expect(
    await client.rename({ ...intent, key: 2 }, "requested-name", () => true),
  ).toEqual({ kind: "stale" });
  expect(states.at(-1)?.repositories).toEqual([]);
  expect(states.at(-1)?.error).toBe(
    "Repository access changed. Refresh the repository list.",
  );
});

it("loads multiple hosted repositories and accepts browse-only or empty authorized inventories", async () => {
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(response([repo, secondary]))
    .mockResolvedValueOnce(response([secondary]))
    .mockResolvedValueOnce(response([]));
  vi.stubGlobal("fetch", fetcher);
  const { states, client } = setup();
  await client.refresh();
  expect(fetcher.mock.calls[0]?.[0]).toBe("/api/v1/repos");
  expect(states.at(-1)?.repositories.map((row) => row.id)).toEqual([
    repo.id,
    secondary.id,
  ]);
  expect(states.at(-1)?.repositories[0]).toMatchObject({
    capability: "workbench",
    isDefault: true,
    headCommit: repo.head_commit,
  });
  await client.refresh();
  expect(states.at(-1)?.repositories).toHaveLength(1);
  expect(states.at(-1)?.error).toBeNull();
  await client.refresh();
  expect(states.at(-1)).toEqual({
    repositories: [],
    loading: false,
    error: null,
  });
});

it("preserves long branch metadata including worst-case catalog JSON escaping", async () => {
  const longBranch = `agent/${Array.from({ length: 24 }, (_, i) => `segment${i}${"x".repeat(40)}`).join("/")}`;
  const boundary = `agent/${"가".repeat(10920)}ab`;
  const quotes = '"'.repeat(32768);
  const worst = Array.from({ length: 32 }, (_, i) => ({
    ...secondary,
    id: `${i.toString(16).padStart(8, "0")}-aaaa-4aaa-8aaa-aaaaaaaaaaaa`,
    branch: quotes,
  }));
  const payload = JSON.stringify({ repositories: worst });
  expect(new TextEncoder().encode(payload).length).toBeGreaterThan(
    2 * 1024 * 1024,
  );
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockResolvedValueOnce(response([{ ...repo, branch: longBranch }]))
      .mockResolvedValueOnce(response([{ ...repo, branch: boundary }]))
      .mockResolvedValueOnce(new Response(payload))
      .mockResolvedValueOnce(response([{ ...repo, branch: boundary + "c" }])),
  );
  const { states, client } = setup();
  await client.refresh();
  expect(states.at(-1)?.repositories[0]?.branch).toBe(longBranch);
  await client.refresh();
  expect(new TextEncoder().encode(boundary)).toHaveLength(32768);
  expect(states.at(-1)?.repositories[0]?.branch).toBe(boundary);
  await client.refresh();
  expect(states.at(-1)?.repositories).toHaveLength(32);
  await client.refresh();
  expect(states.at(-1)?.repositories).toEqual([]);
  expect(states.at(-1)?.error).toMatch(/invalid repository metadata/i);
});

it("rejects duplicate identities, invalid capabilities and oversized catalogs rather than partly publishing", async () => {
  const { states, client } = setup();
  const invalid = [
    [repo, { ...repo, id: repo.id.toUpperCase() }],
    [{ ...secondary, is_default: true }],
    [{ ...repo, head_commit: "short" }],
    Array.from({ length: 33 }, () => secondary),
  ];
  const fetcher = vi.fn();
  invalid.forEach((rows) => fetcher.mockResolvedValueOnce(response(rows)));
  vi.stubGlobal("fetch", fetcher);
  for (const _case of invalid) {
    await client.refresh();
    expect(states.at(-1)?.repositories).toEqual([]);
    expect(states.at(-1)?.error).toMatch(/invalid repository metadata/i);
  }
});

it("clears previously loaded rows on denied refresh and body errors", async () => {
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockResolvedValueOnce(response([repo]))
      .mockResolvedValueOnce(new Response("Denied", { status: 403 }))
      .mockResolvedValueOnce(new Response("not json")),
  );
  const { states, client } = setup();
  await client.refresh();
  expect(states.at(-1)?.repositories).toHaveLength(1);
  await client.refresh();
  expect(states.at(-1)).toEqual({
    repositories: [],
    loading: false,
    error: "Unable to load repositories (HTTP 403)",
  });
  await client.refresh();
  expect(states.at(-1)?.repositories).toEqual([]);
  expect(states.at(-1)?.error).toBeTruthy();
});

it("drops out-of-order fetches and bodies after reset or session replacement", async () => {
  const pending: Array<(response: Response) => void> = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(() => new Promise<Response>((resolve) => pending.push(resolve))),
  );
  const { states, client } = setup();
  const first = client.refresh(),
    second = client.refresh();
  pending[1]!(response([secondary]));
  await second;
  pending[0]!(response([repo]));
  await first;
  expect(states.at(-1)?.repositories[0]?.id).toBe(secondary.id);
  let body!: (text: string) => void;
  const delayed = {
    ok: true,
    text: () =>
      new Promise<string>((resolve) => {
        body = resolve;
      }),
  } as Response;
  const old = client.refresh();
  pending[2]!(delayed);
  await Promise.resolve();
  token = "session-b";
  const fresh = client.refresh();
  expect(states.at(-1)?.repositories).toEqual([]);
  pending[3]!(response([secondary]));
  await fresh;
  body(JSON.stringify({ repositories: [repo] }));
  await old;
  expect(states.at(-1)?.repositories[0]?.id).toBe(secondary.id);
  const final = client.refresh();
  client.reset();
  pending[4]!(response([repo]));
  await final;
  expect(states.at(-1)).toEqual({
    repositories: [],
    loading: false,
    error: null,
  });
});
