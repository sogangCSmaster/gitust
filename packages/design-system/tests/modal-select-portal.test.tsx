// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "../src/components/ui/select";
const settleVisibility = () =>
  new Promise<void>((resolve) =>
    requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
  );
beforeEach(() => {
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
});
afterEach(async () => {
  cleanup();
  await settleVisibility();
  vi.restoreAllMocks();
});
it.each(["dialog", "sheet"])(
  "keeps Select accessible inside a %s after visibility settles and restores its modal on close",
  async (kind) => {
    const Modal = kind === "dialog" ? Dialog : Sheet;
    const Content = kind === "dialog" ? DialogContent : SheetContent;
    const Title = kind === "dialog" ? DialogTitle : SheetTitle;
    const Description =
      kind === "dialog" ? DialogDescription : SheetDescription;
    const [open, setOpen] = createSignal(true);
    const [value, setValue] = createSignal("Codex");
    render(() => (
      <>
        <button>Background action</button>
        <Modal open={open()} onOpenChange={setOpen}>
          <Content>
            <Title>Create worktree</Title>
            <Description>Choose an agent</Description>
            <Select
              options={["Codex", "Cursor", "Claude"]}
              value={value()}
              onChange={(item) => {
                if (item) setValue(item);
              }}
              itemComponent={(item) => (
                <SelectItem item={item.item}>{item.item.rawValue}</SelectItem>
              )}
            >
              <SelectLabel>Agent</SelectLabel>
              <SelectTrigger>
                <SelectValue<string>>
                  {(state) => state.selectedOption()}
                </SelectValue>
              </SelectTrigger>
              <SelectContent />
            </Select>
          </Content>
        </Modal>
      </>
    ));
    await settleVisibility();
    expect(
      screen.queryByRole("button", { name: "Background action" }),
    ).toBeNull();
    await fireEvent.keyDown(
      screen.getByRole("button", { name: "Agent Codex" }),
      {
        key: "ArrowDown",
      },
    );
    await settleVisibility();
    const options = screen.getAllByRole("option");
    expect(options).toHaveLength(3);
    expect(
      options.every((option) => !option.closest('[aria-hidden="true"]')),
    ).toBe(true);
    await fireEvent.click(screen.getByRole("option", { name: "Cursor" }));
    await settleVisibility();
    expect(
      screen.getByRole("dialog", { name: "Create worktree" }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Agent Cursor" })).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: "Background action" }),
    ).toBeNull();
    await fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    await settleVisibility();
    expect(
      screen.getByRole("button", { name: "Background action" }),
    ).toBeTruthy();
  },
);

it("uses the nearest modal owner for nested Select content", async () => {
  const [innerOpen, setInnerOpen] = createSignal(false);
  render(() => (
    <>
      <Select
        options={["Standalone"]}
        defaultValue="Standalone"
        itemComponent={(item) => (
          <SelectItem item={item.item}>{item.item.rawValue}</SelectItem>
        )}
      >
        <SelectLabel>Outside agent</SelectLabel>
        <SelectTrigger>
          <SelectValue<string>>{(state) => state.selectedOption()}</SelectValue>
        </SelectTrigger>
        <SelectContent />
      </Select>
      <Dialog open>
        <DialogContent data-testid="outer-modal">
          <DialogTitle>Outer modal</DialogTitle>
          <DialogDescription>Outer content</DialogDescription>
          <button onClick={() => setInnerOpen(true)}>Open inner</button>
          <Dialog open={innerOpen()} onOpenChange={setInnerOpen}>
            <DialogContent data-testid="inner-modal">
              <DialogTitle>Inner modal</DialogTitle>
              <DialogDescription>Inner content</DialogDescription>
              <Select
                options={["Nested"]}
                defaultValue="Nested"
                itemComponent={(item) => (
                  <SelectItem item={item.item}>{item.item.rawValue}</SelectItem>
                )}
              >
                <SelectLabel>Nested agent</SelectLabel>
                <SelectTrigger>
                  <SelectValue<string>>
                    {(state) => state.selectedOption()}
                  </SelectValue>
                </SelectTrigger>
                <SelectContent />
              </Select>
            </DialogContent>
          </Dialog>
        </DialogContent>
      </Dialog>
    </>
  ));
  await settleVisibility();
  await fireEvent.click(screen.getByRole("button", { name: "Open inner" }));
  await settleVisibility();
  await fireEvent.keyDown(
    screen.getByRole("button", { name: "Nested agent Nested" }),
    { key: "ArrowDown" },
  );
  await settleVisibility();
  const option = screen.getByRole("option", { name: "Nested" });
  expect(option.closest('[data-testid="inner-modal"]')).toBe(
    screen.getByTestId("inner-modal"),
  );
});

it("keeps standalone Select portals mounted under the document body", async () => {
  render(() => (
    <Select
      options={["Standalone"]}
      defaultValue="Standalone"
      itemComponent={(item) => (
        <SelectItem item={item.item}>{item.item.rawValue}</SelectItem>
      )}
    >
      <SelectLabel>Outside agent</SelectLabel>
      <SelectTrigger>
        <SelectValue<string>>{(state) => state.selectedOption()}</SelectValue>
      </SelectTrigger>
      <SelectContent />
    </Select>
  ));
  await fireEvent.keyDown(
    screen.getByRole("button", { name: "Outside agent Standalone" }),
    { key: "ArrowDown" },
  );
  await settleVisibility();
  const option = screen.getByRole("option", { name: "Standalone" });
  expect(option.closest('[role="dialog"]')).toBeNull();
  expect(document.body.contains(option)).toBe(true);
});
