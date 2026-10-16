import { createSignal, Show, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "../components/ui/dialog";
import { Alert, AlertDescription } from "../components/ui/alert";
import { VerificationResultDetails } from "./VerificationResultDetails";
import { Field, StatusBadge } from "../components/workspace/presentation";
import type { AgentSandboxTask } from "./AgentSwarmView";
export interface AgentVerificationDialogProps {
  task: AgentSandboxTask;
  canRun: boolean;
  onRun: (id: number, command: string) => void;
  onRefresh: () => void;
  onClose: () => void;
}
export const AgentVerificationDialog: Component<
  AgentVerificationDialogProps
> = (props) => {
  const [command, setCommand] = createSignal(
    props.task.verificationResult?.requestedCommand ??
      props.task.verificationResult?.command ??
      "",
  );
  const terminal = () =>
    props.task.sandboxStatus === "merged" ||
    props.task.sandboxStatus === "discarded";
  const blocked = () =>
    !props.canRun ||
    terminal() ||
    Boolean(props.task.operation) ||
    props.task.sandboxStatus === "compiling" ||
    props.task.verificationNeedsRefresh;
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) props.onClose();
      }}
    >
      <DialogContent class="max-h-[90svh] w-[calc(100%-2rem)] max-w-2xl rounded-lg">
        <DialogHeader>
          <DialogTitle>Worktree verification</DialogTitle>
          <DialogDescription>
            Verify a clean, committed revision before approval. Every required
            check must pass. Execution requires a SiteAdmin and trusted
            single-user sandbox execution on the server.
          </DialogDescription>
        </DialogHeader>
        <div class="flex flex-wrap items-center gap-3">
          <code class="break-all text-xs text-muted-foreground">
            {props.task.branch}
          </code>
          <StatusBadge
            tone={
              props.task.sandboxStatus === "verified" ? "success" : "neutral"
            }
          >
            {props.task.verificationNeedsRefresh
              ? "Server state unconfirmed"
              : props.task.sandboxStatus}
          </StatusBadge>
        </div>
        <Show when={props.task.verifiedCommit}>
          {(sha) => (
            <p class="break-all font-mono text-xs text-muted-foreground">
              Verified commit: {sha()}
            </p>
          )}
        </Show>
        <Show when={!terminal()}>
          <Field
            label="Verification command"
            value={command()}
            onValueChange={setCommand}
            placeholder="Enter the command you want to verify"
            disabled={Boolean(props.task.operation)}
          />
          <div class="flex flex-wrap gap-2">
            <Button
              disabled={Boolean(blocked()) || !command().trim()}
              onClick={() => props.onRun(props.task.id, command())}
            >
              {props.task.operation === "verifying"
                ? "Verifying..."
                : "Run verification"}
            </Button>
            <Button
              variant="outline"
              disabled={Boolean(props.task.operation)}
              onClick={props.onRefresh}
            >
              Refresh worktrees
            </Button>
          </div>
        </Show>
        <Show when={props.task.verificationError}>
          {(error) => (
            <Alert variant="destructive">
              <AlertDescription>{error()}</AlertDescription>
            </Alert>
          )}
        </Show>
        <Show when={props.task.verificationResult}>
          {(result) => (
            <div class="min-w-0 space-y-4">
              <h3 class="font-medium">Last verification result</h3>
              <VerificationResultDetails result={result()} />
            </div>
          )}
        </Show>
      </DialogContent>
    </Dialog>
  );
};
