import { createSignal, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import { Icon } from "../components/workspace/icon";
import {
  VirtualFileTree,
  type FileTreeNode,
} from "../patterns/VirtualFileTree";
import {
  VirtualDiffViewer,
  type DiffLine,
} from "../patterns/VirtualDiffViewer";

export const RepositoryFilesView: Component<{
  files: FileTreeNode[];
  selectedFilePath: string;
  lines: DiffLine[];
  onSelect: (file: FileTreeNode) => void;
  label?: string;
  initialDetail?: boolean;
}> = (props) => {
  const [detail, setDetail] = createSignal(props.initialDetail ?? false);
  let list: HTMLDivElement | undefined;
  let back: HTMLButtonElement | undefined;
  return (
    <div class="grid min-w-0 gap-3 md:grid-cols-[minmax(10rem,0.7fr)_minmax(0,1.3fr)]">
      <div ref={list} class={detail() ? "hidden min-w-0 md:block" : "min-w-0"}>
        <VirtualFileTree
          label={props.label}
          files={props.files}
          activePath={props.selectedFilePath}
          onSelect={(file) => {
            if (file.kind === "file") {
              setDetail(true);
              queueMicrotask(() => {
                if (back?.isConnected && back.getClientRects().length)
                  back.focus();
              });
            }
            props.onSelect(file);
          }}
          height={420}
        />
      </div>
      <div
        class={
          detail() ? "min-w-0 space-y-2" : "hidden min-w-0 space-y-2 md:block"
        }
      >
        <Button
          ref={back}
          variant="ghost"
          size="sm"
          class="-ml-2 md:hidden"
          aria-label="Back to files"
          onClick={() => {
            setDetail(false);
            const target =
              list?.querySelector<HTMLElement>('button[data-active="true"]') ??
              list?.querySelector<HTMLInputElement>("input");
            target?.focus();
          }}
        >
          <Icon name="chevron-right" class="rotate-180" />
          Files
        </Button>
        <VirtualDiffViewer
          filePath={props.selectedFilePath}
          lines={props.lines}
          height={420}
        />
      </div>
    </div>
  );
};
