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
import { AgentSwarmHeaderBanners } from "./AgentSwarmHeaderBanners";

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
      <AgentSwarmHeaderBanners
        stats={stats()}
        onTriggerGithubSync={props.onTriggerGithubSync}
      />

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
                    task.testsPassed === task.testsTotal ? "success" : "danger"
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
