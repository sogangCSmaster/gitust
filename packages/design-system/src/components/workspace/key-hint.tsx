import { For, type Component } from "solid-js";
import { formatShortcutKeys } from "../../keyboard/shortcut-manager";

export const KeyHint: Component<{ keys: string }> = (props) => (
  <span class="inline-flex items-center gap-1 text-xs text-muted-foreground">
    <For each={formatShortcutKeys(props.keys)}>
      {(key) => (
        <kbd class="inline-flex min-w-5 items-center justify-center rounded border bg-muted px-1 py-0.5 font-mono text-[10px]">
          {key}
        </kbd>
      )}
    </For>
  </span>
);
