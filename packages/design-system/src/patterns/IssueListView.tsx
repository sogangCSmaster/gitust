import { Component, createMemo, createSignal, For, Show } from "solid-js";
import { Badge, BadgeVariant } from "../primitives/Badge";
import { Icon, IconName } from "../primitives/Icon";
import { Input } from "../primitives/Input";
import { Kbd } from "../primitives/Kbd";
import { Avatar } from "../primitives/TabsAndAvatar";

export interface IssueItem {
  id: number;
  kind: "issue" | "pr";
  title: string;
  status: "open" | "closed" | "merged" | "draft";
  author: string;
  labels: { name: string; variant: BadgeVariant }[];
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
}

export const IssueListView: Component<IssueListViewProps> = (props) => {
  const [filterQuery, setFilterQuery] = createSignal("");
  const [selectedIds, setSelectedIds] = createSignal<Set<number>>(new Set());

  const filteredItems = createMemo(() => {
    const q = filterQuery().trim().toLowerCase();
    if (!q) return props.items;
    return props.items.filter((item) => {
      if (q.includes("is:open") && item.status !== "open") return false;
      if (q.includes("is:closed") && item.status !== "closed") return false;
      if (q.includes("is:merged") && item.status !== "merged") return false;
      if (q.includes("is:pr") && item.kind !== "pr") return false;
      if (q.includes("is:issue") && item.kind !== "issue") return false;

      const cleanText = q
        .replace(/is:(open|closed|merged|pr|issue)/g, "")
        .trim();
      if (!cleanText) return true;
      return (
        item.title.toLowerCase().includes(cleanText) ||
        item.author.toLowerCase().includes(cleanText) ||
        `#${item.id}`.includes(cleanText) ||
        item.labels.some((l) => l.name.toLowerCase().includes(cleanText))
      );
    });
  });

  const statusBadgeVariant = (status: IssueItem["status"]): BadgeVariant => {
    switch (status) {
      case "open":
        return "success";
      case "closed":
        return "danger";
      case "merged":
        return "merged";
      case "draft":
        return "info";
    }
  };

  const itemIconName = (item: IssueItem): IconName => {
    if (item.kind === "pr") {
      if (item.status === "merged") return "git-merge";
      return "git-pull-request";
    }
    if (item.status === "closed") return "issue-closed";
    if (item.status === "draft") return "issue-draft";
    return "issue-opened";
  };

  const toggleSelect = (id: number) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  return (
    <div
      style={{
        display: "flex",
        "flex-direction": "column",
        gap: "var(--ds-space-3)",
      }}
    >
      {/* Superhuman Instant Filter Bar */}
      <div
        style={{
          display: "flex",
          "align-items": "center",
          gap: "var(--ds-space-2)",
        }}
      >
        <Input
          inputRef={props.filterRef}
          icon={<Icon name="search" size={14} />}
          placeholder="Instant filter (try: is:open, is:pr, gix, russh) — 0ms local index"
          value={filterQuery()}
          onInput={(e) => {
            setFilterQuery(e.currentTarget.value);
            props.onFocusIndexChange(0);
          }}
          shortcut="/"
        />
        <div
          style={{
            display: "flex",
            "align-items": "center",
            gap: "var(--ds-space-1-5)",
            "flex-shrink": 0,
            color: "var(--ds-fg-muted)",
            "font-size": "var(--ds-font-size-xs)",
          }}
        >
          <span>Navigate:</span>
          <Kbd keys="j" />
          <Kbd keys="k" />
          <span>Toggle state (Optimistic 0ms):</span>
          <Kbd keys="e" />
        </div>
      </div>

      {/* High-Density Issue / PR List */}
      <div
        class="ds-issue-list"
        role="listbox"
        aria-label="Issues and Pull Requests"
      >
        <For each={filteredItems()}>
          {(item, index) => {
            const isFocused = () => props.focusedIndex === index();
            const isChecked = () => selectedIds().has(item.id);

            return (
              <div
                role="option"
                aria-selected={isFocused()}
                data-focused={isFocused()}
                class="ds-issue-row"
                onMouseEnter={() => props.onFocusIndexChange(index())}
                onClick={() => props.onToggleStatus(item.id)}
              >
                <div class="ds-issue-left">
                  <input
                    type="checkbox"
                    checked={isChecked()}
                    onClick={(e) => {
                      e.stopPropagation();
                      toggleSelect(item.id);
                    }}
                    style={{
                      "accent-color": "var(--ds-accent-primary)",
                      cursor: "pointer",
                    }}
                  />
                  <span class="ds-issue-id">#{item.id}</span>
                  <Badge variant={statusBadgeVariant(item.status)}>
                    <Icon name={itemIconName(item)} size={12} />
                    <span>{item.status}</span>
                  </Badge>
                  <span class="ds-issue-title">{item.title}</span>
                  <For each={item.labels}>
                    {(lbl) => <Badge variant={lbl.variant}>{lbl.name}</Badge>}
                  </For>
                </div>

                <div class="ds-issue-right">
                  <Show when={item.branch}>
                    {(br) => (
                      <Badge variant="neutral" mono>
                        <Icon name="git-branch" size={12} />
                        <span>{br()}</span>
                      </Badge>
                    )}
                  </Show>
                  <Show when={item.ciStatus}>
                    {(ci) => (
                      <span
                        title={`CI: ${ci()}`}
                        style={{
                          display: "inline-flex",
                          "align-items": "center",
                          color:
                            ci() === "success"
                              ? "var(--ds-success-fg)"
                              : ci() === "failed"
                                ? "var(--ds-danger-fg)"
                                : "var(--ds-warning-fg)",
                        }}
                      >
                        <Icon
                          name={
                            ci() === "success"
                              ? "check"
                              : ci() === "failed"
                                ? "x"
                                : "clock"
                          }
                          size={14}
                        />
                      </span>
                    )}
                  </Show>
                  <span
                    class="ds-mono"
                    style={{
                      display: "inline-flex",
                      "align-items": "center",
                      "justify-content": "flex-end",
                      gap: "4px",
                      "font-size": "var(--ds-font-size-xs)",
                      color: "var(--ds-fg-muted)",
                      width: "42px",
                    }}
                  >
                    <Icon name="comment" size={12} />
                    <span>{item.commentsCount}</span>
                  </span>
                  <Avatar name={item.author} size="xs" />
                  <span
                    style={{
                      "font-size": "var(--ds-font-size-xs)",
                      color: "var(--ds-fg-muted)",
                      width: "48px",
                      "text-align": "right",
                    }}
                  >
                    {item.updatedAt}
                  </span>
                </div>
              </div>
            );
          }}
        </For>
      </div>
    </div>
  );
};
