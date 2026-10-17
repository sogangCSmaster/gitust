import {
  createEffect,
  createMemo,
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
  type Component,
} from "solid-js";
import { Button } from "../components/ui/button";
import { Alert, AlertDescription } from "../components/ui/alert";
import { EmptyState, StatusBadge } from "../components/workspace/presentation";
import { Icon } from "../components/workspace/icon";
import {
  activityLabel,
  activityTone,
} from "../patterns/run-activity-presentation";
import type { AutomaticRunJob } from "../patterns/run-history-types";
import { LiveRunHistoryDialog } from "./LiveRunHistoryDialog";
import {
  createLiveActionsClient,
  type LiveActionsState,
} from "./live-actions-client";
import type { AutomaticWorktreeGroup } from "./live-actions-page";

export const LiveActionsView: Component<{
  repositoryId?: string;
  role?: string;
}> = (props) => {
  const [state, setState] = createSignal<LiveActionsState>({
    status: "idle",
    page: null,
  });
  const [target, setTarget] = createSignal<{
    worktreeId: string;
    branch: string;
    runId: string;
  } | null>(null);
  const canRead = () => props.role === "SiteAdmin";
  const client = createLiveActionsClient(setState, {
    repositoryId: () => props.repositoryId,
    canRead,
  });
  const groups = createMemo(
    () => state().page?.worktrees.filter((group) => group.runs.length) ?? [],
  );
  createEffect(() => {
    const repositoryId = props.repositoryId;
    const role = props.role;
    setTarget(null);
    client.clear();
    if (repositoryId && role === "SiteAdmin") void client.load(repositoryId);
  });
  const sessionChanged = (event: StorageEvent) => {
    if (event.key === "gitust_token" || event.key === null) {
      setTarget(null);
      client.clear();
    }
  };
  onMount(() => window.addEventListener("storage", sessionChanged));
  onCleanup(() => {
    client.clear();
    window.removeEventListener("storage", sessionChanged);
  });
  const openActivity = (
    group: AutomaticWorktreeGroup,
    run: AutomaticRunJob,
  ) => {
    if (
      client.isCurrent() &&
      canRead() &&
      state().page?.worktrees.some(
        (current) =>
          current.worktreeId === group.worktreeId &&
          current.runs.some((job) => job.runId === run.runId),
      )
    )
      setTarget({
        worktreeId: group.worktreeId,
        branch: group.branch,
        runId: run.runId,
      });
  };
  const load = (cursor: string | null = null) => {
    setTarget(null);
    if (props.repositoryId) void client.load(props.repositoryId, cursor);
  };
  return (
    <section class="min-w-0 space-y-3" aria-label="Automatic Actions">
      <Show
        when={props.role && props.repositoryId}
        fallback={
          <p role="status" class="text-sm text-muted-foreground">
            Checking repository access…
          </p>
        }
      >
        <Show
          when={canRead()}
          fallback={
            <EmptyState
              title="Administrator access required"
              description="Sign in as a SiteAdmin to view automatic Checks diagnostics."
            />
          }
        >
          <div class="flex min-w-0 items-center justify-between gap-3">
            <p class="min-w-0 text-xs text-muted-foreground">
              Automatic Checks for this page of worktrees.
            </p>
            <Button
              size="sm"
              variant="outline"
              class="shrink-0"
              onClick={() => load()}
            >
              <Icon name="rotate-ccw" /> Refresh
            </Button>
          </div>
          <Show
            when={state().status === "idle" || state().status === "loading"}
          >
            <p role="status" class="text-sm text-muted-foreground">
              Loading automatic Actions…
            </p>
          </Show>
          <Show when={state().status === "error"}>
            <Alert variant="destructive">
              <AlertDescription>{state().error}</AlertDescription>
            </Alert>
          </Show>
          <Show when={state().status === "ready"}>
            <Show
              when={groups().length}
              fallback={
                <EmptyState
                  title="No attempts on this page"
                  description="Enabled automatic Checks run after MCP commits or Git pushes to managed worktrees."
                />
              }
            >
              <div class="min-w-0 space-y-3">
                <For each={groups()}>
                  {(group) => (
                    <section
                      class="overflow-hidden rounded-lg border bg-card"
                      aria-label={`Automatic attempts for ${group.branch}`}
                    >
                      <h2 class="flex min-w-0 items-center gap-2 border-b px-3 py-2 text-xs font-medium">
                        <Icon
                          name="git-branch"
                          size={13}
                          class="shrink-0 text-muted-foreground"
                        />
                        <span class="truncate" title={group.branch}>
                          {group.branch}
                        </span>
                      </h2>
                      <ol class="min-w-0 divide-y">
                        <For each={group.runs}>
                          {(run) => {
                            const entry = () => ({
                              runId: run.runId,
                              job: run,
                            });
                            return (
                              <li class="min-w-0">
                                <Button
                                  variant="ghost"
                                  class="grid h-11 w-full min-w-0 grid-cols-[minmax(0,1fr)_auto_auto] gap-3 rounded-none px-3 text-left sm:h-9"
                                  aria-label={`${activityLabel(entry())} ${group.branch} ${run.expected.slice(0, 7)} · ${new Date(run.createdAt).toLocaleString()}`}
                                  onClick={() => openActivity(group, run)}
                                >
                                  <span class="min-w-0 truncate">
                                    <StatusBadge
                                      tone={activityTone(entry())}
                                      class="max-w-full truncate text-[11px]"
                                    >
                                      {activityLabel(entry())}
                                    </StatusBadge>
                                  </span>
                                  <code
                                    class="shrink-0 text-[11px] font-normal text-muted-foreground"
                                    title={run.expected}
                                  >
                                    {run.expected.slice(0, 7)}
                                  </code>
                                  <time
                                    class="shrink-0 whitespace-nowrap text-[11px] font-normal text-muted-foreground"
                                    dateTime={run.createdAt}
                                    title={new Date(
                                      run.createdAt,
                                    ).toLocaleString()}
                                  >
                                    {new Date(run.createdAt).toLocaleTimeString(
                                      [],
                                      {
                                        hour: "2-digit",
                                        minute: "2-digit",
                                        hourCycle: "h23",
                                      },
                                    )}
                                  </time>
                                </Button>
                              </li>
                            );
                          }}
                        </For>
                      </ol>
                    </section>
                  )}
                </For>
              </div>
            </Show>
            <div class="flex min-w-0 items-center justify-between gap-3 text-xs text-muted-foreground">
              <p class="min-w-0">
                Refresh starts from the first page of worktrees.
              </p>
              <Show when={state().page?.nextCursor}>
                {(cursor) => (
                  <Button
                    size="sm"
                    variant="outline"
                    class="shrink-0"
                    onClick={() => load(cursor())}
                  >
                    Next <Icon name="chevron-right" />
                  </Button>
                )}
              </Show>
            </div>
          </Show>
        </Show>
      </Show>
      <Show when={target()} keyed>
        {(selected) => (
          <LiveRunHistoryDialog
            worktreeId={selected.worktreeId}
            branch={selected.branch}
            initialRunId={selected.runId}
            onClose={() => setTarget(null)}
          />
        )}
      </Show>
    </section>
  );
};
