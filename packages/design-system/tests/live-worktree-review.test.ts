import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { AgentSandboxTask } from "../src/patterns/AgentSwarmView";
import type { DiffLine } from "../src/patterns/VirtualDiffViewer";
import { createLiveRepoClient, type LiveWorktreeReview } from "../src/showcase/live-repo-client";

const head = "a".repeat(40);
const base = "b".repeat(40);
const task = (overrides: Partial<AgentSandboxTask> = {}): AgentSandboxTask => ({
  id: 1, liveId: "worktree-one", agent: "Codex Swarm", branch: "agent/review/change",
  baseBranch: "release", sandboxStatus: "verified", verifiedCommit: head,
  promptSummary: "Review committed changes", forkLatencyMs: 1, compileTimeMs: 1,
  testsPassed: 0, testsTotal: 0, filesChanged: 2, additions: 2, deletions: 0,
  updatedAt: "now", ...overrides,
});
const diff = (paths = ["one.txt", "two.txt"], content = "AGGREGATE", truncated = false) => ({
  head_sha: head, base_sha: base, files: paths.map((path) => ({ path })),
  files_changed: paths.length, truncated,
  lines: paths.length ? [{ old_line: null, new_line: 1, kind: "add", content }] : [],
});
const response = (data: unknown) => new Response(JSON.stringify(data));
const query = (call = 0) => new URL(fetchMock.mock.calls[call][0], "https://example.invalid").searchParams;
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}
let token: string | null;
let lines: DiffLine[] | null;
let review: LiveWorktreeReview | null;
let truncated: boolean;
let treePaths: string[];
let fetchMock: ReturnType<typeof vi.fn>;
let client: ReturnType<typeof createLiveRepoClient>;
beforeEach(() => {
  token = "review-session";
  lines = null;
  review = null;
  truncated = false;
  treePaths = [];
  fetchMock = vi.fn();
  vi.stubGlobal("window", { localStorage: { getItem: () => token } });
  vi.stubGlobal("fetch", fetchMock);
  client = createLiveRepoClient({
    setActiveCommitRev: vi.fn(), setLookupMicros: vi.fn(), setSelectedFilePath: vi.fn(),
    setLiveDiffLines: (value) => { lines = value; },
    setLiveFileTreeNodes: (value) => { treePaths = value?.map((node) => node.path) ?? []; },
    setLiveCommits: vi.fn(), setLiveDiffFiles: vi.fn(),
    setDiffTruncated: (value) => { truncated = value; }, setTreeTruncated: vi.fn(),
    setWorktreeReview: (value) => { review = value; },
    getActiveCommitRev: () => head, showQuickFeedback: vi.fn(),
  });
});
afterEach(() => vi.unstubAllGlobals());

it("reviews the verified SHA against the actual merge target and fixes every file to server commit IDs", async () => {
  fetchMock.mockResolvedValueOnce(response(diff()))
    .mockResolvedValueOnce(response(diff(["one.txt"], "FIRST")))
    .mockResolvedValueOnce(response(diff(["two.txt"], "SECOND")));
  await client.loadLiveWorktreeDiff(task());
  expect(query().get("head")).toBe(head);
  expect(query().get("base")).toBe("refs/heads/release");
  expect(review).toMatchObject({ state: "ready", branch: "agent/review/change", baseBranch: "release", headSha: head, baseSha: base });
  expect(lines?.map((line) => line.content)).toEqual(["FIRST"]);
  await client.selectCommitDiffFile("two.txt");
  expect(query(2).get("head")).toBe(head);
  expect(query(2).get("base")).toBe(base);
  expect(lines?.map((line) => line.content)).toEqual(["SECOND"]);
});

it("resolves an unverified branch once and does not follow later branch movement", async () => {
  fetchMock.mockResolvedValueOnce(response(diff()))
    .mockResolvedValueOnce(response(diff(["one.txt"], "FIRST")))
    .mockResolvedValueOnce(response(diff(["two.txt"], "SECOND")));
  await client.loadLiveWorktreeDiff(task({ sandboxStatus: "pending", verifiedCommit: undefined }));
  expect(query().get("head")).toBe("refs/heads/agent/review/change");
  await client.selectCommitDiffFile("two.txt");
  expect(query(2).get("head")).toBe(head);
  expect(query(2).get("base")).toBe(base);
});

it.each([{ baseBranch: "" }, { liveId: undefined }, { verifiedCommit: undefined }])("rejects an incomplete live review target without substituting main: %j", async (overrides) => {
  await client.loadLiveWorktreeDiff(task(overrides));
  expect(fetchMock).not.toHaveBeenCalled();
  expect(review?.state).toBe("error");
  expect(lines?.[0]?.type).toBe("del");
});

it.each([
  { head_sha: "c".repeat(40) }, { head_sha: undefined }, { base_sha: undefined }, { base_sha: "b".repeat(64) },
])("rejects a response that cannot identify the requested verified snapshot: %j", async (override) => {
  fetchMock.mockResolvedValueOnce(response({ ...diff(["one.txt"], "WRONG"), ...override }));
  await client.loadLiveWorktreeDiff(task());
  expect(review?.state).toBe("error");
  expect(lines?.map((line) => line.content).join("")).not.toContain("WRONG");
});

it.each(["head_sha", "base_sha"])("refuses a file response with changed %s and allows a matching retry", async (key) => {
  fetchMock.mockResolvedValueOnce(response(diff()))
    .mockResolvedValueOnce(response(diff(["one.txt"], "FIRST")))
    .mockResolvedValueOnce(response({ ...diff(["two.txt"], "WRONG"), [key]: "c".repeat(40) }))
    .mockResolvedValueOnce(response(diff(["two.txt"], "RETRY")));
  await client.loadLiveWorktreeDiff(task());
  await client.selectCommitDiffFile("two.txt");
  expect(review?.state).toBe("error");
  expect(lines?.map((line) => line.content).join("")).not.toContain("WRONG");
  await client.selectCommitDiffFile("two.txt");
  expect(review?.state).toBe("ready");
  expect(lines?.[0]?.content).toBe("RETRY");
});

it("keeps empty, truncated, and failed worktree comparisons explicit", async () => {
  fetchMock.mockResolvedValueOnce(response(diff([])));
  await client.loadLiveWorktreeDiff(task());
  expect(lines?.[0]?.content).toContain("No committed changes");
  fetchMock.mockResolvedValueOnce(response(diff(["one.txt"], "PARTIAL", true)));
  await client.loadLiveWorktreeDiff(task());
  expect(truncated).toBe(true);
  fetchMock.mockResolvedValueOnce(new Response("comparison unavailable", { status: 409 }));
  await client.loadLiveWorktreeDiff(task());
  expect(review?.state).toBe("error");
  expect(lines?.[0]?.content).toContain("409");
  expect(lines?.[0]?.content).not.toContain("PARTIAL");
});

it("drops an old review after another selection and clears its context on session reset", async () => {
  const old = deferred<Response>();
  fetchMock.mockReturnValueOnce(old.promise).mockResolvedValueOnce(response(diff(["one.txt"], "NEW")));
  const pending = client.loadLiveWorktreeDiff(task());
  await client.loadLiveCommitDiff(head);
  old.resolve(response(diff(["one.txt"], "OLD")));
  await pending;
  expect(review).toBeNull();
  expect(lines?.map((line) => line.content)).toEqual(["NEW"]);
  fetchMock.mockResolvedValueOnce(response(diff([])));
  await client.loadLiveWorktreeDiff(task());
  client.resetRepoEpoch();
  expect(review).toBeNull();
});

it("does not republish a prior account's resolved branch context", async () => {
  const old = deferred<Response>();
  fetchMock.mockReturnValueOnce(old.promise);
  const pending = client.loadLiveWorktreeDiff(task({ sandboxStatus: "pending", verifiedCommit: undefined }));
  token = "replacement-session";
  client.resetRepoEpoch();
  old.resolve(response(diff(["one.txt"], "PRIVATE")));
  await pending;
  expect(review).toBeNull();
  expect(lines).toEqual([]);
});

it("leaves worktree review when a HEAD tree file is opened and uses that tree's own commit", async () => {
  const treeSha = "d".repeat(40);
  fetchMock.mockImplementation(async (url: string) => response(
    url.includes("/repo/commits") ? { commits: [] } :
      url.includes("/repo/tree") ? { commit_sha: treeSha, entries: [{ path: "head-only.txt", is_dir: false }] } :
        url.includes("/repo/blob") ? { commit_sha: treeSha, content: "TREE FILE" } : diff(["one.txt"], "REVIEW"),
  ));
  await client.hydrateFromLiveBackend();
  await client.loadLiveWorktreeDiff(task());
  await client.selectFileFromTree({ path: "head-only.txt", kind: "file" });
  expect(review).toBeNull();
  expect(query(fetchMock.mock.calls.length - 1).get("rev")).toBe(treeSha);
  expect(lines?.[0]?.content).toBe("TREE FILE");
});

it.each([undefined, "main", "d".repeat(39)])("requires immutable tree metadata before exposing paths or opening blobs: %j", async (commit) => {
  fetchMock.mockImplementation(async (url: string) => response(url.includes("/repo/commits")
    ? { commits: [] } : { commit_sha: commit, entries: [{ path: "unbound.txt", is_dir: false }] }));
  await client.hydrateFromLiveBackend();
  expect(treePaths).toEqual([]);
  const requests = fetchMock.mock.calls.length;
  await client.selectFileFromTree({ path: "unbound.txt", kind: "file" });
  expect(fetchMock).toHaveBeenCalledTimes(requests);
  expect(lines?.[0]?.content).toContain("revision is unavailable");
});

it("rejects a blob from a different commit than the displayed tree snapshot", async () => {
  fetchMock.mockImplementation(async (url: string) => response(url.includes("/repo/commits")
    ? { commits: [] } : url.includes("/repo/tree")
      ? { commit_sha: base, entries: [{ path: "snapshot.txt", is_dir: false }] } :
      url.includes("/repo/blob") ? { commit_sha: head, content: "WRONG SNAPSHOT" } : diff([])));
  await client.hydrateFromLiveBackend();
  await client.selectFileFromTree({ path: "snapshot.txt", kind: "file" });
  expect(query(fetchMock.mock.calls.length - 1).get("rev")).toBe(base);
  expect(lines?.[0]?.type).toBe("del");
  expect(lines?.[0]?.content).not.toContain("WRONG SNAPSHOT");
});

it("refuses repository hydration that changes the tree SHA without providing a path list", async () => {
  let incomplete = false;
  fetchMock.mockImplementation(async (url: string) => response(url.includes("/repo/commits")
    ? { commits: [] } : url.includes("/repo/tree")
      ? { commit_sha: incomplete ? head : base, entries: incomplete ? {} : [{ path: "old.txt", is_dir: false }] }
      : diff([])));
  await client.hydrateFromLiveBackend();
  expect(treePaths).toEqual(["old.txt"]);
  incomplete = true;
  await client.hydrateFromLiveBackend();
  expect(treePaths).toEqual([]);
  const requests = fetchMock.mock.calls.length;
  await client.selectFileFromTree({ path: "old.txt", kind: "file" });
  expect(fetchMock).toHaveBeenCalledTimes(requests);
});
