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
import { VerificationResultDetails } from "../patterns/VerificationResultDetails";
import {
  createLocalChecksClient,
  type LocalChecksState,
} from "./live-local-checks-client";
import type { RepositoryActionsProps } from "./LiveRepositoryActionsView";

export const LiveLocalChecksView: Component<RepositoryActionsProps> = (
  props,
) => {
  const [state, setState] = createSignal<LocalChecksState>({
    status: "idle",
    records: [],
  });
  const [selectedId, setSelectedId] = createSignal<string | null>(null);
  const client = createLocalChecksClient(setState, {
    repositoryId: () => props.repositoryId,
    selectionEpoch: () => props.selectionEpoch,
    canRead: () => props.isPrimary && props.role === "SiteAdmin",
  });
  const selected = createMemo(() =>
    state().records.find((record) => record.runId === selectedId()),
  );
  createEffect(() => {
    const id = props.repositoryId;
    const epoch = props.selectionEpoch;
    const allowed = props.isPrimary && props.role === "SiteAdmin";
    setSelectedId(null);
    client.clear();
    if (id && allowed && Number.isSafeInteger(epoch)) void client.load();
  });
  createEffect(() => {
    if (state().status === "ready" && selectedId() === null)
      setSelectedId(state().records[0]?.runId ?? null);
  });
  const sessionChanged = (event: StorageEvent) => {
    if (event.key === "gitust_token" || event.key === null) {
      setSelectedId(null);
      client.clear();
    }
  };
  onMount(() => window.addEventListener("storage", sessionChanged));
  onCleanup(() => {
    client.clear();
    window.removeEventListener("storage", sessionChanged);
  });
  return (
    <section class="min-w-0 space-y-3" aria-label="Recorded Local Checks">
      <div class="flex min-w-0 items-center justify-between gap-3">
        <p class="min-w-0 text-xs text-muted-foreground">
          Latest five recorded runs. Diagnostics only.
        </p>
        <Button
          size="sm"
          variant="outline"
          class="shrink-0"
          onClick={() => void client.load()}
        >
          <Icon name="rotate-ccw" /> Refresh
        </Button>
      </div>
      <Show when={state().status === "idle"}>
        <p role="status" class="text-sm text-muted-foreground">
          Refresh to load Local Checks.
        </p>
      </Show>
      <Show when={state().status === "loading"}>
        <p role="status" class="text-sm text-muted-foreground">
          Loading Local Checks…
        </p>
      </Show>
      <Show when={state().status === "error"}>
        <Alert variant="destructive">
          <AlertDescription>{state().error}</AlertDescription>
        </Alert>
      </Show>
      <Show when={state().status === "ready"}>
        <Show
          when={state().records.length}
          fallback={
            <EmptyState
              title="No recorded Local Checks"
              description="Run local Checks with --record from the stopped repository to retain completed results."
            />
          }
        >
          <div class="grid min-w-0 gap-3 sm:grid-cols-[12rem_minmax(0,1fr)]">
            <ol
              class="max-h-56 min-w-0 space-y-1 overflow-y-auto sm:max-h-[60svh]"
              aria-label="Recorded runs"
            >
              <For each={state().records}>
                {(record) => (
                  <li class="min-w-0">
                    <Button
                      variant={
                        selectedId() === record.runId ? "secondary" : "ghost"
                      }
                      class="grid h-11 w-full min-w-0 grid-cols-[minmax(0,1fr)_auto_auto] gap-2 px-2 text-left sm:h-9"
                      aria-pressed={selectedId() === record.runId}
                      aria-label={`${record.status === "passed" ? "Passed" : "Failed"} ${record.checkedCommit.slice(0, 7)} · ${new Date(record.completedAt).toLocaleString()}`}
                      onClick={() => {
                        if (client.isCurrent()) setSelectedId(record.runId);
                      }}
                    >
                      <span class="min-w-0 truncate text-xs">
                        {record.status === "passed" ? "Passed" : "Failed"}
                      </span>
                      <code
                        class="shrink-0 text-[10px] font-normal text-muted-foreground"
                        title={record.checkedCommit}
                      >
                        {record.checkedCommit.slice(0, 7)}
                      </code>
                      <time
                        class="shrink-0 whitespace-nowrap text-[10px] font-normal text-muted-foreground"
                        dateTime={record.completedAt}
                      >
                        {new Date(record.completedAt).toLocaleTimeString([], {
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
            <Show
              when={selected()}
              fallback={
                <EmptyState
                  title="This run is no longer retained"
                  description="Choose a listed run or refresh to check the current retained results."
                />
              }
              keyed
            >
              {(record) => (
                <section
                  class="min-w-0 space-y-3"
                  aria-label="Local Checks result"
                >
                  <div class="min-w-0 space-y-2 rounded-lg border p-3 text-xs">
                    <div class="flex flex-wrap items-center gap-2">
                      <StatusBadge
                        tone={record.status === "passed" ? "success" : "danger"}
                      >
                        {record.status === "passed" ? "Passed" : "Failed"}
                      </StatusBadge>
                      <span class="text-muted-foreground">
                        Local Checks · {record.result.durationMs}ms
                      </span>
                    </div>
                    <p class="truncate" title={record.sourceRef}>
                      {record.sourceRef.slice(11)}
                    </p>
                    <time dateTime={record.completedAt}>
                      {new Date(record.completedAt).toLocaleString()}
                    </time>
                    <details class="min-w-0 text-muted-foreground">
                      <summary class="min-h-touch cursor-pointer py-3.5 sm:min-h-8 sm:py-2">
                        Run details
                      </summary>
                      <div class="space-y-1 pt-2">
                        <p class="break-all font-mono">Run: {record.runId}</p>
                        <p class="break-all font-mono">
                          Checked commit: {record.checkedCommit}
                        </p>
                        <p class="break-all font-mono">
                          Checks policy: {record.checksBlobOid}
                        </p>
                      </div>
                    </details>
                  </div>
                  <Show when={record.historyTruncated}>
                    <StatusBadge tone="warning">
                      Stored command or output was shortened
                    </StatusBadge>
                  </Show>
                  <VerificationResultDetails
                    result={record.result}
                    commandsLabel="Local Checks commands"
                  />
                </section>
              )}
            </Show>
          </div>
        </Show>
      </Show>
    </section>
  );
};
