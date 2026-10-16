// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { shortcutManager } from "../src/keyboard/shortcut-manager";
const unregister: Array<() => void> = [];
afterEach(() => {
  unregister.splice(0).forEach((dispose) => dispose());
  document.body.replaceChildren();
});
it("blocks destructive background shortcuts inside Kobalte modal dialogs", () => {
  const handler = vi.fn();
  unregister.push(
    shortcutManager.register({
      id: "test.approve",
      title: "Approve",
      category: "Actions",
      keys: "a",
      handler,
    }),
  );
  const dialog = document.createElement("div");
  dialog.setAttribute("role", "dialog");
  dialog.setAttribute("aria-modal", "true");
  document.body.append(dialog);
  dialog.dispatchEvent(
    new KeyboardEvent("keydown", { key: "a", bubbles: true }),
  );
  expect(handler).not.toHaveBeenCalled();
  dialog.remove();
  document.body.dispatchEvent(
    new KeyboardEvent("keydown", { key: "a", bubbles: true }),
  );
  expect(handler).toHaveBeenCalledOnce();
});
