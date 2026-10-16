import { createMemo, For, type Component } from "solid-js";
import {
  CommandDialog,
  CommandInput,
  CommandList,
  CommandEmpty,
  CommandGroup,
  CommandItem,
  CommandShortcut,
} from "../components/ui/command";
import { DialogTitle, DialogDescription } from "../components/ui/dialog";
import { KeyHint } from "../components/workspace/presentation";
import {
  shortcutManager,
  type ShortcutDefinition,
} from "../keyboard/shortcut-manager";
export interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  extraCommands?: ShortcutDefinition[];
}
export const CommandPalette: Component<CommandPaletteProps> = (props) => {
  const commands = createMemo(() => {
    const registered = shortcutManager.shortcuts();
    const ids = new Set(registered.map((command) => command.id));
    return [
      ...registered,
      ...(props.extraCommands ?? []).filter((command) => !ids.has(command.id)),
    ];
  });
  const categories = createMemo(() => [
    ...new Set(commands().map((command) => command.category)),
  ]);
  const execute = (command: ShortcutDefinition) => {
    props.onClose();
    if (!shortcutManager.executeById(command.id))
      command.handler(new KeyboardEvent("keydown", { key: command.keys }));
  };
  return (
    <CommandDialog
      open={props.open}
      onOpenChange={(open) => {
        if (!open) props.onClose();
      }}
    >
      <DialogTitle class="sr-only">Command palette</DialogTitle>
      <DialogDescription class="sr-only">
        Search workspace views and actions. Use arrow keys to navigate and Enter
        to run a command.
      </DialogDescription>
      <CommandInput
        placeholder="Search commands..."
        aria-label="Search commands"
      />
      <CommandList>
        <CommandEmpty>No matching commands.</CommandEmpty>
        <For each={categories()}>
          {(category) => (
            <CommandGroup heading={category}>
              <For
                each={commands().filter(
                  (command) => command.category === category,
                )}
              >
                {(command) => (
                  <CommandItem
                    value={command.id}
                    keywords={[
                      command.title,
                      command.category,
                      command.description ?? "",
                      command.keys,
                    ]}
                    onSelect={() => execute(command)}
                  >
                    <span class="min-w-0 flex-1 truncate">{command.title}</span>
                    <CommandShortcut>
                      <KeyHint keys={command.keys} />
                    </CommandShortcut>
                  </CommandItem>
                )}
              </For>
            </CommandGroup>
          )}
        </For>
      </CommandList>
    </CommandDialog>
  );
};
