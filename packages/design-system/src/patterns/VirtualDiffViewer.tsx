import {
  createEffect,
  createMemo,
  createSignal,
  For,
  on,
  type Component,
} from "solid-js";
import { Card } from "../components/ui/card";
import { StatusBadge } from "../components/workspace/presentation";
import { cn } from "../lib/utils";
export type DiffLineType = "context" | "add" | "del" | "hunk";

export interface DiffLine {
  index: number;
  oldLineNumber: number | null;
  newLineNumber: number | null;
  type: DiffLineType;
  content: string;
}

export interface VirtualDiffViewerProps {
  filePath: string;
  lines: DiffLine[];
  height?: number;
  rowHeight?: number;
}

export const VirtualDiffViewer: Component<VirtualDiffViewerProps> = (props) => {
  const rowHeight = () => props.rowHeight ?? 24;
  const viewportHeight = () => props.height ?? 360;
  const [scrollTop, setScrollTop] = createSignal(0);
  const [selected, setSelected] = createSignal<number | null>(null);
  let viewport: HTMLDivElement | undefined;
  createEffect(
    on(
      () => props.filePath,
      () => {
        setScrollTop(0);
        setSelected(null);
        if (viewport) {
          viewport.scrollTop = 0;
          viewport.scrollLeft = 0;
        }
      },
    ),
  );
  const count = () => Math.ceil(viewportHeight() / rowHeight()) + 28;
  const start = createMemo(() =>
    Math.min(
      Math.max(0, Math.floor(scrollTop() / rowHeight()) - 14),
      Math.max(0, props.lines.length - count()),
    ),
  );
  const visible = createMemo(() =>
    props.lines.slice(start(), start() + count()),
  );
  const stats = createMemo(() => ({
    add: props.lines.filter((line) => line.type === "add").length,
    del: props.lines.filter((line) => line.type === "del").length,
  }));
  return (
    <Card class="min-w-0 overflow-hidden">
      <div class="flex flex-wrap items-center justify-between gap-3 border-b bg-muted/30 px-4 py-3">
        <code class="min-w-0 break-all text-xs">{props.filePath}</code>
        <div class="flex items-center gap-3 text-xs">
          <span class="text-success-foreground">+{stats().add}</span>
          <span class="text-error-foreground">−{stats().del}</span>
          <StatusBadge>{props.lines.length.toLocaleString()} lines</StatusBadge>
        </div>
      </div>
      <div
        ref={viewport}
        class="relative overflow-auto overscroll-contain font-mono text-xs"
        style={{ height: `${viewportHeight()}px` }}
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
        tabIndex={0}
        aria-label="Repository code and diff"
      >
        <div
          class="relative min-w-full"
          style={{ height: `${props.lines.length * rowHeight()}px` }}
        >
          <For each={visible()}>
            {(line, index) => (
              <div
                class={cn(
                  "absolute left-0 flex min-w-full cursor-pointer items-center whitespace-pre border-l-2 border-transparent",
                  line.type === "add" && "bg-success text-success-foreground",
                  line.type === "del" && "bg-error text-error-foreground",
                  line.type === "hunk" && "bg-info text-info-foreground",
                  selected() === line.index && "border-primary",
                )}
                style={{
                  height: `${rowHeight()}px`,
                  transform: `translateY(${(start() + index()) * rowHeight()}px)`,
                }}
                onClick={() => setSelected(line.index)}
              >
                <span class="w-12 shrink-0 select-none px-2 text-right text-muted-foreground">
                  {line.oldLineNumber ?? ""}
                </span>
                <span class="w-12 shrink-0 select-none px-2 text-right text-muted-foreground">
                  {line.newLineNumber ?? ""}
                </span>
                <span class="w-8 shrink-0 select-none text-center">
                  {line.type === "add"
                    ? "+"
                    : line.type === "del"
                      ? "−"
                      : line.type === "hunk"
                        ? "@@"
                        : " "}
                </span>
                <span class="pr-4">{line.content}</span>
              </div>
            )}
          </For>
        </div>
      </div>
    </Card>
  );
};
