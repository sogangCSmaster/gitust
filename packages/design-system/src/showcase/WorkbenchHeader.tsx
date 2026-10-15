import { Component, For, Show } from "solid-js";
import {
  Badge,
  Button,
  Dialog,
  getTheme,
  Icon,
  Kbd,
  shortcutManager,
  toggleTheme,
} from "../index";

export interface WorkbenchHeaderProps {
  onOpenCommandPalette: () => void;
  onOpenShortcutsModal: () => void;
}

export const WorkbenchHeader: Component<WorkbenchHeaderProps> = (props) => {
  return (
    <header
      style={{
        display: "flex",
        "align-items": "center",
        "justify-content": "space-between",
        height: "48px",
        padding: "0 var(--ds-space-4)",
        "background-color": "var(--ds-bg-primary)",
        "border-bottom": "1px solid var(--ds-border-default)",
        position: "sticky",
        top: 0,
        "z-index": 20,
      }}
    >
      <div
        style={{
          display: "flex",
          "align-items": "center",
          gap: "var(--ds-space-3)",
        }}
      >
        <div
          style={{
            display: "flex",
            "align-items": "center",
            gap: "var(--ds-space-2)",
            "font-weight": 700,
            "font-size": "var(--ds-font-size-md)",
          }}
        >
          <span
            style={{
              display: "inline-flex",
              "align-items": "center",
              "justify-content": "center",
              width: "22px",
              height: "22px",
              "border-radius": "var(--ds-radius-xs)",
              "background-color": "var(--ds-accent-primary)",
              color: "#fff",
            }}
          >
            <Icon name="repo" size={13} />
          </span>
          <span>gitust</span>
          <span style={{ color: "var(--ds-fg-muted)", "font-weight": 400 }}>
            /
          </span>
          <span
            class="ds-mono"
            style={{ "font-size": "var(--ds-font-size-sm)" }}
          >
            agent-native-git
          </span>
        </div>

        <Badge variant="accent" dot>
          Agent + Sandbox Engine
        </Badge>
        <Badge variant="success" mono>
          <Icon name="zap" size={11} />
          <span>&lt;16ms Frame</span>
        </Badge>
      </div>

      {/* Live Keystroke Latency HUD & Global Actions */}
      <div
        style={{
          display: "flex",
          "align-items": "center",
          gap: "var(--ds-space-2)",
        }}
      >
        <Show when={shortcutManager.lastTelemetry()}>
          {(t) => (
            <div
              class="ds-mono"
              style={{
                display: "inline-flex",
                "align-items": "center",
                gap: "var(--ds-space-1-5)",
                padding: "2px 8px",
                "border-radius": "var(--ds-radius-sm)",
                "background-color": "var(--ds-bg-secondary)",
                border: "1px solid var(--ds-border-subtle)",
                "font-size": "var(--ds-font-size-2xs)",
                color: "var(--ds-fg-secondary)",
              }}
            >
              <span
                style={{
                  color: "var(--ds-success-fg)",
                  display: "inline-flex",
                }}
              >
                <Icon name="zap" size={12} />
              </span>
              <span>{t().title}</span>
              <span
                style={{
                  color: "var(--ds-success-fg)",
                  "font-weight": 600,
                }}
              >
                {t().dispatchLatencyMs}ms
              </span>
            </div>
          )}
        </Show>

        <Button
          variant="secondary"
          size="sm"
          icon={<Icon name="terminal" size={13} />}
          shortcut="mod+k"
          onClick={props.onOpenCommandPalette}
        >
          Command Palette
        </Button>

        <Button
          variant="ghost"
          size="sm"
          icon={<Icon name="keyboard" size={14} />}
          shortcut="?"
          onClick={props.onOpenShortcutsModal}
        >
          Shortcuts
        </Button>

        <Button
          variant="ghost"
          size="sm"
          icon={
            <Icon name={getTheme() === "dark" ? "sun" : "moon"} size={14} />
          }
          onClick={() => toggleTheme()}
          title="Toggle Dark/Light Theme"
        >
          {getTheme() === "dark" ? "Light" : "Dark"}
        </Button>
      </div>
    </header>
  );
};

export const ShortcutsReferenceModal: Component<{
  open: boolean;
  onClose: () => void;
}> = (props) => {
  return (
    <Dialog
      open={props.open}
      onClose={props.onClose}
      title="Keyboard Shortcuts Reference (Superhuman Mode)"
    >
      <div
        style={{
          display: "flex",
          "flex-direction": "column",
          gap: "var(--ds-space-2)",
        }}
      >
        <For each={shortcutManager.shortcuts()}>
          {(sc) => (
            <div
              style={{
                display: "flex",
                "align-items": "center",
                "justify-content": "space-between",
                padding: "var(--ds-space-1-5) 0",
                "border-bottom": "1px solid var(--ds-border-subtle)",
              }}
            >
              <div>
                <div style={{ "font-weight": 500 }}>{sc.title}</div>
                <div
                  style={{
                    "font-size": "var(--ds-font-size-xs)",
                    color: "var(--ds-fg-muted)",
                  }}
                >
                  {sc.description}
                </div>
              </div>
              <Kbd keys={sc.keys} />
            </div>
          )}
        </For>
      </div>
    </Dialog>
  );
};
