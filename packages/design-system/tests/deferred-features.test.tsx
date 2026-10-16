// @vitest-environment jsdom
import { createSignal, lazy } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@solidjs/testing-library";
import { CommandPalette } from "../src/patterns/CommandPalette";
import { DeferredView } from "../src/components/workspace/deferred-view";
import { shortcutManager } from "../src/keyboard/shortcut-manager";

beforeEach(() => {
  // jsdom has no layout observer; these tests exercise keyboard/module lifecycle only.
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("opens the modal immediately, blocks background shortcuts, then focuses command search", async () => {
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  const backgroundAction = vi.fn();
  const unregister = shortcutManager.register({
    id: "deferred.test",
    title: "Deferred test action",
    category: "Actions",
    keys: "s",
    handler: backgroundAction,
  });
  const [open, setOpen] = createSignal(false);
  try {
    render(() => (
      <>
        <button onClick={() => setOpen(true)}>Open commands</button>
        <CommandPalette open={open()} onClose={() => setOpen(false)} />
      </>
    ));
    expect(screen.queryByRole("dialog")).toBeNull();
    // A direct click keeps this assertion before the module download microtask completes.
    screen.getByRole("button", { name: "Open commands" }).click();
    const dialog = screen.getByRole("dialog", { name: "Command palette" });
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(screen.getByRole("status").textContent).toContain(
      "Opening Command palette",
    );
    dialog.dispatchEvent(
      new KeyboardEvent("keydown", { key: "s", bubbles: true }),
    );
    expect(backgroundAction).not.toHaveBeenCalled();
    const search = await screen.findByRole("combobox", {
      name: "Search commands",
    });
    await waitFor(() => expect(document.activeElement).toBe(search));
    await fireEvent.input(search, {
      target: { value: "Deferred test action" },
    });
    await fireEvent.click(
      await screen.findByRole("option", { name: /Deferred test action/ }),
    );
    expect(backgroundAction).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog")).toBeNull();
  } finally {
    unregister();
  }
});

it("keeps a rejected feature download recoverable without removing the surrounding workspace", async () => {
  const loggedError = vi.spyOn(console, "error").mockImplementation(() => {});
  let rejectDownload!: (error: Error) => void;
  const module = new Promise<{ default: () => never }>((_, reject) => {
    rejectDownload = reject;
  });
  const Feature = lazy(() => module);
  const close = vi.fn();
  render(() => (
    <>
      <p>Workspace remains available</p>
      <DeferredView label="Account tools" onCancel={close}>
        <Feature />
      </DeferredView>
    </>
  ));
  expect(screen.getByRole("status").textContent).toContain(
    "Opening Account tools",
  );
  rejectDownload(new Error("missing deployment chunk"));
  expect((await screen.findByRole("alert")).textContent).toContain(
    "Could not load Account tools",
  );
  expect(loggedError).toHaveBeenCalled();
  expect(screen.getByText("Workspace remains available")).toBeTruthy();
  expect(
    screen.getByRole("button", { name: "Reload application" }),
  ).toBeTruthy();
  await fireEvent.click(
    screen.getByRole("button", { name: "Close", exact: true }),
  );
  expect(close).toHaveBeenCalledTimes(1);
});
