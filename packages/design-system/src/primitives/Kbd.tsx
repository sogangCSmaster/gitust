import { Component, For, Show } from "solid-js";
import { formatShortcutKeys } from "../keyboard/shortcut-manager";

export interface KbdProps {
  keys: string;
  class?: string;
}

export const Kbd: Component<KbdProps> = (props) => {
  const isChord = () => props.keys.trim().includes(" ");
  const tokens = () => formatShortcutKeys(props.keys);

  return (
    <span class={`ds-kbd-group ${props.class ?? ""}`}>
      <For each={tokens()}>
        {(token, index) => (
          <>
            <Show when={isChord() && index() > 0}>
              <span class="ds-kbd-then">then</span>
            </Show>
            <kbd class="ds-kbd">{token}</kbd>
          </>
        )}
      </For>
    </span>
  );
};
