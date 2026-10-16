import { createMemo, createSignal, For, type Component } from "solid-js";
import { Card } from "../components/ui/card";
import { Button } from "../components/ui/button";
import { Icon } from "../components/workspace/icon";
import { Field } from "../components/workspace/presentation";
export interface FileTreeNode {
  path: string;
  kind: "file" | "dir";
  sizeBytes?: number;
  lastCommitMsg?: string;
}

export interface VirtualFileTreeProps {
  files: FileTreeNode[];
  activePath?: string;
  onSelect: (file: FileTreeNode) => void;
  height?: number;
}

export const VirtualFileTree: Component<VirtualFileTreeProps> = (props) => {
  const rowHeight = 44;
  const height = () => props.height ?? 400;
  const [query, setQuery] = createSignal("");
  const [scrollTop, setScrollTop] = createSignal(0);
  let viewport: HTMLDivElement | undefined;
  const files = createMemo(() =>
    props.files.filter((file) =>
      file.path.toLowerCase().includes(query().trim().toLowerCase()),
    ),
  );
  const count = () => Math.ceil(height() / rowHeight) + 20;
  const start = createMemo(() =>
    Math.min(
      Math.max(0, Math.floor(scrollTop() / rowHeight) - 10),
      Math.max(0, files().length - count()),
    ),
  );
  const visible = createMemo(() => files().slice(start(), start() + count()));
  return (
    <Card class="min-w-0 overflow-hidden">
      <div class="border-b p-3">
        <Field
          label="Search repository paths"
          hideLabel
          type="search"
          placeholder={`Find a file in ${props.files.length.toLocaleString()} paths...`}
          value={query()}
          onValueChange={(value) => {
            setQuery(value);
            setScrollTop(0);
            if (viewport) viewport.scrollTop = 0;
          }}
        />
      </div>
      <div
        ref={viewport}
        class="relative overflow-auto overscroll-contain"
        style={{ height: `${height()}px` }}
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
        aria-label="Repository files"
      >
        <div
          class="relative"
          style={{ height: `${files().length * rowHeight}px` }}
        >
          <For each={visible()}>
            {(file, index) => (
              <Button
                variant="ghost"
                class="absolute left-0 flex h-11 w-full justify-between rounded-none px-4 font-mono text-xs data-[active=true]:bg-accent"
                data-active={props.activePath === file.path}
                aria-label={file.path}
                title={file.path}
                style={{
                  transform: `translateY(${(start() + index()) * rowHeight}px)`,
                }}
                onClick={() => props.onSelect(file)}
              >
                <span class="flex min-w-0 items-center gap-2">
                  <Icon name={file.kind === "dir" ? "folder" : "file-code"} />
                  <span class="truncate">{file.path}</span>
                </span>
                <span class="shrink-0 text-muted-foreground">
                  {file.sizeBytes
                    ? `${(file.sizeBytes / 1024).toFixed(1)} KB`
                    : ""}
                </span>
              </Button>
            )}
          </For>
        </div>
      </div>
    </Card>
  );
};
