import { filterAgentSwarmTasks } from "./worktree-filter";
export { filterAgentSwarmTasks } from "./worktree-filter";
import {
  createEffect,
  createMemo,
  createSignal,
  For,
  Show,
  untrack,
  type Component,
} from "solid-js";
import { Button } from "../components/ui/button";
import { Card } from "../components/ui/card";
import { EmptyState, Field } from "../components/workspace/presentation";
import { Icon } from "../components/workspace/icon";
import { AgentSwarmHeaderBanners } from "./AgentSwarmHeaderBanners";
import { AgentVerificationDialog } from "./AgentVerificationDialog";
import {
  AgentWorktreeDetails,
  worktreeStatusLabel,
  worktreeStatusTone,
} from "./AgentWorktreeDetails";
import { cn } from "../lib/utils";

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
  operation?:
    "creating" | "approving" | "discarding" | "publishing" | "verifying";
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
  onReviewWorktree?: (id: number) => void;
  canRunVerification: boolean;
  liveMode: boolean;
  onSpawnBatchSwarm: () => void;
  onTriggerGithubSync: () => void;
  filterQuery?: string;
  onFilterQueryChange?: (query: string) => void;
}

const taskKey = (task: AgentSandboxTask) => task.liveId ?? `demo-${task.id}`;

export const AgentSwarmView: Component<AgentSwarmViewProps> = (props) => {
  const [localFilter, setLocalFilter] = createSignal("");
  const [verificationId, setVerificationId] = createSignal<string | null>(null);
  const [mobileDetail, setMobileDetail] = createSignal(false);
  const filter = () => props.filterQuery ?? localFilter();
  const filteredTasks = createMemo(() =>
    filterAgentSwarmTasks(props.tasks, filter()),
  );
  const initialTask = filteredTasks()[props.focusedIndex];
  const [selectedKey, setSelectedKey] = createSignal<string | undefined>(
    initialTask ? taskKey(initialTask) : undefined,
  );
  const selectedTask = createMemo(() =>
    filteredTasks().find((task) => taskKey(task) === selectedKey()),
  );
  const verificationTask = () =>
    props.tasks.find((task) => task.liveId === verificationId());
  let previousIndex = props.focusedIndex;
  let list: HTMLUListElement | undefined;
  let detail: HTMLElement | undefined;
  createEffect(() => {
    const tasks = filteredTasks();
    const requested = props.focusedIndex;
    const key = untrack(selectedKey);
    const retained = tasks.findIndex((task) => taskKey(task) === key);
    const indexChanged = requested !== previousIndex;
    previousIndex = requested;
    const next =
      !indexChanged && retained >= 0
        ? retained
        : Math.max(0, Math.min(requested, tasks.length - 1));
    if (retained < 0 && key !== undefined) setMobileDetail(false);
    setSelectedKey(tasks[next] ? taskKey(tasks[next]) : undefined);
    if (tasks.length && requested !== next) props.onFocusIndexChange(next);
    list?.children[next]?.scrollIntoView?.({ block: "nearest" });
  });
  createEffect(() => {
    const id = verificationId();
    if (id && !props.tasks.some((task) => task.liveId === id))
      setVerificationId(null);
  });
  const backToList = () => {
    setMobileDetail(false);
    list?.querySelector<HTMLButtonElement>('[aria-pressed="true"]')?.focus();
  };
  const stats = createMemo(() => ({
    total: props.tasks.length,
    verified: props.tasks.filter(
      (task) =>
        task.sandboxStatus === "verified" && !task.verificationNeedsRefresh,
    ).length,
    compiling: props.tasks.filter((task) => task.sandboxStatus === "compiling")
      .length,
    failed: props.tasks.filter((task) => task.sandboxStatus === "failed")
      .length,
    merged: props.tasks.filter((task) => task.sandboxStatus === "merged")
      .length,
  }));
  return (
    <Card class="min-w-0 overflow-hidden shadow-none">
      <div
        class="grid min-w-0 md:min-h-[34rem] md:grid-cols-[minmax(15rem,0.85fr)_minmax(0,1.5fr)] lg:grid-cols-[minmax(16rem,0.75fr)_minmax(0,1.65fr)]"
        data-mobile-detail={mobileDetail()}
      >
        <section
          aria-label="Worktree list"
          class={cn(
            "min-w-0 flex-col border-border bg-muted/20 md:flex md:border-r",
            mobileDetail() ? "hidden" : "flex",
          )}
        >
          <div class="space-y-3 border-b p-3 sm:p-4">
            <div class="flex min-w-0 items-center justify-between gap-2">
              <h2 class="truncate text-sm font-semibold">
                Worktrees{" "}
                <span class="ml-1 text-xs font-normal text-muted-foreground">
                  {props.tasks.length}
                </span>
              </h2>
              <div class="flex shrink-0 items-center gap-1">
                <Show when={props.liveMode}>
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label="Refresh worktrees"
                    title="Refresh worktrees"
                    onClick={props.onRefreshWorktrees}
                  >
                    <Icon name="rotate-ccw" />
                  </Button>
                </Show>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={
                    props.liveMode ? "New worktree" : "New demo worktrees"
                  }
                  title={props.liveMode ? "New worktree" : "New demo worktrees"}
                  onClick={props.onSpawnBatchSwarm}
                >
                  <Icon name="plus" />
                </Button>
              </div>
            </div>
            <Field
              label="Search worktrees"
              hideLabel
              type="search"
              placeholder="Search worktrees"
              value={filter()}
              onValueChange={(value) => {
                setLocalFilter(value);
                setSelectedKey(undefined);
                setMobileDetail(false);
                props.onFilterQueryChange?.(value);
                props.onFocusIndexChange(0);
              }}
            />
          </div>
          <Show when={filteredTasks().length === 0}>
            <EmptyState
              title={filter() ? "No matching worktrees" : "A clean slate"}
              description={
                filter()
                  ? "Try a different search."
                  : "Create a worktree to start a change."
              }
            />
          </Show>
          <ul
            ref={list}
            aria-label="Agent worktrees"
            class="min-w-0 flex-1 space-y-1 p-2 md:max-h-[40rem] md:overflow-y-auto"
          >
            <For each={filteredTasks()}>
              {(task, index) => (
                <li class="min-w-0">
                  <Button
                    variant="ghost"
                    class={cn(
                      "h-auto w-full min-w-0 justify-start gap-2.5 px-3 py-3 text-left font-normal",
                      selectedKey() === taskKey(task) &&
                        "bg-accent text-accent-foreground",
                    )}
                    aria-pressed={selectedKey() === taskKey(task)}
                    onClick={() => {
                      setSelectedKey(taskKey(task));
                      props.onFocusIndexChange(index());
                      setMobileDetail(true);
                      queueMicrotask(() => {
                        const back = detail?.querySelector<HTMLButtonElement>(
                          '[aria-label="Back to worktrees"]',
                        );
                        if (back?.isConnected && back.getClientRects().length)
                          back.focus();
                      });
                    }}
                  >
                    <span
                      class={cn(
                        "mt-0.5 self-start",
                        worktreeStatusTone(task) === "success"
                          ? "text-success-foreground"
                          : worktreeStatusTone(task) === "danger"
                            ? "text-error-foreground"
                            : "text-muted-foreground",
                      )}
                    >
                      <Icon
                        name={
                          task.sandboxStatus === "verified"
                            ? "shield-check"
                            : task.sandboxStatus === "merged"
                              ? "git-merge"
                              : task.sandboxStatus === "compiling"
                                ? "play"
                                : "git-branch"
                        }
                      />
                    </span>
                    <span class="min-w-0 flex-1 space-y-1">
                      <span
                        class="block truncate text-sm font-medium"
                        title={task.promptSummary}
                      >
                        {task.promptSummary}
                      </span>
                      <span
                        class="block truncate font-mono text-[11px] text-muted-foreground"
                        title={task.branch}
                      >
                        {task.branch}
                      </span>
                      <span class="block truncate text-[11px] text-muted-foreground">
                        {worktreeStatusLabel(task)}
                      </span>
                    </span>
                  </Button>
                </li>
              )}
            </For>
          </ul>
          <div class="border-t px-3 py-2 sm:px-4">
            <AgentSwarmHeaderBanners
              stats={stats()}
              onTriggerGithubSync={props.onTriggerGithubSync}
            />
          </div>
        </section>
        <section
          ref={detail}
          aria-label="Selected worktree"
          class={cn("min-w-0 md:block", mobileDetail() ? "block" : "hidden")}
        >
          <Show
            when={selectedTask()}
            fallback={
              <div class="hidden p-8 text-sm text-muted-foreground md:block">
                Select a worktree to review its changes.
              </div>
            }
          >
            {(task) => (
              <AgentWorktreeDetails
                task={task()}
                liveMode={props.liveMode}
                canRunVerification={props.canRunVerification}
                onBack={backToList}
                onApproveMerge={props.onApproveMerge}
                onDiscardBranch={props.onDiscardBranch}
                onPublishGithub={props.onPublishGithub}
                onReviewWorktree={props.onReviewWorktree}
                onViewRunHistory={props.onViewRunHistory}
                onVerify={() => setVerificationId(task().liveId ?? null)}
                onRefresh={props.onRefreshWorktrees}
              />
            )}
          </Show>
        </section>
      </div>
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
    </Card>
  );
};
