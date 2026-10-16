import { lazy, type Component } from "solid-js";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "../components/ui/dialog";
import { DeferredView } from "../components/workspace/deferred-view";
import type { RunHistoryState } from "./run-history-types";

export interface RunHistoryDialogProps {
  branch: string;
  state: RunHistoryState;
  onRefresh: () => void;
  onClose: () => void;
}
const Content = lazy(() =>
  import("./RunHistoryContent").then((module) => ({
    default: module.RunHistoryContent,
  })),
);

export const RunHistoryDialog: Component<RunHistoryDialogProps> = (props) => (
  <Dialog
    open
    onOpenChange={(open) => {
      if (!open) props.onClose();
    }}
  >
    <DialogContent class="max-h-[90svh] w-[calc(100%-2rem)] max-w-4xl rounded-lg">
      <DialogHeader class="pr-10 text-left">
        <DialogTitle>Run history</DialogTitle>
        <DialogDescription>
          The latest five completed runs, including saved results from before a
          server restart. Past results do not grant approval.
        </DialogDescription>
      </DialogHeader>
      <code class="min-w-0 break-all text-xs text-muted-foreground">
        {props.branch}
      </code>
      <DeferredView label="Run history" onCancel={props.onClose}>
        <Content {...props} />
      </DeferredView>
    </DialogContent>
  </Dialog>
);
