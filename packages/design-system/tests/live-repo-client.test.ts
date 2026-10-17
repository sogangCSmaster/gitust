import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createLiveRepoClient } from "../src/showcase/live-repo-client";
import type { DiffLine } from "../src/patterns/VirtualDiffViewer";

let token: string | null;
let repositoryId = "11111111-1111-4111-8111-111111111111";
let workbench = true;
let unavailable: ReturnType<typeof vi.fn>;
let lines: DiffLine[] | null;
let blobLines: DiffLine[] | null;
let feedback: ReturnType<typeof vi.fn>;
let fetchMock: ReturnType<typeof vi.fn>;
let selectRevision: ReturnType<typeof vi.fn>;
let client: ReturnType<typeof createLiveRepoClient>;
let selectedPath: string;
let diffFiles: string[];
let diffTruncated: boolean;
let treeTruncated: boolean;
const response = (data: unknown) => new Response(JSON.stringify(data));
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}
async function hydrateTree(view: ReturnType<typeof createLiveRepoClient>) {
  fetchMock
    .mockResolvedValueOnce(response({ commits: [] }))
    .mockResolvedValueOnce(response({ commit_sha: head, entries: [] }))
    .mockResolvedValueOnce(
      response({ head_sha: head, base_sha: base, files: [], lines: [] }),
    );
  await view.hydrateFromLiveBackend();
  fetchMock.mockReset();
}
beforeEach(async () => {
  token = "session-a";
  repositoryId = "11111111-1111-4111-8111-111111111111";
  workbench = true;
  unavailable = vi.fn();
  lines = null;
  blobLines = null;
  feedback = vi.fn();
  selectedPath = "";
  diffFiles = [];
  diffTruncated = treeTruncated = false;
  fetchMock = vi.fn();
  selectRevision = vi.fn();
  vi.stubGlobal("window", { localStorage: { getItem: () => token } });
  vi.stubGlobal("fetch", fetchMock);
  client = createLiveRepoClient({
    getRepositoryId: () => repositoryId,
    canWorkbench: () => workbench,
    onRepositoryUnavailable: (id) => unavailable(id),
    setLiveBlobLines: (value) => {
      blobLines = value;
    },
    setSelectedTreePath: vi.fn(),
    setActiveCommitRev: selectRevision,
    setLookupMicros: vi.fn(),
    setSelectedFilePath: (path) => {
      selectedPath = path;
    },
    setLiveDiffFiles: (files) => {
      diffFiles = files;
    },
    setDiffTruncated: (value) => {
      diffTruncated = value;
    },
    setTreeTruncated: (value) => {
      treeTruncated = value;
    },
    setLiveDiffLines: (value) => {
      lines = value;
    },
    setLiveFileTreeNodes: vi.fn(),
    setLiveCommits: vi.fn(),
    getActiveCommitRev: () => "HEAD",
    showQuickFeedback: feedback,
  });
  await hydrateTree(client);
  lines = null;
  blobLines = null;
});
afterEach(() => vi.unstubAllGlobals());

it("selects the resolved commit when the initial HEAD comparison loads", () => {
  expect(selectRevision).toHaveBeenLastCalledWith(head);
});

it("binds slow fetch and body responses to the selected UUID across A to B to A", async () => {
  const slow = deferred<Response>();
  fetchMock.mockReturnValueOnce(slow.promise);
  const first = client.loadLiveCommitDiff("old-ref");
  repositoryId = "22222222-2222-4222-8222-222222222222";
  client.resetRepoEpoch();
  fetchMock.mockResolvedValueOnce(response(diff("new.txt", "NEW REPOSITORY")));
  await client.loadLiveCommitDiff("new-ref");
  expect(fetchMock.mock.calls[1]?.[0]).toContain(
    `/repos/${repositoryId}/diff?`,
  );
  expect(lines?.[0]?.content).toBe("NEW REPOSITORY");
  repositoryId = "11111111-1111-4111-8111-111111111111";
  client.resetRepoEpoch();
  slow.resolve(response(diff("old.txt", "OLD REPOSITORY")));
  await first;
  expect(lines).toEqual([]);
  await hydrateTree(client);
  const body = deferred<unknown>();
  fetchMock.mockResolvedValueOnce({
    ok: true,
    status: 200,
    json: () => body.promise,
  });
  const blob = client.selectFileFromTree({ path: "old.txt", kind: "file" });
  await Promise.resolve();
  repositoryId = "22222222-2222-4222-8222-222222222222";
  client.resetRepoEpoch();
  body.resolve({ commit_sha: head, content: "OLD BLOB BODY" });
  await blob;
  expect(blobLines?.[0]?.content).toContain("Select a file");
  expect(lines).toEqual([]);
});

it("clears denied current repository access without reacting to an obsolete denial", async () => {
  fetchMock.mockResolvedValueOnce(new Response("Denied", { status: 403 }));
  await client.loadLiveCommitDiff("HEAD");
  expect(unavailable).toHaveBeenCalledExactlyOnceWith(repositoryId);
  unavailable.mockClear();
  const old = deferred<Response>();
  fetchMock.mockReturnValueOnce(old.promise);
  const pending = client.loadLiveCommitDiff("HEAD");
  repositoryId = "22222222-2222-4222-8222-222222222222";
  old.resolve(new Response("Removed", { status: 404 }));
  await pending;
  expect(unavailable).not.toHaveBeenCalled();
});

it("does not issue a primary sync request while a browse-only repository is selected", () => {
  workbench = false;
  client.triggerGithubSync();
  expect(fetchMock).not.toHaveBeenCalled();
});

it("does not accept a previous account's blob or sync response", async () => {
  const pending = deferred<Response>();
  fetchMock.mockReturnValueOnce(pending.promise);
  const lookup = client.selectFileFromTree({
    path: "private.txt",
    kind: "file",
  });
  token = "session-b";
  pending.resolve(response({ content: "private account content" }));
  await lookup;
  expect(blobLines).toEqual([]);
  token = "session-a";
  const sync = deferred<Response>();
  fetchMock.mockReturnValueOnce(sync.promise);
  client.triggerGithubSync();
  feedback.mockClear();
  token = "session-b";
  sync.resolve(response({ last_synced_commit: "abc" }));
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(feedback).not.toHaveBeenCalled();
  expect(fetchMock).toHaveBeenCalledTimes(2);
});
it("keeps the newest selected viewer response", async () => {
  const old = deferred<Response>();
  fetchMock
    .mockReturnValueOnce(old.promise)
    .mockResolvedValueOnce(response({ commit_sha: head, content: "new" }));
  const first = client.selectFileFromTree({ path: "old", kind: "file" });
  await client.selectFileFromTree({ path: "new", kind: "file" });
  old.resolve(response({ content: "old" }));
  await first;
  expect(blobLines?.[0].content).toBe("new");
});
it("invalidates a pending file response when a directory is selected", async () => {
  const selected: string[] = [];
  const view = createLiveRepoClient({
    getRepositoryId: () => repositoryId,
    canWorkbench: () => workbench,
    onRepositoryUnavailable: (id) => unavailable(id),
    setLiveBlobLines: (value) => {
      blobLines = value;
    },
    setSelectedTreePath: (path) => selected.push(path),
    setActiveCommitRev: vi.fn(),
    setLookupMicros: vi.fn(),
    setSelectedFilePath: vi.fn(),
    setLiveDiffFiles: vi.fn(),
    setDiffTruncated: vi.fn(),
    setTreeTruncated: vi.fn(),
    setLiveDiffLines: (value) => {
      lines = value;
    },
    setLiveFileTreeNodes: vi.fn(),
    setLiveCommits: vi.fn(),
    getActiveCommitRev: () => "HEAD",
    showQuickFeedback: feedback,
  });
  await hydrateTree(view);
  const pending = deferred<Response>();
  fetchMock.mockReturnValueOnce(pending.promise);
  const first = view.selectFileFromTree({ path: "old.txt", kind: "file" });
  await view.selectFileFromTree({ path: "src", kind: "dir" });
  pending.resolve(response({ content: "old file content" }));
  await first;
  expect(selected.at(-1)).toBe("src");
  expect(blobLines?.[0]?.content).toContain("Directory: src");
  expect(fetchMock).toHaveBeenCalledOnce();
});

const head = "a".repeat(40);
const base = "b".repeat(40);
const diff = (path: string, content: string, truncated = false) => ({
  head_sha: head,
  base_sha: base,
  files: [{ path }],
  files_changed: 1,
  truncated,
  lines: [{ old_line: null, new_line: 1, kind: "add", content }],
});

it("never attributes a multi-file commit's combined lines to its first file", async () => {
  fetchMock
    .mockResolvedValueOnce(
      response({
        ...diff("one.txt", "ONE"),
        files: [{ path: "one.txt" }, { path: "two.txt" }],
        files_changed: 2,
        lines: [
          ...diff("one.txt", "ONE").lines,
          ...diff("two.txt", "TWO").lines,
        ],
      }),
    )
    .mockResolvedValueOnce(response(diff("one.txt", "ONE")))
    .mockResolvedValueOnce(response(diff("two.txt", "TWO")));
  await client.loadLiveCommitDiff("main");
  expect(diffFiles).toEqual(["one.txt", "two.txt"]);
  expect(selectedPath).toBe("one.txt");
  expect(lines?.map((line) => line.content)).toEqual(["ONE"]);
  expect(fetchMock.mock.calls[1][0]).toBe(
    `/api/v1/repos/${repositoryId}/diff?head=${head}&base=${base}&path=one.txt`,
  );
  await client.selectCommitDiffFile("two.txt");
  expect(selectedPath).toBe("two.txt");
  expect(lines?.map((line) => line.content)).toEqual(["TWO"]);
  expect(fetchMock.mock.calls[2][0]).toBe(
    `/api/v1/repos/${repositoryId}/diff?head=${head}&base=${base}&path=two.txt`,
  );
});

it.each(["logo.png", "empty.txt", "renamed.txt", "executable.sh"])(
  "reports changed %s with no text hunks without claiming an unchanged commit",
  async (path) => {
    fetchMock.mockResolvedValueOnce(
      response({
        head_sha: head,
        base_sha: base,
        files: [{ path }],
        files_changed: 1,
        lines: [],
        truncated: false,
      }),
    );
    await client.loadLiveCommitDiff("main");
    expect(selectedPath).toBe(path);
    expect(lines?.[0]?.content).toMatch(/changed file.*no textual hunks/i);
    expect(lines?.[0]?.content).not.toMatch(/No file changes/);
  },
);

it("retains explicit truncation state for both a partial diff and a partial tree", async () => {
  fetchMock.mockImplementation(async (url: string) =>
    response(
      url.includes("/commits?")
        ? { commits: [] }
        : url.includes("/tree?")
          ? {
              commit_sha: head,
              entries: [{ path: "one.txt", is_dir: false }],
              truncated: true,
            }
          : diff("one.txt", "PARTIAL DIFF", true),
    ),
  );
  await client.hydrateFromLiveBackend();
  expect(diffTruncated).toBe(true);
  expect(treeTruncated).toBe(true);
  await client.selectFileFromTree({ path: "src", kind: "dir" });
  expect(diffTruncated).toBe(true);
  expect(treeTruncated).toBe(true);
});

it("invalidates a pending file lookup on session reset even if the same token returns", async () => {
  const pending = deferred<Response>();
  fetchMock.mockReturnValueOnce(pending.promise);
  const first = client.selectFileFromTree({ path: "old.txt", kind: "file" });
  client.resetRepoEpoch();
  pending.resolve(response({ content: "OBSOLETE CONTENT" }));
  await first;
  expect(blobLines?.[0]?.content).toContain("Select a file");
  expect(lines).toEqual([]);
});

it("keeps a selected file when the first file's automatic diff arrives later", async () => {
  const firstFile = deferred<Response>();
  fetchMock
    .mockResolvedValueOnce(
      response({
        ...diff("one.txt", "ONE"),
        files: [{ path: "one.txt" }, { path: "two.txt" }],
        files_changed: 2,
      }),
    )
    .mockReturnValueOnce(firstFile.promise)
    .mockResolvedValueOnce(response(diff("two.txt", "TWO")));
  const opening = client.loadLiveCommitDiff("main");
  await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
  await client.selectCommitDiffFile("two.txt");
  firstFile.resolve(response(diff("one.txt", "ONE")));
  await opening;
  expect(selectedPath).toBe("two.txt");
  expect(lines?.map((line) => line.content)).toEqual(["TWO"]);
});

it("encodes literal changed paths and ignores file selections absent from the snapshot", async () => {
  const path = "src/[a] a&b.txt";
  fetchMock
    .mockResolvedValueOnce(
      response({
        ...diff("one.txt", "ONE"),
        files: [{ path: "one.txt" }, { path }],
        files_changed: 2,
      }),
    )
    .mockResolvedValueOnce(response(diff("one.txt", "ONE")))
    .mockResolvedValueOnce(response(diff(path, "SPECIAL PATH")));
  await client.loadLiveCommitDiff("main");
  await client.selectCommitDiffFile(path);
  const query = new URL(fetchMock.mock.calls[2][0], "https://example.invalid")
    .searchParams;
  expect(query.get("head")).toBe(head);
  expect(query.get("base")).toBe(base);
  expect(query.get("path")).toBe(path);
  await client.selectCommitDiffFile("unlisted.txt");
  expect(fetchMock).toHaveBeenCalledTimes(3);
});

it("loads each root-commit file without sending its empty tree as an explicit base commit", async () => {
  const emptyTree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
  fetchMock.mockImplementation(async (url: string) => {
    const query = new URL(url, "https://example.invalid").searchParams;
    if (query.get("base") === emptyTree)
      return new Response("Explicit base must be a commit", { status: 404 });
    const path = query.get("path");
    return response({
      ...diff(path ?? "one.txt", path === "two.txt" ? "ROOT TWO" : "ROOT ONE"),
      base_sha: emptyTree,
      files: path ? [{ path }] : [{ path: "one.txt" }, { path: "two.txt" }],
      files_changed: path ? 1 : 2,
    });
  });
  await client.loadLiveCommitDiff("root");
  expect(lines?.map((line) => line.content)).toEqual(["ROOT ONE"]);
  await client.selectCommitDiffFile("two.txt");
  expect(lines?.map((line) => line.content)).toEqual(["ROOT TWO"]);
  for (const [url] of fetchMock.mock.calls.slice(1)) {
    const query = new URL(url, "https://example.invalid").searchParams;
    expect(query.get("head")).toBe(head);
    expect(query.has("base")).toBe(false);
  }
});
