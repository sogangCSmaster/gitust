import { lazy, type Component } from "solid-js";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "../components/ui/dialog";
import { DeferredView } from "../components/workspace/deferred-view";
import type { ConnectedRepository } from "./connected-repository-client";
import type { NewWorktreeInput, NewWorktreeResult } from "./new-worktree-input";
export interface NewWorktreeDialogProps {
  repository: ConnectedRepository;
  submitting: boolean;
  onSubmit: (input: NewWorktreeInput) => Promise<NewWorktreeResult>;
  onClose: () => void;
  onRefresh: () => void;
}
const Form = lazy(() =>
  import("./NewWorktreeForm").then((module) => ({
    default: module.NewWorktreeForm,
  })),
);
export const NewWorktreeDialog: Component<NewWorktreeDialogProps> = (props) => (
  <Dialog
    open
    onOpenChange={(open) => {
      if (!open) props.onClose();
    }}
  >
    <DialogContent class="max-h-[90svh] w-[calc(100%-2rem)] max-w-md rounded-lg">
      <DialogHeader class="pr-8 text-left">
        <DialogTitle>New worktree</DialogTitle>
        <DialogDescription>
          Create an isolated workspace. Start your agent separately.
        </DialogDescription>
      </DialogHeader>
      <p
        class="truncate text-xs text-muted-foreground"
        title={props.repository.name}
      >
        {props.repository.name}
      </p>
      <DeferredView label="New worktree" onCancel={props.onClose}>
        <Form {...props} />
      </DeferredView>
    </DialogContent>
  </Dialog>
);
