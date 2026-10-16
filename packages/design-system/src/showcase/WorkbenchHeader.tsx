import { For, type Component } from "solid-js";
import { Button } from "../components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "../components/ui/dialog";
import { SidebarTrigger } from "../components/ui/sidebar";
import { Separator } from "../components/ui/separator";
import { Icon } from "../components/workspace/icon";
import { KeyHint } from "../components/workspace/presentation";
import { getTheme, toggleTheme } from "../tokens/theme";
import { shortcutManager } from "../keyboard/shortcut-manager";
export interface WorkbenchHeaderProps {
  onOpenCommandPalette: () => void;
  onOpenShortcutsModal: () => void;
}
export const WorkbenchHeader: Component<WorkbenchHeaderProps> = (props) => (
  <header class="flex h-14 shrink-0 items-center justify-between gap-3 border-b px-4 sm:px-6">
    <div class="flex items-center gap-3">
      <SidebarTrigger class="size-9" />
      <Separator orientation="vertical" class="h-5" />
      <span class="hidden text-xs text-muted-foreground sm:inline">
        Workspace
      </span>
      <span class="text-sm font-medium">Gitust</span>
    </div>
    <div class="flex items-center gap-1">
      <Button
        variant="ghost"
        class="hidden gap-4 text-muted-foreground sm:inline-flex"
        onClick={props.onOpenCommandPalette}
      >
        <Icon name="search" />
        Commands
        <KeyHint keys="mod+k" />
      </Button>
      <Button
        size="icon"
        variant="ghost"
        class="sm:hidden"
        aria-label="Search commands"
        onClick={props.onOpenCommandPalette}
      >
        <Icon name="search" />
      </Button>
      <Button
        size="icon"
        variant="ghost"
        aria-label="Keyboard shortcuts"
        onClick={props.onOpenShortcutsModal}
      >
        <Icon name="keyboard" />
      </Button>
      <Button
        size="icon"
        variant="ghost"
        aria-label="Toggle theme"
        onClick={toggleTheme}
      >
        <Icon name={getTheme() === "dark" ? "sun" : "moon"} />
      </Button>
    </div>
  </header>
);
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
        <For each={shortcutManager.shortcuts()}>
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
