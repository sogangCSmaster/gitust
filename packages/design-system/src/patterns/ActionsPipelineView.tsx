import { createMemo, createSignal, For, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  CardDescription,
} from "../components/ui/card";
import { Field, StatusBadge } from "../components/workspace/presentation";
import { Icon } from "../components/workspace/icon";
import { cn } from "../lib/utils";
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

export const ActionsPipelineView: Component<ActionsPipelineViewProps> = (
  props,
) => {
  const rowHeight = 24;
  const height = () => props.height ?? 420;
  const [query, setQuery] = createSignal("");
  const [scrollTop, setScrollTop] = createSignal(0);
  let viewport: HTMLDivElement | undefined;
  const logs = createMemo(() =>
    props.logs.filter((line) =>
      `${line.level} ${line.text}`
        .toLowerCase()
        .includes(query().trim().toLowerCase()),
    ),
  );
  const count = () => Math.ceil(height() / rowHeight) + 28;
  const start = createMemo(() =>
    Math.min(
      Math.max(0, Math.floor(scrollTop() / rowHeight) - 14),
      Math.max(0, logs().length - count()),
    ),
  );
  const visible = createMemo(() => logs().slice(start(), start() + count()));
  return (
    <div class="grid min-w-0 gap-5 lg:grid-cols-[16rem_minmax(0,1fr)]">
      <Card class="min-w-0 self-start">
        <CardHeader>
          <CardTitle class="break-all text-sm">{props.workflowName}</CardTitle>
          <CardDescription class="font-mono text-xs">
            {props.branch} · {props.commitSha}
          </CardDescription>
        </CardHeader>
        <CardContent class="space-y-2">
          <For each={props.jobs}>
            {(job) => (
              <Button
                variant={props.activeJobId === job.id ? "secondary" : "ghost"}
                class="h-auto w-full justify-between gap-3 whitespace-normal px-3 py-3 text-left"
                onClick={() => props.onSelectJob(job.id)}
              >
                <span class="flex items-center gap-2">
                  <Icon
                    name={
                      job.status === "success"
                        ? "check"
                        : job.status === "failed"
                          ? "x"
                          : "clock"
                    }
                  />
                  <span>{job.name}</span>
                </span>
                <StatusBadge
                  tone={
                    job.status === "success"
                      ? "success"
                      : job.status === "failed"
                        ? "danger"
                        : job.status === "running"
                          ? "warning"
                          : "neutral"
                  }
                >
                  {job.duration}
                </StatusBadge>
              </Button>
            )}
          </For>
          <p class="pt-3 text-xs text-muted-foreground">
            Sample runner ·{" "}
            {props.jobs.find((job) => job.id === props.activeJobId)?.runner}
          </p>
          <Button variant="outline" class="w-full" onClick={props.onRerunAll}>
            <Icon name="rotate-ccw" />
            Replay demo jobs
          </Button>
        </CardContent>
      </Card>
      <Card class="min-w-0 overflow-hidden">
        <div class="flex flex-wrap items-center justify-between gap-3 border-b p-4">
          <div>
            <h2 class="font-medium">Job console</h2>
            <p class="mt-1 text-xs text-muted-foreground">
              Sample logs · {logs().length.toLocaleString()} lines
            </p>
          </div>
          <Field
            label="Filter console logs"
            hideLabel
            type="search"
            class="w-full sm:max-w-xs"
            value={query()}
            onValueChange={(value) => {
              setQuery(value);
              setScrollTop(0);
              if (viewport) viewport.scrollTop = 0;
            }}
            placeholder="Filter logs..."
          />
        </div>
        <div
          ref={viewport}
          class="relative overflow-auto overscroll-contain font-mono text-xs"
          style={{ height: `${height()}px` }}
          onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
          tabIndex={0}
          aria-label="Sample job console"
        >
          <div
            class="relative min-w-full"
            style={{ height: `${logs().length * rowHeight}px` }}
          >
            <For each={visible()}>
              {(line, index) => (
                <div
                  class={cn(
                    "absolute flex min-w-full items-center gap-4 whitespace-pre px-4",
                    line.level === "error" && "text-error-foreground",
                    line.level === "success" && "text-success-foreground",
                    line.level === "warn" && "text-warning-foreground",
                  )}
                  style={{
                    height: `${rowHeight}px`,
                    transform: `translateY(${(start() + index()) * rowHeight}px)`,
                  }}
                >
                  <span class="w-10 shrink-0 select-none text-muted-foreground">
                    {line.index + 1}
                  </span>
                  <span class="w-20 shrink-0 text-muted-foreground">
                    {line.timestamp}
                  </span>
                  <span>{line.text}</span>
                </div>
              )}
            </For>
          </div>
        </div>
      </Card>
    </div>
  );
};
