import { createMemo, onMount, Show, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { Icon } from "../components/workspace/icon";
import type { ConnectedRepository } from "./connected-repository-client";
import type { RepositoryBranch, RepositoryBranches } from "./live-branches";

interface Choice {
  key: string;
  label: string;
  branch: RepositoryBranch | null;
}
export const RepositoryBranchSelect: Component<{
  repository: ConnectedRepository;
  branches: RepositoryBranches;
}> = (props) => {
  onMount(() => props.branches.ensure());
  const defaultChoice = createMemo<Choice>(() => ({
    key: "default",
    branch: null,
    label: `Default (${props.repository.branch})`,
  }));
  const choice = (branch: RepositoryBranch): Choice => ({
    key: `${branch.ref_name}@${branch.commit_sha}`,
    label: `${branch.name} · ${branch.commit_sha.slice(0, 8)}`,
    branch,
  });
  const selected = createMemo(() => {
    const branch = props.branches.state().selected;
    return branch ? choice(branch) : defaultChoice();
  });
  const choices = createMemo(() => {
    const rows = props.branches.state().branches.map(choice);
    const current = selected();
    if (current.branch && !rows.some((row) => row.key === current.key))
      rows.unshift(current);
    return [defaultChoice(), ...rows];
  });
  return (
    <div class="w-full min-w-0 space-y-1 sm:max-w-sm">
      <div class="flex min-w-0 items-center gap-1">
        <Select<Choice>
          class="min-w-0 flex-1"
          sameWidth
          options={choices()}
          optionValue="key"
          optionTextValue="label"
          value={selected()}
          onChange={(value) => {
            if (value && value.key !== selected().key)
              props.branches.select(value.branch);
          }}
          itemComponent={(item) => (
            <SelectItem item={item.item}>
              <span class="truncate" title={item.item.rawValue.label}>
                {item.item.rawValue.label}
              </span>
            </SelectItem>
          )}
        >
          <SelectTrigger aria-label="Select branch" class="h-8 min-w-0">
            <Icon name="git-branch" size={13} class="shrink-0" />
            <SelectValue<Choice> class="min-w-0 truncate">
              {(state) => state.selectedOption().label}
            </SelectValue>
          </SelectTrigger>
          <SelectContent class="max-h-80 max-w-[calc(100vw-2rem)] overflow-y-auto" />
        </Select>
        <Button
          variant="ghost"
          size="icon"
          class="shrink-0"
          aria-label="Refresh branch list"
          title="Refresh branch list; keep the selected commit"
          disabled={props.branches.state().loading}
          onClick={() => void props.branches.refresh()}
        >
          <Icon name="rotate-ccw" />
        </Button>
        <Show when={props.branches.state().cursor}>
          <Button
            variant="ghost"
            size="sm"
            disabled={props.branches.state().loading}
            onClick={() => void props.branches.refresh()}
          >
            First
          </Button>
        </Show>
        <Show when={props.branches.state().next_cursor}>
          <Button
            variant="ghost"
            size="sm"
            disabled={props.branches.state().loading}
            onClick={() => void props.branches.next()}
          >
            Next
          </Button>
        </Show>
      </div>
      <p class="truncate text-[11px] text-muted-foreground" role="status">
        {props.branches.state().loading
          ? "Loading branches…"
          : (props.branches.state().error ??
            (props.branches.state().branches.length === 0
              ? "No branches on this page."
              : `Snapshot ${props.branches.state().selected?.commit_sha.slice(0, 8) ?? props.repository.headCommit?.slice(0, 8) ?? "—"}. Select a branch to change it.`))}
      </p>
    </div>
  );
};
