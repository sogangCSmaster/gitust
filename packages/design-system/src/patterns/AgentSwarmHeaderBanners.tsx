import { Component } from "solid-js";
import { Badge } from "../primitives/Badge";
import { Button } from "../primitives/Button";
import { Icon } from "../primitives/Icon";
import { Kbd } from "../primitives/Kbd";

export interface AgentSwarmStats {
  verified: number;
  compiling: number;
  merged: number;
  failed: number;
  total: number;
}

export interface AgentSwarmHeaderBannersProps {
  stats: AgentSwarmStats;
  onTriggerGithubSync: () => void;
}

export const AgentSwarmHeaderBanners: Component<AgentSwarmHeaderBannersProps> = (
  props
) => {
  return (
    <>
      {/* Top Architecture Banner: Gitea + Container-Use + Microsandbox + Bi-Directional GitHub Sync */}
      <div
        style={{
          display: "grid",
          "grid-template-columns": "1.25fr 1fr",
          gap: "var(--ds-space-3)",
        }}
      >
        {/* Left Banner: 100% On-Premise Privacy & Bi-Directional GitHub Sync Control */}
        <div class="ds-card">
          <div class="ds-card-header">
            <span
              style={{
                display: "inline-flex",
                "align-items": "center",
                gap: "var(--ds-space-1-5)",
              }}
            >
              <Icon name="shield-check" size={14} />
              <span>
                On-Premise Worktrees &amp; GitHub Sync
              </span>
            </span>
            <Badge variant="success" mono>
              Private worktrees · approved publication
            </Badge>
          </div>
          <div
            class="ds-card-body"
            style={{
              padding: "var(--ds-space-3)",
              display: "flex",
              "flex-direction": "column",
              gap: "var(--ds-space-2-5)",
            }}
          >
            <div
              style={{
                display: "flex",
                "align-items": "center",
                "justify-content": "space-between",
                gap: "var(--ds-space-2)",
              }}
            >
              <div>
                <div
                  style={{
                    "font-weight": 600,
                    "font-size": "var(--ds-font-size-xs)",
                  }}
                >
                  Two-Way Live Mirror:{" "}
                  <span class="ds-mono" style={{ color: "var(--ds-fg-link)" }}>
                    github.com/corp/core-monorepo ⇄ gitust.internal/corp/core
                  </span>
                </div>
                <div
                  style={{
                    "font-size": "var(--ds-font-size-2xs)",
                    color: "var(--ds-fg-muted)",
                  }}
                >
                  Approve and merge locally with <Kbd keys="a" />, then select
                  Publish to GitHub on the approved worktree.
                </div>
              </div>
              <Button
                size="xs"
                variant="secondary"
                icon={<Icon name="rotate-ccw" size={12} />}
                onClick={props.onTriggerGithubSync}
              >
                Fetch from GitHub
              </Button>
            </div>

            <div
              style={{
                display: "flex",
                "align-items": "center",
                "justify-content": "space-between",
                "padding-top": "var(--ds-space-2)",
                "border-top": "1px solid var(--ds-border-subtle)",
                "font-size": "var(--ds-font-size-xs)",
              }}
            >
              <div
                style={{
                  display: "flex",
                  "align-items": "center",
                  gap: "var(--ds-space-2)",
                }}
              >
                <Badge variant="info" mono>
                  1. gitoxide (Git)
                </Badge>
                <Badge variant="accent" mono>
                  2. container-use (MCP Worktree)
                </Badge>
                <Badge variant="merged" mono>
                  3. microsandbox (libkrun CI)
                </Badge>
              </div>

              <div
                style={{
                  display: "flex",
                  "align-items": "center",
                  gap: "var(--ds-space-2)",
                }}
              >
                <span
                  style={{
                    "font-size": "var(--ds-font-size-2xs)",
                    color: "var(--ds-fg-secondary)",
                  }}
                >
                  GitHub publication requires an explicit action
                </span>

              </div>
            </div>
          </div>
        </div>

        {/* Right Banner: Unified Single-Binary `gitust` CLI & Agent MCP Quickstart */}
        <div class="ds-card">
          <div class="ds-card-header">
            <span
              style={{
                display: "inline-flex",
                "align-items": "center",
                gap: "var(--ds-space-1-5)",
              }}
            >
              <Icon name="terminal" size={14} />
              <span>Unified Single-Binary CLI (`gitust`) &amp; MCP Bridge</span>
            </span>
            <Badge variant="neutral" mono>
              Zero External Daemons
            </Badge>
          </div>
          <div
            class="ds-card-body ds-mono"
            style={{
              padding: "var(--ds-space-2-5) var(--ds-space-3)",
              "background-color": "var(--ds-bg-canvas)",
              "font-size": "var(--ds-font-size-2xs)",
              gap: "4px",
              color: "var(--ds-fg-secondary)",
            }}
          >
            <div>
              <span style={{ color: "var(--ds-success-fg)" }}>$</span> gitust
              serve --sync-github corp/core-monorepo{" "}
              <span style={{ color: "var(--ds-fg-muted)" }}>
                # All-in-one Git + Web + Microsandbox
              </span>
            </div>
            <div>
              <span style={{ color: "var(--ds-success-fg)" }}>$</span> claude
              mcp add gitust -- gitust mcp{" "}
              <span style={{ color: "var(--ds-fg-muted)" }}>
                # Replaces container-use &amp; cloud VM
              </span>
            </div>
            <div>
              <span style={{ color: "var(--ds-success-fg)" }}>$</span> gitust
              sandbox run "cargo test --workspace"{" "}
              <span style={{ color: "var(--ds-fg-muted)" }}>
                # &lt;1ms CoW fork + 84ms microsandbox
              </span>
            </div>
            <div>
              <span style={{ color: "var(--ds-success-fg)" }}>$</span> gitust
              triage approve agent/claude/task-1 --repo /srv/gitust/repo{" "}
              <span style={{ color: "var(--ds-fg-muted)" }}>
                # Approve and merge locally; publication is separate
              </span>
            </div>
          </div>
        </div>
      </div>

      {/* 4 Telemetry Cards: Zero-Copy Branching & Sandbox Fleet Throughput */}
      <div
        style={{
          display: "grid",
          "grid-template-columns": "repeat(4, 1fr)",
          gap: "var(--ds-space-3)",
        }}
      >
        <div class="ds-card">
          <div class="ds-card-body" style={{ padding: "var(--ds-space-3)" }}>
            <div
              style={{
                display: "flex",
                "align-items": "center",
                "justify-content": "space-between",
                color: "var(--ds-fg-secondary)",
                "font-size": "var(--ds-font-size-xs)",
              }}
            >
              <span>Container-Use Worktrees</span>
              <Icon name="layers" size={14} />
            </div>
            <div
              class="ds-mono"
              style={{
                "font-size": "var(--ds-font-size-xl)",
                "font-weight": 700,
              }}
            >
              {props.stats.total}{" "}
              <span
                style={{
                  "font-size": "var(--ds-font-size-xs)",
                  color: "var(--ds-success-fg)",
                  "font-weight": 500,
                }}
              >
                (0.35ms CoW)
              </span>
            </div>
            <div
              style={{
                "font-size": "var(--ds-font-size-2xs)",
                color: "var(--ds-fg-muted)",
              }}
            >
              Unlimited On-Prem Quota · 0 Byte Clone Overhead
            </div>
          </div>
        </div>

        <div class="ds-card">
          <div class="ds-card-body" style={{ padding: "var(--ds-space-3)" }}>
            <div
              style={{
                display: "flex",
                "align-items": "center",
                "justify-content": "space-between",
                color: "var(--ds-fg-secondary)",
                "font-size": "var(--ds-font-size-xs)",
              }}
            >
              <span>Microsandbox Verified</span>
              <Icon name="check" size={14} />
            </div>
            <div
              class="ds-mono"
              style={{
                "font-size": "var(--ds-font-size-xl)",
                "font-weight": 700,
                color: "var(--ds-success-fg)",
              }}
            >
              {props.stats.verified} Ready · {props.stats.merged} Merged
            </div>
            <div
              style={{
                "font-size": "var(--ds-font-size-2xs)",
                color: "var(--ds-fg-muted)",
              }}
            >
              Press <Kbd keys="a" /> to approve &amp; merge locally
            </div>
          </div>
        </div>

        <div class="ds-card">
          <div class="ds-card-body" style={{ padding: "var(--ds-space-3)" }}>
            <div
              style={{
                display: "flex",
                "align-items": "center",
                "justify-content": "space-between",
                color: "var(--ds-fg-secondary)",
                "font-size": "var(--ds-font-size-xs)",
              }}
            >
              <span>Microsandbox Engine (`msb`)</span>
              <Icon name="cpu" size={14} />
            </div>
            <div
              class="ds-mono"
              style={{
                "font-size": "var(--ds-font-size-xl)",
                "font-weight": 700,
                color: "var(--ds-info-fg)",
              }}
            >
              {props.stats.compiling} Active · 84ms avg
            </div>
            <div
              style={{
                "font-size": "var(--ds-font-size-2xs)",
                color: "var(--ds-fg-muted)",
              }}
            >
              Rust libkrun MicroVM + Shared Build Cache
            </div>
          </div>
        </div>

        <div class="ds-card">
          <div class="ds-card-body" style={{ padding: "var(--ds-space-3)" }}>
            <div
              style={{
                display: "flex",
                "align-items": "center",
                "justify-content": "space-between",
                color: "var(--ds-fg-secondary)",
                "font-size": "var(--ds-font-size-xs)",
              }}
            >
              <span>GitHub Bi-Directional Mirror</span>
              <Icon name="rotate-ccw" size={14} />
            </div>
            <div
              class="ds-mono"
              style={{
                "font-size": "var(--ds-font-size-xl)",
                "font-weight": 700,
                color: "var(--ds-merged-fg)",
              }}
            >
              Live ⇄ 11ms
            </div>
            <div
              style={{
                "font-size": "var(--ds-font-size-2xs)",
                color: "var(--ds-fg-muted)",
              }}
            >
              Inbound Webhook + Outbound Verified Push
            </div>
          </div>
        </div>
      </div>
    </>
  );
};
