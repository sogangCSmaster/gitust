import { lazy, Show, type Component } from "solid-js";
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
} from "../components/ui/dialog";
import { DeferredView } from "../components/workspace/deferred-view";
import type { ShortcutDefinition } from "../keyboard/shortcut-manager";

export interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  extraCommands?: ShortcutDefinition[];
}
const Content = lazy(() =>
  import("./CommandPaletteContent").then((module) => ({
    default: module.CommandPaletteContent,
  })),
);

// The modal boundary is immediate, even while the optional command module downloads.
export const CommandPalette: Component<CommandPaletteProps> = (props) => (
  <Show when={props.open}>
    <Dialog
      open={props.open}
      onOpenChange={(open) => {
        if (!open) props.onClose();
      }}
    >
      <DialogContent class="w-[calc(100%-2rem)] overflow-hidden rounded-lg p-0">
        <DialogTitle class="sr-only">Command palette</DialogTitle>
        <DialogDescription class="sr-only">
          Search workspace views and actions. Use arrow keys to navigate and
          Enter to run a command.
        </DialogDescription>
        <DeferredView label="Command palette" onCancel={props.onClose}>
          <Content {...props} />
        </DeferredView>
      </DialogContent>
    </Dialog>
  </Show>
);
