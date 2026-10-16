import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createLiveRepoClient } from "../src/showcase/live-repo-client";
import type { DiffLine } from "../src/patterns/VirtualDiffViewer";

let token: string | null;
let lines: DiffLine[] | null;
let feedback: ReturnType<typeof vi.fn>;
let fetchMock: ReturnType<typeof vi.fn>;
let client: ReturnType<typeof createLiveRepoClient>;
const response = (data: unknown) => new Response(JSON.stringify(data));
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}
beforeEach(() => {
  token = "session-a";
  lines = null;
  feedback = vi.fn();
  fetchMock = vi.fn();
  vi.stubGlobal("window", { localStorage: { getItem: () => token } });
  vi.stubGlobal("fetch", fetchMock);
  client = createLiveRepoClient({
    setActiveCommitRev: vi.fn(), setLookupMicros: vi.fn(), setSelectedFilePath: vi.fn(),
    setLiveDiffLines: (value) => { lines = value; }, setLiveFileTreeNodes: vi.fn(),
    setLiveCommits: vi.fn(), getActiveCommitRev: () => "HEAD", showQuickFeedback: feedback,
  });
});
afterEach(() => vi.unstubAllGlobals());

it("does not accept a previous account's blob or sync response", async () => {
  const pending = deferred<Response>();
  fetchMock.mockReturnValueOnce(pending.promise);
  const lookup = client.selectFileFromTree({ path: "private.txt", kind: "file" });
  token = "session-b";
  pending.resolve(response({ content: "private account content" }));
  await lookup;
  expect(lines).toEqual([]);
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
  fetchMock.mockReturnValueOnce(old.promise).mockResolvedValueOnce(response({ content: "new" }));
  const first = client.selectFileFromTree({ path: "old", kind: "file" });
  await client.selectFileFromTree({ path: "new", kind: "file" });
  old.resolve(response({ content: "old" }));
  await first;
  expect(lines?.[0].content).toBe("new");
});
