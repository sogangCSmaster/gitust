import {
  createEffect,
  createMemo,
  createSignal,
  For,
  Show,
  type Component,
} from "solid-js";
import { Button } from "../components/ui/button";
import { Card } from "../components/ui/card";
import {
  Collapsible,
  CollapsibleTrigger,
  CollapsibleContent,
} from "../components/ui/collapsible";
import {
  EmptyState,
  Field,
  StatusBadge,
  type StatusTone,
} from "../components/workspace/presentation";
import { Icon } from "../components/workspace/icon";
import { AgentSwarmHeaderBanners } from "./AgentSwarmHeaderBanners";
import { AgentVerificationDialog } from "./AgentVerificationDialog";

export type AgentProvider =
  "Claude Code" | "Cursor Agent" | "Codex Swarm" | "Internal Bot";

export interface AgentVerificationStep {
  command: string;
  required: boolean;
  exitCode: number | null;
  durationMs: number | null;
  output?: { stdout: string; stderr: string; truncated: boolean };
}

export interface AgentVerificationResult {
  requestedCommand?: string;
  checkedCommit?: string | null;
  command: string;
  exitCode: number;
  durationMs: number;
  stdout: string;
  stderr: string;
  outputTruncated: boolean;
  steps?: AgentVerificationStep[];
}

export interface AgentSandboxTask {
  id: number;
  liveId?: string;
  agent: AgentProvider;
  branch: string;
  baseBranch: string;
  forkLatencyMs: number;
  promptSummary: string;
  sandboxStatus:
    "pending" | "verified" | "compiling" | "failed" | "merged" | "discarded";
  compileTimeMs: number;
  testsPassed: number;
  testsTotal: number;
  filesChanged: number;
  additions: number;
  deletions: number;
  githubSynced?: boolean;
  operation?: "approving" | "discarding" | "publishing" | "verifying";
  verifiedCommit?: string;
  verificationResult?: AgentVerificationResult;
  verificationError?: string;
  verificationNeedsRefresh?: boolean;
  publicationError?: string;
  updatedAt: string;
}

export interface AgentSwarmViewProps {
  tasks: AgentSandboxTask[];
  focusedIndex: number;
  onFocusIndexChange: (idx: number) => void;
  onApproveMerge: (id: number) => void;
  onDiscardBranch: (id: number) => void;
  onPublishGithub: (id: number) => void;
  onVerifyWorktree: (id: number, command: string) => void;
  onRefreshWorktrees: () => void;
  onViewRunHistory?: (worktreeId: string) => void;
  canRunVerification: boolean;
  liveMode: boolean;
  onSpawnBatchSwarm: () => void;
  onTriggerGithubSync: () => void;
  filterQuery?: string;
  onFilterQueryChange?: (query: string) => void;
}

export function filterAgentSwarmTasks(
  tasks: AgentSandboxTask[],
  rawQuery: string,
): AgentSandboxTask[] {
  const q = rawQuery.trim().toLowerCase();
  if (!q) return tasks;
  return tasks.filter(
    (t) =>
      t.branch.toLowerCase().includes(q) ||
      t.agent.toLowerCase().includes(q) ||
      t.promptSummary.toLowerCase().includes(q) ||
      t.sandboxStatus.toLowerCase().includes(q),
  );
}

export const AgentSwarmView: Component<AgentSwarmViewProps> = (props) => {
  const [localFilter, setLocalFilter] = createSignal("");
  const [verificationId, setVerificationId] = createSignal<string | null>(null);
  const verificationTask = () =>
    props.tasks.find((task) => task.liveId === verificationId());
  const filter = () => props.filterQuery ?? localFilter();
  const filteredTasks = createMemo(() =>
    filterAgentSwarmTasks(props.tasks, filter()),
  );
  let list: HTMLUListElement | undefined;
  createEffect(() => {
    const index = props.focusedIndex;
    filteredTasks();
    list?.children[index]?.scrollIntoView?.({ block: "nearest" });
  });
  const stats = createMemo(() => ({
    total: props.tasks.length,
    verified: props.tasks.filter((task) => task.sandboxStatus === "verified")
      .length,
    compiling: props.tasks.filter((task) => task.sandboxStatus === "compiling")
      .length,
    failed: props.tasks.filter((task) => task.sandboxStatus === "failed")
      .length,
    merged: props.tasks.filter((task) => task.sandboxStatus === "merged")
      .length,
  }));
  const tone = (task: AgentSandboxTask): StatusTone =>
    task.sandboxStatus === "verified"
      ? "success"
      : task.sandboxStatus === "failed"
        ? "danger"
        : task.sandboxStatus === "merged"
          ? "merged"
          : task.sandboxStatus === "compiling"
            ? "warning"
            : "neutral";
  return (
    <div class="space-y-3">
      <AgentSwarmHeaderBanners
        stats={stats()}
        onTriggerGithubSync={props.onTriggerGithubSync}
      />
      <div class="flex flex-wrap items-center justify-between gap-3">
        <Field
          class="w-full sm:max-w-xs"
          label="Search worktrees"
          hideLabel
          type="search"
          placeholder="Search worktrees..."
          value={filter()}
          onValueChange={(value) => {
            setLocalFilter(value);
            props.onFilterQueryChange?.(value);
            props.onFocusIndexChange(0);
          }}
        />
        <div class="ml-auto flex items-center gap-2">
          <Show when={props.liveMode}>
            <Button
              variant="ghost"
              size="icon"
              aria-label="Refresh worktrees"
              onClick={props.onRefreshWorktrees}
            >
              <Icon name="rotate-ccw" />
            </Button>
          </Show>
          <Button onClick={props.onSpawnBatchSwarm}>
            <Icon name="plus" />
            {props.liveMode ? "New worktree" : "New demo worktrees"}
          </Button>
        </div>
      </div>
      <Card class="overflow-hidden">
        <Show when={filteredTasks().length === 0}>
          <EmptyState
            title={filter() ? "No matching worktrees" : "A clean slate"}
            description={
              filter()
                ? "Try a different search."
                : "Create a worktree to start your next change."
            }
          />
        </Show>
        <ul ref={list} aria-label="Agent worktrees" class="divide-y">
          <For each={filteredTasks()}>
            {(task, index) => (
              <li
                class="space-y-3 p-4 transition-colors data-[focused=true]:bg-muted/50"
                data-focused={index() === props.focusedIndex}
                onMouseEnter={() => props.onFocusIndexChange(index())}
                onClick={() => props.onFocusIndexChange(index())}
              >
                <div class="flex flex-wrap items-start justify-between gap-3">
                  <div class="min-w-0 flex-1 basis-60">
                    <h2 class="break-words font-medium">
                      {task.promptSummary}
                    </h2>
                    <div class="mt-2 flex flex-wrap gap-x-3 gap-y-1.5 text-xs text-muted-foreground">
                      <span class="break-all font-mono">
                        <Icon name="git-branch" size={12} /> {task.branch}
                      </span>
                      <span>{task.agent}</span>
                    </div>
                  </div>
                  <StatusBadge tone={tone(task)}>
                    {task.operation
                      ? task.operation
                      : task.verificationNeedsRefresh
                        ? "State unconfirmed"
                        : task.sandboxStatus === "verified"
                          ? "Ready for review"
                          : task.sandboxStatus}
                  </StatusBadge>
                </div>
                <div class="flex flex-wrap items-end justify-between gap-3">
                  <Collapsible class="min-w-0 text-xs text-muted-foreground">
                    <CollapsibleTrigger class="inline-flex items-center gap-1 rounded py-1 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                      <Icon name="chevron-right" size={12} />
                      Execution details
                    </CollapsibleTrigger>
                    <CollapsibleContent class="mt-2 space-y-1 break-words">
                      <p>
                        Worktree: {task.forkLatencyMs}ms · Verification:{" "}
                        {task.compileTimeMs}ms
                      </p>
                      <p>
                        {task.testsTotal
                          ? `${task.testsPassed}/${task.testsTotal} tests`
                          : "No test counts"}{" "}
                        · +{task.additions} / −{task.deletions}
                      </p>
                      <Show when={task.verifiedCommit}>
                        <p class="break-all font-mono">
                          Verified {task.verifiedCommit}
                        </p>
                      </Show>
                      <Show when={task.sandboxStatus === "merged"}>
                        <p>
                          {task.githubSynced
                            ? "Published upstream"
                            : "Merged locally. Publication is separate."}
                        </p>
                      </Show>
                    </CollapsibleContent>
                  </Collapsible>
                  <div class="flex flex-wrap gap-2">
                    <Show
                      when={
                        props.liveMode &&
                        props.canRunVerification &&
                        props.onViewRunHistory &&
                        task.liveId
                      }
                    >
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={(event) => {
                          event.currentTarget.focus();
                          props.onViewRunHistory?.(task.liveId!);
                        }}
                      >
                        <Icon name="clock" /> Run history
                      </Button>
                    </Show>
                    <Show
                      when={
                        task.liveId &&
                        ((task.sandboxStatus !== "merged" &&
                          task.sandboxStatus !== "discarded") ||
                          task.verificationResult)
                      }
                    >
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() => setVerificationId(task.liveId ?? null)}
                      >
                        {task.verificationResult
                          ? "Verification result"
                          : "Verify command"}
                      </Button>
                    </Show>
                    <Show
                      when={
                        task.sandboxStatus === "merged" &&
                        !task.githubSynced &&
                        task.liveId
                      }
                    >
                      <Button
                        size="sm"
                        disabled={Boolean(task.operation)}
                        title={
                          task.publicationError ||
                          "Publish this approved commit to the configured upstream"
                        }
                        onClick={() => props.onPublishGithub(task.id)}
                      >
                        {task.operation === "publishing"
                          ? "Publishing..."
                          : task.publicationError
                            ? "Retry publication"
                            : "Publish upstream"}
                      </Button>
                      <Show when={task.publicationError}>
                        <p
                          role="status"
                          class="basis-full text-xs text-error-foreground"
                        >
                          {task.publicationError}
                        </p>
                      </Show>
                    </Show>
                    <Show
                      when={
                        task.sandboxStatus !== "merged" &&
                        task.sandboxStatus !== "discarded"
                      }
                    >
                      <Button
                        size="sm"
                        variant="secondary"
                        disabled={
                          Boolean(task.operation) ||
                          task.sandboxStatus !== "verified" ||
                          Boolean(
                            task.liveId &&
                            (!task.verifiedCommit ||
                              task.verificationNeedsRefresh),
                          )
                        }
                        onClick={() => props.onApproveMerge(task.id)}
                      >
                        <Icon name="git-merge" />
                        Approve &amp; merge
                      </Button>
                      <Button
                        size="sm"
                        variant="ghost"
                        disabled={
                          Boolean(task.operation) ||
                          task.sandboxStatus === "compiling"
                        }
                        onClick={() => props.onDiscardBranch(task.id)}
                      >
                        Discard
                      </Button>
                    </Show>
                  </div>
                </div>
              </li>
            )}
          </For>
        </ul>
      </Card>
      <Show when={verificationTask()}>
        {(task) => (
          <AgentVerificationDialog
            task={task()}
            canRun={props.canRunVerification}
            onRun={props.onVerifyWorktree}
            onRefresh={props.onRefreshWorktrees}
            onClose={() => setVerificationId(null)}
          />
        )}
      </Show>
    </div>
  );
};
