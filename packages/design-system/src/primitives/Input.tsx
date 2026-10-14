import { Component, JSX, Show, splitProps } from "solid-js";
import { Kbd } from "./Kbd";

export interface InputProps extends JSX.InputHTMLAttributes<HTMLInputElement> {
  icon?: JSX.Element;
  shortcut?: string;
  inputRef?: (el: HTMLInputElement) => void;
}

export const Input: Component<InputProps> = (props) => {
  const [local, rest] = splitProps(props, [
    "icon",
    "shortcut",
    "inputRef",
    "class",
  ]);

  return (
    <div class={`ds-input-wrapper ${local.class ?? ""}`}>
      <Show when={local.icon}>
        <span class="ds-input-icon">{local.icon}</span>
      </Show>
      <input ref={local.inputRef} class="ds-input" {...rest} />
      <Show when={local.shortcut}>
        {(sc) => <Kbd keys={sc()} />}
      </Show>
    </div>
  );
};
