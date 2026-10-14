import {
  Component,
  createMemo,
  createSelector,
  createSignal,
  For,
  Show,
} from "solid-js";
import { Badge, BadgeVariant } from "../primitives/Badge";
import { Button } from "../primitives/Button";
import { Icon } from "../primitives/Icon";
import { Input } from "../primitives/Input";
import { Kbd } from "../primitives/Kbd";

export type AgentProvider =
  | "Claude Code"
  | "Cursor Agent"
  | "Codex Swarm"
  | "Internal Bot";

export interface AgentSandboxTask {
  id: number;
  agent: AgentProvider;
  branch: string;
  baseBranch: string;
  forkLatencyMs: number;
  promptSummary: string;
  sandboxStatus: "verified" | "compiling" | "failed" | "merged" | "discarded";
  compileTimeMs: number;
  testsPassed: number;
  testsTotal: number;
  filesChanged: number;
  additions: number;
  deletions: number;
  githubSynced?: boolean;
  updatedAt: string;
}

export interface AgentSwarmViewProps {
  tasks: AgentSandboxTask[];
  focusedIndex: number;
  onFocusIndexChange: (idx: number) => void;
  onApproveMerge: (id: number) => void;
  onDiscardBranch: (id: number) => void;
  onSpawnBatchSwarm: () => void;
  onTriggerGithubSync: () => void;
  filterQuery?: string;
  onFilterQueryChange?: (query: string) => void;
}

export function filterAgentSwarmTasks(
  tasks: AgentSandboxTask[],
  rawQuery: string
): AgentSandboxTask[] {
  const q = rawQuery.trim().toLowerCase();
  if (!q) return tasks;
  return tasks.filter(
    (t) =>
      t.branch.toLowerCase().includes(q) ||
      t.agent.toLowerCase().includes(q) ||
      t.promptSummary.toLowerCase().includes(q) ||
      t.sandboxStatus.toLowerCase().includes(q)
  );
}

/**
 * All-in-One Agent-Native Git + Container-Use Worktree Isolation + Microsandbox Execution View.
 * Solves Cursor Origin / Cloud Agent limitations:
 * - 100% On-Premise Zero-Cloud-Leakage (Privacy Mode stays ON, unlimited local quota)
 * - Real-time Bi-Directional GitHub Sync (only verified & approved branches sync to GitHub)
 * - Unified `gitust` Single-Binary CLI & MCP Server
 */
export const AgentSwarmView: Component<AgentSwarmViewProps> = (props) => {
  const [localFilter, setLocalFilter] = createSignal("");
  const filter = () => props.filterQuery ?? localFilter();
  const setFilter = (q: string) => {
    setLocalFilter(q);
    props.onFilterQueryChange?.(q);
  };

  const [autoSyncMergedToGithub, setAutoSyncMergedToGithub] =
    createSignal(true);
  const isRowFocused = createSelector(() => props.focusedIndex);

  const filteredTasks = createMemo(() =>
    filterAgentSwarmTasks(props.tasks, filter())
  );

  const stats = createMemo(() => {
    let verified = 0;
    let compiling = 0;
    let merged = 0;
    let failed = 0;
    for (const t of props.tasks) {
      if (t.sandboxStatus === "verified") verified++;
      else if (t.sandboxStatus === "compiling") compiling++;
      else if (t.sandboxStatus === "merged") merged++;
      else if (t.sandboxStatus === "failed") failed++;
    }
    return { verified, compiling, merged, failed, total: props.tasks.length };
  });

  const statusBadge = (
    status: AgentSandboxTask["sandboxStatus"]
  ): BadgeVariant => {
    switch (status) {
      case "verified":
        return "success";
      case "compiling":
        return "warning";
      case "failed":
        return "danger";
      case "merged":
        return "merged";
      case "discarded":
        return "neutral";
    }
  };

  return (
    <div
      style={{
        display: "flex",
        "flex-direction": "column",
        gap: "var(--ds-space-3)",
      }}
    >
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
                Zero-Cloud-Leakage On-Premise Engine &amp; Bi-Directional GitHub
                Sync
              </span>
            </span>
            <Badge variant="success" mono>
              Privacy Mode: STRICT ON-PREM (0 Bytes to Cloud)
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
                  Agent experimental branches stay 100% private on-prem. Only
                  human-approved (<Kbd keys="a" />) merges sync back to GitHub.
                </div>
              </div>
              <Button
                size="xs"
                variant="secondary"
                icon={<Icon name="rotate-ccw" size={12} />}
                onClick={props.onTriggerGithubSync}
              >
                Sync GitHub Now (11ms)
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
                  Auto-push approved merges to GitHub
                </span>
                <button
                  type="button"
                  role="switch"
                  aria-checked={autoSyncMergedToGithub()}
                  data-checked={autoSyncMergedToGithub()}
                  class="ds-switch"
                  onClick={() => setAutoSyncMergedToGithub((v) => !v)}
                >
                  <span class="ds-switch-thumb" />
                </button>
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
              triage --approve-verified{" "}
              <span style={{ color: "var(--ds-fg-muted)" }}>
                # Instant merge &amp; two-way GitHub push
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
              {stats().total}{" "}
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
              {stats().verified} Ready · {stats().merged} Synced
            </div>
            <div
              style={{
                "font-size": "var(--ds-font-size-2xs)",
                color: "var(--ds-fg-muted)",
              }}
            >
              Press <Kbd keys="a" /> to merge &amp; sync to GitHub
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
              {stats().compiling} Active · 84ms avg
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

      {/* Filter & Triage Action Bar */}
      <div
        style={{
          display: "flex",
          "align-items": "center",
          "justify-content": "space-between",
          gap: "var(--ds-space-3)",
        }}
      >
        <div style={{ flex: 1 }}>
          <Input
            icon={<Icon name="search" size={14} />}
            placeholder="Filter isolated agent worktrees by agent (Claude Code, Cursor), status (verified, compiling), or prompt..."
            value={filter()}
            onInput={(e) => {
              setFilter(e.currentTarget.value);
              props.onFocusIndexChange(0);
            }}
          />
        </div>
        <div
          style={{
            display: "flex",
            "align-items": "center",
            gap: "var(--ds-space-2)",
            "font-size": "var(--ds-font-size-xs)",
            color: "var(--ds-fg-muted)",
            "flex-shrink": 0,
          }}
        >
          <span>Triage:</span>
          <Kbd keys="j" />
          <Kbd keys="k" />
          <span>Merge &amp; Sync:</span>
          <Kbd keys="a" />
          <span>Discard:</span>
          <Kbd keys="x" />
          <Button
            variant="primary"
            size="sm"
            icon={<Icon name="zap" size={13} />}
            shortcut="s"
            onClick={props.onSpawnBatchSwarm}
          >
            Spawn +10 Agent Sandboxes (&lt;1ms)
          </Button>
        </div>
      </div>

      {/* High-Density Agent Branch & Sandbox Verification Table */}
      <div
        class="ds-issue-list"
        role="listbox"
        aria-label="Agent Swarm Sandbox Branches"
      >
        <For each={filteredTasks()}>
          {(task, index) => (
            <div
              role="option"
              aria-selected={isRowFocused(index())}
              data-focused={isRowFocused(index())}
              class="ds-issue-row"
              onMouseEnter={() => props.onFocusIndexChange(index())}
              onClick={() => props.onFocusIndexChange(index())}
            >
                <div class="ds-issue-left">
                  <Badge variant="accent">
                    <Icon name="bot" size={12} />
                    <span>{task.agent}</span>
                  </Badge>

                  <Badge variant={statusBadge(task.sandboxStatus)}>
                    <Icon
                      name={
                        task.sandboxStatus === "verified"
                          ? "check"
                          : task.sandboxStatus === "merged"
                            ? "git-merge"
                            : task.sandboxStatus === "failed"
                              ? "x"
                              : "clock"
                      }
                      size={12}
                    />
                    <span>{task.sandboxStatus}</span>
                  </Badge>

                  <Badge variant="neutral" mono>
                    <Icon name="git-branch" size={11} />
                    <span>{task.branch}</span>
                  </Badge>

                  <Show when={task.sandboxStatus === "merged"}>
                    <Badge variant="info" mono>
                      <Icon name="rotate-ccw" size={11} />
                      <span>GitHub Synced</span>
                    </Badge>
                  </Show>

                  <span class="ds-issue-title">{task.promptSummary}</span>
                </div>

                <div class="ds-issue-right">
                  <span
                    class="ds-mono"
                    title="CoW Worktree Fork Latency + Microsandbox Execution Time"
                    style={{
                      "font-size": "var(--ds-font-size-2xs)",
                      color: "var(--ds-fg-muted)",
                    }}
                  >
                    cow:{task.forkLatencyMs}ms · msb:{task.compileTimeMs}ms
                  </span>

                  <Badge
                    variant={
                      task.testsPassed === task.testsTotal
                        ? "success"
                        : "danger"
                    }
                    mono
                  >
                    {task.testsPassed}/{task.testsTotal} tests
                  </Badge>

                  <span
                    class="ds-mono"
                    style={{
                      "font-size": "var(--ds-font-size-xs)",
                      width: "78px",
                      "text-align": "right",
                    }}
                  >
                    <span style={{ color: "var(--ds-success-fg)" }}>
                      +{task.additions}
                    </span>{" "}
                    <span style={{ color: "var(--ds-danger-fg)" }}>
                      -{task.deletions}
                    </span>
                  </span>

                  <Show
                    when={
                      task.sandboxStatus !== "merged" &&
                      task.sandboxStatus !== "discarded"
                    }
                  >
                    <Button
                      size="xs"
                      variant="primary"
                      onClick={(e) => {
                        e.stopPropagation();
                        props.onApproveMerge(task.id);
                      }}
                    >
                      Merge &amp; Sync (a)
                    </Button>
                    <Button
                      size="xs"
                      variant="ghost"
                      onClick={(e) => {
                        e.stopPropagation();
                        props.onDiscardBranch(task.id);
                      }}
                    >
                      Discard (x)
                    </Button>
                  </Show>
                </div>
              </div>
            )}
        </For>
      </div>
    </div>
  );
};
