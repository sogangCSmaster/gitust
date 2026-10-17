import { filterIssueItems } from "./issue-filter";
export { filterIssueItems } from "./issue-filter";
import {
  createEffect,
  createMemo,
  createSignal,
  For,
  Show,
  type Component,
} from "solid-js";
import { Card } from "../components/ui/card";
import { Button } from "../components/ui/button";
import { Checkbox } from "../components/ui/checkbox";
import { Icon } from "../components/workspace/icon";
import {
  EmptyState,
  Field,
  PersonAvatar,
  StatusBadge,
  type StatusTone,
} from "../components/workspace/presentation";

export interface IssueItem {
  id: number;
  kind: "issue" | "pr";
  title: string;
  status: "open" | "closed" | "merged" | "draft";
  author: string;
  labels: { name: string; variant: StatusTone }[];
  commentsCount: number;
  updatedAt: string;
  branch?: string;
  ciStatus?: "success" | "pending" | "failed";
}

export interface IssueListViewProps {
  items: IssueItem[];
  focusedIndex: number;
  onFocusIndexChange: (idx: number) => void;
  onToggleStatus: (id: number) => void;
  filterRef?: (el: HTMLInputElement) => void;
  filterQuery?: string;
  onFilterQueryChange?: (query: string) => void;
}

export const IssueListView: Component<IssueListViewProps> = (props) => {
  const [localFilter, setLocalFilter] = createSignal("");
  const [selectedIds, setSelectedIds] = createSignal<Set<number>>(new Set());
  const query = () => props.filterQuery ?? localFilter();
  const items = createMemo(() => filterIssueItems(props.items, query()));
  let list: HTMLUListElement | undefined;
  createEffect(() => {
    const index = props.focusedIndex;
    items();
    list?.children[index]?.scrollIntoView?.({ block: "nearest" });
  });
  const toggleSelected = (id: number, checked: boolean) =>
    setSelectedIds((previous) => {
      const next = new Set(previous);
      checked ? next.add(id) : next.delete(id);
      return next;
    });
  return (
    <div class="space-y-4">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <Field
          class="w-full sm:max-w-sm"
          label="Filter issues and pull requests"
          hideLabel
          type="search"
          placeholder="Search, or try is:open..."
          inputRef={props.filterRef}
          value={query()}
          onValueChange={(value) => {
            setLocalFilter(value);
            props.onFilterQueryChange?.(value);
            props.onFocusIndexChange(0);
          }}
        />
        <p class="text-xs text-muted-foreground">
          {selectedIds().size
            ? `${selectedIds().size} selected`
            : `${items().length} results`}
        </p>
      </div>
      <Card class="overflow-hidden">
        <Show when={!items().length}>
          <EmptyState
            title="No matching issues"
            description="Try a different search or status."
          />
        </Show>
        <ul ref={list} aria-label="Issues and pull requests" class="divide-y">
          <For each={items()}>
            {(item, index) => (
              <li
                class="space-y-2 p-4 data-[focused=true]:bg-muted/50"
                data-focused={props.focusedIndex === index()}
                onMouseEnter={() => props.onFocusIndexChange(index())}
              >
                <div class="flex items-start gap-3">
                  <Checkbox
                    aria-label={`Select issue ${item.id}`}
                    checked={selectedIds().has(item.id)}
                    onChange={(checked) => toggleSelected(item.id, checked)}
                    class="mt-1"
                  />
                  <div class="min-w-0 flex-1">
                    <div class="flex flex-wrap items-center gap-2">
                      <span class="font-mono text-xs text-muted-foreground">
                        #{item.id}
                      </span>
                      <StatusBadge
                        tone={
                          item.status === "open"
                            ? "success"
                            : item.status === "closed"
                              ? "danger"
                              : item.status === "merged"
                                ? "merged"
                                : "info"
                        }
                      >
                        <Icon
                          name={
                            item.kind === "pr"
                              ? "git-pull-request"
                              : "issue-opened"
                          }
                          size={12}
                        />
                        {item.status}
                      </StatusBadge>
                      <For each={item.labels}>
                        {(label) => (
                          <StatusBadge tone={label.variant}>
                            {label.name}
                          </StatusBadge>
                        )}
                      </For>
                    </div>
                    <Button
                      variant="link"
                      class="mt-1 h-auto justify-start whitespace-normal p-0 text-left text-foreground"
                      onClick={() => props.onToggleStatus(item.id)}
                    >
                      {item.title}
                    </Button>
                    <div class="mt-3 flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
                      <PersonAvatar name={item.author} class="size-5" />
                      <span>{item.author}</span>
                      <span>{item.updatedAt}</span>
                      <span class="inline-flex items-center gap-1">
                        <Icon name="comment" size={12} />
                        {item.commentsCount}
                      </span>
                      <Show when={item.branch}>
                        <code class="break-all">{item.branch}</code>
                      </Show>
                      <Show when={item.ciStatus}>
                        <span>CI: {item.ciStatus}</span>
                      </Show>
                    </div>
                  </div>
                </div>
              </li>
            )}
          </For>
        </ul>
      </Card>
    </div>
  );
};
