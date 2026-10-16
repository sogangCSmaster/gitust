import { For, Show, type Component } from "solid-js";
import { Card, CardContent } from "../components/ui/card";
import { StatusBadge } from "../components/workspace/presentation";
import type { AgentVerificationResult } from "./AgentSwarmView";

export const VerificationResultDetails: Component<{
  result: AgentVerificationResult;
}> = (props) => (
  <div class="min-w-0 space-y-4">
    <Show when={props.result.steps?.length}>
      <ol aria-label="Verification commands" class="space-y-3">
        <For each={props.result.steps}>
          {(step) => (
            <li>
              <Card>
                <CardContent class="space-y-3 p-4">
                  <div class="flex items-center justify-between gap-3">
                    <span class="text-xs text-muted-foreground">
                      {step.required ? "Required check" : "Your command"}
                    </span>
                    <StatusBadge
                      tone={
                        step.exitCode === null
                          ? "neutral"
                          : step.exitCode === 0
                            ? "success"
                            : "danger"
                      }
                    >
                      {step.exitCode === null
                        ? "Skipped"
                        : step.exitCode === 0
                          ? "Passed"
                          : "Failed"}
                    </StatusBadge>
                  </div>
                  <pre class="whitespace-pre-wrap break-all font-mono text-xs">
                    {step.command}
                  </pre>
                  <p class="text-xs text-muted-foreground">
                    {step.exitCode === null
                      ? "Not run because an earlier command failed."
                      : `Exit code: ${step.exitCode} · Duration: ${step.durationMs}ms`}
                  </p>
                </CardContent>
              </Card>
            </li>
          )}
        </For>
      </ol>
    </Show>
    <h3 class="font-medium">Combined command and output</h3>
    <pre class="whitespace-pre-wrap break-all rounded-md bg-muted p-3 font-mono text-xs">
      {props.result.command}
    </pre>
    <p class="font-mono text-xs text-muted-foreground">
      Exit code: {props.result.exitCode} · Duration: {props.result.durationMs}
      ms
    </p>
    <Show when={props.result.outputTruncated}>
      <StatusBadge tone="warning">Output truncated by server</StatusBadge>
    </Show>
    <For
      each={[
        { label: "stdout", value: props.result.stdout },
        { label: "stderr", value: props.result.stderr },
      ]}
    >
      {(output) => (
        <div class="space-y-2">
          <h4 class="text-xs text-muted-foreground">{output.label}</h4>
          <pre class="max-h-60 overflow-auto whitespace-pre-wrap break-all rounded-md bg-muted p-3 font-mono text-xs">
            {output.value || `No ${output.label}`}
          </pre>
        </div>
      )}
    </For>
  </div>
);
