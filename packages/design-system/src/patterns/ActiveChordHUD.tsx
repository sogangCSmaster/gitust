import { Component, Show } from "solid-js";
import { shortcutManager } from "../keyboard/shortcut-manager";
import { Kbd } from "../primitives/Kbd";

export const ActiveChordHUD: Component = () => {
  return (
    <Show when={shortcutManager.activeChord()}>
      {(prefix) => (
        <div class="ds-chord-floater" role="status" aria-live="polite">
          <span>Waiting for second key of chord:</span>
          <Kbd keys={prefix()} />
          <span style={{ color: "var(--ds-fg-muted)" }}>
            (try <Kbd keys="i" />, <Kbd keys="d" />, <Kbd keys="a" />,{" "}
            <Kbd keys="s" />, <Kbd keys="t" />, <Kbd keys="p" />)
          </span>
        </div>
      )}
    </Show>
  );
};
