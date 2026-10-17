import { commitHistory } from "./fixtures/commit-history";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createLiveRepoClient } from "../src/showcase/live-repo-client";
import type { DiffLine } from "../src/patterns/VirtualDiffViewer";

let token: string | null;
let repositoryId = "11111111-1111-4111-8111-111111111111";
let workbench = true;
let repositoryHead: string | null = "a".repeat(40);
let repositoryChanged: ReturnType<typeof vi.fn>;
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
let publishedCommits: Array<{ sha: string }>;
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
    .mockResolvedValueOnce(response(commitHistory(head)))
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
  repositoryHead = "a".repeat(40);
  repositoryChanged = vi.fn(async () => {});
  unavailable = vi.fn();
  lines = null;
  blobLines = null;
  publishedCommits = [];
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
    getRepositoryHead: () => repositoryHead,
    onRepositoryChanged: () => repositoryChanged(),
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
    setLiveCommits: (value) => {
      publishedCommits = value;
    },
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
    getRepositoryHead: () => "a".repeat(40),
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
        ? commitHistory(head)
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

it("skips unborn HEAD requests and invalidates slow HEAD and blob bodies without treating them as authorization errors", async () => {
  const pendingDiff = deferred<unknown>();
  const pendingBlob = deferred<unknown>();
  fetchMock.mockResolvedValueOnce({
    ok: true,
    status: 200,
    json: () => pendingDiff.promise,
  });
  const diffRequest = client.loadLiveCommitDiff("HEAD");
  fetchMock.mockResolvedValueOnce({
    ok: true,
    status: 200,
    json: () => pendingBlob.promise,
  });
  const blobRequest = client.selectFileFromTree({
    path: "old.txt",
    kind: "file",
  });
  await Promise.resolve();
  repositoryHead = null;
  await client.hydrateFromLiveBackend();
  pendingDiff.resolve(diff("old.txt", "OLD HEAD"));
  pendingBlob.resolve({ commit_sha: head, content: "OLD FILE" });
  await Promise.all([diffRequest, blobRequest]);
  expect(fetchMock).toHaveBeenCalledTimes(2);
  expect(lines).toEqual([]);
  expect(blobLines?.[0]?.content).toContain("Select a file");
  expect(unavailable).not.toHaveBeenCalled();
  expect(feedback).not.toHaveBeenCalled();
  await client.loadLiveCommitDiff("HEAD");
  expect(fetchMock).toHaveBeenCalledTimes(2);
  repositoryHead = head;
  await hydrateTree(client);
  expect(diffFiles).toEqual([]);
});

it("refreshes catalog metadata after sync instead of hydrating a stale unborn HEAD", async () => {
  repositoryHead = null;
  fetchMock.mockResolvedValueOnce(response({ last_synced_commit: head }));
  client.triggerGithubSync();
  await vi.waitFor(() => expect(repositoryChanged).toHaveBeenCalledOnce());
  expect(fetchMock).toHaveBeenCalledOnce();
  expect(fetchMock.mock.calls[0][0]).toBe("/api/v1/sync/github");
});

it("pins initial default commits, tree and diff to the catalog SHA", async () => {
  fetchMock.mockImplementation(async (url: string) =>
    response(
      url.includes("/commits?")
        ? commitHistory(head)
        : url.includes("/tree?")
          ? { commit_sha: head, entries: [] }
          : { head_sha: head, base_sha: base, files: [], lines: [] },
    ),
  );
  await client.hydrateFromLiveBackend();
  for (const [url] of fetchMock.mock.calls) {
    const query = new URL(url, "https://example.invalid").searchParams;
    expect(query.get("rev") ?? query.get("head")).toBe(head);
  }
});

it("pins branch reads and rejects earlier branch snapshots across A to B to A", async () => {
  const a = "d".repeat(40),
    b = "e".repeat(40);
  const oldTree = deferred<Response>();
  let hold = true;
  fetchMock.mockImplementation(async (url: string) => {
    const query = new URL(url, "https://example.invalid").searchParams;
    const rev = query.get("rev") ?? query.get("head");
    if (url.includes("/branches?"))
      return response({
        branches: [
          { name: "a", ref_name: "refs/heads/a", commit_sha: a },
          { name: "b", ref_name: "refs/heads/b", commit_sha: b },
        ],
        next_cursor: null,
      });
    if (url.includes("/commits?")) return response(commitHistory(rev!));
    if (url.includes("/tree?")) {
      if (hold) {
        hold = false;
        return oldTree.promise;
      }
      return response({
        commit_sha: rev,
        entries: [{ path: "current.txt", is_dir: false }],
      });
    }
    if (url.includes("/blob?"))
      return response({ commit_sha: rev, content: "CURRENT BRANCH FILE" });
    return response({
      ...diff("current.txt", "CURRENT BRANCH DIFF"),
      head_sha: rev,
    });
  });
  await client.branches.refresh();
  const [first, second] = client.branches.state().branches;
  client.branches.select(first!);
  await vi.waitFor(() =>
    expect(
      fetchMock.mock.calls.filter(([url]) => url.includes("/tree?")),
    ).toHaveLength(1),
  );
  client.branches.select(second!);
  await vi.waitFor(() =>
    expect(lines?.[0]?.content).toBe("CURRENT BRANCH DIFF"),
  );
  client.branches.select(first!);
  await vi.waitFor(() => expect(selectRevision).toHaveBeenLastCalledWith(a));
  oldTree.resolve(
    response({
      commit_sha: a,
      entries: [{ path: "obsolete.txt", is_dir: false }],
    }),
  );
  await Promise.resolve();
  await client.selectFileFromTree({ path: "current.txt", kind: "file" });
  expect(blobLines?.[0]?.content).toBe("CURRENT BRANCH FILE");
  const url = fetchMock.mock.calls.at(-1)![0];
  expect(new URL(url, "https://example.invalid").searchParams.get("rev")).toBe(
    a,
  );
  expect(lines?.[0]?.content).toBe("CURRENT BRANCH DIFF");
  expect(client.branches.state().selected?.commit_sha).toBe(a);
  for (const [request] of fetchMock.mock.calls.filter(
    ([url]) => !url.includes("/branches?"),
  ))
    expect(request).not.toContain("HEAD");
});

it("keeps an already selected commit comparison during a same-snapshot hydration", async () => {
  fetchMock.mockResolvedValueOnce(
    response({
      ...diff("chosen.txt", "SELECTED COMMIT"),
      head_sha: "d".repeat(40),
    }),
  );
  await client.loadLiveCommitDiff("d".repeat(40));
  fetchMock.mockClear();
  fetchMock.mockImplementation(async (url: string) =>
    response(
      url.includes("/commits?")
        ? commitHistory(head)
        : { commit_sha: head, entries: [] },
    ),
  );
  await client.hydrateFromLiveBackend();
  expect(lines?.[0]?.content).toBe("SELECTED COMMIT");
  expect(fetchMock.mock.calls.some(([url]) => url.includes("/diff?"))).toBe(
    false,
  );
});

it.each(["envelope", "first", "empty"])(
  "rejects commit history with %s identity mismatch before publishing paths or commits",
  async (kind) => {
    const wrong = "d".repeat(40);
    fetchMock.mockImplementation(async (url: string) =>
      response(
        url.includes("/commits?")
          ? {
              head_sha: kind === "envelope" ? wrong : head,
              commits:
                kind === "empty"
                  ? []
                  : [
                      {
                        sha: kind === "envelope" ? head : wrong,
                        short_sha: wrong.slice(0, 7),
                        summary: "OTHER SNAPSHOT",
                        author_name: "Fixture",
                      },
                    ],
            }
          : url.includes("/tree?")
            ? {
                commit_sha: head,
                entries: [{ path: "mixed.txt", is_dir: false }],
              }
            : { ...diff("mixed.txt", "MIXED SNAPSHOT"), head_sha: head },
      ),
    );
    await client.hydrateFromLiveBackend();
    expect(publishedCommits).toEqual([]);
    expect(feedback).toHaveBeenCalledWith(
      expect.stringContaining("Live repository hydration failed"),
    );
    const before = fetchMock.mock.calls.length;
    await client.selectFileFromTree({ path: "mixed.txt", kind: "file" });
    expect(fetchMock).toHaveBeenCalledTimes(before);
    expect(blobLines?.[0]?.content).toContain("revision is unavailable");
  },
);
