import {
  Component,
  createEffect,
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
import { AgentVerificationDialog } from "./AgentVerificationDialog";

export type AgentProvider =
  | "Claude Code"
  | "Cursor Agent"
  | "Codex Swarm"
  | "Internal Bot";

export interface AgentVerificationResult {
  command: string;
  exitCode: number;
  durationMs: number;
  stdout: string;
  stderr: string;
  outputTruncated: boolean;
}

export interface AgentSandboxTask {
  id: number;
  liveId?: string;
  agent: AgentProvider;
  branch: string;
  baseBranch: string;
  forkLatencyMs: number;
  promptSummary: string;
  sandboxStatus: "pending" | "verified" | "compiling" | "failed" | "merged" | "discarded";
  compileTimeMs: number;
  testsPassed: number;
  testsTotal: number;
  filesChanged: number;
  additions: number;
  deletions: number;
  githubSynced?: boolean;
  operation?: "approving" | "discarding" | "publishing" | "verifying";
  verifiedCommit?: string;
  verificationResult?: AgentVerificationResult;
  verificationError?: string;
  verificationNeedsRefresh?: boolean;
  publicationError?: string;
  updatedAt: string;
}

export interface AgentSwarmViewProps {
  tasks: AgentSandboxTask[];
  focusedIndex: number;
  onFocusIndexChange: (idx: number) => void;
  onApproveMerge: (id: number) => void;
  onDiscardBranch: (id: number) => void;
  onPublishGithub: (id: number) => void;
  onVerifyWorktree: (id: number, command: string) => void;
  onRefreshWorktrees: () => void;
  canRunVerification: boolean;
  liveMode: boolean;
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
  const [verificationId, setVerificationId] = createSignal<string | null>(null);
  const verificationTask = () => props.tasks.find((task) => task.liveId === verificationId());
  const filter = () => props.filterQuery ?? localFilter();
  const setFilter = (q: string) => {
    setLocalFilter(q);
    props.onFilterQueryChange?.(q);
  };

  let listEl: HTMLDivElement | undefined;
  const isRowFocused = createSelector(() => props.focusedIndex);

  createEffect(() => {
    const idx = props.focusedIndex;
    const row = listEl?.children[idx] as HTMLElement | undefined;
    row?.scrollIntoView?.({ block: "nearest" });
  });

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
      case "pending":
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
          <Show when={props.liveMode}>
            <Button size="sm" onClick={props.onRefreshWorktrees}>Refresh worktrees</Button>
          </Show>
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
            {props.liveMode ? "Spawn worktree" : "Spawn demo worktrees"}
          </Button>
        </div>
      </div>

      {/* High-Density Agent Branch & Sandbox Verification Table */}
      <div
        ref={(el) => {
          listEl = el;
        }}
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
                  <span>{task.verificationNeedsRefresh ? "state unconfirmed" : task.sandboxStatus}</span>
                </Badge>

                <Badge variant="neutral" mono>
                  <Icon name="git-branch" size={11} />
                  <span>{task.branch}</span>
                </Badge>

                <Show when={task.sandboxStatus === "merged"}>
                  <Badge variant={task.githubSynced ? "info" : "neutral"} mono>
                    <Icon name="rotate-ccw" size={11} />
                    <span>
                      {task.operation === "approving"
                        ? "Approving..."
                        : task.operation === "publishing"
                          ? "Publishing..."
                          : task.githubSynced ? "GitHub Synced" : "Local Merged"}
                    </span>
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
                    task.testsTotal === 0 ? "neutral" : task.testsPassed === task.testsTotal ? "success" : "danger"
                  }
                  mono
                >
                  {task.testsTotal > 0 ? `${task.testsPassed}/${task.testsTotal} tests` : "No test counts"}
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

                <Show when={task.liveId && ((task.sandboxStatus !== "merged" && task.sandboxStatus !== "discarded") || task.verificationResult)}>
                  <Button size="xs" variant="secondary" onClick={(event) => {
                    event.stopPropagation();
                    setVerificationId(task.liveId ?? null);
                  }}>
                    {task.verificationResult ? "Verification result" : "Verify command"}
                  </Button>
                </Show>

                <Show when={task.sandboxStatus === "merged" && !task.githubSynced && task.liveId}>
                  <Show when={task.publicationError}>
                    <Badge variant="danger">Publication failed</Badge>
                  </Show>
                  <Button
                    size="xs"
                    variant="primary"
                    disabled={Boolean(task.operation)}
                    title={task.publicationError || "Publish this approved merge to GitHub"}
                    onClick={(e) => {
                      e.stopPropagation();
                      props.onPublishGithub(task.id);
                    }}
                  >
                    {task.operation === "publishing"
                      ? "Publishing..."
                      : task.publicationError ? "Retry publication" : "Publish to GitHub"}
                  </Button>
                </Show>

                <Show
                  when={
                    task.sandboxStatus !== "merged" &&
                    task.sandboxStatus !== "discarded"
                  }
                >
                  <Button
                    size="xs"
                    variant="primary"
                    disabled={Boolean(task.operation) || task.sandboxStatus !== "verified" || Boolean(task.liveId && (!task.verifiedCommit || task.verificationNeedsRefresh))}
                    onClick={(e) => {
                      e.stopPropagation();
                      props.onApproveMerge(task.id);
                    }}
                  >
                    Approve &amp; Merge (a)
                  </Button>
                  <Button
                    size="xs"
                    variant="ghost"
                    disabled={Boolean(task.operation) || task.sandboxStatus === "compiling"}
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
      <Show when={verificationTask()}>
        {(task) => <AgentVerificationDialog
          task={task()}
          canRun={props.canRunVerification}
          onRun={props.onVerifyWorktree}
          onRefresh={props.onRefreshWorktrees}
          onClose={() => setVerificationId(null)}
        />}
      </Show>
    </div>
  );
};
