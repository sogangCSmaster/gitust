import { Component, createMemo, createSignal, For, Show } from "solid-js";
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
  updatedAt: string;
}

export interface AgentSwarmViewProps {
  tasks: AgentSandboxTask[];
  focusedIndex: number;
  onFocusIndexChange: (idx: number) => void;
  onApproveMerge: (id: number) => void;
  onDiscardBranch: (id: number) => void;
  onSpawnBatchSwarm: () => void;
}

/**
 * Agent-Native Swarm & Micro-Sandbox Fleet Control View.
 * Built for the era where Claude Code / Cursor / AI agents spawn hundreds of concurrent
 * zero-copy CoW branches and validate builds in <100ms sandboxes, while humans triage
 * and merge verified branches at Superhuman speed (j/k -> a to merge, x to discard).
 */
export const AgentSwarmView: Component<AgentSwarmViewProps> = (props) => {
  const [filter, setFilter] = createSignal("");

  const filteredTasks = createMemo(() => {
    const q = filter().trim().toLowerCase();
    if (!q) return props.tasks;
    return props.tasks.filter(
      (t) =>
        t.branch.toLowerCase().includes(q) ||
        t.agent.toLowerCase().includes(q) ||
        t.promptSummary.toLowerCase().includes(q) ||
        t.sandboxStatus.toLowerCase().includes(q)
    );
  });

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
      {/* Top Telemetry Cards: Zero-Copy Branching & Sandbox Fleet Throughput */}
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
              <span>Active Agent Worktrees</span>
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
                (0.4ms CoW Fork)
              </span>
            </div>
            <div
              style={{
                "font-size": "var(--ds-font-size-2xs)",
                color: "var(--ds-fg-muted)",
              }}
            >
              gix In-Memory Object DB · 0 Byte Disk Overhead
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
              <span>Sandbox Verified (Ready)</span>
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
              {stats().verified} Ready · {stats().merged} Merged
            </div>
            <div
              style={{
                "font-size": "var(--ds-font-size-2xs)",
                color: "var(--ds-fg-muted)",
              }}
            >
              Press <Kbd keys="a" /> on focused row to instant-merge
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
              <span>Micro-Sandbox Pool</span>
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
              {stats().compiling} Running · 84ms avg
            </div>
            <div
              style={{
                "font-size": "var(--ds-font-size-2xs)",
                color: "var(--ds-fg-muted)",
              }}
            >
              Shared Cargo/sccache + Firecracker/Namespace Isolation
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
              <span>Agent Protocol Gateway</span>
              <Icon name="bot" size={14} />
            </div>
            <div
              class="ds-mono"
              style={{
                "font-size": "var(--ds-font-size-xl)",
                "font-weight": 700,
                color: "var(--ds-merged-fg)",
              }}
            >
              MCP + Batch RPC
            </div>
            <div
              style={{
                "font-size": "var(--ds-font-size-2xs)",
                color: "var(--ds-fg-muted)",
              }}
            >
              Claude Code · Cursor · Codex Direct Socket
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
            placeholder="Filter agent branches by agent (Claude Code, Cursor), status (verified, compiling), or prompt..."
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
          <span>Approve &amp; Merge:</span>
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
          {(task, index) => {
            const isFocused = () => props.focusedIndex === index();

            return (
              <div
                role="option"
                aria-selected={isFocused()}
                data-focused={isFocused()}
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

                  <span class="ds-issue-title">{task.promptSummary}</span>
                </div>

                <div class="ds-issue-right">
                  <span
                    class="ds-mono"
                    title="CoW Branch Fork Latency + Sandbox Compile Time"
                    style={{
                      "font-size": "var(--ds-font-size-2xs)",
                      color: "var(--ds-fg-muted)",
                    }}
                  >
                    fork:{task.forkLatencyMs}ms · build:{task.compileTimeMs}ms
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
                      Merge (a)
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
            );
          }}
        </For>
      </div>
    </div>
  );
};
