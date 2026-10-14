import { Component, For, JSX, Show } from "solid-js";
import { Kbd } from "./Kbd";

export interface AvatarProps {
  name: string;
  size?: "xs" | "sm" | "md";
}

export const Avatar: Component<AvatarProps> = (props) => {
  const initials = () =>
    props.name
      .split(/[\s-_]+/)
      .slice(0, 2)
      .map((part) => part.charAt(0).toUpperCase())
      .join("");

  return (
    <span
      class={`ds-avatar ds-avatar--${props.size ?? "sm"}`}
      title={props.name}
    >
      {initials()}
    </span>
  );
};

export interface TabItem<T extends string = string> {
  id: T;
  label: string;
  count?: number;
  shortcut?: string;
  icon?: JSX.Element;
}

export interface TabsProps<T extends string = string> {
  items: TabItem<T>[];
  activeId: T;
  onChange: (id: T) => void;
}

export function Tabs<T extends string = string>(props: TabsProps<T>): JSX.Element {
  return (
    <div class="ds-tabs-list" role="tablist">
      <For each={props.items}>
        {(item) => (
          <button
            type="button"
            role="tab"
            aria-selected={props.activeId === item.id}
            data-active={props.activeId === item.id}
            class="ds-tab-trigger"
            onClick={() => props.onChange(item.id)}
          >
            <Show when={item.icon}>{item.icon}</Show>
            <span>{item.label}</span>
            <Show when={item.count !== undefined}>
              <span class="ds-tab-count">{item.count}</span>
            </Show>
            <Show when={item.shortcut}>
              {(sc) => <Kbd keys={sc()} />}
            </Show>
          </button>
        )}
      </For>
    </div>
  );
}
