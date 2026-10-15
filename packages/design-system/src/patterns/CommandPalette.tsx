import {
  Component,
  createEffect,
  createMemo,
  createSelector,
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
} from "solid-js";
import {
  ShortcutDefinition,
  shortcutManager,
} from "../keyboard/shortcut-manager";
import { Icon } from "../primitives/Icon";
import { Kbd } from "../primitives/Kbd";

export interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  extraCommands?: ShortcutDefinition[];
}

export const CommandPalette: Component<CommandPaletteProps> = (props) => {
  let dialogEl: HTMLDialogElement | undefined;
  let inputEl: HTMLInputElement | undefined;

  const [query, setQuery] = createSignal("");
  const [selectedIndex, setSelectedIndex] = createSignal(0);
  const isRowSelected = createSelector(selectedIndex);

  const allCommands = createMemo(() => {
    const registered = shortcutManager.shortcuts();
    const extra = props.extraCommands ?? [];
    const seen = new Set(registered.map((c) => c.id));
    return [...registered, ...extra.filter((c) => !seen.has(c.id))];
  });

  const filteredWithTiming = createMemo(() => {
    const t0 = performance.now();
    const q = query().trim().toLowerCase();
    const list = allCommands();

    const items = !q
      ? list
      : list.filter(
          (cmd) =>
            cmd.title.toLowerCase().includes(q) ||
            cmd.category.toLowerCase().includes(q) ||
            (cmd.description && cmd.description.toLowerCase().includes(q)) ||
            cmd.keys.toLowerCase().includes(q)
        );

    const latencyMs = Number((performance.now() - t0).toFixed(3));
    return { items, latencyMs };
  });

  const filteredCommands = () => filteredWithTiming().items;
  const filterLatencyMs = () => filteredWithTiming().latencyMs;

  createEffect(() => {
    // Reset selection index whenever filtered list changes
    filteredCommands();
    setSelectedIndex(0);
  });

  createEffect(() => {
    selectedIndex();
    if (!props.open) return;
    queueMicrotask(() => {
      dialogEl
        ?.querySelector<HTMLElement>('.ds-cmd-item[data-active="true"]')
        ?.scrollIntoView({ block: "nearest" });
    });
  });

  createEffect(() => {
    if (!dialogEl) return;
    if (props.open && !dialogEl.open) {
      setQuery("");
      setSelectedIndex(0);
      dialogEl.showModal();
      queueMicrotask(() => inputEl?.focus());
    } else if (!props.open && dialogEl.open) {
      dialogEl.close();
    }
  });

  onMount(() => {
    if (!dialogEl) return;
    const handleClose = () => {
      if (props.open) props.onClose();
    };
    dialogEl.addEventListener("close", handleClose);

    // Fallback for browsers without <dialog closedby="any">
    const handleBackdropClick = (event: MouseEvent) => {
      if (!dialogEl || "closedBy" in HTMLDialogElement.prototype) return;
      if (event.target !== dialogEl) return;
      const rect = dialogEl.getBoundingClientRect();
      const inside =
        rect.top <= event.clientY &&
        event.clientY <= rect.top + rect.height &&
        rect.left <= event.clientX &&
        event.clientX <= rect.left + rect.width;
      if (!inside) dialogEl.close();
    };

    dialogEl.addEventListener("click", handleBackdropClick);
    onCleanup(() => {
      dialogEl?.removeEventListener("close", handleClose);
      dialogEl?.removeEventListener("click", handleBackdropClick);
    });
  });

  const executeSelected = (cmd: ShortcutDefinition | undefined) => {
    if (!cmd) return;
    props.onClose();
    if (!shortcutManager.executeById(cmd.id)) {
      cmd.handler(new KeyboardEvent("keydown", { key: cmd.keys }));
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    const items = filteredCommands();
    if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) {
      e.preventDefault();
      if (items.length > 0) {
        setSelectedIndex((i) => (i + 1) % items.length);
      }
    } else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p")) {
      e.preventDefault();
      if (items.length > 0) {
        setSelectedIndex((i) => (i - 1 + items.length) % items.length);
      }
    } else if (e.key === "Enter") {
      e.preventDefault();
      executeSelected(items[selectedIndex()]);
    }
  };

  return (
    <dialog
      ref={dialogEl}
      closedby="any"
      aria-label="Command Palette"
      class="ds-cmd-dialog"
      onKeyDown={handleKeyDown}
    >
      <div class="ds-cmd-header">
        <span
          style={{
            color: "var(--ds-accent-primary)",
            display: "inline-flex",
          }}
        >
          <Icon name="terminal" size={16} />
        </span>
        <input
          ref={inputEl}
          type="text"
          class="ds-cmd-input"
          placeholder="Type a command, jump to repo, issue, or press shortcut..."
          value={query()}
          onInput={(e) => setQuery(e.currentTarget.value)}
        />
        <Kbd keys="esc" />
      </div>

      <div class="ds-cmd-list" role="listbox">
        <Show
          when={filteredCommands().length > 0}
          fallback={
            <div
              style={{
                padding: "var(--ds-space-6)",
                "text-align": "center",
                color: "var(--ds-fg-muted)",
              }}
            >
              No matching commands found.
            </div>
          }
        >
          <For each={filteredCommands()}>
            {(cmd, idx) => (
              <div
                role="option"
                aria-selected={isRowSelected(idx())}
                data-active={isRowSelected(idx())}
                class="ds-cmd-item"
                onMouseMove={() => setSelectedIndex(idx())}
                onClick={() => executeSelected(cmd)}
              >
                <div class="ds-cmd-item-left">
                  <span
                    class="ds-mono"
                    style={{
                      "font-size": "var(--ds-font-size-2xs)",
                      color: "var(--ds-fg-muted)",
                      width: "92px",
                      "flex-shrink": 0,
                    }}
                  >
                    {cmd.category}
                  </span>
                  <span class="ds-cmd-item-title">{cmd.title}</span>
                  <Show when={cmd.description}>
                    <span class="ds-cmd-item-desc">— {cmd.description}</span>
                  </Show>
                </div>
                <Kbd keys={cmd.keys} />
              </div>
            )}
          </For>
        </Show>
      </div>

      <div class="ds-cmd-footer">
        <div style={{ display: "flex", gap: "var(--ds-space-3)" }}>
          <span>
            <Kbd keys="↑" /> <Kbd keys="↓" /> to navigate
          </span>
          <span>
            <Kbd keys="enter" /> to execute
          </span>
        </div>
        <span class="ds-mono">
          {filteredCommands().length} commands · filtered in {filterLatencyMs()}
          ms
        </span>
      </div>
    </dialog>
  );
};
