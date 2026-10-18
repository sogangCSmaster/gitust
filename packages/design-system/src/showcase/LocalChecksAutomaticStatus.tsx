import { For, Show, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import { Alert, AlertDescription } from "../components/ui/alert";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "../components/ui/collapsible";
import { StatusBadge } from "../components/workspace/presentation";
import type { LocalChecksRecord } from "./live-local-checks";
import {
  matchingLocalChecksRecord,
  type LocalChecksAutomaticRun,
  type LocalChecksAutomaticState,
} from "./live-local-checks-automatic";

const reasonText: Record<string, string> = {
  command_failed: "A required check failed.",
  execution_disabled: "Execution is disabled.",
  authority_unavailable: "Actor access changed.",
  repository_unavailable: "Repository data is unavailable.",
  policy_disabled: "Automatic Checks are disabled by policy.",
  policy_missing: "No committed Checks policy.",
  policy_invalid: "The committed Checks policy is invalid.",
  push_not_confirmed: "Push acceptance could not be confirmed.",
  latest_queued_replaced: "Replaced by a newer queued update.",
  default_changed: "The default branch changed.",
  target_changed: "The target commit changed.",
  acceptance_unconfirmed: "Push acceptance could not be confirmed.",
  service_stopped: "The service stopped.",
  unconfirmed_execution: "Completion could not be confirmed.",
};
const active = (run: LocalChecksAutomaticRun) =>
  run.phase === "running" || run.phase === "queued";
export const LocalChecksAutomaticStatus: Component<{
  state: LocalChecksAutomaticState;
  records: LocalChecksRecord[];
  selectedId: string | null;
  onSelect: (runId: string) => void;
}> = (props) => {
  const recent = () => props.state.runs.filter((run) => !active(run));
  const visible = () => [
    ...props.state.runs.filter(active),
    ...recent().slice(0, 1),
  ];
  const earlier = () => recent().slice(1);
  const row = (run: LocalChecksAutomaticRun) => (
    <li class="min-w-0 space-y-1 rounded-md border px-3 py-2">
      <div class="flex min-w-0 flex-wrap items-center gap-2">
        <p class="min-w-0 flex-1 truncate text-xs" title={run.sourceRef}>
          {run.sourceRef.slice(11)} ·{" "}
          <code title={run.expectedCommit}>
            {run.expectedCommit.slice(0, 7)}
          </code>
        </p>
        <StatusBadge
          tone={
            run.phase === "failed"
              ? "danger"
              : run.phase === "interrupted"
                ? "warning"
                : "neutral"
          }
        >
          {run.phase.charAt(0).toUpperCase() + run.phase.slice(1)}
        </StatusBadge>
        <Show when={matchingLocalChecksRecord(run, props.records)}>
          <Button
            size="sm"
            variant="ghost"
            aria-pressed={props.selectedId === run.runId}
            onClick={() => props.onSelect(run.runId)}
          >
            View result
          </Button>
        </Show>
      </div>
      <Show when={run.history === "failed"}>
        <p class="text-xs text-muted-foreground">Saving not confirmed</p>
      </Show>
      <Show when={run.reason && reasonText[run.reason]}>
        <p class="text-xs text-muted-foreground">{reasonText[run.reason!]}</p>
      </Show>
      <Show
        when={!active(run) && !matchingLocalChecksRecord(run, props.records)}
      >
        <p class="text-xs text-muted-foreground">
          {run.history === "not_attempted"
            ? "No completed result available."
            : "Output is not in loaded history."}
        </p>
      </Show>
    </li>
  );
  return (
    <section
      class="min-w-0 space-y-2 rounded-lg border p-3"
      aria-label="Automatic Local Checks"
    >
      <h2 class="text-xs font-medium">Default-branch automation</h2>
      <Show when={props.state.status === "idle"}>
        <p role="status" class="text-xs text-muted-foreground">
          Refresh to load automatic status.
        </p>
      </Show>
      <Show when={props.state.status === "loading"}>
        <p role="status" class="text-xs text-muted-foreground">
          Loading automatic status…
        </p>
      </Show>
      <Show when={props.state.status === "error"}>
        <Alert variant="destructive">
          <AlertDescription>{props.state.error}</AlertDescription>
        </Alert>
      </Show>
      <Show when={props.state.status === "ready"}>
        <Show
          when={props.state.runs.length}
          fallback={
            <p class="text-xs text-muted-foreground">
              No automatic attempts recorded.
            </p>
          }
        >
          <ol class="min-w-0 space-y-1" aria-label="Current automatic attempts">
            <For each={visible()}>{row}</For>
          </ol>
          <Show when={earlier().length}>
            <Collapsible>
              <CollapsibleTrigger as={Button} size="sm" variant="ghost">
                Earlier attempts ({earlier().length})
              </CollapsibleTrigger>
              <CollapsibleContent class="pt-2">
                <ol
                  class="min-w-0 space-y-1"
                  aria-label="Earlier automatic attempts"
                >
                  <For each={earlier()}>{row}</For>
                </ol>
              </CollapsibleContent>
            </Collapsible>
          </Show>
          <p class="text-[10px] text-muted-foreground">
            Refresh updates automatic status. Results are diagnostics only.
          </p>
        </Show>
      </Show>
    </section>
  );
};
