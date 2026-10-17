import {
  createSignal,
  onCleanup,
  onMount,
  Show,
  type Component,
} from "solid-js";
import { Button } from "../components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import {
  createVisibilityMutation,
  type VisibilityActions,
  type VisibilityIntent,
} from "./repository-visibility";

const choices = [
  { key: "team", label: "Team only", value: true },
  { key: "signed-in", label: "Signed-in users", value: false },
];
type Choice = (typeof choices)[number];
export const RepositoryVisibilityForm: Component<{
  intent: VisibilityIntent;
  actions: VisibilityActions;
  submitting: boolean;
  onSubmitting: (value: boolean) => void;
  onClose: () => void;
  onInvite?: () => void;
}> = (props) => {
  const [selected, setSelected] = createSignal(
    choices.find((choice) => choice.value === props.intent.expected)!,
  );
  const [error, setError] = createSignal<string | null>(null);
  const mutation = createVisibilityMutation(props.actions);
  const current = () => props.actions.isCurrent(props.intent);
  let mounted = true;
  let trigger: HTMLButtonElement | undefined;
  onMount(() => {
    if (current()) trigger?.focus();
    else props.onClose();
  });
  onCleanup(() => {
    mounted = false;
    mutation.cancel();
  });
  const submit = async (event: Event) => {
    event.preventDefault();
    if (!current()) {
      props.onClose();
      return;
    }
    if (props.submitting || selected().value === props.intent.expected) return;
    props.onSubmitting(true);
    setError(null);
    const result = await mutation.save(props.intent, selected().value);
    if (!mounted) return;
    props.onSubmitting(false);
    if (!current() || result.kind === "stale" || result.kind === "saved")
      props.onClose();
    else setError(result.error);
  };
  return (
    <form
      class="min-w-0 space-y-4"
      onSubmit={submit}
      aria-busy={props.submitting}
    >
      <Select<Choice>
        options={choices}
        optionValue="key"
        optionTextValue="label"
        value={selected()}
        disabled={props.submitting}
        onChange={(value) => {
          if (value) setSelected(value);
        }}
        itemComponent={(item) => (
          <SelectItem item={item.item}>{item.item.rawValue.label}</SelectItem>
        )}
      >
        <SelectTrigger
          ref={trigger}
          aria-label="Who can access this repository"
          class="min-w-0"
        >
          <SelectValue<Choice>>
            {(state) => state.selectedOption()?.label}
          </SelectValue>
        </SelectTrigger>
        <SelectContent />
      </Select>
      <p class="text-xs text-muted-foreground">
        {selected().value
          ? "Only the owning team and site administrators can access this repository."
          : "All active signed-in users can access this repository. Existing role permissions still apply."}
      </p>
      <Show when={error()}>
        <p role="alert" class="text-xs text-error-foreground">
          {error()}
        </p>
      </Show>
      <Show when={props.onInvite}>
        <Button
          type="button"
          variant="outline"
          class="w-full"
          disabled={props.submitting}
          onClick={props.onInvite}
        >
          Invite to owning group
        </Button>
      </Show>
      <div class="flex items-center justify-between gap-2">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={props.submitting}
          onClick={async () => {
            if (current()) await props.actions.refresh(props.intent);
            if (mounted && current()) setError(null);
          }}
        >
          Refresh
        </Button>
        <div class="flex justify-end gap-2">
          <Button
            type="button"
            variant="outline"
            disabled={props.submitting}
            onClick={props.onClose}
          >
            Cancel
          </Button>
          <Button
            type="submit"
            disabled={
              props.submitting || selected().value === props.intent.expected
            }
          >
            Save access
          </Button>
        </div>
      </div>
    </form>
  );
};
