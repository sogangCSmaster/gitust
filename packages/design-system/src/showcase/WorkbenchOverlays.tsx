import { lazy, Show, type Component } from "solid-js";
import type { AgentSandboxTask } from "../index";
import { DeferredView } from "../components/workspace/deferred-view";
import type { createNewWorktreeController } from "./new-worktree-controller";

const NewWorktreeDialog = lazy(() =>
  import("./NewWorktreeDialog").then((module) => ({
    default: module.NewWorktreeDialog,
  })),
);
const LiveRunHistoryDialog = lazy(() =>
  import("./LiveRunHistoryDialog").then((module) => ({
    default: module.LiveRunHistoryDialog,
  })),
);
const CommandPalette = lazy(() =>
  import("../patterns/CommandPalette").then((module) => ({
    default: module.CommandPalette,
  })),
);
const ShortcutsReferenceModal = lazy(() =>
  import("./ShortcutsReferenceModal").then((module) => ({
    default: module.ShortcutsReferenceModal,
  })),
);

export const WorkbenchOverlays: Component<{
  newWorktree: ReturnType<typeof createNewWorktreeController>;
  onRefreshWorktrees: () => void;
  historyTarget: AgentSandboxTask | null;
  onCloseHistory: () => void;
  cmdOpen: boolean;
  onCloseCmd: () => void;
  shortcutsModalOpen: boolean;
  onCloseShortcutsModal: () => void;
}> = (props) => (
  <>
    <Show when={props.newWorktree.target()} keyed>
      {(repository) => (
        <DeferredView label="New worktree" onCancel={props.newWorktree.close}>
          <NewWorktreeDialog
            repository={repository}
            submitting={props.newWorktree.submitting()}
            onSubmit={props.newWorktree.submit}
            onClose={props.newWorktree.close}
            onRefresh={props.onRefreshWorktrees}
          />
        </DeferredView>
      )}
    </Show>
    <Show when={props.historyTarget} keyed>
      {(task) => (
        <DeferredView label="Activity" onCancel={props.onCloseHistory}>
          <LiveRunHistoryDialog
            worktreeId={task.liveId!}
            branch={task.branch}
            onClose={props.onCloseHistory}
          />
        </DeferredView>
      )}
    </Show>
    <Show when={props.cmdOpen}>
      <DeferredView label="Command palette" onCancel={props.onCloseCmd}>
        <CommandPalette open={props.cmdOpen} onClose={props.onCloseCmd} />
      </DeferredView>
    </Show>
    <Show when={props.shortcutsModalOpen}>
      <DeferredView
        label="Keyboard shortcuts"
        onCancel={props.onCloseShortcutsModal}
      >
        <ShortcutsReferenceModal
          open={props.shortcutsModalOpen}
          onClose={props.onCloseShortcutsModal}
        />
      </DeferredView>
    </Show>
  </>
);
