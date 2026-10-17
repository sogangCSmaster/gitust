import {
  createMemo,
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
  createRepositoryBranches,
  type RepositoryBranch,
} from "./live-branches";
import {
  createDefaultBranchMutation,
  isDefaultBranchCandidate,
  type DefaultBranchActions,
  type DefaultBranchIntent,
} from "./repository-default-branch";

interface Choice {
  key: string;
  label: string;
  branch: RepositoryBranch;
}
export const RepositoryDefaultBranchForm: Component<{
  intent: DefaultBranchIntent;
  actions: DefaultBranchActions;
  onClose: () => void;
  submitting: boolean;
  onSubmitting: (value: boolean) => void;
}> = (props) => {
  const [selected, setSelected] = createSignal<RepositoryBranch | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const current = () => props.actions.isCurrent(props.intent);
  const branches = createRepositoryBranches({
    getRepositoryId: () => (current() ? props.intent.repoId : null),
    onSelect: () => {},
    onUnavailable: () => {
      if (current()) props.actions.unavailable(props.intent);
    },
  });
  const mutation = createDefaultBranchMutation({
    isCurrent: props.actions.isCurrent,
    invalidateReads: props.actions.invalidateReads,
    confirmed: props.actions.confirmed,
    unavailable: props.actions.unavailable,
  });
  let mounted = true;
  let trigger: HTMLButtonElement | undefined;
  onMount(() => {
    if (current()) trigger?.focus();
    branches.ensure();
  });
  onCleanup(() => {
    mounted = false;
    mutation.cancel();
    branches.reset();
  });
  const choice = (branch: RepositoryBranch): Choice => ({
    key: `${branch.ref_name}@${branch.commit_sha}`,
    label: `${branch.name} · ${branch.commit_sha.slice(0, 8)}`,
    branch,
  });
  const options = createMemo(() => {
    const rows = branches
      .state()
      .branches.filter(
        (row) =>
          row.ref_name !== props.intent.currentRef &&
          isDefaultBranchCandidate(row),
      );
    const own = selected();
    if (
      own &&
      !rows.some(
        (row) =>
          row.ref_name === own.ref_name && row.commit_sha === own.commit_sha,
      )
    )
      rows.unshift(own);
    return rows.map(choice);
  });
  const refresh = async () => {
    if (!current() || props.submitting) return;
    await props.actions.refresh(props.intent);
    if (mounted && current()) await branches.refresh();
  };
  const submit = async (event: Event) => {
    event.preventDefault();
    const target = selected();
    if (!current()) {
      props.onClose();
      return;
    }
    if (!target || props.submitting) return;
    props.onSubmitting(true);
    setError(null);
    const result = await mutation.save(props.intent, target);
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
        sameWidth
        placeholder="Choose a branch"
        options={options()}
        optionValue="key"
        optionTextValue="label"
        value={selected() ? choice(selected()!) : null}
        disabled={props.submitting}
        onChange={(value) => {
          if (value) setSelected(value.branch);
        }}
        itemComponent={(item) => (
          <SelectItem item={item.item}>
            <span class="truncate" title={item.item.rawValue.label}>
              {item.item.rawValue.label}
            </span>
          </SelectItem>
        )}
      >
        <SelectTrigger
          ref={trigger}
          aria-label="New default branch"
          class="min-w-0"
        >
          <SelectValue<Choice> class="min-w-0 truncate">
            {(state) => state.selectedOption()?.label ?? "Choose a branch"}
          </SelectValue>
        </SelectTrigger>
        <SelectContent class="max-h-64 max-w-[calc(100vw-2rem)] overflow-y-auto" />
      </Select>
      <div class="flex min-w-0 items-center justify-between gap-2 text-xs text-muted-foreground">
        <span class="min-w-0" role="status">
          {branches.state().loading
            ? "Loading branches…"
            : (branches.state().error ??
              (options().length
                ? "Choose an existing committed branch."
                : "No eligible branches on this page."))}
        </span>
        <div class="flex shrink-0 items-center gap-1">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={props.submitting || branches.state().loading}
            onClick={() => void refresh()}
          >
            Refresh
          </Button>
          <Show when={branches.state().cursor}>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={props.submitting || branches.state().loading}
              onClick={() => void branches.refresh()}
            >
              First
            </Button>
          </Show>
          <Show when={branches.state().next_cursor}>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={props.submitting || branches.state().loading}
              onClick={() => void branches.next()}
            >
              Next
            </Button>
          </Show>
        </div>
      </div>
      <Show when={error()}>
        <p role="alert" class="text-xs text-error-foreground">
          {error()}
        </p>
      </Show>
      <div class="flex justify-end gap-2">
        <Button
          type="button"
          variant="outline"
          disabled={props.submitting}
          onClick={props.onClose}
        >
          Cancel
        </Button>
        <Button type="submit" disabled={!selected() || props.submitting}>
          {props.submitting ? "Changing…" : "Change default"}
        </Button>
      </div>
    </form>
  );
};
