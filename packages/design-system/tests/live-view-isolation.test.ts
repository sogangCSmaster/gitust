import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { DiffLine } from "../src/patterns/VirtualDiffViewer";
import {
  createLiveRepoClient,
  type LiveWorktreeReview,
} from "../src/showcase/live-repo-client";
const tree = "c".repeat(40);
const head = "a".repeat(40);
const base = "b".repeat(40);
const response = (data: unknown) => new Response(JSON.stringify(data));
const diff = (content: string) => ({
  head_sha: head,
  base_sha: base,
  files: [{ path: "change.txt" }],
  files_changed: 1,
  lines: [{ old_line: null, new_line: 1, kind: "add", content }],
});
const deferred = () => {
  let resolve!: (value: Response) => void;
  const promise = new Promise<Response>((done) => {
    resolve = done;
  });
  return { promise, resolve };
};
let token: string | null;
let changes: DiffLine[] | null;
let blob: DiffLine[] | null;
let changePath: string;
let treePath: string;
let revision: string;
let review: LiveWorktreeReview | null;
let fetcher: ReturnType<typeof vi.fn>;
let client: ReturnType<typeof createLiveRepoClient>;
beforeEach(async () => {
  token = "session";
  changes = blob = null;
  changePath = treePath = revision = "";
  review = null;
  vi.stubGlobal("window", { localStorage: { getItem: () => token } });
  fetcher = vi.fn(async (url: string) =>
    response(
      url.includes("/repo/commits")
        ? { commits: [] }
        : url.includes("/repo/tree")
          ? {
              commit_sha: tree,
              entries: [{ path: "source.txt", is_dir: false }],
            }
          : diff("INITIAL DIFF"),
    ),
  );
  vi.stubGlobal("fetch", fetcher);
  client = createLiveRepoClient({
    setActiveCommitRev: (value) => {
      revision = value;
    },
    getActiveCommitRev: () => revision,
    setSelectedFilePath: (value) => {
      changePath = value;
    },
    setLiveDiffLines: (value) => {
      changes = value;
    },
    setSelectedTreePath: (value) => {
      treePath = value;
    },
    setLiveBlobLines: (value) => {
      blob = value;
    },
    setWorktreeReview: (value) => {
      review = value;
    },
    setLiveFileTreeNodes: vi.fn(),
    setLiveCommits: vi.fn(),
    setLookupMicros: vi.fn(),
    setLiveDiffFiles: vi.fn(),
    setDiffTruncated: vi.fn(),
    setTreeTruncated: vi.fn(),
    showQuickFeedback: vi.fn(),
  });
  await client.hydrateFromLiveBackend();
  fetcher.mockClear();
});
afterEach(() => vi.unstubAllGlobals());

it("lets in-flight file and comparison reads finish independently without crossing their content or revision", async () => {
  const pendingBlob = deferred();
  fetcher
    .mockReturnValueOnce(pendingBlob.promise)
    .mockResolvedValueOnce(response(diff("NEW DIFF")));
  const file = client.selectFileFromTree({ path: "source.txt", kind: "file" });
  await client.loadLiveCommitDiff(head);
  pendingBlob.resolve(response({ commit_sha: tree, content: "FILE CONTENT" }));
  await file;
  expect(changes?.map((line) => line.content)).toEqual(["NEW DIFF"]);
  expect(blob?.map((line) => line.content)).toEqual(["FILE CONTENT"]);
  expect(changePath).toBe("change.txt");
  expect(treePath).toBe("source.txt");
  expect(revision).toBe(head);

  const pendingDiff = deferred();
  fetcher
    .mockReturnValueOnce(pendingDiff.promise)
    .mockResolvedValueOnce(response({ commit_sha: tree, content: "NEW FILE" }));
  const comparison = client.loadLiveCommitDiff(head);
  await client.selectFileFromTree({ path: "other.txt", kind: "file" });
  pendingDiff.resolve(response(diff("LATER DIFF")));
  await comparison;
  expect(changes?.map((line) => line.content)).toEqual(["LATER DIFF"]);
  expect(blob?.map((line) => line.content)).toEqual(["NEW FILE"]);
  expect(treePath).toBe("other.txt");
});
it("discards both viewer responses after a session epoch changes even when its token returns", async () => {
  const pendingBlob = deferred();
  const pendingDiff = deferred();
  fetcher
    .mockReturnValueOnce(pendingBlob.promise)
    .mockReturnValueOnce(pendingDiff.promise);
  const file = client.selectFileFromTree({ path: "private.txt", kind: "file" });
  const comparison = client.loadLiveCommitDiff(head);
  client.resetRepoEpoch();
  token = "replacement";
  token = "session";
  pendingBlob.resolve(response({ commit_sha: tree, content: "PRIVATE FILE" }));
  pendingDiff.resolve(response(diff("PRIVATE DIFF")));
  await Promise.all([file, comparison]);
  expect(changes).toEqual([]);
  expect(blob?.[0]?.content).toContain("Select a file");
  expect(treePath).toBe("");
  expect(review).toBeNull();
});
it("does not let background metadata hydration replace a newer comparison selection", async () => {
  const commits = deferred();
  const paths = deferred();
  fetcher
    .mockReturnValueOnce(commits.promise)
    .mockReturnValueOnce(paths.promise)
    .mockResolvedValueOnce(response(diff("SELECTED COMPARISON")));
  const hydration = client.hydrateFromLiveBackend();
  await client.loadLiveCommitDiff(head);
  commits.resolve(response({ commits: [] }));
  paths.resolve(
    response({
      commit_sha: tree,
      entries: [{ path: "source.txt", is_dir: false }],
    }),
  );
  await hydration;
  expect(changes?.[0]?.content).toBe("SELECTED COMPARISON");
  expect(changePath).toBe("change.txt");
  expect(revision).toBe(head);
  expect(fetcher).toHaveBeenCalledTimes(3);
});
