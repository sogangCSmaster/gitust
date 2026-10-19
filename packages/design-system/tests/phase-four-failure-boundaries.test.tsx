// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@solidjs/testing-library";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createSignal } from "solid-js";
import {
  createLivePullRequestClient,
  INITIAL_PULL_REQUEST_STATE,
  type LivePullRequestState,
} from "../src/showcase/live-pull-request-client";
import { PullRequestCollaborationPanel } from "../src/patterns/PullRequestCollaborationPanel";
import { RepositoryFilesView } from "../src/showcase/RepositoryFilesView";

const repo = "11111111-1111-4111-8111-111111111111";
const sha = "a".repeat(40);
const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status });
beforeEach(() => window.localStorage.setItem("gitust_token", "session"));
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.localStorage.clear();
});

it("keeps merge authority unavailable when the post-create detail request fails", async () => {
  let state: LivePullRequestState = INITIAL_PULL_REQUEST_STATE;
  const client = createLivePullRequestClient(
    () => state,
    (value) => {
      state = typeof value === "function" ? value(state) : value;
    },
    { repositoryId: () => repo, selectionEpoch: () => 0 },
  );
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_url, init) =>
      init?.method === "POST"
        ? json({
            pull: {
              id: "pr",
              number: 1,
              repo_id: repo,
              title: "Review",
              base_branch: "main",
              head_branch: "feature",
              base_sha: sha,
              head_sha: sha,
              status: "open",
            },
          })
        : new Response("Detail unavailable", { status: 503 }),
    ),
  );
  expect(
    await client.createPull({
      title: "Review",
      base_branch: "main",
      head_branch: "feature",
    }),
  ).toBe(true);
  expect(state.pulls).toHaveLength(1);
  expect(state.detail).toBeNull();
  expect(state.error).toContain("503");
  expect(state.feedback).toContain("Created pull request");
});

it("preserves the PR form contents on a rejected creation", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url, init) =>
      init?.method === "POST"
        ? new Response("Branch does not exist", { status: 409 })
        : json(
            String(url).endsWith("branch-protection")
              ? { rules: [] }
              : { pulls: [] },
          ),
    ),
  );
  render(() => (
    <PullRequestCollaborationPanel repositoryId={repo} defaultBranch="main" />
  ));
  await waitFor(() =>
    expect(screen.getByText("No pull requests")).toBeTruthy(),
  );
  fireEvent.click(screen.getByRole("button", { name: /New pull request/i }));
  fireEvent.input(screen.getByLabelText("Pull request title"), {
    target: { value: "Keep my draft" },
  });
  fireEvent.input(screen.getByLabelText("Pull request description"), {
    target: { value: "Context matters" },
  });
  fireEvent.click(screen.getByRole("button", { name: /Create pull request/i }));
  await waitFor(() =>
    expect(screen.getByText(/Request failed.*409/)).toBeTruthy(),
  );
  expect(
    (screen.getByLabelText("Pull request title") as HTMLInputElement).value,
  ).toBe("Keep my draft");
  expect(
    (screen.getByLabelText("Pull request description") as HTMLInputElement)
      .value,
  ).toBe("Context matters");
});

it("rejects search results from another snapshot and clears earlier results on revision changes", async () => {
  const [revision, setRevision] = createSignal(sha);
  const fetcher = vi.fn(async () =>
    json({
      revision: sha,
      commitSha: sha,
      query: "needle",
      kind: "all",
      indexedFileCount: 1,
      indexCacheHit: true,
      hits: [
        { path: "old.rs", lineNumber: 1, snippet: "needle", matchKind: "code" },
      ],
    }),
  );
  vi.stubGlobal("fetch", fetcher);
  render(() => (
    <RepositoryFilesView
      files={[]}
      liveMode
      repositoryId={repo}
      revision={revision()}
    />
  ));
  fireEvent.input(screen.getByLabelText("Search repository code and symbols"), {
    target: { value: "needle" },
  });
  fireEvent.click(screen.getByRole("button", { name: /^Search$/ }));
  await waitFor(() => expect(screen.getByText("old.rs:1")).toBeTruthy());
  expect(String(fetcher.mock.calls[0]?.[0])).toContain(`rev=${sha}`);
  setRevision("b".repeat(40));
  await waitFor(() => expect(screen.queryByText("old.rs:1")).toBeNull());
  fireEvent.click(screen.getByRole("button", { name: /^Search$/ }));
  await waitFor(() =>
    expect(screen.getByText(/different repository snapshot/)).toBeTruthy(),
  );
  expect(screen.queryByText("old.rs:1")).toBeNull();
});

it("keeps creation pending until its detail refresh settles and rejects duplicate submission", async () => {
  let state: LivePullRequestState = INITIAL_PULL_REQUEST_STATE;
  let finishDetail!: (response: Response) => void;
  const detail = new Promise<Response>((resolve) => {
    finishDetail = resolve;
  });
  const client = createLivePullRequestClient(
    () => state,
    (value) => {
      state = typeof value === "function" ? value(state) : value;
    },
    { repositoryId: () => repo, selectionEpoch: () => 0 },
  );
  const fetcher = vi.fn(async (_url, init) =>
    init?.method === "POST"
      ? json({
          pull: {
            id: "pr",
            number: 1,
            repo_id: repo,
            title: "Review",
            base_branch: "main",
            head_branch: "feature",
            base_sha: sha,
            head_sha: sha,
            status: "open",
          },
        })
      : detail,
  );
  vi.stubGlobal("fetch", fetcher);
  const input = {
    title: "Review",
    base_branch: "main",
    head_branch: "feature",
  };
  const pending = client.createPull(input);
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));
  expect(state.submitting).toBe(true);
  expect(await client.createPull(input)).toBe(false);
  expect(fetcher).toHaveBeenCalledTimes(2);
  finishDetail(new Response("Detail unavailable", { status: 503 }));
  expect(await pending).toBe(true);
  expect(state.submitting).toBe(false);
});
