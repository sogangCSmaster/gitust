import { Component, createSignal, Show } from "solid-js";
import { Badge } from "../primitives/Badge";
import { Button } from "../primitives/Button";
import { Dialog } from "../primitives/Dialog";
import { Input } from "../primitives/Input";
import type { AgentSandboxTask } from "./AgentSwarmView";

export interface AgentVerificationDialogProps {
  task: AgentSandboxTask;
  canRun: boolean;
  onRun: (id: number, command: string) => void;
  onRefresh: () => void;
  onClose: () => void;
}

export const AgentVerificationDialog: Component<AgentVerificationDialogProps> = (props) => {
  const [command, setCommand] = createSignal(
    props.task.verificationResult?.requestedCommand ?? props.task.verificationResult?.command ?? "",
  );
  const terminal = () => props.task.sandboxStatus === "merged" || props.task.sandboxStatus === "discarded";
  const blocked = () => !props.canRun || terminal() || Boolean(props.task.operation) ||
    props.task.sandboxStatus === "compiling" || props.task.verificationNeedsRefresh;
  const outputStyle = {
    "white-space": "pre-wrap", "overflow-wrap": "anywhere", "font-family": "var(--ds-font-mono)",
    "font-size": "var(--ds-font-size-xs)", margin: "var(--ds-space-0)",
    padding: "var(--ds-space-2)", "background-color": "var(--ds-bg-canvas)",
  } as const;

  return (
    <Dialog open onClose={props.onClose} title="Worktree verification">
      <div style={{ display: "flex", "flex-direction": "column", gap: "var(--ds-space-3)" }}>
        <div class="ds-mono">{props.task.branch}</div>
        <p style={{ margin: "var(--ds-space-0)", color: "var(--ds-fg-secondary)" }}>
          Run a command in this worktree. A clean committed revision must pass verification before approval.
          Required checks configured on the merge target run after your command; every command must pass.
          Execution requires a SiteAdmin and a server started with trusted single-user sandbox execution enabled.
        </p>
        <Badge variant={props.task.sandboxStatus === "verified" ? "success" : "neutral"}>
          {props.task.verificationNeedsRefresh ? "Server state unconfirmed" : props.task.sandboxStatus}
        </Badge>
        <Show when={props.task.verifiedCommit}>
          {(sha) => <span class="ds-mono">Verified commit: {sha()}</span>}
        </Show>
        <Show when={!terminal()}>
          <label>
            Verification command
            <Input
              aria-label="Verification command"
              value={command()}
              onInput={(event) => setCommand(event.currentTarget.value)}
              placeholder="Enter the command you want to verify"
              disabled={Boolean(props.task.operation)}
            />
          </label>
          <div style={{ display: "flex", gap: "var(--ds-space-2)" }}>
            <Button variant="primary" disabled={Boolean(blocked()) || !command().trim()}
              onClick={() => props.onRun(props.task.id, command())}>
              {props.task.operation === "verifying" ? "Verifying..." : "Run verification"}
            </Button>
            <Button onClick={props.onRefresh} disabled={Boolean(props.task.operation)}>Refresh worktrees</Button>
          </div>
        </Show>
        <Show when={props.task.verificationError}>
          {(error) => <p role="status" style={{ margin: "var(--ds-space-0)", color: "var(--ds-danger-fg)" }}>{error()}</p>}
        </Show>
        <Show when={props.task.verificationResult}>
          {(result) => (
            <div style={{ display: "flex", "flex-direction": "column", gap: "var(--ds-space-2)" }}>
              <strong>Last command result</strong>
              <code style={outputStyle}>{result().command}</code>
              <div class="ds-mono">Exit code: {result().exitCode} · Duration: {result().durationMs}ms</div>
              <Show when={result().outputTruncated}>
                <Badge variant="warning">Output truncated by server</Badge>
              </Show>
              <span>stdout</span>
              <pre style={outputStyle}>{result().stdout || "No stdout"}</pre>
              <span>stderr</span>
              <pre style={outputStyle}>{result().stderr || "No stderr"}</pre>
            </div>
          )}
        </Show>
      </div>
    </Dialog>
  );
};
