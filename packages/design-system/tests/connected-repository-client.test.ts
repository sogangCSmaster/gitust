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
};
afterEach(() => {
  vi.unstubAllGlobals();
  token = "session-a";
});
it("preserves existing long branch names and enforces the UTF-8 readback limit", async () => {
  const longBranch = `agent/${Array.from({ length: 24 }, (_, index) => `segment${String(index).padStart(3, "0")}${"x".repeat(40)}`).join("/")}`;
  const boundary = `agent/${"가".repeat(10920)}ab`;
  const states: ConnectedRepositoryState[] = [];
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ ...repo, branch: longBranch })),
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ ...repo, branch: boundary })),
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ ...repo, branch: `${boundary}c` })),
      ),
  );
  const client = createConnectedRepositoryClient((state) => states.push(state));
  await client.refresh();
  expect(states.at(-1)?.repository?.branch).toBe(longBranch);
  await client.refresh();
  expect(new TextEncoder().encode(boundary)).toHaveLength(32 * 1024);
  expect(states.at(-1)?.repository?.branch).toBe(boundary);
  await client.refresh();
  expect(states.at(-1)?.repository).toBeNull();
  expect(states.at(-1)?.error).toMatch(/invalid repository metadata/i);
});
it("loads the configured repository and rejects malformed or denied metadata without demo fallback", async () => {
  const states: ConnectedRepositoryState[] = [];
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(new Response(JSON.stringify(repo)))
    .mockResolvedValueOnce(new Response("Denied", { status: 403 }))
    .mockResolvedValueOnce(
      new Response(JSON.stringify({ ...repo, head_commit: "short" })),
    );
  vi.stubGlobal("fetch", fetcher);
  const client = createConnectedRepositoryClient((state) => states.push(state));
  await client.refresh();
  expect(states.at(-1)!.repository).toEqual({
    id: repo.id,
    name: repo.name,
    branch: "main",
    headCommit: repo.head_commit,
  });
  await client.refresh();
  expect(states.at(-1)).toMatchObject({
    repository: null,
    loading: false,
    error: "Unable to load repository (HTTP 403)",
  });
  await client.refresh();
  expect(states.at(-1)!.error).toMatch(/invalid repository metadata/i);
});
it("drops late responses after navigation, replacement sessions, and reset", async () => {
  let finish!: (response: Response) => void;
  const states: ConnectedRepositoryState[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(
      () =>
        new Promise<Response>((resolve) => {
          finish = resolve;
        }),
    ),
  );
  const client = createConnectedRepositoryClient((state) => states.push(state));
  const pending = client.refresh();
  client.invalidate();
  finish(new Response(JSON.stringify(repo)));
  await pending;
  expect(states.at(-1)!.repository).toBeNull();
  const replacement = client.refresh();
  token = "session-b";
  finish(new Response(JSON.stringify(repo)));
  await replacement;
  expect(states.at(-1)!.repository).toBeNull();
  const reset = client.refresh();
  client.reset();
  finish(new Response(JSON.stringify(repo)));
  await reset;
  expect(states.at(-1)).toEqual({
    repository: null,
    loading: false,
    error: null,
  });
});

it("keeps the latest metadata when refreshes resolve out of order", async () => {
  const pending: Array<(response: Response) => void> = [];
  const states: ConnectedRepositoryState[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(() => new Promise<Response>((resolve) => pending.push(resolve))),
  );
  const client = createConnectedRepositoryClient((state) => states.push(state));
  const first = client.refresh();
  const second = client.refresh();
  pending[1]!(
    new Response(JSON.stringify({ ...repo, head_commit: "b".repeat(40) })),
  );
  await second;
  pending[0]!(new Response(JSON.stringify(repo)));
  await first;
  expect(states.at(-1)!.repository?.headCommit).toBe("b".repeat(40));
});
