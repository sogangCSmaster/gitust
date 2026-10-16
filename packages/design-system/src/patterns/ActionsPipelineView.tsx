import { Component, createMemo, createSignal, For } from "solid-js";
import { Badge, BadgeVariant } from "../primitives/Badge";
import { Button } from "../primitives/Button";
import { Icon } from "../primitives/Icon";
import { Input } from "../primitives/Input";

export interface ActionJobStep {
  id: string;
  name: string;
  status: "success" | "running" | "failed" | "queued";
  duration: string;
  runner: string;
}

export interface ActionLogLine {
  index: number;
  timestamp: string;
  level: "info" | "warn" | "error" | "success";
  text: string;
}

export interface ActionsPipelineViewProps {
  workflowName: string;
  commitSha: string;
  branch: string;
  jobs: ActionJobStep[];
  activeJobId: string;
  onSelectJob: (id: string) => void;
  onRerunAll: () => void;
  logs: ActionLogLine[];
  height?: number;
}

/**
 * CI/CD pipeline interface and viewport-sliced log viewer.
 * Built for Self-Hosted Enterprise runners with instant log filtering across 10,000+ lines.
 */
export const ActionsPipelineView: Component<ActionsPipelineViewProps> = (
  props
) => {
  const rowHeight = 20;
  const viewportHeight = () => props.height ?? 420;
  const overscan = 14;

  let viewportRef: HTMLDivElement | undefined;
  const [scrollTop, setScrollTop] = createSignal(0);
  const [logQuery, setLogQuery] = createSignal("");

  const filteredLogs = createMemo(() => {
    const q = logQuery().trim().toLowerCase();
    if (!q) return props.logs;
    return props.logs.filter(
      (l) =>
        l.text.toLowerCase().includes(q) || l.level.toLowerCase().includes(q)
    );
  });

  const visibleCount = createMemo(
    () => Math.ceil(viewportHeight() / rowHeight) + overscan * 2
  );

  const startIdx = createMemo(() => {
    const total = filteredLogs().length;
    const rawStart = Math.max(0, Math.floor(scrollTop() / rowHeight) - overscan);
    const maxStart = Math.max(0, total - visibleCount());
    return Math.min(rawStart, maxStart);
  });

  const visibleLogs = createMemo(() => {
    const list = filteredLogs();
    const start = startIdx();
    const end = Math.min(list.length, start + visibleCount());
    return list.slice(start, end);
  });

  const statusBadge = (status: ActionJobStep["status"]): BadgeVariant => {
    switch (status) {
      case "success":
        return "success";
      case "running":
        return "warning";
      case "failed":
        return "danger";
      case "queued":
        return "neutral";
    }
  };

  return (
    <div class="ds-actions-layout">
      {/* Left Sidebar: Workflow Jobs & Self-Hosted Runner Assignment */}
      <div class="ds-actions-sidebar">
        <div class="ds-actions-sidebar-header">
          <span
            style={{
              display: "inline-flex",
              "align-items": "center",
              gap: "var(--ds-space-1-5)",
            }}
          >
            <Icon name="play" size={13} />
            <span>{props.workflowName}</span>
          </span>
          <Badge variant="neutral" mono>
            {props.commitSha}
          </Badge>
        </div>

        <For each={props.jobs}>
          {(job) => (
            <div
              class="ds-actions-job-row"
              data-active={props.activeJobId === job.id}
              onClick={() => props.onSelectJob(job.id)}
            >
              <div
                style={{
                  display: "flex",
                  "align-items": "center",
                  gap: "var(--ds-space-2)",
                  "min-width": 0,
                }}
              >
                <span
                  style={{
                    display: "inline-flex",
                    color:
                      job.status === "success"
                        ? "var(--ds-success-fg)"
                        : job.status === "failed"
                          ? "var(--ds-danger-fg)"
                          : "var(--ds-warning-fg)",
                  }}
                >
                  <Icon
                    name={
                      job.status === "success"
                        ? "check"
                        : job.status === "failed"
                          ? "x"
                          : "clock"
                    }
                    size={14}
                  />
                </span>
                <span
                  style={{
                    overflow: "hidden",
                    "text-overflow": "ellipsis",
                    "white-space": "nowrap",
                  }}
                >
                  {job.name}
                </span>
              </div>
              <div
                style={{
                  display: "flex",
                  "align-items": "center",
                  gap: "var(--ds-space-1-5)",
                }}
              >
                <Badge variant={statusBadge(job.status)} mono>
                  {job.duration}
                </Badge>
              </div>
            </div>
          )}
        </For>

        <div
          style={{
            padding: "var(--ds-space-2-5)",
            "background-color": "var(--ds-bg-secondary)",
            "border-top": "1px solid var(--ds-border-subtle)",
            display: "flex",
            "flex-direction": "column",
            gap: "var(--ds-space-2)",
          }}
        >
          <div
            style={{
              display: "flex",
              "align-items": "center",
              "justify-content": "space-between",
              "font-size": "var(--ds-font-size-2xs)",
              color: "var(--ds-fg-muted)",
            }}
          >
            <span>Self-Hosted Runner</span>
            <span class="ds-mono" style={{ color: "var(--ds-success-fg)" }}>
              corp-runner-linux-01 (Online)
            </span>
          </div>
          <Button
            variant="secondary"
            size="sm"
            icon={<Icon name="rotate-ccw" size={13} />}
            shortcut="r"
            onClick={props.onRerunAll}
          >
            Re-run All Jobs (Optimistic)
          </Button>
        </div>
      </div>

      {/* Right Pane: Virtualized 10,000-Line Live Build Console */}
      <div class="ds-diff-container">
        <div class="ds-diff-toolbar">
          <div
            style={{
              display: "flex",
              "align-items": "center",
              gap: "var(--ds-space-2)",
            }}
          >
            <Icon name="terminal" size={14} />
            <span style={{ "font-weight": 600 }}>
              Job Console Stream ({filteredLogs().length.toLocaleString()}{" "}
              lines)
            </span>
            <Badge variant="info" mono>
              <Icon name="git-branch" size={11} />
              <span>{props.branch}</span>
            </Badge>
          </div>
          <div style={{ width: "280px" }}>
            <Input
              icon={<Icon name="search" size={13} />}
              placeholder="Filter console logs (e.g. error, cargo)..."
              value={logQuery()}
              onInput={(e) => {
                setLogQuery(e.currentTarget.value);
                setScrollTop(0);
                if (viewportRef) viewportRef.scrollTop = 0;
              }}
            />
          </div>
        </div>

        <div
          ref={viewportRef}
          class="ds-diff-viewport"
          style={{ height: `${viewportHeight()}px` }}
          onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
        >
          <div
            class="ds-diff-spacer"
            style={{ height: `${filteredLogs().length * rowHeight}px` }}
          >
            <For each={visibleLogs()}>
              {(line, idx) => (
                <div
                  class="ds-log-row"
                  data-level={line.level}
                  style={{
                    transform: `translateY(${(startIdx() + idx()) * rowHeight}px)`,
                  }}
                >
                  <span
                    style={{
                      width: "44px",
                      color: "var(--ds-fg-subtle)",
                      "user-select": "none",
                      "flex-shrink": 0,
                    }}
                  >
                    {line.index + 1}
                  </span>
                  <span
                    style={{
                      width: "92px",
                      color: "var(--ds-fg-muted)",
                      "user-select": "none",
                      "flex-shrink": 0,
                    }}
                  >
                    {line.timestamp}
                  </span>
                  <span>{line.text}</span>
                </div>
              )}
            </For>
          </div>
        </div>
      </div>
    </div>
  );
};
