import { Component, For, JSX } from "solid-js";

export type IconName =
  | "git-pull-request"
  | "git-merge"
  | "git-branch"
  | "git-commit"
  | "issue-opened"
  | "issue-closed"
  | "issue-draft"
  | "repo"
  | "search"
  | "folder"
  | "file-code"
  | "comment"
  | "check"
  | "x"
  | "clock"
  | "zap"
  | "sun"
  | "moon"
  | "terminal"
  | "plus"
  | "chevron-right"
  | "shield-check"
  | "keyboard"
  | "diff"
  | "sliders"
  | "play"
  | "rotate-ccw"
  | "users"
  | "lock"
  | "key"
  | "server"
  | "bot"
  | "cpu"
  | "layers"
  | "pencil";

interface IconDef {
  /** SVG inner elements (path, circle, etc.) on a 16x16 viewBox */
  paths: string[];
}

/**
 * Crisp 16x16 vector paths tailored for high-density developer interfaces.
 * Rendered once into a global hidden <svg><defs> sprite and instantiated via <use href="#gitust-icon-..." />
 * so workspace icons reuse a shared set of SVG path definitions.
 */
export const ICON_DEFS: Record<IconName, IconDef> = {
  "git-pull-request": {
    paths: [
      "M4.5 4.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Z",
      "M4.5 14.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Z",
      "M11.5 14.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Z",
      "M4.5 4.5v7",
      "M11.5 11.5V6a2 2 0 0 0-2-2H7",
      "M9 2 7 4l2 2",
    ],
  },
  "git-merge": {
    paths: [
      "M4.5 4.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Z",
      "M4.5 14.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Z",
      "M11.5 9.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Z",
      "M4.5 4.5v7",
      "M4.5 4.5c0 2.5 2.5 3.5 5.5 3.5",
    ],
  },
  "git-branch": {
    paths: [
      "M4.5 4.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Z",
      "M4.5 14.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Z",
      "M11.5 6.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Z",
      "M4.5 4.5v7",
      "M11.5 6.5c0 2.5-2 3.5-7 3.5",
    ],
  },
  "git-commit": {
    paths: [
      "M8 10.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5Z",
      "M1.5 8h4",
      "M10.5 8h4",
    ],
  },
  "issue-opened": {
    paths: [
      "M8 14A6 6 0 1 0 8 2a6 6 0 0 0 0 12Z",
      "M8 7.5a.5.5 0 1 0 0 1 .5.5 0 0 0 0-1Z",
    ],
  },
  "issue-closed": {
    paths: ["M8 14A6 6 0 1 0 8 2a6 6 0 0 0 0 12Z", "m5.75 8 1.5 1.5 3-3"],
  },
  "issue-draft": {
    paths: [
      "M8 2a6 6 0 0 1 6 6",
      "M14 8a6 6 0 0 1-6 6",
      "M8 14a6 6 0 0 1-4.24-10.24",
      "M8 7.5a.5.5 0 1 0 0 1 .5.5 0 0 0 0-1Z",
    ],
  },
  repo: {
    paths: [
      "M3 2.5A1.5 1.5 0 0 1 4.5 1h8A.5.5 0 0 1 13 1.5v12a.5.5 0 0 1-.5.5h-8A1.5 1.5 0 0 1 3 12.5v-10Z",
      "M3 12.5A1.5 1.5 0 0 1 4.5 11H13",
      "M6 4.5h4",
    ],
  },
  search: {
    paths: [
      "M7.25 12.5a5.25 5.25 0 1 0 0-10.5 5.25 5.25 0 0 0 0 10.5Z",
      "M11 11l3 3",
    ],
  },
  folder: {
    paths: [
      "M2 4a1 1 0 0 1 1-1h3.3a1 1 0 0 1 .7.3l1.2 1.2a1 1 0 0 0 .7.3H13a1 1 0 0 1 1 1v6.2a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V4Z",
    ],
  },
  "file-code": {
    paths: [
      "M4 1.5h5.5L13 5v9a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V2.5a1 1 0 0 1 1-1Z",
      "M9 1.5V5h3.5",
      "m6.75 8.25-1.5 1.5 1.5 1.5",
      "m9.25 8.25 1.5 1.5-1.5 1.5",
    ],
  },
  comment: {
    paths: [
      "M2.5 3.5a1 1 0 0 1 1-1h9a1 1 0 0 1 1 1v6.5a1 1 0 0 1-1 1H6l-3.5 2.5V3.5Z",
    ],
  },
  check: {
    paths: ["M3 8.5 6.5 12 13 4.5"],
  },
  x: {
    paths: ["m4.5 4.5 7 7", "m11.5 4.5-7 7"],
  },
  clock: {
    paths: ["M8 14A6 6 0 1 0 8 2a6 6 0 0 0 0 12Z", "M8 5v3.25l2.25 1.25"],
  },
  zap: {
    paths: ["M9 1.5 3.5 9H8l-1 5.5L12.5 7H8l1-5.5Z"],
  },
  sun: {
    paths: [
      "M8 11a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z",
      "M8 1.5v1.5",
      "M8 13v1.5",
      "m3.4 3.4 1.06 1.06",
      "m11.54 11.54 1.06 1.06",
      "M1.5 8H3",
      "M13 8h1.5",
      "m3.4 12.6 1.06-1.06",
      "m11.54 4.46 1.06-1.06",
    ],
  },
  moon: {
    paths: ["M13.2 10.2A5.5 5.5 0 0 1 5.8 2.8a5.5 5.5 0 1 0 7.4 7.4Z"],
  },
  terminal: {
    paths: ["m3.5 5 3.5 3-3.5 3", "M8.5 11h4"],
  },
  plus: {
    paths: ["M8 3.25v9.5", "M3.25 8h9.5"],
  },
  pencil: {
    paths: ["m10.5 2.5 3 3-8 8-4 1 1-4 8-8Z", "m8.5 4.5 3 3"],
  },
  "chevron-right": {
    paths: ["m6 4 4 4-4 4"],
  },
  "shield-check": {
    paths: [
      "M8 1.75 13 3.75v4c0 3.25-2.15 5.65-5 6.5-2.85-.85-5-3.25-5-6.5v-4l5-2Z",
      "m6 8 1.5 1.5 3-3",
    ],
  },
  keyboard: {
    paths: [
      "M2 4.5A1.5 1.5 0 0 1 3.5 3h9A1.5 1.5 0 0 1 14 4.5v7a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5v-7Z",
      "M4.5 6.5h1",
      "M7.5 6.5h1",
      "M10.5 6.5h1",
      "M5 9.5h6",
    ],
  },
  diff: {
    paths: ["M8 3v5", "M5.5 5.5h5", "M5.5 12h5", "M2.5 2.5h11v11h-11v-11Z"],
  },
  sliders: {
    paths: [
      "M2.5 4.5h11",
      "M2.5 8h11",
      "M2.5 11.5h11",
      "M5.5 3v3",
      "M10.5 6.5v3",
      "M7 10v3",
    ],
  },
  play: {
    paths: ["M4.5 3.25 12.5 8l-8 4.75V3.25Z"],
  },
  "rotate-ccw": {
    paths: ["M2.5 4v3.5H6", "M3 7.5A5.5 5.5 0 1 1 4.2 12"],
  },
  users: {
    paths: [
      "M6 7.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5Z",
      "M1.75 13.5c0-2.1 1.9-3.75 4.25-3.75s4.25 1.65 4.25 3.75",
      "M11 3.2a2.2 2.2 0 0 1 0 4.1",
      "M14.25 13.5c0-1.65-1.15-3.05-2.75-3.5",
    ],
  },
  lock: {
    paths: [
      "M3.5 7.5h9a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1v-5a1 1 0 0 1 1-1Z",
      "M5.25 7.5V5.25a2.75 2.75 0 0 1 5.5 0V7.5",
    ],
  },
  key: {
    paths: [
      "M10.5 8.5a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z",
      "m8.2 7.8-5.7 5.7",
      "m4 12 1.5 1.5",
      "m6 10 1.5 1.5",
    ],
  },
  server: {
    paths: [
      "M2.5 3.5a1 1 0 0 1 1-1h9a1 1 0 0 1 1 1v3a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1v-3Z",
      "M2.5 9.5a1 1 0 0 1 1-1h9a1 1 0 0 1 1 1v3a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1v-3Z",
      "M5 5h.5",
      "M5 11h.5",
    ],
  },
  bot: {
    paths: [
      "M8 2v2",
      "M3.5 4h9a1 1 0 0 1 1 1v6.5a1.5 1.5 0 0 1-1.5 1.5h-8A1.5 1.5 0 0 1 2.5 11.5V5a1 1 0 0 1 1-1Z",
      "M6 8v1",
      "M10 8v1",
      "M1.25 8h1.25",
      "M13.5 8h1.25",
    ],
  },
  cpu: {
    paths: [
      "M4.5 4.5h7v7h-7v-7Z",
      "M6.5 6.5h3v3h-3v-3Z",
      "M6 2v2.5",
      "M10 2v2.5",
      "M6 11.5V14",
      "M10 11.5V14",
      "M2 6h2.5",
      "M2 10h2.5",
      "M11.5 6H14",
      "M11.5 10H14",
    ],
  },
  layers: {
    paths: [
      "M8 2 1.75 5.25 8 8.5l6.25-3.25L8 2Z",
      "m1.75 8.25 6.25 3.25 6.25-3.25",
      "m1.75 11.25 6.25 3.25 6.25-3.25",
    ],
  },
};

const ICON_ENTRIES = Object.entries(ICON_DEFS) as [IconName, IconDef][];

/**
 * Mounts once at the root of the app. Defines all <symbol id="gitust-icon-*"> nodes
 * so every <Icon /> in the app is a lightweight 2-node <svg><use /></svg> SVG instance.
 */
export const IconSpriteSheet: Component = () => {
  return (
    <svg
      aria-hidden="true"
      class="pointer-events-none absolute size-0 overflow-hidden"
    >
      <defs>
        <For each={ICON_ENTRIES}>
          {([name, def]) => (
            <symbol
              id={`gitust-icon-${name}`}
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="1.5"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <For each={def.paths}>{(d) => <path d={d} />}</For>
            </symbol>
          )}
        </For>
      </defs>
    </svg>
  );
};

export interface IconProps extends JSX.SvgSVGAttributes<SVGSVGElement> {
  name: IconName;
  size?: number;
}

export const Icon: Component<IconProps> = (props) => {
  const size = () => props.size ?? 14;

  return (
    <svg
      width={size()}
      height={size()}
      viewBox="0 0 16 16"
      aria-hidden="true"
      class={`inline-block shrink-0 align-middle ${props.class ?? ""}`}
      style={props.style}
    >
      <use href={`#gitust-icon-${props.name}`} />
    </svg>
  );
};
