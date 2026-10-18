import { createMemo, createSignal, For, Show, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import { TextField, TextFieldInput } from "../components/ui/text-field";
import { Icon } from "../components/workspace/icon";
import { StatusBadge } from "../components/workspace/presentation";
import {
  evaluatePullRequestMergeGate,
  type PullRequestDetailResponse,
  type PullRequestReviewVerdict,
} from "../showcase/live-pull-request-client";

export interface PullRequestDetailPanelProps {
  detail: PullRequestDetailResponse;
  submitting: boolean;
  onSubmitReview: (input: {
    verdict: PullRequestReviewVerdict;
    body?: string;
    reviewed_head_sha: string;
    expected_head_sha?: string;
  }) => void;
  onAddComment: (input: {
    body: string;
    file_path?: string;
    line_number?: number;
    commit_sha?: string;
  }) => void;
  onUpdateStatus: (status: "open" | "draft" | "closed") => void;
  onMerge: (input: {
    expected_base_sha: string;
    expected_head_sha: string;
    commit_message?: string;
  }) => void;
}

export const PullRequestDetailPanel: Component<PullRequestDetailPanelProps> = (
  props,
) => {
  const [reviewBody, setReviewBody] = createSignal("");
  const [commentBody, setCommentBody] = createSignal("");
  const [commentFilePath, setCommentFilePath] = createSignal("");
  const [commentLineNumber, setCommentLineNumber] = createSignal("");
  const [mergeCommitMessage, setMergeCommitMessage] = createSignal("");

  const gate = createMemo(() => evaluatePullRequestMergeGate(props.detail));
  const pull = () => props.detail.pull;
  const preview = () => props.detail.merge_preview;
  const protection = () => props.detail.protection;

  const statusTone = () => {
    const s = pull().status;
    if (s === "merged") return "info" as const;
    if (s === "open") return "success" as const;
    if (s === "closed") return "danger" as const;
    return "neutral" as const;
  };

  const handleReview = (verdict: PullRequestReviewVerdict) => {
    const body = reviewBody().trim();
    props.onSubmitReview({
      verdict,
      ...(body ? { body } : {}),
      reviewed_head_sha: pull().head_sha,
    });
    setReviewBody("");
  };

  const handleAddComment = (e: Event) => {
    e.preventDefault();
    const body = commentBody().trim();
    if (!body) return;
    const filePath = commentFilePath().trim();
    const parsedLine = Number.parseInt(commentLineNumber().trim(), 10);
    props.onAddComment({
      body,
      ...(filePath ? { file_path: filePath } : {}),
      ...(Number.isFinite(parsedLine) && parsedLine > 0
        ? { line_number: parsedLine }
        : {}),
      commit_sha: pull().head_sha,
    });
    setCommentBody("");
    setCommentFilePath("");
    setCommentLineNumber("");
  };

  const handleMerge = () => {
    if (!gate().canMerge) return;
    const msg = mergeCommitMessage().trim();
    props.onMerge({
      expected_base_sha: preview().base_sha,
      expected_head_sha: preview().head_sha,
      ...(msg ? { commit_message: msg } : {}),
    });
    setMergeCommitMessage("");
  };

  return (
    <div class="min-w-0 space-y-4">
      <div class="space-y-2 rounded-lg border bg-card p-4">
        <div class="flex flex-wrap items-start justify-between gap-2">
          <div class="min-w-0 space-y-1">
            <div class="flex flex-wrap items-center gap-2">
              <StatusBadge tone={statusTone()}>
                <Icon
                  name={
                    pull().status === "merged"
                      ? "git-merge"
                      : "git-pull-request"
                  }
                  size={12}
                />
                <span>
                  {pull().status === "merged"
                    ? "Merged"
                    : pull().status === "draft"
                      ? "Draft"
                      : pull().status === "closed"
                        ? "Closed"
                        : "Open"}
                </span>
              </StatusBadge>
              <span class="text-xs text-muted-foreground">
                #{pull().number} opened by @{pull().author_username}
              </span>
            </div>
            <h3 class="text-base font-semibold tracking-tight">
              {pull().title}
            </h3>
          </div>
          <div class="flex flex-wrap items-center gap-1.5">
            <Show when={pull().status === "draft"}>
              <Button
                size="sm"
                variant="outline"
                disabled={props.submitting}
                onClick={() => props.onUpdateStatus("open")}
              >
                Ready for review
              </Button>
            </Show>
            <Show when={pull().status === "open"}>
              <Button
                size="sm"
                variant="outline"
                disabled={props.submitting}
                onClick={() => props.onUpdateStatus("draft")}
              >
                Convert to draft
              </Button>
            </Show>
            <Show when={pull().status === "open" || pull().status === "draft"}>
              <Button
                size="sm"
                variant="ghost"
                disabled={props.submitting}
                onClick={() => props.onUpdateStatus("closed")}
              >
                Close
              </Button>
            </Show>
            <Show when={pull().status === "closed"}>
              <Button
                size="sm"
                variant="outline"
                disabled={props.submitting}
                onClick={() => props.onUpdateStatus("open")}
              >
                Reopen
              </Button>
            </Show>
          </div>
        </div>
        <div class="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
          <Icon name="git-branch" size={12} />
          <code>{pull().head_branch}</code>
          <span>into</span>
          <code>{pull().base_branch}</code>
          <span class="ml-auto font-mono">
            {pull().base_sha.slice(0, 8)}..{pull().head_sha.slice(0, 8)}
          </span>
        </div>
        <Show when={pull().description}>
          <p class="whitespace-pre-wrap text-xs text-muted-foreground">
            {pull().description}
          </p>
        </Show>
      </div>

      {/* Merge Preview & Branch Protection Gate */}
      <div class="space-y-3 rounded-lg border bg-card p-4">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <h4 class="flex items-center gap-1.5 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
            <Icon name="shield-check" size={13} />
            Merge preview & branch protection
          </h4>
          <div class="flex flex-wrap items-center gap-2 text-xs">
            <StatusBadge
              tone={
                protection().approved_count >= protection().required_approvals
                  ? "success"
                  : "warning"
              }
            >
              Approvals: {protection().approved_count} /{" "}
              {protection().required_approvals}
            </StatusBadge>
            <Show when={protection().require_passing_checks}>
              <StatusBadge
                tone={protection().checks_passed ? "success" : "danger"}
              >
                {protection().checks_passed
                  ? "Checks passed"
                  : "Checks required"}
              </StatusBadge>
            </Show>
            <Show when={protection().changes_requested}>
              <StatusBadge tone="danger">Changes requested</StatusBadge>
            </Show>
          </div>
        </div>

        <Show when={preview().conflicting_files.length > 0}>
          <div class="rounded-md border border-destructive/40 bg-destructive/10 p-2.5 text-xs">
            <p class="font-medium text-error-foreground">
              Conflicting files preventing automatic merge:
            </p>
            <ul class="mt-1 space-y-1 font-mono">
              <For each={preview().conflicting_files}>
                {(file) => (
                  <li class="flex items-center gap-1.5">
                    <Icon name="x" size={12} />
                    <span>{file}</span>
                  </li>
                )}
              </For>
            </ul>
          </div>
        </Show>

        <Show when={gate().reasons.length > 0}>
          <ul class="space-y-1 text-xs text-muted-foreground">
            <For each={gate().reasons}>
              {(reason) => (
                <li class="flex items-center gap-1.5">
                  <Icon name="clock" size={12} />
                  <span>{reason}</span>
                </li>
              )}
            </For>
          </ul>
        </Show>

        <div class="flex flex-col gap-2 pt-1 sm:flex-row sm:items-center">
          <TextField class="min-w-0 flex-1">
            <TextFieldInput
              aria-label="Merge commit message"
              placeholder={`Merge pull request #${pull().number} from ${pull().head_branch}`}
              value={mergeCommitMessage()}
              onInput={(e) => setMergeCommitMessage(e.currentTarget.value)}
            />
          </TextField>
          <Button
            size="sm"
            disabled={!gate().canMerge || props.submitting}
            onClick={handleMerge}
          >
            <Icon name="git-merge" />
            Merge pull request
          </Button>
        </div>
      </div>

      {/* Submit Review */}
      <div class="space-y-3 rounded-lg border bg-card p-4">
        <h4 class="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          Review head commit ({pull().head_sha.slice(0, 8)})
        </h4>
        <div class="flex flex-col gap-2 sm:flex-row sm:items-center">
          <TextField class="min-w-0 flex-1">
            <TextFieldInput
              aria-label="Review summary"
              placeholder="Leave a review summary bound to the current head SHA..."
              value={reviewBody()}
              onInput={(e) => setReviewBody(e.currentTarget.value)}
            />
          </TextField>
          <div class="flex flex-wrap items-center gap-1.5">
            <Button
              size="sm"
              disabled={props.submitting}
              onClick={() => handleReview("approve")}
            >
              <Icon name="check" />
              Approve
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={props.submitting}
              onClick={() => handleReview("request_changes")}
            >
              <Icon name="x" />
              Request changes
            </Button>
            <Button
              size="sm"
              variant="ghost"
              disabled={props.submitting}
              onClick={() => handleReview("comment")}
            >
              <Icon name="comment" />
              Comment review
            </Button>
          </div>
        </div>

        <Show when={pull().reviews.length > 0}>
          <ul class="space-y-2 border-t pt-3 text-xs">
            <For each={pull().reviews}>
              {(rev) => (
                <li class="rounded-md border bg-muted/20 p-2.5">
                  <div class="flex flex-wrap items-center justify-between gap-2">
                    <span class="font-medium">@{rev.author_username}</span>
                    <StatusBadge
                      tone={
                        rev.verdict === "approve"
                          ? "success"
                          : rev.verdict === "request_changes"
                            ? "danger"
                            : "neutral"
                      }
                    >
                      {rev.verdict === "approve"
                        ? "Approved"
                        : rev.verdict === "request_changes"
                          ? "Changes requested"
                          : "Reviewed"}{" "}
                      · {rev.reviewed_head_sha.slice(0, 7)}
                    </StatusBadge>
                  </div>
                  <Show when={rev.body}>
                    <p class="mt-1 text-muted-foreground">{rev.body}</p>
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </div>

      {/* Review Threads & Inline Comments */}
      <div class="space-y-3 rounded-lg border bg-card p-4">
        <h4 class="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          Discussion & inline diff comments ({pull().comments.length})
        </h4>
        <Show when={pull().comments.length > 0}>
          <ul class="space-y-2 text-xs">
            <For each={pull().comments}>
              {(comment) => (
                <li class="rounded-md border bg-muted/20 p-2.5">
                  <div class="flex flex-wrap items-center justify-between gap-2">
                    <span class="font-medium">@{comment.author_username}</span>
                    <Show when={comment.file_path}>
                      {(path) => (
                        <code class="rounded bg-muted px-1.5 py-0.5 text-[11px]">
                          {path()}
                          {comment.line_number ? `:${comment.line_number}` : ""}
                        </code>
                      )}
                    </Show>
                  </div>
                  <p class="mt-1 text-muted-foreground">{comment.body}</p>
                </li>
              )}
            </For>
          </ul>
        </Show>

        <form class="space-y-2 border-t pt-3" onSubmit={handleAddComment}>
          <div class="grid gap-2 sm:grid-cols-[minmax(0,1fr)_8rem]">
            <TextField>
              <TextFieldInput
                aria-label="Inline file path"
                placeholder="Optional file path (e.g. src/lib.rs)"
                value={commentFilePath()}
                onInput={(e) => setCommentFilePath(e.currentTarget.value)}
              />
            </TextField>
            <TextField>
              <TextFieldInput
                aria-label="Inline line number"
                placeholder="Line #"
                value={commentLineNumber()}
                onInput={(e) => setCommentLineNumber(e.currentTarget.value)}
              />
            </TextField>
          </div>
          <div class="flex flex-col gap-2 sm:flex-row sm:items-center">
            <TextField class="min-w-0 flex-1">
              <TextFieldInput
                aria-label="Comment body"
                placeholder="Add a general or inline diff comment..."
                value={commentBody()}
                onInput={(e) => setCommentBody(e.currentTarget.value)}
              />
            </TextField>
            <Button
              type="submit"
              size="sm"
              variant="outline"
              disabled={props.submitting || !commentBody().trim()}
            >
              <Icon name="comment" />
              Add comment
            </Button>
          </div>
        </form>
      </div>
    </div>
  );
};
