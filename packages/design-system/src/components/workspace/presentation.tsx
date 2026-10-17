import { For, Show, type Component, type JSX } from "solid-js";
import { Badge } from "../ui/badge";
import { Avatar, AvatarFallback } from "../ui/avatar";
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
} from "../ui/card";
import { TextField, TextFieldInput, TextFieldLabel } from "../ui/text-field";
import { formatShortcutKeys } from "../../keyboard/shortcut-manager";
import { cn } from "../../lib/utils";

export type StatusTone =
  "neutral" | "accent" | "success" | "warning" | "danger" | "merged" | "info";
const statusVariant = {
  neutral: "secondary",
  accent: "default",
  success: "success",
  warning: "warning",
  danger: "error",
  merged: "outline",
  info: "outline",
} as const;
export const StatusBadge: Component<{
  tone?: StatusTone;
  children: JSX.Element;
  class?: string;
}> = (props) => (
  <Badge
    variant={statusVariant[props.tone ?? "neutral"]}
    class={cn(
      "gap-1.5 font-medium",
      props.tone === "merged" &&
        "border-violet-500/25 text-violet-500 dark:text-violet-300",
      props.tone === "info" &&
        "border-blue-500/25 text-blue-600 dark:text-blue-300",
      props.class,
    )}
  >
    {props.children}
  </Badge>
);
export const PersonAvatar: Component<{ name: string; class?: string }> = (
  props,
) => (
  <Avatar class={cn("size-7", props.class)} title={props.name}>
    <AvatarFallback class="text-xs">
      {props.name
        .split(/[\s-_]+/)
        .slice(0, 2)
        .map((part) => part.charAt(0).toUpperCase())
        .join("")}
    </AvatarFallback>
  </Avatar>
);
export const KeyHint: Component<{ keys: string }> = (props) => (
  <span class="inline-flex items-center gap-1 text-xs text-muted-foreground">
    <For each={formatShortcutKeys(props.keys)}>
      {(key) => (
        <kbd class="inline-flex min-w-5 items-center justify-center rounded border bg-muted px-1 py-0.5 font-mono text-[10px]">
          {key}
        </kbd>
      )}
    </For>
  </span>
);
export const Field: Component<{
  label: string;
  value?: string;
  onValueChange?: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
  type?: "text" | "password" | "email" | "search";
  class?: string;
  inputRef?: (element: HTMLInputElement) => void;
  hideLabel?: boolean;
  autocomplete?: string;
}> = (props) => (
  <TextField
    value={props.value}
    onChange={props.onValueChange}
    disabled={props.disabled}
    class={cn("min-w-0 gap-2", props.class)}
  >
    <TextFieldLabel class={props.hideLabel ? "sr-only" : "text-xs font-medium"}>
      {props.label}
    </TextFieldLabel>
    <TextFieldInput
      ref={props.inputRef}
      type={props.type ?? "text"}
      placeholder={props.placeholder}
      autocomplete={props.autocomplete}
    />
  </TextField>
);
export const Section: Component<{
  title: string;
  description?: string;
  children: JSX.Element;
  class?: string;
}> = (props) => (
  <Card class={cn("min-w-0", props.class)}>
    <CardHeader>
      <CardTitle class="text-sm">{props.title}</CardTitle>
      <Show when={props.description}>
        <CardDescription>{props.description}</CardDescription>
      </Show>
    </CardHeader>
    <CardContent class="space-y-3">{props.children}</CardContent>
  </Card>
);
export const PageHeading: Component<{
  title: string;
  description: string;
  action?: JSX.Element;
}> = (props) => (
  <div class="flex flex-wrap items-center justify-between gap-3">
    <div class="min-w-0 flex-1">
      <h1
        class="truncate text-lg font-semibold tracking-tight"
        title={props.title}
      >
        {props.title}
      </h1>
      <p
        class="mt-1 hidden max-w-2xl truncate text-xs text-muted-foreground lg:block"
        title={props.description}
      >
        {props.description}
      </p>
    </div>
    {props.action}
  </div>
);
export const EmptyState: Component<{ title: string; description: string }> = (
  props,
) => (
  <div class="p-10 text-center">
    <h2 class="font-medium">{props.title}</h2>
    <p class="mt-2 text-sm text-muted-foreground">{props.description}</p>
  </div>
);
