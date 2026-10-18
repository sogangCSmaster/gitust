import { For, type Component } from "solid-js";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "../components/ui/dialog";
import { KeyHint } from "../components/workspace/presentation";
import { shortcutManager } from "../keyboard/shortcut-manager";

export const ShortcutsReferenceModal: Component<{
  open: boolean;
  onClose: () => void;
}> = (props) => (
  <Dialog
    open={props.open}
    onOpenChange={(open) => {
      if (!open) props.onClose();
    }}
  >
    <DialogContent class="max-h-[85svh] w-[calc(100%-2rem)] rounded-lg">
      <DialogHeader>
        <DialogTitle>Keyboard shortcuts</DialogTitle>
        <DialogDescription>
          Move through your workspace without leaving the keyboard.
        </DialogDescription>
      </DialogHeader>
      <div class="divide-y">
        <For
          each={shortcutManager
            .shortcuts()
            .filter((command) => command.enabled?.() !== false)}
        >
          {(shortcut) => (
            <div class="flex items-center justify-between gap-4 py-3">
              <div class="min-w-0">
                <p class="text-sm font-medium">{shortcut.title}</p>
                <p class="mt-1 text-xs text-muted-foreground">
                  {shortcut.description}
                </p>
              </div>
              <KeyHint keys={shortcut.keys} />
            </div>
          )}
        </For>
      </div>
    </DialogContent>
  </Dialog>
);
