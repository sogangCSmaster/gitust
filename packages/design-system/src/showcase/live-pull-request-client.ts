import { formatErr, getLiveToken } from "./live-repo-context";
import {
  deriveProtectionFromReviews,
  evaluatePullRequestMergeGate,
  getCsrfToken,
  INITIAL_PULL_REQUEST_STATE,
  mutationHeaders,
  parseDetailResponse,
  parseProtectionRules,
  parsePullRequestRecord,
  type BranchProtectionRule,
  type LivePullRequestState,
  type PullRequestComment,
  type PullRequestDetailResponse,
  type PullRequestMergePreview,
  type PullRequestProtectionStatus,
  type PullRequestRecord,
  type PullRequestReview,
  type PullRequestReviewVerdict,
  type PullRequestStatus,
} from "./live-pull-request-types";

export {
  evaluatePullRequestMergeGate,
  getCsrfToken,
  INITIAL_PULL_REQUEST_STATE,
  type BranchProtectionRule,
  type LivePullRequestState,
  type PullRequestComment,
  type PullRequestDetailResponse,
  type PullRequestMergePreview,
  type PullRequestProtectionStatus,
  type PullRequestRecord,
  type PullRequestReview,
  type PullRequestReviewVerdict,
  type PullRequestStatus,
};

export function createLivePullRequestClient(
  getState: () => LivePullRequestState,
  setState: (
    updater:
      | LivePullRequestState
      | ((prev: LivePullRequestState) => LivePullRequestState),
  ) => void,
  context: {
    repositoryId: () => string | undefined;
    selectionEpoch: () => number;
  },
) {
  let listGen = 0;
  let detailGen = 0;
  let mutGen = 0;
  let listAbort: AbortController | undefined;
  let detailAbort: AbortController | undefined;

  const isContextCurrent = (
    repoId: string | undefined,
    epoch: number,
    token: string | null,
  ) =>
    Boolean(repoId) &&
    Boolean(token) &&
    context.repositoryId() === repoId &&
    context.selectionEpoch() === epoch &&
    getLiveToken() === token;

  const clear = () => {
    listGen++;
    detailGen++;
    mutGen++;
    listAbort?.abort();
    detailAbort?.abort();
    listAbort = undefined;
    detailAbort = undefined;
    setState(INITIAL_PULL_REQUEST_STATE);
  };

  const selectPull = async (prId: string, silent = false) => {
    const repoId = context.repositoryId();
    const epoch = context.selectionEpoch();
    const token = getLiveToken();
    if (!repoId || !token) return;
    const reqId = ++detailGen;
    detailAbort?.abort();
    const controller = new AbortController();
    detailAbort = controller;
    if (!silent) {
      setState((prev) => ({
        ...prev,
        selectedPullId: prId,
        loadingDetail: true,
        error: null,
      }));
    }
    try {
      const res = await fetch(
        `/api/v1/repos/${encodeURIComponent(repoId)}/pulls/${encodeURIComponent(prId)}`,
        {
          headers: { Authorization: `Bearer ${token}` },
          cache: "no-store",
          signal: controller.signal,
        },
      );
      if (reqId !== detailGen || !isContextCurrent(repoId, epoch, token)) return;
      if (!res.ok) {
        if (!silent) {
          const text = (await res.text().catch(() => "")).trim();
          setState((prev) => ({
            ...prev,
            loadingDetail: false,
            error: `Unable to load pull request detail (HTTP ${res.status}${text ? `: ${text}` : ""})`,
          }));
        }
        return;
      }
      const raw = await res.json();
      if (reqId !== detailGen || !isContextCurrent(repoId, epoch, token)) return;
      const detail = parseDetailResponse(raw);
      setState((prev) => ({
        ...prev,
        selectedPullId: detail.pull.id,
        detail,
        pulls: prev.pulls.some((p) => p.id === detail.pull.id)
          ? prev.pulls.map((p) => (p.id === detail.pull.id ? detail.pull : p))
          : [detail.pull, ...prev.pulls],
        loadingDetail: false,
        error: null,
      }));
    } catch (err) {
      if (reqId !== detailGen || !isContextCurrent(repoId, epoch, token)) return;
      if (!silent) {
        setState((prev) => ({
          ...prev,
          loadingDetail: false,
          error: `Pull request detail error: ${formatErr(err)}`,
        }));
      }
    }
  };

  const loadAll = async () => {
    const repoId = context.repositoryId();
    const epoch = context.selectionEpoch();
    const token = getLiveToken();
    if (!repoId || !token) {
      clear();
      return;
    }
    const reqId = ++listGen;
    listAbort?.abort();
    const controller = new AbortController();
    listAbort = controller;
    setState((prev) => ({
      ...prev,
      loadingList: true,
      error: null,
    }));
    try {
      const [pullsRes, rulesRes] = await Promise.all([
        fetch(`/api/v1/repos/${encodeURIComponent(repoId)}/pulls`, {
          headers: { Authorization: `Bearer ${token}` },
          cache: "no-store",
          signal: controller.signal,
        }),
        fetch(`/api/v1/repos/${encodeURIComponent(repoId)}/branch-protection`, {
          headers: { Authorization: `Bearer ${token}` },
          cache: "no-store",
          signal: controller.signal,
        }),
      ]);
      if (reqId !== listGen || !isContextCurrent(repoId, epoch, token)) return;
      if (!pullsRes.ok) {
        setState((prev) => ({
          ...prev,
          loadingList: false,
          error: `Unable to load pull requests (HTTP ${pullsRes.status})`,
        }));
        return;
      }
      const pullsRaw = (await pullsRes.json()) as { pulls?: unknown[] };
      const rulesRaw = rulesRes.ok ? await rulesRes.json() : { rules: [] };
      if (reqId !== listGen || !isContextCurrent(repoId, epoch, token)) return;
      const pulls = Array.isArray(pullsRaw?.pulls)
        ? pullsRaw.pulls.map(parsePullRequestRecord)
        : [];
      const rules = parseProtectionRules(rulesRaw);
      const currentSelected = getState().selectedPullId;
      const nextSelected =
        pulls.find((p) => p.id === currentSelected)?.id ?? pulls[0]?.id ?? null;
      setState((prev) => ({
        ...prev,
        loadingList: false,
        pulls,
        rules,
        selectedPullId: nextSelected,
        detail: nextSelected ? prev.detail : null,
        error: null,
      }));
      if (nextSelected) {
        await selectPull(nextSelected);
      }
    } catch (err) {
      if (reqId !== listGen || !isContextCurrent(repoId, epoch, token)) return;
      setState((prev) => ({
        ...prev,
        loadingList: false,
        error: `Unable to load pull requests: ${formatErr(err)}`,
      }));
    }
  };

  const applyUpdatedPull = async (pull: PullRequestRecord, feedback: string) => {
    setState((prev) => {
      const exists = prev.pulls.some((p) => p.id === pull.id);
      const pulls = exists
        ? prev.pulls.map((p) => (p.id === pull.id ? pull : p))
        : [pull, ...prev.pulls];
      const detail: PullRequestDetailResponse =
        prev.detail && prev.detail.pull.id === pull.id
          ? {
              ...prev.detail,
              pull,
              protection: deriveProtectionFromReviews(
                pull,
                prev.detail.protection,
              ),
            }
          : {
              pull,
              merge_preview: {
                can_merge: true,
                fast_forward: true,
                conflicting_files: [],
                base_sha: pull.base_sha,
                head_sha: pull.head_sha,
              },
              protection: deriveProtectionFromReviews(pull, {
                protected: false,
                require_pull_request: false,
                required_approvals: 0,
                approved_count: 0,
                changes_requested: false,
                require_passing_checks: false,
                checks_passed: true,
              }),
            };
      return {
        ...prev,
        submitting: false,
        pulls,
        selectedPullId: pull.id,
        detail,
        error: null,
        feedback,
      };
    });
    await selectPull(pull.id, true);
  };

  const runMutation = async (
    requestFn: (repoId: string, token: string) => Promise<Response>,
    onSuccess: (data: Record<string, unknown>) => Promise<void>,
  ) => {
    const repoId = context.repositoryId();
    const epoch = context.selectionEpoch();
    const token = getLiveToken();
    if (!repoId || !token) return;
    const reqId = ++mutGen;
    setState((prev) => ({
      ...prev,
      submitting: true,
      error: null,
      feedback: null,
    }));
    try {
      const res = await requestFn(repoId, token);
      if (reqId !== mutGen || !isContextCurrent(repoId, epoch, token)) return;
      if (!res.ok) {
        const text = (await res.text().catch(() => "")).trim();
        setState((prev) => ({
          ...prev,
          submitting: false,
          error: `Request failed (HTTP ${res.status}${text ? `: ${text}` : ""})`,
        }));
        return;
      }
      const raw = (await res.json()) as Record<string, unknown>;
      if (reqId !== mutGen || !isContextCurrent(repoId, epoch, token)) return;
      await onSuccess(raw);
    } catch (err) {
      if (reqId !== mutGen || !isContextCurrent(repoId, epoch, token)) return;
      setState((prev) => ({
        ...prev,
        submitting: false,
        error: `Mutation error: ${formatErr(err)}`,
      }));
    }
  };

  return {
    clear,
    loadAll,
    selectPull,
    createPull: (input: {
      title: string;
      description?: string;
      base_branch: string;
      head_branch: string;
      draft?: boolean;
    }) =>
      runMutation(
        (repoId, token) =>
          fetch(`/api/v1/repos/${encodeURIComponent(repoId)}/pulls`, {
            method: "POST",
            headers: mutationHeaders(token),
            body: JSON.stringify(input),
          }),
        async (raw) => {
          const pull = parsePullRequestRecord(raw.pull);
          await applyUpdatedPull(pull, `Created pull request #${pull.number}`);
        },
      ),
    submitReview: (
      prId: string,
      input: {
        verdict: PullRequestReviewVerdict;
        body?: string;
        reviewed_head_sha?: string;
        expected_head_sha?: string;
      },
    ) => {
      const reviewedHeadSha =
        input.reviewed_head_sha ?? input.expected_head_sha;
      const payload = {
        verdict: input.verdict,
        ...(input.body !== undefined ? { body: input.body } : {}),
        ...(reviewedHeadSha !== undefined
          ? { reviewed_head_sha: reviewedHeadSha }
          : {}),
      };
      return runMutation(
        (repoId, token) =>
          fetch(
            `/api/v1/repos/${encodeURIComponent(repoId)}/pulls/${encodeURIComponent(prId)}/reviews`,
            {
              method: "POST",
              headers: mutationHeaders(token),
              body: JSON.stringify(payload),
            },
          ),
        async (raw) => {
          const pull = parsePullRequestRecord(raw.pull);
          await applyUpdatedPull(pull, `Submitted review (${input.verdict})`);
        },
      );
    },
    addComment: (
      prId: string,
      input: {
        body: string;
        file_path?: string;
        line_number?: number;
        commit_sha?: string;
      },
    ) =>
      runMutation(
        (repoId, token) =>
          fetch(
            `/api/v1/repos/${encodeURIComponent(repoId)}/pulls/${encodeURIComponent(prId)}/comments`,
            {
              method: "POST",
              headers: mutationHeaders(token),
              body: JSON.stringify(input),
            },
          ),
        async (raw) => {
          const pull = parsePullRequestRecord(raw.pull);
          await applyUpdatedPull(pull, "Added pull request comment");
        },
      ),
    updateStatus: (prId: string, status: "open" | "draft" | "closed") =>
      runMutation(
        (repoId, token) =>
          fetch(
            `/api/v1/repos/${encodeURIComponent(repoId)}/pulls/${encodeURIComponent(prId)}/status`,
            {
              method: "POST",
              headers: mutationHeaders(token),
              body: JSON.stringify({ status }),
            },
          ),
        async (raw) => {
          const pull = parsePullRequestRecord(raw.pull);
          await applyUpdatedPull(pull, `Pull request marked ${status}`);
        },
      ),
    mergePull: (
      prId: string,
      input: {
        expected_base_sha: string;
        expected_head_sha: string;
        commit_message?: string;
      },
    ) =>
      runMutation(
        (repoId, token) =>
          fetch(
            `/api/v1/repos/${encodeURIComponent(repoId)}/pulls/${encodeURIComponent(prId)}/merge`,
            {
              method: "POST",
              headers: mutationHeaders(token),
              body: JSON.stringify(input),
            },
          ),
        async (raw) => {
          const pull = parsePullRequestRecord(raw.pull);
          const mergedSha =
            typeof raw.merged_sha === "string"
              ? raw.merged_sha
              : (pull.merged_sha ?? "");
          await applyUpdatedPull(
            pull,
            `Merged pull request #${pull.number} (${mergedSha.slice(0, 8)})`,
          );
        },
      ),
    saveBranchProtection: (rule: BranchProtectionRule) =>
      runMutation(
        (repoId, token) =>
          fetch(`/api/v1/repos/${encodeURIComponent(repoId)}/branch-protection`, {
            method: "PUT",
            headers: mutationHeaders(token),
            body: JSON.stringify(rule),
          }),
        async (raw) => {
          const rules = parseProtectionRules(raw);
          setState((prev) => ({
            ...prev,
            submitting: false,
            rules,
            error: null,
            feedback: `Saved branch protection rule for ${rule.branch_pattern}`,
          }));
        },
      ),
  };
}
