import { Show, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "../components/ui/collapsible";
import { Alert, AlertDescription } from "../components/ui/alert";
import {
  StatusBadge,
  type StatusTone,
} from "../components/workspace/presentation";
import { Icon } from "../components/workspace/icon";
import type { AgentSandboxTask } from "./AgentSwarmView";

export const worktreeStatusTone = (task: AgentSandboxTask): StatusTone =>
  task.verificationNeedsRefresh
    ? "warning"
    : task.sandboxStatus === "verified"
      ? "success"
      : task.sandboxStatus === "failed"
        ? "danger"
        : task.sandboxStatus === "merged"
          ? "merged"
          : task.sandboxStatus === "compiling"
            ? "warning"
            : "neutral";
export const worktreeStatusLabel = (task: AgentSandboxTask): string =>
  task.operation
    ? task.operation.charAt(0).toUpperCase() + task.operation.slice(1) + "..."
    : task.verificationNeedsRefresh
      ? "State unconfirmed"
      : task.sandboxStatus === "verified"
        ? "Ready for review"
        : task.sandboxStatus === "compiling"
          ? "Running"
          : task.sandboxStatus === "merged"
            ? task.githubSynced
              ? "Published"
              : "Merged locally"
            : task.sandboxStatus === "pending"
              ? "Needs verification"
              : task.sandboxStatus === "failed"
                ? "Failed"
                : "Discarded";

interface AgentWorktreeDetailsProps {
  task: AgentSandboxTask;
  liveMode: boolean;
  canRunVerification: boolean;
  onBack: () => void;
  onApproveMerge: (id: number) => void;
  onDiscardBranch: (id: number) => void;
  onPublishGithub: (id: number) => void;
  onReviewWorktree?: (id: number) => void;
  onViewRunHistory?: (id: string) => void;
  onVerify: () => void;
  onRefresh: () => void;
}

export const AgentWorktreeDetails: Component<AgentWorktreeDetailsProps> = (
  props,
) => {
  const terminal = () =>
    props.task.sandboxStatus === "merged" ||
    props.task.sandboxStatus === "discarded";
  const canApprove = () =>
    !props.task.operation &&
    props.task.sandboxStatus === "verified" &&
    (!props.task.liveId ||
      Boolean(
        props.task.verifiedCommit && !props.task.verificationNeedsRefresh,
      ));
  const explanation = () =>
    props.task.verificationNeedsRefresh
      ? "Refresh the server state before verifying or approving this worktree."
      : props.task.sandboxStatus === "verified"
        ? "Review the verified revision, then approve a local merge."
        : props.task.sandboxStatus === "merged"
          ? props.task.githubSynced
            ? "The approved commit has been published upstream."
            : "Merged locally. Publish upstream when you are ready."
          : props.task.sandboxStatus === "compiling"
            ? "A command is running. Approval is unavailable until verification completes."
            : props.task.sandboxStatus === "discarded"
              ? "This worktree has been discarded."
              : "Verify a clean, committed revision before approving its merge.";
  return (
    <div class="min-w-0">
      <div class="flex min-w-0 items-center justify-between gap-2 border-b px-3 py-2 sm:px-5">
        <Button
          variant="ghost"
          size="sm"
          class="-ml-2 md:hidden"
          aria-label="Back to worktrees"
          onClick={props.onBack}
        >
          <Icon name="chevron-right" class="rotate-180" /> Worktrees
        </Button>
        <span class="hidden text-xs text-muted-foreground md:block">
          Worktree
        </span>
        <Show
          when={
            props.liveMode &&
            props.canRunVerification &&
            props.onViewRunHistory &&
            props.task.liveId
          }
        >
          <Button
            variant="ghost"
            size="sm"
            class="ml-auto"
            onClick={(event) => {
              event.currentTarget.focus();
              props.onViewRunHistory?.(props.task.liveId!);
            }}
          >
            <Icon name="clock" /> Run history
          </Button>
        </Show>
      </div>
      <div class="space-y-5 p-4 sm:p-5 lg:p-6">
        <header class="min-w-0 space-y-3">
          <StatusBadge
            tone={worktreeStatusTone(props.task)}
            class="max-w-full truncate"
          >
            {worktreeStatusLabel(props.task)}
          </StatusBadge>
          <h2
            class="truncate text-lg font-semibold tracking-tight sm:text-xl"
            title={props.task.promptSummary}
          >
            {props.task.promptSummary}
          </h2>
          <div class="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
            <Icon name="git-branch" />
            <span class="truncate font-mono" title={props.task.branch}>
              {props.task.branch}
            </span>
          </div>
        </header>
        <p class="max-w-lg text-sm leading-relaxed text-muted-foreground">
          {explanation()}
        </p>
        <Show when={props.task.verificationNeedsRefresh}>
          <Button
            variant="outline"
            size="sm"
            disabled={Boolean(props.task.operation)}
            onClick={props.onRefresh}
          >
            <Icon name="rotate-ccw" /> Refresh worktrees
          </Button>
        </Show>
        <Show when={props.task.verificationError}>
          <Alert variant="destructive" class="min-w-0 break-words">
            <AlertDescription>{props.task.verificationError}</AlertDescription>
          </Alert>
        </Show>
        <Show when={props.task.publicationError}>
          <Alert variant="destructive" class="min-w-0 break-words">
            <AlertDescription>{props.task.publicationError}</AlertDescription>
          </Alert>
        </Show>
        <Show when={props.task.verifiedCommit}>
          <div class="min-w-0 space-y-1 rounded-lg border p-3 text-xs">
            <span class="text-muted-foreground">Verified revision</span>
            <code class="block break-all">{props.task.verifiedCommit}</code>
          </div>
        </Show>
        <div class="flex flex-wrap items-center gap-2 border-y py-4">
          <Show
            when={
              props.task.liveId &&
              (!terminal() || props.task.verificationResult)
            }
          >
            <Button
              size="sm"
              variant={
                props.task.sandboxStatus === "verified" || terminal()
                  ? "outline"
                  : "default"
              }
              onClick={props.onVerify}
            >
              <Icon name="shield-check" />{" "}
              {props.task.verificationResult
                ? "Verification result"
                : "Verify command"}
            </Button>
          </Show>
          <Show when={!terminal() && props.task.sandboxStatus === "verified"}>
            <Button
              size="sm"
              disabled={!canApprove()}
              onClick={() => props.onApproveMerge(props.task.id)}
            >
              <Icon name="git-merge" /> Approve &amp; merge
            </Button>
          </Show>
          <Show
            when={
              props.task.sandboxStatus === "merged" &&
              !props.task.githubSynced &&
              props.task.liveId
            }
          >
            <Button
              size="sm"
              disabled={Boolean(props.task.operation)}
              title={
                props.task.publicationError ||
                "Publish this approved commit to the configured upstream"
              }
              onClick={() => props.onPublishGithub(props.task.id)}
            >
              <Icon name="git-branch" />
              {props.task.operation === "publishing"
                ? "Publishing..."
                : props.task.publicationError
                  ? "Retry publication"
                  : "Publish upstream"}
            </Button>
          </Show>
          <Show
            when={props.liveMode && props.task.liveId && props.onReviewWorktree}
          >
            <Button
              size="sm"
              variant="outline"
              aria-label="Review committed changes"
              onClick={() => props.onReviewWorktree?.(props.task.id)}
            >
              <Icon name="diff" /> Review changes
            </Button>
          </Show>
        </div>
        <Collapsible>
          <CollapsibleTrigger
            as={Button}
            variant="ghost"
            size="sm"
            class="-ml-2 text-muted-foreground"
          >
            <Icon name="sliders" /> Worktree details
          </CollapsibleTrigger>
          <CollapsibleContent class="space-y-4 pt-3">
            <dl class="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-3 text-xs">
              <dt class="text-muted-foreground">Agent</dt>
              <dd class="truncate" title={props.task.agent}>
                {props.task.agent}
              </dd>
              <dt class="text-muted-foreground">Merge target</dt>
              <dd class="truncate font-mono" title={props.task.baseBranch}>
                {props.task.baseBranch}
              </dd>
              <Show when={props.task.liveId}>
                <dt class="text-muted-foreground">Worktree ID</dt>
                <dd class="truncate font-mono" title={props.task.liveId}>
                  {props.task.liveId}
                </dd>
              </Show>
              <Show when={props.task.verificationResult}>
                <dt class="text-muted-foreground">Last run</dt>
                <dd>
                  {props.task.verificationResult?.durationMs}ms · Exit{" "}
                  {props.task.verificationResult?.exitCode}
                </dd>
              </Show>
              <Show when={!props.liveMode}>
                <dt class="text-muted-foreground">Sample changes</dt>
                <dd>
                  +{props.task.additions} / −{props.task.deletions}
                </dd>
              </Show>
            </dl>
            <Show when={!terminal()}>
              <Button
                size="sm"
                variant="ghost"
                class="text-destructive"
                disabled={
                  Boolean(props.task.operation) ||
                  props.task.sandboxStatus === "compiling"
                }
                onClick={() => props.onDiscardBranch(props.task.id)}
              >
                Discard worktree
              </Button>
            </Show>
          </CollapsibleContent>
        </Collapsible>
      </div>
    </div>
  );
};
