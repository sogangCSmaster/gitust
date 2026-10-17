import { createMemo, createSignal, For, Show, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import { Alert, AlertDescription } from "../components/ui/alert";
import { EmptyState, StatusBadge } from "../components/workspace/presentation";
import { Icon } from "../components/workspace/icon";
import { VerificationResultDetails } from "./VerificationResultDetails";
import type { RunHistoryDialogProps } from "./RunHistoryDialog";

export const RunHistoryContent: Component<RunHistoryDialogProps> = (props) => {
  const [selectedId, setSelectedId] = createSignal<string | null>(null);
  const selected = createMemo(
    () =>
      props.state.records.find((record) => record.runId === selectedId()) ??
      props.state.records[0],
  );
  return (
    <div class="min-w-0 space-y-4">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <p class="text-xs text-muted-foreground">
          Completed runs only. Timeouts and interrupted attempts are not stored.
        </p>
        <Button size="sm" variant="outline" onClick={props.onRefresh}>
          <Icon name="rotate-ccw" /> Refresh history
        </Button>
      </div>
      <Show
        when={props.state.status === "idle" || props.state.status === "loading"}
      >
        <p role="status" class="text-sm text-muted-foreground">
          Loading stored runs…
        </p>
      </Show>
      <Show when={props.state.status === "error"}>
        <Alert variant="destructive">
          <AlertDescription>{props.state.error}</AlertDescription>
        </Alert>
      </Show>
      <Show
        when={props.state.status === "ready" && !props.state.records.length}
      >
        <EmptyState
          title="No stored runs"
          description="Completed runs appear here after execution. Older runs from before history storage was added are unavailable."
        />
      </Show>
      <Show when={props.state.status === "ready" && props.state.records.length}>
        <div class="grid min-w-0 gap-4 sm:grid-cols-[12rem_minmax(0,1fr)]">
          <ol
            aria-label="Stored runs"
            class="max-h-60 space-y-2 overflow-y-auto sm:max-h-[60svh]"
          >
            <For each={props.state.records}>
              {(record) => (
                <li>
                  <Button
                    variant={
                      selected()?.runId === record.runId
                        ? "secondary"
                        : "outline"
                    }
                    class="h-auto w-full min-w-0 flex-col items-start gap-1 whitespace-normal p-3 text-left"
                    aria-pressed={selected()?.runId === record.runId}
                    aria-label={`View run ${record.runId}`}
                    onClick={() => setSelectedId(record.runId)}
                  >
                    <span class="text-xs">
                      {record.result.exitCode === 0
                        ? "Passed"
                        : `Failed · exit ${record.result.exitCode}`}
                    </span>
                    <time
                      dateTime={record.completedAt}
                      class="text-xs font-normal text-muted-foreground"
                    >
                      {new Date(record.completedAt).toLocaleString()}
                    </time>
                    <span class="text-xs font-normal">
                      {record.automatic
                        ? "Automatic checks"
                        : record.verificationRequested
                          ? "Verification run"
                          : "Command run"}{" "}
                      · {record.result.durationMs}ms
                    </span>
                  </Button>
                </li>
              )}
            </For>
          </ol>
          <Show when={selected()} keyed>
            {(record) => (
              <section
                aria-label="Stored run details"
                class="min-w-0 space-y-4"
              >
                <div class="space-y-2 rounded-lg border p-3 text-xs">
                  <div class="flex flex-wrap items-center gap-2">
                    <StatusBadge
                      tone={record.result.exitCode === 0 ? "success" : "danger"}
                    >
                      Stored result · exit {record.result.exitCode}
                    </StatusBadge>
                    <span>Diagnostic record</span>
                  </div>
                  <p class="break-all font-mono">Run: {record.runId}</p>
                  <p class="break-all font-mono">Run by: {record.userId}</p>
                  <Show when={record.automatic}>
                    <p>
                      Automatic checks do not grant approval. Run manual
                      verification before approving.
                    </p>
                  </Show>
                  <Show when={record.checkedCommit}>
                    <p class="break-all font-mono">
                      Checked commit at execution: {record.checkedCommit}
                    </p>
                  </Show>
                  <Show when={record.checksBlobOid}>
                    <p class="break-all font-mono">
                      Checks policy: {record.checksBlobOid}
                    </p>
                  </Show>
                </div>
                <Show when={record.historyTruncated}>
                  <StatusBadge tone="warning">
                    Stored command or output was shortened
                  </StatusBadge>
                </Show>
                <VerificationResultDetails result={record.result} />
              </section>
            )}
          </Show>
        </div>
      </Show>
    </div>
  );
};
