import { Component, For } from "solid-js";
import {
  Avatar,
  Badge,
  Button,
  ICON_DEFS,
  Icon,
  IconName,
  Input,
  Kbd,
} from "../index";

const ALL_ICON_NAMES = Object.keys(ICON_DEFS) as IconName[];

export const PrimitivesTab: Component = () => {
  return (
    <div
      style={{
        display: "grid",
        "grid-template-columns": "repeat(auto-fit, minmax(360px, 1fr))",
        gap: "var(--ds-space-4)",
      }}
    >
      {/* Card 1: GPU-Instanced SVG Sprite Icon System */}
      <section
        style={{
          padding: "var(--ds-space-4)",
          "background-color": "var(--ds-bg-primary)",
          border: "1px solid var(--ds-border-default)",
          "border-radius": "var(--ds-radius-md)",
          display: "flex",
          "flex-direction": "column",
          gap: "var(--ds-space-3)",
          "grid-column": "1 / -1",
        }}
      >
        <div
          style={{
            display: "flex",
            "align-items": "center",
            "justify-content": "space-between",
          }}
        >
          <div>
            <h2 style={{ "font-size": "var(--ds-font-size-md)" }}>
              Zero-Dependency SVG &lt;symbol&gt; Sprite Icon System
            </h2>
            <p
              style={{
                "font-size": "var(--ds-font-size-xs)",
                color: "var(--ds-fg-secondary)",
              }}
            >
              Parsed once in memory via &lt;defs&gt; and rendered via GPU
              instanced &lt;use href="#ds-icon-..." /&gt; with 0 external
              runtime dependencies.
            </p>
          </div>
          <Badge variant="success" mono>
            {ALL_ICON_NAMES.length} Icons · ~1.4 KB Gzip
          </Badge>
        </div>

        <div
          style={{
            display: "grid",
            "grid-template-columns": "repeat(auto-fill, minmax(140px, 1fr))",
            gap: "var(--ds-space-2)",
          }}
        >
          <For each={ALL_ICON_NAMES}>
            {(iconName) => (
              <div
                class="ds-mono"
                style={{
                  display: "flex",
                  "align-items": "center",
                  gap: "var(--ds-space-2)",
                  padding: "var(--ds-space-2) var(--ds-space-2-5)",
                  "background-color": "var(--ds-bg-secondary)",
                  border: "1px solid var(--ds-border-subtle)",
                  "border-radius": "var(--ds-radius-sm)",
                  "font-size": "var(--ds-font-size-2xs)",
                  color: "var(--ds-fg-secondary)",
                }}
              >
                <span
                  style={{
                    color: "var(--ds-fg-primary)",
                    display: "inline-flex",
                  }}
                >
                  <Icon name={iconName} size={15} />
                </span>
                <span
                  style={{
                    overflow: "hidden",
                    "text-overflow": "ellipsis",
                    "white-space": "nowrap",
                  }}
                >
                  {iconName}
                </span>
              </div>
            )}
          </For>
        </div>
      </section>

      {/* Card 2: Buttons */}
      <section
        style={{
          padding: "var(--ds-space-4)",
          "background-color": "var(--ds-bg-primary)",
          border: "1px solid var(--ds-border-default)",
          "border-radius": "var(--ds-radius-md)",
          display: "flex",
          "flex-direction": "column",
          gap: "var(--ds-space-3)",
        }}
      >
        <h2 style={{ "font-size": "var(--ds-font-size-md)" }}>
          Buttons &amp; Shortcut Pills
        </h2>
        <div
          style={{
            display: "flex",
            "flex-wrap": "wrap",
            gap: "var(--ds-space-2)",
            "align-items": "center",
          }}
        >
          <Button
            variant="primary"
            icon={<Icon name="git-merge" size={14} />}
            shortcut="mod+enter"
          >
            Merge Pull Request
          </Button>
          <Button
            variant="secondary"
            icon={<Icon name="git-branch" size={14} />}
            shortcut="c"
          >
            New Branch
          </Button>
          <Button
            variant="ghost"
            icon={<Icon name="issue-opened" size={14} />}
            shortcut="g i"
          >
            Issues
          </Button>
          <Button variant="danger" icon={<Icon name="x" size={14} />}>
            Delete Repo
          </Button>
        </div>
        <div
          style={{
            display: "flex",
            "flex-wrap": "wrap",
            gap: "var(--ds-space-2)",
            "align-items": "center",
          }}
        >
          <Button size="xs" variant="secondary">
            Size XS (22px)
          </Button>
          <Button size="sm" variant="secondary">
            Size SM (26px)
          </Button>
          <Button size="md" variant="secondary">
            Size MD (30px)
          </Button>
          <Button size="lg" variant="secondary">
            Size LG (36px)
          </Button>
        </div>
      </section>

      {/* Card 3: Git Domain Badges */}
      <section
        style={{
          padding: "var(--ds-space-4)",
          "background-color": "var(--ds-bg-primary)",
          border: "1px solid var(--ds-border-default)",
          "border-radius": "var(--ds-radius-md)",
          display: "flex",
          "flex-direction": "column",
          gap: "var(--ds-space-3)",
        }}
      >
        <h2 style={{ "font-size": "var(--ds-font-size-md)" }}>
          Semantic &amp; Git Domain Badges
        </h2>
        <div
          style={{
            display: "flex",
            "flex-wrap": "wrap",
            gap: "var(--ds-space-2)",
          }}
        >
          <Badge variant="success">
            <Icon name="git-pull-request" size={12} />
            <span>Open</span>
          </Badge>
          <Badge variant="merged">
            <Icon name="git-merge" size={12} />
            <span>Merged</span>
          </Badge>
          <Badge variant="danger">
            <Icon name="issue-closed" size={12} />
            <span>Closed</span>
          </Badge>
          <Badge variant="warning">
            <Icon name="clock" size={12} />
            <span>CI Pending</span>
          </Badge>
          <Badge variant="info">
            <Icon name="issue-draft" size={12} />
            <span>Draft PR</span>
          </Badge>
          <Badge variant="neutral" mono>
            <Icon name="shield-check" size={12} />
            <span>a94f8c1 (Verified)</span>
          </Badge>
        </div>
        <div
          style={{
            display: "flex",
            "align-items": "center",
            gap: "var(--ds-space-2)",
          }}
        >
          <Avatar name="Yu Seungjae" size="xs" />
          <Avatar name="Gitoxide Core" size="sm" />
          <Avatar name="Superhuman UX" size="md" />
          <span style={{ color: "var(--ds-fg-secondary)" }}>
            Zero-image deterministic initials avatars
          </span>
        </div>
      </section>

      {/* Card 4: Inputs & Chord Keybindings */}
      <section
        style={{
          padding: "var(--ds-space-4)",
          "background-color": "var(--ds-bg-primary)",
          border: "1px solid var(--ds-border-default)",
          "border-radius": "var(--ds-radius-md)",
          display: "flex",
          "flex-direction": "column",
          gap: "var(--ds-space-3)",
        }}
      >
        <h2 style={{ "font-size": "var(--ds-font-size-md)" }}>
          High-Density Inputs &amp; Chords
        </h2>
        <Input
          icon={<Icon name="search" size={14} />}
          placeholder="Search commits by SHA, author, or message..."
          shortcut="mod+f"
        />
        <div
          style={{
            display: "flex",
            "flex-wrap": "wrap",
            gap: "var(--ds-space-3)",
            "align-items": "center",
            "font-size": "var(--ds-font-size-xs)",
            color: "var(--ds-fg-secondary)",
          }}
        >
          <span>
            Swarm: <Kbd keys="g w" />
          </span>
          <span>
            Issues: <Kbd keys="g i" />
          </span>
          <span>
            Diff: <Kbd keys="g d" />
          </span>
          <span>
            Actions: <Kbd keys="g a" />
          </span>
          <span>
            Admin: <Kbd keys="g s" />
          </span>
        </div>
      </section>
    </div>
  );
};
