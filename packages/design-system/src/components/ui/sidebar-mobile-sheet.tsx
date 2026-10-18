// SPDX-License-Identifier: MIT
import type { Component, ComponentProps, JSX } from "solid-js";
import { splitProps } from "solid-js";
import { Sheet, SheetContent, SheetDescription, SheetTitle } from "./sheet";
import { SIDEBAR_WIDTH_MOBILE } from "./sidebar-context";

export type SidebarMobileSheetProps = ComponentProps<"div"> & {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  side?: "left" | "right";
  children?: JSX.Element;
};

export const SidebarMobileSheet: Component<SidebarMobileSheetProps> = (
  props,
) => {
  const [local, others] = splitProps(props, [
    "open",
    "onOpenChange",
    "side",
    "children",
  ]);
  return (
    <Sheet open={local.open} onOpenChange={local.onOpenChange} {...others}>
      <SheetContent
        data-sidebar="sidebar"
        data-mobile="true"
        class="w-[var(--sidebar-width)] bg-sidebar p-0 text-sidebar-foreground [&>button]:hidden"
        style={{
          "--sidebar-width": SIDEBAR_WIDTH_MOBILE,
        }}
        position={local.side}
      >
        <SheetTitle class="sr-only">Workspace navigation</SheetTitle>
        <SheetDescription class="sr-only">
          Choose a workspace view.
        </SheetDescription>
        <div class="flex size-full flex-col">{local.children}</div>
      </SheetContent>
    </Sheet>
  );
};
