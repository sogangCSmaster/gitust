// SPDX-License-Identifier: MIT
// Shared menu-button rendering from solid-ui; see LICENSE.solid-ui and PROVENANCE.md.
import type { ComponentProps, ValidComponent } from "solid-js";
import { mergeProps, splitProps } from "solid-js";
import type { PolymorphicProps } from "@kobalte/core";
import { Polymorphic } from "@kobalte/core";
import type { VariantProps } from "class-variance-authority";
import { cva } from "class-variance-authority";
import { cn } from "../../lib/utils";

const sidebarMenuButtonVariants = cva(
  "peer/menu-button flex w-full items-center gap-2 overflow-hidden rounded-md p-2 text-left text-sm outline-none ring-sidebar-ring transition-[width,height,padding] hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2 active:bg-sidebar-accent active:text-sidebar-accent-foreground disabled:pointer-events-none disabled:opacity-50 group-has-[[data-sidebar=menu-action]]/menu-item:pr-8 aria-disabled:pointer-events-none aria-disabled:opacity-50 data-[active=true]:bg-sidebar-accent data-[active=true]:font-medium data-[active=true]:text-sidebar-accent-foreground data-[state=open]:hover:bg-sidebar-accent data-[state=open]:hover:text-sidebar-accent-foreground group-data-[collapsible=icon]:!size-8 group-data-[collapsible=icon]:!p-2 [&>span:last-child]:truncate [&>svg]:size-4 [&>svg]:shrink-0",
  {
    variants: {
      variant: {
        default: "hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
        outline:
          "bg-background shadow-[0_0_0_1px_hsl(var(--sidebar-border))] hover:bg-sidebar-accent hover:text-sidebar-accent-foreground hover:shadow-[0_0_0_1px_hsl(var(--sidebar-accent))]",
      },
      size: {
        default: "h-8 text-sm",
        sm: "h-7 text-xs",
        lg: "h-12 text-sm group-data-[collapsible=icon]:!p-0",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

type SidebarMenuButtonBaseProps<T extends ValidComponent = "button"> =
  ComponentProps<T> &
    VariantProps<typeof sidebarMenuButtonVariants> & {
      isActive?: boolean;
    };

const SidebarMenuButtonBase = <T extends ValidComponent = "button">(
  rawProps: PolymorphicProps<T, SidebarMenuButtonBaseProps<T>>,
) => {
  const props = mergeProps(
    { isActive: false, variant: "default", size: "default" },
    rawProps,
  );
  const [local, others] = splitProps(props as SidebarMenuButtonBaseProps, [
    "isActive",
    "variant",
    "size",
    "class",
  ]);

  return (
    <Polymorphic<SidebarMenuButtonBaseProps>
      as="button"
      data-sidebar="menu-button"
      data-size={local.size}
      data-active={local.isActive}
      class={cn(
        sidebarMenuButtonVariants({ variant: local.variant, size: local.size }),
        local.class,
      )}
      {...others}
    />
  );
};

export { SidebarMenuButtonBase };
export type { SidebarMenuButtonBaseProps };
