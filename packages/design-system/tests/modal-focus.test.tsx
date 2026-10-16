// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@solidjs/testing-library";
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
} from "../src/components/ui/dialog";
import {
  Sheet,
  SheetContent,
  SheetTitle,
  SheetDescription,
} from "../src/components/ui/sheet";

beforeEach(() => vi.spyOn(window, "scrollTo").mockImplementation(() => {}));
afterEach(async () => {
  cleanup();
  // Kobalte queues focus/scroll cleanup after unmount.
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
  vi.restoreAllMocks();
});

it.each(["dialog", "sheet"])(
  "restores the latest opener of an initially closed controlled %s",
  async (kind) => {
    const [open, setOpen] = createSignal(false);
    render(() => (
      <>
        <button onClick={() => setOpen(true)}>First opener</button>
        <button onClick={() => setOpen(true)}>Second opener</button>
        {kind === "dialog" ? (
          <Dialog open={open()} onOpenChange={setOpen}>
            <DialogContent>
              <DialogTitle>Controlled modal</DialogTitle>
              <DialogDescription>Test dialog</DialogDescription>
              <button>Inside</button>
            </DialogContent>
          </Dialog>
        ) : (
          <Sheet open={open()} onOpenChange={setOpen}>
            <SheetContent>
              <SheetTitle>Controlled modal</SheetTitle>
              <SheetDescription>Test sheet</SheetDescription>
              <button>Inside</button>
            </SheetContent>
          </Sheet>
        )}
      </>
    ));
    for (const name of ["First opener", "Second opener"]) {
      const opener = screen.getByRole("button", { name });
      opener.focus();
      await fireEvent.click(opener);
      await fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
      await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
      await waitFor(() => expect(document.activeElement).toBe(opener));
    }
  },
);
