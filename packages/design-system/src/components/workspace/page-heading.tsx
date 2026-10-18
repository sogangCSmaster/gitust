import type { Component, JSX } from "solid-js";

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
