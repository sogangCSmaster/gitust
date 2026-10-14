import { Component, JSX, Show, splitProps } from "solid-js";
import { Kbd } from "./Kbd";

export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";
export type ButtonSize = "xs" | "sm" | "md" | "lg";

export interface ButtonProps extends JSX.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  shortcut?: string;
  icon?: JSX.Element;
}

export const Button: Component<ButtonProps> = (props) => {
  const [local, rest] = splitProps(props, [
    "variant",
    "size",
    "shortcut",
    "icon",
    "children",
    "class",
  ]);

  const variant = () => local.variant ?? "secondary";
  const size = () => local.size ?? "md";

  return (
    <button
      type="button"
      class={`ds-btn ds-btn--${variant()} ds-btn--${size()} ${local.class ?? ""}`}
      {...rest}
    >
      <Show when={local.icon}>
        <span class="ds-btn-icon">{local.icon}</span>
      </Show>
      <span>{local.children}</span>
      <Show when={local.shortcut}>
        {(sc) => <Kbd keys={sc()} />}
      </Show>
    </button>
  );
};
