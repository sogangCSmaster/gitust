import { createMemo, For, onMount, type Component } from "solid-js";
import {
  Command,
  CommandInput,
  CommandList,
  CommandEmpty,
  CommandGroup,
  CommandItem,
  CommandShortcut,
} from "../components/ui/command";
import { KeyHint } from "../components/workspace/presentation";
import {
  shortcutManager,
  type ShortcutDefinition,
} from "../keyboard/shortcut-manager";
import type { CommandPaletteProps } from "./CommandPalette";
export const CommandPaletteContent: Component<CommandPaletteProps> = (
  props,
) => {
  let searchInput: HTMLInputElement | undefined;
  onMount(() => searchInput?.focus());
  const commands = createMemo(() => {
    const registered = shortcutManager
      .shortcuts()
      .filter((command) => command.enabled?.() !== false);
    const ids = new Set(registered.map((command) => command.id));
    return [
      ...registered,
      ...(props.extraCommands ?? []).filter(
        (command) => !ids.has(command.id) && command.enabled?.() !== false,
      ),
    ];
  });
  const categories = createMemo(() => [
    ...new Set(commands().map((command) => command.category)),
  ]);
  const execute = (command: ShortcutDefinition) => {
    if (command.enabled?.() === false) return;
    props.onClose();
    if (
      !shortcutManager.executeById(command.id) &&
      command.enabled?.() !== false
    )
      command.handler(new KeyboardEvent("keydown", { key: command.keys }));
  };
  return (
    <Command
      label="Search commands"
      class="[&_[cmdk-input]]:h-12 [&_[cmdk-item]]:py-3"
    >
      <CommandInput
        ref={searchInput}
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
    </Command>
  );
};
