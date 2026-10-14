import {
  Component,
  createMemo,
  createSelector,
  createSignal,
  For,
} from "solid-js";
import { Icon } from "../primitives/Icon";
import { Input } from "../primitives/Input";

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
  const rowHeight = 28;
  const viewportHeight = () => props.height ?? 320;
  const overscan = 10;

  let viewportRef: HTMLDivElement | undefined;
  const [filter, setFilter] = createSignal("");
  const [scrollTop, setScrollTop] = createSignal(0);
  const isPathSelected = createSelector(() => props.activePath);

  const filteredFiles = createMemo(() => {
    const q = filter().trim().toLowerCase();
    if (!q) return props.files;
    return props.files.filter((f) => f.path.toLowerCase().includes(q));
  });

  const visibleCount = createMemo(
    () => Math.ceil(viewportHeight() / rowHeight) + overscan * 2
  );

  const startIdx = createMemo(() => {
    const total = filteredFiles().length;
    const rawStart = Math.max(0, Math.floor(scrollTop() / rowHeight) - overscan);
    const maxStart = Math.max(0, total - visibleCount());
    return Math.min(rawStart, maxStart);
  });

  const visibleSlice = createMemo(() => {
    const list = filteredFiles();
    const start = startIdx();
    const end = Math.min(list.length, start + visibleCount());
    return list.slice(start, end);
  });

  return (
    <div class="ds-tree-container">
      <div
        style={{
          padding: "var(--ds-space-2)",
          "border-bottom": "1px solid var(--ds-border-default)",
          "background-color": "var(--ds-bg-secondary)",
        }}
      >
        <Input
          icon={<Icon name="search" size={14} />}
          placeholder={`Jump to file across ${props.files.length.toLocaleString()} paths...`}
          value={filter()}
          onInput={(e) => {
            setFilter(e.currentTarget.value);
            setScrollTop(0);
            if (viewportRef) viewportRef.scrollTop = 0;
          }}
          shortcut="t"
        />
      </div>

      <div
        ref={viewportRef}
        style={{
          position: "relative",
          height: `${viewportHeight()}px`,
          "overflow-y": "auto",
          "overscroll-behavior": "contain",
          contain: "strict",
        }}
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
      >
        <div
          style={{
            position: "relative",
            height: `${filteredFiles().length * rowHeight}px`,
          }}
        >
          <For each={visibleSlice()}>
            {(item, idx) => (
              <div
                class="ds-tree-row ds-mono"
                data-active={isPathSelected(item.path)}
                style={{
                  transform: `translateY(${(startIdx() + idx()) * rowHeight}px)`,
                }}
                onClick={() => props.onSelect(item)}
              >
                <span
                  style={{
                    display: "inline-flex",
                    "align-items": "center",
                    gap: "var(--ds-space-2)",
                    overflow: "hidden",
                    "text-overflow": "ellipsis",
                    "white-space": "nowrap",
                  }}
                >
                  <span
                    style={{
                      display: "inline-flex",
                      color:
                        item.kind === "dir"
                          ? "var(--ds-info-fg)"
                          : "var(--ds-fg-muted)",
                    }}
                  >
                    <Icon
                      name={item.kind === "dir" ? "folder" : "file-code"}
                      size={14}
                    />
                  </span>
                  <span>{item.path}</span>
                </span>
                <span style={{ color: "var(--ds-fg-muted)" }}>
                  {item.sizeBytes
                    ? `${(item.sizeBytes / 1024).toFixed(1)} KB`
                    : ""}
                </span>
              </div>
            )}
          </For>
        </div>
      </div>
    </div>
  );
};
