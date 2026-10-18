import {
  createEffect,
  createMemo,
  createSignal,
  on,
  Show,
  type Component,
} from "solid-js";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Icon } from "../components/workspace/icon";
import { RepositoryCodeSearchPanel } from "../patterns/RepositoryCodeSearchPanel";
import {
  VirtualFileTree,
  type FileTreeNode,
} from "../patterns/VirtualFileTree";
import {
  VirtualDiffViewer,
  type DiffLine,
} from "../patterns/VirtualDiffViewer";

export interface RepositoryFileEntry extends FileTreeNode {
  content?: string;
}

export interface RepositoryFilesViewProps {
  files: RepositoryFileEntry[];
  selectedFilePath?: string;
  lines?: DiffLine[];
  onSelect?: (file: FileTreeNode) => void;
  label?: string;
  initialDetail?: boolean;
  liveMode?: boolean;
  repositoryId?: string;
  revision?: string;
}

export const RepositoryFilesView: Component<RepositoryFilesViewProps> = (
  props,
) => {
  const [detail, setDetail] = createSignal(props.initialDetail ?? false);
  const [internalSelectedPath, setInternalSelectedPath] = createSignal(
    props.files[0]?.path ?? "",
  );
  const [highlightedLine, setHighlightedLine] = createSignal<number | null>(
    null,
  );

  const activePath = () => props.selectedFilePath ?? internalSelectedPath();

  const activeLines = createMemo<DiffLine[]>(() => {
    if (props.lines !== undefined) return props.lines;
    const target = props.files.find((f) => f.path === activePath());
    if (!target?.content) return [];
    return target.content.split(/\r?\n/).map((line, index) => ({
      index,
      type: "context" as const,
      oldLineNumber: index + 1,
      newLineNumber: index + 1,
      content: line,
    }));
  });

  createEffect(
    on(
      () => [props.repositoryId, props.revision] as const,
      () => {
        setHighlightedLine(null);
      },
      { defer: true },
    ),
  );

  let list: HTMLDivElement | undefined;
  let back: HTMLButtonElement | undefined;
  createEffect(() => {
    if (activePath() !== "") return;
    const returnFocus = document.activeElement === back;
    setDetail(false);
    if (returnFocus)
      queueMicrotask(() => {
        if (
          document.activeElement === back ||
          document.activeElement === document.body
        )
          list?.querySelector<HTMLInputElement>("input")?.focus();
      });
  });

  const handleSelectHit = (path: string, lineNumber: number) => {
    const matched: FileTreeNode = props.files.find((f) => f.path === path) ?? {
      path,
      kind: "file",
    };
    setDetail(true);
    setInternalSelectedPath(path);
    setHighlightedLine(lineNumber);
    props.onSelect?.(matched);
  };

  return (
    <div class="space-y-3">
      <Show when={!props.label}>
        <RepositoryCodeSearchPanel
          files={props.files}
          liveMode={props.liveMode}
          repositoryId={props.repositoryId}
          revision={props.revision}
          onSelectHit={handleSelectHit}
        />
      </Show>
      <div class="grid min-w-0 gap-3 md:grid-cols-[minmax(10rem,0.7fr)_minmax(0,1.3fr)]">
        <div
          ref={list}
          class={detail() ? "hidden min-w-0 md:block" : "min-w-0"}
        >
          <VirtualFileTree
            label={props.label}
            files={props.files}
            activePath={activePath()}
            onSelect={(file) => {
              if (file.kind === "file") {
                setDetail(true);
                setInternalSelectedPath(file.path);
                setHighlightedLine(null);
                queueMicrotask(() => {
                  if (back?.isConnected && back.getClientRects().length)
                    back.focus();
                });
              }
              props.onSelect?.(file);
            }}
            height={420}
          />
        </div>
        <div
          class={
            detail()
              ? "min-w-0 space-y-2"
              : "hidden min-w-0 space-y-2 md:block"
          }
        >
          <div class="flex flex-wrap items-center justify-between gap-2">
            <Button
              ref={back}
              variant="ghost"
              size="sm"
              class="-ml-2 md:hidden"
              aria-label="Back to files"
              onClick={() => {
                setDetail(false);
                const target =
                  list?.querySelector<HTMLElement>(
                    'button[data-active="true"]',
                  ) ?? list?.querySelector<HTMLInputElement>("input");
                target?.focus();
              }}
            >
              <Icon name="chevron-right" class="rotate-180" />
              Files
            </Button>
            <Show when={highlightedLine()}>
              {(line) => (
                <Badge variant="secondary" class="font-mono text-[10px]">
                  Line {line()}
                </Badge>
              )}
            </Show>
          </div>
          <VirtualDiffViewer
            filePath={activePath()}
            lines={activeLines()}
            height={420}
          />
        </div>
      </div>
    </div>
  );
};
