// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@solidjs/testing-library";
import {
  SidebarMenuButton,
  SidebarProvider,
} from "../src/components/ui/sidebar";
import { SidebarMenuButtonBase } from "../src/components/ui/sidebar-menu-button";

beforeEach(() => {
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: vi.fn(() => ({
      matches: false,
      addEventListener() {},
      removeEventListener() {},
    })),
  });
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("keeps a navigation link's identity, attributes and events on the shared renderer", async () => {
  const click = vi.fn(),
    key = vi.fn();
  let reference: HTMLAnchorElement | undefined;
  render(() => (
    <SidebarMenuButtonBase
      as="a"
      href="#repository"
      ref={reference}
      aria-current="page"
      isActive
      class="h-10"
      onClick={click}
      onKeyDown={key}
    >
      Repository
    </SidebarMenuButtonBase>
  ));
  const link = screen.getByRole("link", { name: "Repository" });
  expect(reference).toBe(link);
  expect(link.getAttribute("href")).toBe("#repository");
  expect(link.getAttribute("aria-current")).toBe("page");
  expect(link.getAttribute("data-active")).toBe("true");
  link.focus();
  await fireEvent.keyDown(link, { key: "Enter" });
  await fireEvent.click(link);
  expect(document.activeElement).toBe(link);
  expect(key).toHaveBeenCalledTimes(1);
  expect(click).toHaveBeenCalledTimes(1);
});

it("retains the existing optional tooltip and underlying action without remounting its button", async () => {
  const action = vi.fn();
  let reference: HTMLButtonElement | undefined;
  render(() => (
    <SidebarProvider defaultOpen={false}>
      <SidebarMenuButton
        tooltip="Repository navigation"
        ref={reference}
        data-testid="menu-action"
        onClick={action}
      >
        Open repository
      </SidebarMenuButton>
    </SidebarProvider>
  ));
  const button = screen.getByTestId("menu-action");
  expect(reference).toBe(button);
  expect(
    screen.getAllByRole("button", { name: "Open repository" }),
  ).toHaveLength(1);
  button.focus();
  const tooltip = await screen.findByRole("tooltip");
  expect(tooltip.textContent).toBe("Repository navigation");
  expect(button.getAttribute("aria-describedby")?.split(" ")).toContain(
    tooltip.id,
  );
  expect(screen.getByTestId("menu-action")).toBe(button);
  await fireEvent.click(button);
  expect(action).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(screen.queryByRole("tooltip")).toBeNull());
});

it("preserves an anchor renderer and its reference when a tooltip is enabled", async () => {
  const action = vi.fn();
  let reference: HTMLAnchorElement | undefined;
  render(() => (
    <SidebarProvider defaultOpen={false}>
      <SidebarMenuButton
        as="a"
        href="#files"
        tooltip="Repository files"
        ref={reference}
        onClick={action}
      >
        Files
      </SidebarMenuButton>
    </SidebarProvider>
  ));
  const link = screen.getByRole("link", { name: "Files" });
  expect(reference).toBe(link);
  expect(link.getAttribute("href")).toBe("#files");
  expect(screen.queryByRole("button")).toBeNull();
  link.focus();
  const tooltip = await screen.findByRole("tooltip");
  expect(link.getAttribute("aria-describedby")?.split(" ")).toContain(
    tooltip.id,
  );
  await fireEvent.click(link);
  expect(action).toHaveBeenCalledTimes(1);
});
