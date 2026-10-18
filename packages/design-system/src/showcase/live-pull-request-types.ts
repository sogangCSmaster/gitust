export type PullRequestStatus = "open" | "draft" | "merged" | "closed";
export type PullRequestReviewVerdict =
  | "approve"
  | "request_changes"
  | "comment";

export interface PullRequestReview {
  id: string;
  author_user_id: string;
  author_username: string;
  verdict: PullRequestReviewVerdict;
  body: string;
  reviewed_head_sha: string;
  created_at_epoch_secs: number;
}

export interface PullRequestComment {
  id: string;
  author_user_id: string;
  author_username: string;
  body: string;
  file_path?: string | null;
  line_number?: number | null;
  commit_sha?: string | null;
  created_at_epoch_secs: number;
}

export interface PullRequestRecord {
  id: string;
  number: number;
  repo_id: string;
  title: string;
  description: string;
  author_user_id: string;
  author_username: string;
  base_branch: string;
  head_branch: string;
  base_sha: string;
  head_sha: string;
  status: PullRequestStatus;
  merged_sha?: string | null;
  merged_by_username?: string | null;
  reviews: PullRequestReview[];
  comments: PullRequestComment[];
  created_at_epoch_secs: number;
  updated_at_epoch_secs: number;
}

export interface PullRequestMergePreview {
  can_merge: boolean;
  fast_forward: boolean;
  conflicting_files: string[];
  base_sha: string;
  head_sha: string;
}

export interface PullRequestProtectionStatus {
  protected: boolean;
  require_pull_request: boolean;
  required_approvals: number;
  approved_count: number;
  changes_requested: boolean;
  require_passing_checks: boolean;
  checks_passed: boolean;
}

export interface PullRequestDetailResponse {
  pull: PullRequestRecord;
  merge_preview: PullRequestMergePreview;
  protection: PullRequestProtectionStatus;
}

export interface BranchProtectionRule {
  branch_pattern: string;
  require_pull_request: boolean;
  required_approvals: number;
  require_passing_checks: boolean;
  restrict_direct_pushes: boolean;
}

export interface LivePullRequestState {
  loadingList: boolean;
  loadingDetail: boolean;
  submitting: boolean;
  pulls: PullRequestRecord[];
  selectedPullId: string | null;
  detail: PullRequestDetailResponse | null;
  rules: BranchProtectionRule[];
  error: string | null;
  feedback: string | null;
}

export const INITIAL_PULL_REQUEST_STATE: LivePullRequestState = {
  loadingList: false,
  loadingDetail: false,
  submitting: false,
  pulls: [],
  selectedPullId: null,
  detail: null,
  rules: [],
  error: null,
  feedback: null,
};

export const getCsrfToken = (sessionToken: string): string => {
  try {
    if (typeof window !== "undefined") {
      const stored =
        window.localStorage.getItem("gitust_csrf_token") ??
        window.localStorage.getItem("gitust_csrf");
      if (stored && stored.trim()) return stored.trim();
    }
    if (typeof document !== "undefined" && document.cookie) {
      for (const part of document.cookie.split(";")) {
        const [rawName, ...rest] = part.trim().split("=");
        if (rawName === "gitust_csrf" || rawName === "csrf_token") {
          const val = rest.join("=").trim();
          if (val) return decodeURIComponent(val);
        }
      }
    }
  } catch {
    // Ignore storage/cookie read errors in restricted environments
  }
  return `csrf-${sessionToken.slice(0, 16)}`;
};

export const mutationHeaders = (token: string): Record<string, string> => ({
  Authorization: `Bearer ${token}`,
  "Content-Type": "application/json",
  "X-CSRF-Token": getCsrfToken(token),
});

export function evaluatePullRequestMergeGate(
  detail: PullRequestDetailResponse,
): { canMerge: boolean; reasons: string[] } {
  const reasons: string[] = [];
  if (detail.pull.status !== "open") {
    reasons.push(
      detail.pull.status === "draft"
        ? "Pull request is still a draft."
        : `Pull request is ${detail.pull.status}.`,
    );
  }
  if (
    !detail.merge_preview.can_merge ||
    detail.merge_preview.conflicting_files.length > 0
  ) {
    reasons.push("Merge conflicts must be resolved before merging.");
  }
  if (detail.protection.changes_requested) {
    reasons.push("Changes requested by a reviewer must be resolved.");
  }
  if (detail.protection.approved_count < detail.protection.required_approvals) {
    reasons.push(
      `Requires ${detail.protection.required_approvals} approval(s); currently has ${detail.protection.approved_count}.`,
    );
  }
  if (
    detail.protection.require_passing_checks &&
    !detail.protection.checks_passed
  ) {
    reasons.push("Required checks have not passed for this head commit.");
  }
  return {
    canMerge: reasons.length === 0,
    reasons,
  };
}

const isRecord = (val: unknown): val is Record<string, unknown> =>
  Boolean(val) && typeof val === "object" && !Array.isArray(val);

export const parsePullRequestRecord = (raw: unknown): PullRequestRecord => {
  if (!isRecord(raw)) throw new Error("Invalid pull request record payload");
  if (
    typeof raw.id !== "string" ||
    typeof raw.number !== "number" ||
    typeof raw.repo_id !== "string" ||
    typeof raw.title !== "string" ||
    typeof raw.base_branch !== "string" ||
    typeof raw.head_branch !== "string" ||
    typeof raw.base_sha !== "string" ||
    typeof raw.head_sha !== "string" ||
    !["open", "draft", "merged", "closed"].includes(String(raw.status))
  ) {
    throw new Error("Malformed pull request record fields");
  }
  const reviews: PullRequestReview[] = Array.isArray(raw.reviews)
    ? raw.reviews
        .filter(isRecord)
        .map((r) => ({
          id: String(r.id ?? ""),
          author_user_id: String(r.author_user_id ?? ""),
          author_username: String(r.author_username ?? "unknown"),
          verdict: (["approve", "request_changes", "comment"].includes(
            String(r.verdict),
          )
            ? r.verdict
            : "comment") as PullRequestReviewVerdict,
          body: String(r.body ?? ""),
          reviewed_head_sha: String(r.reviewed_head_sha ?? ""),
          created_at_epoch_secs: Number(r.created_at_epoch_secs ?? 0),
        }))
    : [];
  const comments: PullRequestComment[] = Array.isArray(raw.comments)
    ? raw.comments
        .filter(isRecord)
        .map((c) => ({
          id: String(c.id ?? ""),
          author_user_id: String(c.author_user_id ?? ""),
          author_username: String(c.author_username ?? "unknown"),
          body: String(c.body ?? ""),
          file_path: typeof c.file_path === "string" ? c.file_path : null,
          line_number:
            typeof c.line_number === "number" ? c.line_number : null,
          commit_sha: typeof c.commit_sha === "string" ? c.commit_sha : null,
          created_at_epoch_secs: Number(c.created_at_epoch_secs ?? 0),
        }))
    : [];
  return {
    id: raw.id,
    number: raw.number,
    repo_id: raw.repo_id,
    title: raw.title,
    description: typeof raw.description === "string" ? raw.description : "",
    author_user_id: String(raw.author_user_id ?? ""),
    author_username: String(raw.author_username ?? "unknown"),
    base_branch: raw.base_branch,
    head_branch: raw.head_branch,
    base_sha: raw.base_sha,
    head_sha: raw.head_sha,
    status: raw.status as PullRequestStatus,
    merged_sha: typeof raw.merged_sha === "string" ? raw.merged_sha : null,
    merged_by_username:
      typeof raw.merged_by_username === "string"
        ? raw.merged_by_username
        : null,
    reviews,
    comments,
    created_at_epoch_secs: Number(raw.created_at_epoch_secs ?? 0),
    updated_at_epoch_secs: Number(raw.updated_at_epoch_secs ?? 0),
  };
};

export const parseDetailResponse = (raw: unknown): PullRequestDetailResponse => {
  if (!isRecord(raw) || !isRecord(raw.merge_preview) || !isRecord(raw.protection)) {
    throw new Error("Invalid pull request detail payload");
  }
  const pull = parsePullRequestRecord(raw.pull);
  const mp = raw.merge_preview;
  const pr = raw.protection;
  return {
    pull,
    merge_preview: {
      can_merge: Boolean(mp.can_merge),
      fast_forward: Boolean(mp.fast_forward),
      conflicting_files: Array.isArray(mp.conflicting_files)
        ? mp.conflicting_files.filter((f): f is string => typeof f === "string")
        : [],
      base_sha: String(mp.base_sha ?? pull.base_sha),
      head_sha: String(mp.head_sha ?? pull.head_sha),
    },
    protection: {
      protected: Boolean(pr.protected),
      require_pull_request: Boolean(pr.require_pull_request),
      required_approvals: Number(pr.required_approvals ?? 0),
      approved_count: Number(pr.approved_count ?? 0),
      changes_requested: Boolean(pr.changes_requested),
      require_passing_checks: Boolean(pr.require_passing_checks),
      checks_passed: Boolean(pr.checks_passed),
    },
  };
};

export const parseProtectionRules = (raw: unknown): BranchProtectionRule[] => {
  if (!isRecord(raw) || !Array.isArray(raw.rules)) {
    throw new Error("Invalid branch protection payload");
  }
  return raw.rules.filter(isRecord).map((r) => ({
    branch_pattern: String(r.branch_pattern ?? ""),
    require_pull_request: Boolean(r.require_pull_request),
    required_approvals: Number(r.required_approvals ?? 0),
    require_passing_checks: Boolean(r.require_passing_checks),
    restrict_direct_pushes: Boolean(r.restrict_direct_pushes),
  }));
};

export const deriveProtectionFromReviews = (
  pull: PullRequestRecord,
  prev: PullRequestProtectionStatus,
): PullRequestProtectionStatus => {
  const latestByAuthor = new Map<
    string,
    { verdict: PullRequestReviewVerdict; sha: string }
  >();
  for (const rev of pull.reviews) {
    latestByAuthor.set(rev.author_user_id || rev.author_username, {
      verdict: rev.verdict,
      sha: rev.reviewed_head_sha,
    });
  }
  let approvedCount = 0;
  let changesRequested = false;
  for (const entry of latestByAuthor.values()) {
    if (entry.verdict === "approve" && entry.sha === pull.head_sha) {
      approvedCount++;
    }
    if (entry.verdict === "request_changes") {
      changesRequested = true;
    }
  }
  return {
    ...prev,
    approved_count: approvedCount,
    changes_requested: changesRequested,
  };
};
