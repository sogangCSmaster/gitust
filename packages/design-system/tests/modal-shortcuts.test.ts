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

it("rechecks dynamic workspace capability for keyboard and command-palette dispatch", () => {
  let allowed = true;
  const handler = vi.fn();
  unregister.push(
    shortcutManager.register({
      id: "test.capability",
      title: "Primary action",
      category: "Actions",
      keys: "a",
      enabled: () => allowed,
      handler,
    }),
  );
  allowed = false;
  document.body.dispatchEvent(
    new KeyboardEvent("keydown", { key: "a", bubbles: true }),
  );
  expect(shortcutManager.executeById("test.capability")).toBe(false);
  expect(handler).not.toHaveBeenCalled();
  allowed = true;
  document.body.dispatchEvent(
    new KeyboardEvent("keydown", { key: "a", bubbles: true }),
  );
  expect(shortcutManager.executeById("test.capability")).toBe(true);
  expect(handler).toHaveBeenCalledTimes(2);
});

it("maintains < 16ms keyboard input-to-DOM-feedback latency across 100 dispatches", () => {
  const statusEl = document.createElement("div");
  document.body.append(statusEl);
  let count = 0;
  unregister.push(
    shortcutManager.register({
      id: "perf.next_item",
      title: "Select Next Item",
      category: "Navigation",
      keys: "j",
      allowRepeat: true,
      handler: () => {
        count += 1;
        statusEl.textContent = `row-${count}`;
      },
    }),
  );

  const samplesMs: number[] = [];
  for (let i = 0; i < 100; i += 1) {
    const t0 = performance.now();
    document.body.dispatchEvent(
      new KeyboardEvent("keydown", { key: "j", bubbles: true }),
    );
    const elapsed = performance.now() - t0;
    samplesMs.push(elapsed);
  }
  expect(count).toBe(100);
  expect(statusEl.textContent).toBe("row-100");
  samplesMs.sort((a, b) => a - b);
  const p99Ms = samplesMs[98] ?? 0;
  const telemetry = shortcutManager.lastTelemetry();
  expect(telemetry).not.toBeNull();
  expect(telemetry!.dispatchLatencyMs).toBeLessThan(16);
  expect(p99Ms).toBeLessThan(16);
});
