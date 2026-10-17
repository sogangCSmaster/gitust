import { createSignal, onMount, Show, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import { Alert, AlertDescription } from "../components/ui/alert";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "../components/ui/collapsible";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { Field } from "../components/workspace/presentation";
import { Icon } from "../components/workspace/icon";
import {
  createWorktreeBranch,
  validateNewWorktreeInput,
  WORKTREE_AGENTS,
  type WorktreeAgentKind,
} from "./new-worktree-input";
import type { NewWorktreeDialogProps } from "./NewWorktreeDialog";
const agents = [...WORKTREE_AGENTS];
export const NewWorktreeForm: Component<NewWorktreeDialogProps> = (props) => {
  const [name, setName] = createSignal("");
  const [base, setBase] = createSignal(
    props.repository.branch === "detached" ? "" : props.repository.branch,
  );
  const [branch, setBranch] = createSignal(createWorktreeBranch());
  const [agent, setAgent] = createSignal<WorktreeAgentKind>("CodexSwarm");
  const [error, setError] = createSignal<string | null>(null);
  const [uncertain, setUncertain] = createSignal(false);
  let nameInput: HTMLInputElement | undefined;
  onMount(() => nameInput?.focus());
  const submit = async () => {
    if (props.submitting) return;
    const input = {
      repoId: props.repository.id,
      promptSummary: name().trim(),
      baseBranch: base().trim(),
      branch: branch().trim(),
      agent: agent(),
    };
    const validation = validateNewWorktreeInput(input);
    if (validation) {
      setError(validation);
      return;
    }
    setError(null);
    setUncertain(false);
    const result = await props.onSubmit(input);
    if (result.kind === "error") {
      setError(result.error);
      setUncertain(Boolean(result.uncertain));
    }
  };
  return (
    <form
      class="min-w-0 space-y-4"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <Field
        label="Task name"
        value={name()}
        onValueChange={setName}
        placeholder="What are you working on?"
        disabled={props.submitting}
        inputRef={(element) => {
          nameInput = element;
        }}
      />
      <Field
        label="Base branch"
        value={base()}
        onValueChange={setBase}
        placeholder="Existing repository branch"
        disabled={props.submitting}
      />
      <div class="min-w-0 space-y-1 text-xs">
        <span class="text-muted-foreground">New branch</span>
        <code
          data-testid="new-worktree-branch"
          class="block truncate"
          title={branch().trim()}
        >
          {branch().trim()}
        </code>
      </div>
      <Collapsible class="min-w-0">
        <CollapsibleTrigger
          as={Button}
          type="button"
          variant="ghost"
          size="sm"
          class="-ml-2 gap-2 text-xs font-normal text-muted-foreground"
          disabled={props.submitting}
        >
          <Icon name="sliders" size={13} />
          Advanced
        </CollapsibleTrigger>
        <CollapsibleContent class="space-y-4 pt-3">
          <Field
            label="Branch"
            value={branch()}
            onValueChange={setBranch}
            disabled={props.submitting}
          />
          <Select
            options={agents}
            value={agents.find((option) => option.value === agent())}
            onChange={(option) => {
              if (option) setAgent(option.value);
            }}
            optionValue="value"
            optionTextValue="label"
            disabled={props.submitting}
            itemComponent={(item) => (
              <SelectItem item={item.item}>
                {item.item.rawValue.label}
              </SelectItem>
            )}
          >
            <SelectLabel>Agent</SelectLabel>
            <SelectTrigger class="mt-2">
              <SelectValue<(typeof agents)[number]>>
                {(state) => state.selectedOption().label}
              </SelectValue>
            </SelectTrigger>
            <SelectContent />
          </Select>
        </CollapsibleContent>
      </Collapsible>
      <Show when={error()}>
        {(message) => (
          <Alert variant="destructive">
            <AlertDescription class="break-words">{message()}</AlertDescription>
          </Alert>
        )}
      </Show>
      <Show when={uncertain()}>
        <div class="space-y-2 text-xs text-muted-foreground">
          <p>
            The request may have reached the server. Refresh worktrees before
            retrying. The branch shown above stays the same.
          </p>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={props.onRefresh}
          >
            Refresh worktrees
          </Button>
        </div>
      </Show>
      <div class="flex justify-end gap-2 border-t pt-4">
        <Button
          type="button"
          variant="outline"
          disabled={props.submitting}
          onClick={props.onClose}
        >
          Cancel
        </Button>
        <Button type="submit" disabled={props.submitting}>
          {props.submitting ? "Creating…" : "Create worktree"}
        </Button>
      </div>
    </form>
  );
};
