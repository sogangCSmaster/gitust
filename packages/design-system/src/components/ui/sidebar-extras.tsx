// SPDX-License-Identifier: MIT
import type { Component, ComponentProps, ValidComponent } from "solid-js";
import { Show, splitProps } from "solid-js";
import type { PolymorphicProps } from "@kobalte/core/polymorphic";
import { TextField, TextFieldInput } from "./text-field";
import { Tooltip, TooltipContent, TooltipTrigger } from "./tooltip";
import {
  SidebarMenuButtonBase,
  type SidebarMenuButtonBaseProps,
} from "./sidebar-menu-button";
import { useSidebar } from "./sidebar-context";
import { cn } from "../../lib/utils";

type SidebarMenuButtonProps<T extends ValidComponent = "button"> =
  SidebarMenuButtonBaseProps<T> & { tooltip?: string };

const SidebarMenuButton = <T extends ValidComponent = "button">(
  props: PolymorphicProps<T, SidebarMenuButtonProps<T>>,
) => {
  const [local, others] = splitProps(
    props as SidebarMenuButtonProps & { as?: ValidComponent },
    ["tooltip", "as", "type"],
  );
  const { isMobile, state } = useSidebar();
  const renderButton: Component<ComponentProps<"button">> = (triggerProps) => (
    <SidebarMenuButtonBase
      {...triggerProps}
      as={local.as ?? "button"}
      type={local.type ?? triggerProps.type}
    />
  );
  return (
    <Show
      when={local.tooltip}
      fallback={
        <SidebarMenuButtonBase
          {...others}
          as={local.as ?? "button"}
          type={local.type}
        />
      }
    >
      <Tooltip placement="right">
        <TooltipTrigger as={renderButton} {...others} />
        <TooltipContent hidden={state() !== "collapsed" || isMobile()}>
          {local.tooltip}
        </TooltipContent>
      </Tooltip>
    </Show>
  );
};

const SidebarInput: Component<ComponentProps<typeof TextFieldInput>> = (
  props,
) => {
  const [local, others] = splitProps(props, ["class"]);
  return (
    <TextField>
      <TextFieldInput
        data-sidebar="input"
        class={cn(
          "h-8 w-full bg-background shadow-none focus-visible:ring-2 focus-visible:ring-sidebar-ring",
          local.class,
        )}
        {...others}
      />
    </TextField>
  );
};

export { SidebarMenuButton, SidebarInput };
export type { SidebarMenuButtonProps };
