// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IconSpriteSheet } from "../src/components/workspace/icon";
import { PullRequestCollaborationPanel } from "../src/patterns/PullRequestCollaborationPanel";
import { RepositoryChangesView } from "../src/showcase/RepositoryChangesView";
import {
  evaluatePullRequestMergeGate,
  type BranchProtectionRule,
  type PullRequestDetailResponse,
  type PullRequestRecord,
} from "../src/showcase/live-pull-request-client";
import { deriveProtectionFromReviews } from "../src/showcase/live-pull-request-types";

const REPO_ID = "11111111-1111-4111-8111-111111111111";
const PR_ID = "22222222-2222-4222-8222-222222222222";
const BASE_SHA = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HEAD_SHA = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const MERGED_SHA = "cccccccccccccccccccccccccccccccccccccccc";
const OLD_SHA = "9999999999999999999999999999999999999999";

const makePullRecord = (
  overrides: Partial<PullRequestRecord> = {},
): PullRequestRecord => ({
  id: PR_ID,
  number: 42,
  repo_id: REPO_ID,
  title: "Enforce lock-time ref CAS on pull request merge",
  description: "Adds fast-forward and merge-commit ref transactions.",
  author_user_id: "user-1",
  author_username: "alice",
  base_branch: "main",
  head_branch: "feat/pr-merge",
  base_sha: BASE_SHA,
  head_sha: HEAD_SHA,
  status: "open",
  merged_sha: null,
  merged_by_username: null,
  reviews: [],
  comments: [],
  created_at_epoch_secs: 1760000000,
  updated_at_epoch_secs: 1760000100,
  ...overrides,
});

const makeDetailResponse = (
  overrides: {
    pull?: Partial<PullRequestRecord>;
    merge_preview?: Partial<PullRequestDetailResponse["merge_preview"]>;
    protection?: Partial<PullRequestDetailResponse["protection"]>;
  } = {},
): PullRequestDetailResponse => ({
  pull: makePullRecord(overrides.pull),
  merge_preview: {
    can_merge: true,
    fast_forward: true,
    conflicting_files: [],
    base_sha: BASE_SHA,
    head_sha: HEAD_SHA,
    ...overrides.merge_preview,
  },
  protection: {
    protected: true,
    require_pull_request: true,
    required_approvals: 1,
    approved_count: 0,
    changes_requested: false,
    require_passing_checks: true,
    checks_passed: true,
    ...overrides.protection,
  },
});

const jsonResponse = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

describe("Slice 4: Full Pull-Request Collaboration & Branch Protection", () => {
  beforeEach(() => {
    window.localStorage.setItem("gitust_token", "gitust_ses_live_pr_token");
    window.localStorage.setItem("gitust_csrf_token", "csrf_test_token_123");
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    window.localStorage.clear();
  });

  it("evaluates merge gates and review-verdict state machine across head_sha advances and comment overrides", () => {
    expect(
      evaluatePullRequestMergeGate(
        makeDetailResponse({
          protection: { required_approvals: 2, approved_count: 1, checks_passed: true },
        }),
      ).canMerge,
    ).toBe(false);
    expect(
      evaluatePullRequestMergeGate(
        makeDetailResponse({
          merge_preview: { can_merge: false, conflicting_files: ["crates/gitust-git/src/lib.rs"] },
          protection: { required_approvals: 1, approved_count: 1, checks_passed: true },
        }),
      ).canMerge,
    ).toBe(false);
    expect(
      evaluatePullRequestMergeGate(
        makeDetailResponse({
          protection: { required_approvals: 1, approved_count: 1, changes_requested: true, checks_passed: true },
        }),
      ).canMerge,
    ).toBe(false);
    expect(
      evaluatePullRequestMergeGate(
        makeDetailResponse({
          protection: { required_approvals: 1, approved_count: 1, require_passing_checks: true, checks_passed: false },
        }),
      ).canMerge,
    ).toBe(false);

    const satisfied = makeDetailResponse({
      protection: {
        required_approvals: 1,
        approved_count: 1,
        changes_requested: false,
        require_passing_checks: true,
        checks_passed: true,
      },
    });
    expect(evaluatePullRequestMergeGate(satisfied)).toEqual({ canMerge: true, reasons: [] });

    const pullWithOldReviews = makePullRecord({
      head_sha: HEAD_SHA,
      reviews: [
        {
          id: "rev-old-approve",
          author_user_id: "user-2",
          author_username: "bob",
          verdict: "approve",
          body: "Approved old commit",
          reviewed_head_sha: OLD_SHA,
          created_at_epoch_secs: 1760000110,
        },
        {
          id: "rev-old-changes",
          author_user_id: "user-3",
          author_username: "carol",
          verdict: "request_changes",
          body: "Needs fix before merge",
          reviewed_head_sha: OLD_SHA,
          created_at_epoch_secs: 1760000120,
        },
      ],
    });
    const derivedAfterAdvance = deriveProtectionFromReviews(pullWithOldReviews, satisfied.protection);
    expect(derivedAfterAdvance.approved_count).toBe(0);
    expect(derivedAfterAdvance.changes_requested).toBe(true);

    const pullAfterCommentOverride = makePullRecord({
      head_sha: HEAD_SHA,
      reviews: [
        ...pullWithOldReviews.reviews,
        {
          id: "rev-comment-clear",
          author_user_id: "user-3",
          author_username: "carol",
          verdict: "comment",
          body: "Addressed in discussion, clearing block",
          reviewed_head_sha: HEAD_SHA,
          created_at_epoch_secs: 1760000130,
        },
      ],
    });
    const derivedAfterComment = deriveProtectionFromReviews(pullAfterCommentOverride, satisfied.protection);
    expect(derivedAfterComment.changes_requested).toBe(false);
  });

  it("lists pull requests, creates a new PR, inspects merge preview & protection checklist, and submits reviews/comments/merge", async () => {
    let currentDetail = makeDetailResponse({
      merge_preview: {
        can_merge: false,
        fast_forward: false,
        conflicting_files: ["src/conflict.rs"],
      },
      protection: {
        protected: true,
        require_pull_request: true,
        required_approvals: 1,
        approved_count: 0,
        changes_requested: false,
        require_passing_checks: true,
        checks_passed: true,
      },
    });
    let pulls: PullRequestRecord[] = [currentDetail.pull];
    const rules: BranchProtectionRule[] = [
      {
        branch_pattern: "main",
        require_pull_request: true,
        required_approvals: 1,
        require_passing_checks: true,
        restrict_direct_pushes: true,
      },
    ];

    const calls: Array<{ url: string; method: string; headers: Headers; body?: unknown }> = [];
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
      const url = typeof input === "string" ? input : input.url;
      const method = init?.method ?? "GET";
      const headers = new Headers(init?.headers);
      const body =
        typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
      calls.push({ url, method, headers, body });

      if (url === `/api/v1/repos/${REPO_ID}/pulls` && method === "GET") {
        return jsonResponse({ pulls });
      }
      if (url === `/api/v1/repos/${REPO_ID}/branch-protection` && method === "GET") {
        return jsonResponse({ rules });
      }
      if (url === `/api/v1/repos/${REPO_ID}/pulls/${PR_ID}` && method === "GET") {
        return jsonResponse(currentDetail);
      }
      if (url === `/api/v1/repos/${REPO_ID}/pulls` && method === "POST") {
        const created = makePullRecord({
          id: "33333333-3333-4333-8333-333333333333",
          number: 43,
          title: (body as { title: string }).title,
          description: (body as { description?: string }).description ?? "",
          base_branch: (body as { base_branch: string }).base_branch,
          head_branch: (body as { head_branch: string }).head_branch,
          status: (body as { draft?: boolean }).draft ? "draft" : "open",
        });
        pulls = [created, ...pulls];
        return jsonResponse({ pull: created }, 201);
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/pulls/33333333-3333-4333-8333-333333333333` &&
        method === "GET"
      ) {
        return jsonResponse({
          pull: pulls[0],
          merge_preview: {
            can_merge: true,
            fast_forward: true,
            conflicting_files: [],
            base_sha: BASE_SHA,
            head_sha: HEAD_SHA,
          },
          protection: {
            protected: true,
            require_pull_request: true,
            required_approvals: 1,
            approved_count: 0,
            changes_requested: false,
            require_passing_checks: true,
            checks_passed: true,
          },
        });
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/pulls/${PR_ID}/reviews` &&
        method === "POST"
      ) {
        const payload = body as {
          verdict: "approve" | "request_changes" | "comment";
          body?: string;
          reviewed_head_sha: string;
        };
        const updatedPull = makePullRecord({
          ...currentDetail.pull,
          reviews: [
            ...currentDetail.pull.reviews,
            {
              id: `rev-${currentDetail.pull.reviews.length + 1}`,
              author_user_id: "user-2",
              author_username: "bob",
              verdict: payload.verdict,
              body: payload.body ?? "",
              reviewed_head_sha: payload.reviewed_head_sha,
              created_at_epoch_secs: 1760000200,
            },
          ],
        });
        currentDetail = {
          pull: updatedPull,
          merge_preview: {
            can_merge: true,
            fast_forward: true,
            conflicting_files: [],
            base_sha: BASE_SHA,
            head_sha: HEAD_SHA,
          },
          protection: {
            ...currentDetail.protection,
            approved_count: payload.verdict === "approve" ? 1 : 0,
            changes_requested: payload.verdict === "request_changes",
          },
        };
        pulls = pulls.map((p) => (p.id === PR_ID ? updatedPull : p));
        return jsonResponse({ pull: updatedPull });
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/pulls/${PR_ID}/comments` &&
        method === "POST"
      ) {
        const payload = body as {
          body: string;
          file_path?: string;
          line_number?: number;
          commit_sha?: string;
        };
        const updatedPull = makePullRecord({
          ...currentDetail.pull,
          comments: [
            ...currentDetail.pull.comments,
            {
              id: "comment-1",
              author_user_id: "user-2",
              author_username: "bob",
              body: payload.body,
              file_path: payload.file_path ?? null,
              line_number: payload.line_number ?? null,
              commit_sha: payload.commit_sha ?? HEAD_SHA,
              created_at_epoch_secs: 1760000300,
            },
          ],
        });
        currentDetail = { ...currentDetail, pull: updatedPull };
        return jsonResponse({ pull: updatedPull });
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/pulls/${PR_ID}/merge` &&
        method === "POST"
      ) {
        const mergedPull = makePullRecord({
          ...currentDetail.pull,
          status: "merged",
          merged_sha: MERGED_SHA,
          merged_by_username: "alice",
        });
        currentDetail = { ...currentDetail, pull: mergedPull };
        pulls = pulls.map((p) => (p.id === PR_ID ? mergedPull : p));
        return jsonResponse({ pull: mergedPull, merged_sha: MERGED_SHA });
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/branch-protection` &&
        method === "PUT"
      ) {
        const rule = body as BranchProtectionRule;
        return jsonResponse({ rules: [rule] });
      }
      return jsonResponse({ error: `Unexpected ${method} ${url}` }, 404);
    });

    render(() => (
      <>
        <IconSpriteSheet />
        <PullRequestCollaborationPanel
          repositoryId={REPO_ID}
          defaultBranch="main"
          selectedBranch="feat/pr-merge"
          selectionEpoch={1}
        />
      </>
    ));

    // Wait for initial PR list and PR #42 detail to load
    await waitFor(() => {
      expect(
        screen.getAllByText(/Enforce lock-time ref CAS on pull request merge/).length,
      ).toBeGreaterThan(0);
      expect(screen.getByText("src/conflict.rs")).toBeTruthy();
    });

    // Conflicting files and missing approvals should block merge initially
    expect(screen.getByText(/Approvals: 0 \/ 1/)).toBeTruthy();
    const mergeButton = screen.getByRole("button", {
      name: /Merge pull request/i,
    }) as HTMLButtonElement;
    expect(mergeButton.disabled).toBe(true);

    // Add an inline file/line comment
    await fireEvent.input(screen.getByLabelText("Comment body"), {
      target: { value: "Verify CAS lock ordering on this line." },
    });
    await fireEvent.input(screen.getByLabelText("Inline file path"), {
      target: { value: "crates/gitust-git/src/ref_transaction.rs" },
    });
    await fireEvent.input(screen.getByLabelText("Inline line number"), {
      target: { value: "84" },
    });
    await fireEvent.click(
      screen.getByRole("button", { name: /Add comment/i }),
    );

    await waitFor(() => {
      expect(
        screen.getByText("Verify CAS lock ordering on this line."),
      ).toBeTruthy();
      expect(
        screen.getByText("crates/gitust-git/src/ref_transaction.rs:84"),
      ).toBeTruthy();
    });

    const commentCall = calls.find(
      (c) => c.url.endsWith(`/pulls/${PR_ID}/comments`) && c.method === "POST",
    );
    expect(commentCall).toBeTruthy();
    expect(commentCall?.headers.get("X-CSRF-Token")).toBe(
      "csrf_test_token_123",
    );
    expect(commentCall?.body).toEqual({
      body: "Verify CAS lock ordering on this line.",
      file_path: "crates/gitust-git/src/ref_transaction.rs",
      line_number: 84,
      commit_sha: HEAD_SHA,
    });

    // Submit an approving review bound to expected_head_sha
    await waitFor(() => expect((screen.getByRole("button", { name: /^Approve$/i }) as HTMLButtonElement).disabled).toBe(false));
    await fireEvent.input(screen.getByLabelText("Review summary"), {
      target: { value: "LGTM, CAS semantics verified." },
    });
    await fireEvent.click(
      screen.getByRole("button", { name: /^Approve$/i }),
    );

    await waitFor(() => {
      expect(screen.getByText("LGTM, CAS semantics verified.")).toBeTruthy();
      expect(screen.getByText(/Approvals: 1 \/ 1/)).toBeTruthy();
    });

    const reviewCall = calls.find(
      (c) => c.url.endsWith(`/pulls/${PR_ID}/reviews`) && c.method === "POST",
    );
    expect(reviewCall?.body).toEqual({
      verdict: "approve",
      body: "LGTM, CAS semantics verified.",
      reviewed_head_sha: HEAD_SHA,
    });

    // Now that conflicts are resolved and 1/1 approvals + checks are satisfied, merge is enabled
    await waitFor(() => {
      expect(
        (
          screen.getByRole("button", {
            name: /Merge pull request/i,
          }) as HTMLButtonElement
        ).disabled,
      ).toBe(false);
    });

    await fireEvent.input(screen.getByLabelText("Merge commit message"), {
      target: { value: "Merge PR #42: Enforce lock-time ref CAS" },
    });
    await fireEvent.click(
      screen.getByRole("button", { name: /Merge pull request/i }),
    );

    await waitFor(() => {
      expect(screen.getAllByText(/Merged/i).length).toBeGreaterThan(0);
    });

    const mergeCall = calls.find(
      (c) => c.url.endsWith(`/pulls/${PR_ID}/merge`) && c.method === "POST",
    );
    expect(mergeCall?.headers.get("X-CSRF-Token")).toBe("csrf_test_token_123");
    expect(mergeCall?.body).toEqual({
      expected_base_sha: BASE_SHA,
      expected_head_sha: HEAD_SHA,
      commit_message: "Merge PR #42: Enforce lock-time ref CAS",
    });

    // Create a new PR
    await fireEvent.click(
      screen.getByRole("button", { name: /New pull request/i }),
    );
    await fireEvent.input(screen.getByLabelText("Pull request title"), {
      target: { value: "Add branch protection rule enforcement" },
    });
    await fireEvent.input(screen.getByLabelText("Pull request description"), {
      target: { value: "Enforces required approvals and status checks." },
    });
    await fireEvent.input(screen.getByLabelText("Base branch"), {
      target: { value: "main" },
    });
    await fireEvent.input(screen.getByLabelText("Head branch"), {
      target: { value: "feat/branch-protection" },
    });
    await waitFor(() => expect((screen.getByRole("button", { name: /Create pull request/i }) as HTMLButtonElement).disabled).toBe(false));
    await fireEvent.click(
      screen.getByRole("button", { name: /Create pull request/i }),
    );

    await waitFor(() => {
      expect(
        screen.getAllByText(/Add branch protection rule enforcement/).length,
      ).toBeGreaterThan(0);
    });

    const createPrCall = calls.find(
      (c) => c.url === `/api/v1/repos/${REPO_ID}/pulls` && c.method === "POST",
    );
    expect(createPrCall?.body).toEqual({
      title: "Add branch protection rule enforcement",
      description: "Enforces required approvals and status checks.",
      base_branch: "main",
      head_branch: "feat/branch-protection",
      draft: false,
    });

    // Configure branch protection rule
    await fireEvent.click(
      screen.getByRole("button", { name: /^Branch protection$/i }),
    );
    await fireEvent.input(screen.getByLabelText("Protected branch pattern"), {
      target: { value: "release/*" },
    });
    await fireEvent.input(screen.getByLabelText("Required approvals"), {
      target: { value: "2" },
    });
    await waitFor(() => expect((screen.getByRole("button", { name: /Save branch protection/i }) as HTMLButtonElement).disabled).toBe(false));
    await fireEvent.click(
      screen.getByRole("button", { name: /Save branch protection/i }),
    );

    await waitFor(() => {
      const putProtectionCall = calls.find(
        (c) =>
          c.url === `/api/v1/repos/${REPO_ID}/branch-protection` &&
          c.method === "PUT",
      );
      expect(putProtectionCall).toBeTruthy();
      expect(putProtectionCall?.headers.get("X-CSRF-Token")).toBe(
        "csrf_test_token_123",
      );
      expect(putProtectionCall?.body).toEqual({
        branch_pattern: "release/*",
        require_pull_request: true,
        required_approvals: 2,
        require_passing_checks: true,
        restrict_direct_pushes: true,
      });
    });
  });

  it("integrates into RepositoryChangesView so live repositories can switch between Commits and Pull requests", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const url = typeof input === "string" ? input : input.url;
      if (url === `/api/v1/repos/${REPO_ID}/pulls`) {
        return jsonResponse({ pulls: [makePullRecord()] });
      }
      if (url === `/api/v1/repos/${REPO_ID}/branch-protection`) {
        return jsonResponse({ rules: [] });
      }
      if (url === `/api/v1/repos/${REPO_ID}/pulls/${PR_ID}`) {
        return jsonResponse(makeDetailResponse());
      }
      return jsonResponse({}, 404);
    });

    render(() => (
      <>
        <IconSpriteSheet />
        <RepositoryChangesView
          liveMode={true}
          repositoryId={REPO_ID}
          defaultBranch="main"
          selectedBranch="feat/pr-merge"
          selectionEpoch={1}
          commits={[]}
          activeCommitRev=""
          onSelectCommit={() => {}}
          lookupMicros={null}
          selectedFilePath="README.md"
          lines={[]}
          files={["README.md"]}
          truncated={false}
          onSelectFile={() => {}}
        />
      </>
    ));

    const pullsTabBtn = screen.getByRole("button", {
      name: /Pull requests/i,
    });
    await fireEvent.click(pullsTabBtn);

    await waitFor(() => {
      expect(
        screen.getAllByText(/Enforce lock-time ref CAS on pull request merge/).length,
      ).toBeGreaterThan(0);
    });
  });
});
