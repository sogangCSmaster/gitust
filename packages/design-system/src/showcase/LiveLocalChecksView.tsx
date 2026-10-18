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
import { getLiveToken } from "./live-repo-context";
import { isOid } from "./live-automatic-runs";
import {
  createLocalChecksRunClient,
  type LocalChecksRunState,
} from "./live-local-checks-run";
import { LocalChecksAutomaticStatus } from "./LocalChecksAutomaticStatus";
import {
  createLocalChecksAutomaticClient,
  type LocalChecksAutomaticState,
} from "./live-local-checks-automatic";
import type { RepositoryActionsProps } from "./LiveRepositoryActionsView";

export const LiveLocalChecksView: Component<RepositoryActionsProps> = (
  props,
) => {
  const [state, setState] = createSignal<LocalChecksState>({
    status: "idle",
    records: [],
  });
  const [selectedId, setSelectedId] = createSignal<string | null>(null);
  const canReadHistory = () =>
    props.role === "SiteAdmin" &&
    (props.isPrimary ||
      props.canRunChecks === true ||
      props.canViewLocalChecksAutomatic === true);
  const client = createLocalChecksClient(setState, {
    repositoryId: () => props.repositoryId,
    selectionEpoch: () => props.selectionEpoch,
    canRead: canReadHistory,
  });
  const [runState, setRunState] = createSignal<LocalChecksRunState>({
    status: "idle",
  });
  const [sessionAvailable, setSessionAvailable] = createSignal(true);
  const [refreshing, setRefreshing] = createSignal(false);
  const [refreshError, setRefreshError] = createSignal<string>();
  let refreshGeneration = 0;
  let disposed = false;
  // A catalog refresh owns hint changes only while its original selection is current.
  let automaticRefreshOwner: (() => boolean) | undefined;
  const canRun = () =>
    props.role === "SiteAdmin" &&
    props.canRunChecks === true &&
    sessionAvailable() &&
    isOid(props.defaultCommit) &&
    !!props.defaultRef;
  const runner = createLocalChecksRunClient(setRunState, {
    repositoryId: () => props.repositoryId,
    selectionEpoch: () => props.selectionEpoch,
    canRun,
    sourceRef: () => props.defaultRef,
    commit: () => props.defaultCommit,
  });
  const [automaticState, setAutomaticState] =
    createSignal<LocalChecksAutomaticState>({ status: "idle", runs: [] });
  const canViewAutomatic = () =>
    props.role === "SiteAdmin" &&
    props.canViewLocalChecksAutomatic === true;
  const automaticClient = createLocalChecksAutomaticClient(setAutomaticState, {
    repositoryId: () => props.repositoryId,
    selectionEpoch: () => props.selectionEpoch,
    canRead: canViewAutomatic,
    onDenied: () => {
      client.clear();
      runner.clear();
      setSelectedId(null);
      setSessionAvailable(false);
    },
  });
  createEffect(() => {
    props.repositoryId;
    props.selectionEpoch;
    const allowed = canViewAutomatic();
    automaticClient.clear();
    if (allowed && !automaticRefreshOwner?.()) void automaticClient.load();
  });
  const selected = createMemo(
    () =>
      state().records.find((record) => record.runId === selectedId()) ??
      (runState().report?.record?.runId === selectedId()
        ? runState().report?.record
        : undefined),
  );
  createEffect(() => {
    props.repositoryId;
    props.selectionEpoch;
    props.role;
    props.isPrimary;
    props.canRunChecks;
    props.defaultRef;
    props.defaultCommit;
    runner.clear();
  });
  const submit = async () => {
    setRefreshError(undefined);
    const report = await runner.run();
    if (!report?.record || !runner.isCurrent()) return;
    setSelectedId(report.record.runId);
    if (report.history.status === "saved") await client.load();
  };
  const refresh = async () => {
    if (runState().status === "running" || refreshing()) return;
    const key = ++refreshGeneration;
    const id = props.repositoryId,
      epoch = props.selectionEpoch,
      token = getLiveToken();
    const current = () =>
      !disposed &&
      key === refreshGeneration &&
      id === props.repositoryId &&
      epoch === props.selectionEpoch &&
      token === getLiveToken();
    automaticRefreshOwner = current;
    setRefreshing(true);
    setRefreshError(undefined);
    try {
      const refreshed = props.refreshRepository
        ? await props.refreshRepository(current)
        : true;
      if (!current()) return;
      if (!refreshed) {
        setRefreshError("Unable to refresh repository context. Try again.");
        return;
      }
      await Promise.all([
        client.load(),
        canViewAutomatic() ? automaticClient.load() : Promise.resolve(),
      ]);
    } catch {
      if (current())
        setRefreshError("Unable to refresh repository context. Try again.");
    } finally {
      if (automaticRefreshOwner === current) automaticRefreshOwner = undefined;
      if (!disposed && key === refreshGeneration) setRefreshing(false);
    }
  };
  createEffect(() => {
    const id = props.repositoryId;
    const epoch = props.selectionEpoch;
    const allowed = canReadHistory();
    refreshGeneration++;
    setRefreshing(false);
    setRefreshError(undefined);
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
      refreshGeneration++;
      setRefreshing(false);
      setRefreshError(undefined);
      setSessionAvailable(false);
      automaticClient.clear();
      runner.clear();
      setSelectedId(null);
      client.clear();
    }
  };
  onMount(() => window.addEventListener("storage", sessionChanged));
  onCleanup(() => {
    disposed = true;
    refreshGeneration++;
    runner.clear();
    automaticClient.clear();
    client.clear();
    window.removeEventListener("storage", sessionChanged);
  });
  return (
    <section class="min-w-0 space-y-3" aria-label="Recorded Local Checks">
      <div class="space-y-2 sm:flex sm:items-center sm:justify-between sm:gap-3 sm:space-y-0">
        <div class="min-w-0 text-xs text-muted-foreground">
          <p>Latest five recorded runs. Diagnostics only.</p>
          <Show when={canRun()}>
            <p class="truncate" title={props.defaultRef}>
              {props.selectedBranch ? "Branch" : "Default"}:{" "}
              {props.defaultRef?.slice(11)} ·{" "}
              {props.defaultCommit?.slice(0, 7)}
            </p>
          </Show>
        </div>
        <div class="flex shrink-0 gap-2">
          <Show when={canRun()}>
            <Button
              size="sm"
              class="flex-1 sm:flex-none"
              disabled={runState().status === "running" || refreshing()}
              onClick={() => void submit()}
            >
              <Icon name="play" />{" "}
              {runState().status === "running" ? "Running…" : "Run Checks"}
            </Button>
          </Show>
          <Button
            size="sm"
            variant="outline"
            class="flex-1 sm:flex-none"
            disabled={
              runState().status === "running" ||
              refreshing() ||
              state().status === "loading"
            }
            onClick={() => void refresh()}
          >
            <Icon name="rotate-ccw" /> Refresh
          </Button>
        </div>
      </div>
      <Show when={canViewAutomatic()}>
        <LocalChecksAutomaticStatus
          state={automaticState()}
          records={state().records}
          selectedId={selectedId()}
          onSelect={(id) => {
            if (automaticClient.isCurrent() && client.isCurrent())
              setSelectedId(id);
          }}
        />
      </Show>
      <Show when={runState().status === "running"}>
        <p role="status" class="text-sm text-muted-foreground">
          Running default branch Checks…
        </p>
      </Show>
      <Show when={runState().report?.record}>
        <p role="status" class="text-sm">
          {runState().report?.status === "passed"
            ? "Checks passed."
            : "Checks failed."}{" "}
          {runState().report?.history.status === "saved"
            ? "Result saved."
            : "Saving this result could not be confirmed. Refresh stored results before another run."}
        </p>
      </Show>
      <Show
        when={
          runState().report?.history.status === "failed" &&
          runState().report?.record
        }
      >
        <Button
          size="sm"
          variant="outline"
          aria-pressed={selectedId() === runState().report?.record?.runId}
          onClick={() => {
            const record = runState().report?.record;
            if (record && runner.isCurrent()) setSelectedId(record.runId);
          }}
        >
          View execution result
        </Button>
      </Show>
      <Show when={runState().report?.status === "unconfirmed"}>
        <Alert>
          <AlertDescription>
            Could not confirm completion. Refresh stored results before another
            run. A new run may repeat commands.
          </AlertDescription>
        </Alert>
      </Show>
      <Show when={runState().error || refreshError()}>
        <Alert variant="destructive">
          <AlertDescription>
            {runState().error ?? refreshError()}
          </AlertDescription>
        </Alert>
      </Show>
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
      <Show when={state().status === "ready" || runState().report?.record}>
        <Show
          when={state().records.length || runState().report?.record}
          fallback={
            <EmptyState
              title="No recorded Local Checks"
              description={
                canRun()
                  ? "Run Checks to record a result for the default branch."
                  : "Run local Checks with --record from the stopped repository to retain completed results."
              }
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
