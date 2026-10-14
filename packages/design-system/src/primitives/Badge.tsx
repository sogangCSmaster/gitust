import { Component, JSX, Show } from "solid-js";

export type BadgeVariant =
  | "neutral"
  | "accent"
  | "success"
  | "danger"
  | "warning"
  | "merged"
  | "info";

export interface BadgeProps {
  variant?: BadgeVariant;
  dot?: boolean;
  mono?: boolean;
  children: JSX.Element;
  class?: string;
}

export const Badge: Component<BadgeProps> = (props) => {
  const variant = () => props.variant ?? "neutral";

  return (
    <span
      class={`ds-badge ds-badge--${variant()} ${props.mono ? "ds-mono" : ""} ${props.class ?? ""}`}
    >
      <Show when={props.dot}>
        <span class="ds-badge--dot" aria-hidden="true" />
      </Show>
      {props.children}
    </span>
  );
};
