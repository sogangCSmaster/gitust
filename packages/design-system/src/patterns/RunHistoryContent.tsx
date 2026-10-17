import { createMemo, createSignal, For, Show, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import { Alert, AlertDescription } from "../components/ui/alert";
import { EmptyState, StatusBadge } from "../components/workspace/presentation";
import { Icon } from "../components/workspace/icon";
import { VerificationResultDetails } from "./VerificationResultDetails";
import {
  activityCommit,
  activityTime,
  mergeRunActivity,
} from "./run-activity-model";
import {
  activityExplanation,
  activityLabel,
  activityTone,
} from "./run-activity-presentation";
import type { RunHistoryDialogProps } from "./RunHistoryDialog";

export const RunHistoryContent: Component<RunHistoryDialogProps> = (props) => {
  const [selectedId, setSelectedId] = createSignal<string | null>(
    props.initialRunId ?? null,
  );
  const activity = createMemo(() =>
    props.state.status === "ready"
      ? mergeRunActivity(props.state.records, props.state.jobs)
      : [],
  );
  const selected = createMemo(
    () =>
      activity().find((entry) => entry.runId === selectedId()) ??
      (props.initialRunId ? undefined : activity()[0]),
  );
  return (
    <div class="min-w-0 space-y-3">
      <div class="flex min-w-0 items-center justify-between gap-3">
        <p class="min-w-0 text-xs text-muted-foreground">Diagnostics only</p>
        <Button size="sm" variant="outline" onClick={props.onRefresh}>
          <Icon name="rotate-ccw" /> Refresh activity
        </Button>
      </div>
      <Show
        when={props.state.status === "idle" || props.state.status === "loading"}
      >
        <p role="status" class="text-sm text-muted-foreground">
          Loading activity…
        </p>
      </Show>
      <Show when={props.state.status === "error"}>
        <Alert variant="destructive">
          <AlertDescription>{props.state.error}</AlertDescription>
        </Alert>
      </Show>
      <Show when={props.state.status === "ready" && !activity().length}>
        <EmptyState
          title={
            props.initialRunId
              ? "This run is no longer retained"
              : "No activity yet"
          }
          description={
            props.initialRunId
              ? "The selected attempt is not available in this worktree's retained activity. Refresh to check again."
              : "Completed runs and automatic attempts appear here. Retained history is limited."
          }
        />
      </Show>
      <Show when={activity().length}>
        <div class="grid min-w-0 gap-3 sm:grid-cols-[12rem_minmax(0,1fr)]">
          <ol
            aria-label="Run activity"
            class="max-h-44 min-w-0 space-y-1 overflow-y-auto sm:max-h-[60svh]"
          >
            <For each={activity()}>
              {(entry) => (
                <li class="min-w-0">
                  <Button
                    variant={
                      selected()?.runId === entry.runId ? "secondary" : "ghost"
                    }
                    class="grid h-11 w-full min-w-0 grid-cols-[minmax(0,1fr)_auto_auto] gap-2 px-2 text-left sm:h-9"
                    aria-pressed={selected()?.runId === entry.runId}
                    aria-label={`${activityLabel(entry)} ${activityCommit(entry)?.slice(0, 7) ?? "command"} · ${new Date(activityTime(entry)).toLocaleString()}`}
                    title={`${activityLabel(entry)} · ${new Date(activityTime(entry)).toLocaleString()}`}
                    onClick={() => setSelectedId(entry.runId)}
                  >
                    <span class="min-w-0 truncate text-xs">
                      {activityLabel(entry)}
                    </span>
                    <code
                      class="shrink-0 text-[10px] font-normal text-muted-foreground"
                      title={activityCommit(entry) ?? undefined}
                    >
                      {activityCommit(entry)?.slice(0, 7) ?? "command"}
                    </code>
                    <time
                      dateTime={activityTime(entry)}
                      class="shrink-0 whitespace-nowrap text-[10px] font-normal text-muted-foreground"
                    >
                      {new Date(activityTime(entry)).toLocaleTimeString([], {
                        hour: "2-digit",
                        minute: "2-digit",
                        hourCycle: "h23",
                      })}
                    </time>
                  </Button>
                </li>
              )}
            </For>
          </ol>
          <Show when={props.initialRunId && !selected()}>
            <EmptyState
              title="This run is no longer retained"
              description="The selected attempt is not available in this worktree's retained activity. Choose another listed run explicitly or refresh."
            />
          </Show>
          <Show when={selected()} keyed>
            {(entry) => (
              <section
                aria-label="Run activity details"
                class="min-w-0 space-y-3"
              >
                <div class="min-w-0 space-y-2 rounded-lg border p-3 text-xs">
                  <div class="flex flex-wrap items-center gap-2">
                    <StatusBadge tone={activityTone(entry)}>
                      {activityLabel(entry)}
                    </StatusBadge>
                    <span class="text-muted-foreground">
                      {entry.job || entry.history?.automatic
                        ? "Automatic checks"
                        : entry.history?.verificationRequested
                          ? "Verification run"
                          : "Command run"}
                      <Show when={entry.history}>
                        {" "}
                        · {entry.history?.result.durationMs}ms
                      </Show>
                    </span>
                  </div>
                  <Show when={entry.job}>
                    <p>{activityExplanation(entry)}</p>
                  </Show>
                  <Show when={entry.job || entry.history?.automatic}>
                    <p>Run manual verification before approving.</p>
                  </Show>
                  <p>
                    <time dateTime={activityTime(entry)}>
                      {new Date(activityTime(entry)).toLocaleString()}
                    </time>
                  </p>
                  <details class="min-w-0 text-muted-foreground">
                    <summary class="min-h-touch cursor-pointer py-3.5 sm:min-h-8 sm:py-2">
                      Run details
                    </summary>
                    <div class="space-y-1 pt-2">
                      <p class="break-all font-mono">Run: {entry.runId}</p>
                      <p class="break-all font-mono">
                        Run by: {entry.job?.userId ?? entry.history?.userId}
                      </p>
                      <Show when={activityCommit(entry)}>
                        <p class="break-all font-mono">
                          {entry.history
                            ? "Checked commit at execution"
                            : "Target commit"}
                          : {activityCommit(entry)}
                        </p>
                      </Show>
                      <Show
                        when={
                          entry.job?.checksOid ?? entry.history?.checksBlobOid
                        }
                      >
                        <p class="break-all font-mono">
                          Checks policy:{" "}
                          {entry.job?.checksOid ?? entry.history?.checksBlobOid}
                        </p>
                      </Show>
                      <Show when={entry.job} keyed>
                        {(job) => (
                          <>
                            <p>
                              Source:{" "}
                              {job.origin === "git_http"
                                ? "Git HTTP"
                                : job.origin === "local_operator"
                                  ? "Local operator"
                                  : "MCP"}
                            </p>
                            <p>
                              Created:{" "}
                              <time dateTime={job.createdAt}>
                                {new Date(job.createdAt).toLocaleString()}
                              </time>
                            </p>
                            <Show when={job.previous}>
                              <p class="break-all font-mono">
                                Previous commit: {job.previous}
                              </p>
                            </Show>
                            <Show when={job.reason}>
                              <p class="break-all">Reason: {job.reason}</p>
                            </Show>
                          </>
                        )}
                      </Show>
                    </div>
                  </details>
                </div>
                <Show
                  when={entry.history}
                  fallback={
                    <EmptyState
                      title="No stored output"
                      description="This activity has no retained command result. Its lifecycle status does not imply a passing test."
                    />
                  }
                  keyed
                >
                  {(record) => (
                    <>
                      <p class="text-xs text-muted-foreground">
                        Stored command result · exit {record.result.exitCode}
                      </p>
                      <Show when={record.historyTruncated}>
                        <StatusBadge tone="warning">
                          Stored command or output was shortened
                        </StatusBadge>
                      </Show>
                      <VerificationResultDetails result={record.result} />
                    </>
                  )}
                </Show>
              </section>
            )}
          </Show>
        </div>
      </Show>
    </div>
  );
};
