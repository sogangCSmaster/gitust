import { Component, For, Show } from "solid-js";
import {
  ActionJobStep,
  ActionsPipelineView,
  AgentSandboxTask,
  AgentSwarmView,
  Badge,
  Button,
  DiffLine,
  EnterpriseAdminView,
  EnterpriseUser,
  FileTreeNode,
  Icon,
  IssueItem,
  IssueListView,
  Kbd,
  VirtualDiffViewer,
  VirtualFileTree,
} from "../index";
import type { LiveCommitSummary } from "./live-repo-client";
import type { ActionLogEntry } from "./mock-data";
import { PrimitivesTab } from "./PrimitivesTab";
import type { WorkbenchTab } from "./WorkbenchApp";

export interface WorkbenchTabViewsProps {
  activeTab: WorkbenchTab;
  swarmTasks: AgentSandboxTask[];
  focusedSwarmIdx: number;
  onFocusedSwarmIdxChange: (idx: number) => void;
  onApproveMergeSwarmTask: (id: number) => void;
  onDiscardSwarmTask: (id: number) => void;
  onSpawnBatchAgentSwarm: () => void;
  onTriggerGithubSync: () => void;
  swarmFilter: string;
  onSwarmFilterChange: (q: string) => void;
  issues: IssueItem[];
  focusedIssueIdx: number;
  onFocusedIssueIdxChange: (idx: number) => void;
  onToggleIssueStatus: (id: number) => void;
  onCreateOptimisticIssue: () => void;
  filterRef: (el: HTMLInputElement) => void;
  issueFilter: string;
  onIssueFilterChange: (q: string) => void;
  liveCommits: LiveCommitSummary[];
  activeCommitRev: string;
  onSelectCommitDiff: (sha: string) => void;
  lookupMicros: number;
  selectedFilePath: string;
  diffLines: DiffLine[];
  actionJobs: ActionJobStep[];
  activeJobId: string;
  onSelectJob: (id: string) => void;
  onRerunAllActionJobs: () => void;
  actionLogs: ActionLogEntry[];
  enterpriseUsers: EnterpriseUser[];
  onToggleEnterpriseUserRole: (id: number) => void;
  onToggleEnterpriseUserStatus: (id: number) => void;
  onPolicyFeedback: (msg: string) => void;
  fileTreeNodes: FileTreeNode[];
  onSelectFileFromTree: (node: { path: string; kind: "file" | "dir" }) => void;
}

export const WorkbenchTabViews: Component<WorkbenchTabViewsProps> = (props) => {
  return (
    <>
      {/* TAB 0: Agent Swarm & Micro-Sandbox Fleet Control */}
      <Show when={props.activeTab === "swarm"}>
        <div
          style={{
            display: "flex",
            "align-items": "center",
            "justify-content": "space-between",
          }}
        >
          <div>
            <h1
              style={{
                "font-size": "var(--ds-font-size-lg)",
                "font-weight": 600,
              }}
            >
              Agent Swarm &amp; Micro-Sandbox Execution Engine
            </h1>
            <p
              style={{
                "font-size": "var(--ds-font-size-xs)",
                color: "var(--ds-fg-secondary)",
              }}
            >
              Claude Code, Cursor &amp; Codex agents spawn isolated{" "}
              <code class="ds-mono">gitoxide</code> CoW branches in{" "}
              <strong>&lt;1ms</strong> and run isolated sandbox builds in{" "}
              <strong>&lt;100ms</strong>. Triage with <Kbd keys="j" /> /{" "}
              <Kbd keys="k" />, press <Kbd keys="a" /> to merge,{" "}
              <Kbd keys="x" /> to discard, or <Kbd keys="s" /> to spawn +10
              sandboxes.
            </p>
          </div>
          <Badge variant="merged" mono>
            <Icon name="cpu" size={12} />
            <span>Enterprise Sandbox Cluster Ready</span>
          </Badge>
        </div>

        <AgentSwarmView
          tasks={props.swarmTasks}
          focusedIndex={props.focusedSwarmIdx}
          onFocusIndexChange={props.onFocusedSwarmIdxChange}
          onApproveMerge={props.onApproveMergeSwarmTask}
          onDiscardBranch={props.onDiscardSwarmTask}
          onSpawnBatchSwarm={props.onSpawnBatchAgentSwarm}
          onTriggerGithubSync={props.onTriggerGithubSync}
          filterQuery={props.swarmFilter}
          onFilterQueryChange={props.onSwarmFilterChange}
        />
      </Show>

      {/* TAB 1: Superhuman Issue & PR List */}
      <Show when={props.activeTab === "issues"}>
        <div
          style={{
            display: "flex",
            "align-items": "center",
            "justify-content": "space-between",
          }}
        >
          <div>
            <h1
              style={{
                "font-size": "var(--ds-font-size-lg)",
                "font-weight": 600,
              }}
            >
              Keyboard-First Issues &amp; Pull Requests
            </h1>
            <p
              style={{
                "font-size": "var(--ds-font-size-xs)",
                color: "var(--ds-fg-secondary)",
              }}
            >
              Use <Kbd keys="j" /> / <Kbd keys="k" /> to move focus,{" "}
              <Kbd keys="e" /> to toggle Open/Closed in 0ms, <Kbd keys="c" /> to
              create an instant issue, or <Kbd keys="/" /> to filter.
            </p>
          </div>
          <Button
            variant="primary"
            size="sm"
            icon={<Icon name="plus" size={13} />}
            shortcut="c"
            onClick={props.onCreateOptimisticIssue}
          >
            New Issue
          </Button>
        </div>

        <IssueListView
          items={props.issues}
          focusedIndex={props.focusedIssueIdx}
          onFocusIndexChange={props.onFocusedIssueIdxChange}
          onToggleStatus={props.onToggleIssueStatus}
          filterRef={props.filterRef}
          filterQuery={props.issueFilter}
          onFilterQueryChange={props.onIssueFilterChange}
        />
      </Show>

      {/* TAB 2: 10,000-Line Virtualized Diff Viewer & gix Commit Selector */}
      <Show when={props.activeTab === "diff"}>
        <div
          style={{
            display: "flex",
            "align-items": "center",
            "justify-content": "space-between",
            "flex-wrap": "wrap",
            gap: "var(--ds-space-2)",
          }}
        >
          <div>
            <h1
              style={{
                "font-size": "var(--ds-font-size-lg)",
                "font-weight": 600,
              }}
            >
              Virtualized Code &amp; Diff Viewer (In-Process gix Engine)
            </h1>
            <p
              style={{
                "font-size": "var(--ds-font-size-xs)",
                color: "var(--ds-fg-secondary)",
                "margin-top": "var(--ds-space-1)",
              }}
            >
              Renders up to 10,000 diff &amp; blob lines using O(1) viewport
              slicing (~45 DOM nodes at 120fps).
            </p>
          </div>
          <div
            style={{
              display: "flex",
              "align-items": "center",
              gap: "var(--ds-space-2)",
              "flex-wrap": "wrap",
            }}
          >
            <For each={props.liveCommits}>
              {(c) => (
                <Button
                  size="xs"
                  variant={
                    props.activeCommitRev === c.sha ||
                    props.activeCommitRev === c.short_sha
                      ? "primary"
                      : "secondary"
                  }
                  icon={<Icon name="git-commit" size={12} />}
                  onClick={() => props.onSelectCommitDiff(c.sha)}
                >
                  {c.short_sha} · {c.summary.slice(0, 28)}
                </Button>
              )}
            </For>
            <Badge variant="success" mono>
              <Icon name="zap" size={12} />
              <span>gix {props.lookupMicros}µs</span>
            </Badge>
          </div>
        </div>

        <VirtualDiffViewer
          filePath={props.selectedFilePath}
          lines={props.diffLines}
          height={480}
        />
      </Show>

      {/* TAB 3: Gitust Actions CI/CD Pipeline & 10,000-Line Log Streamer */}
      <Show when={props.activeTab === "actions"}>
        <div
          style={{
            display: "flex",
            "align-items": "center",
            "justify-content": "space-between",
          }}
        >
          <div>
            <h1
              style={{
                "font-size": "var(--ds-font-size-lg)",
                "font-weight": 600,
              }}
            >
              Gitust Actions — Self-Hosted CI/CD Pipeline &amp; Live Console
            </h1>
            <p
              style={{
                "font-size": "var(--ds-font-size-xs)",
                color: "var(--ds-fg-secondary)",
              }}
            >
              100% compatible with{" "}
              <code class="ds-mono">.github/workflows/*.yml</code> &amp; Gitea{" "}
              <code class="ds-mono">act_runner</code>. Streams 10,000+ console
              lines at 120fps. Press <Kbd keys="r" /> to re-run jobs.
            </p>
          </div>
          <Badge variant="success" mono>
            <Icon name="server" size={12} />
            <span>Private Runner Pool: Active</span>
          </Badge>
        </div>

        <ActionsPipelineView
          workflowName="ci-enterprise-release.yml"
          commitSha="5edd70a"
          branch="main"
          jobs={props.actionJobs}
          activeJobId={props.activeJobId}
          onSelectJob={props.onSelectJob}
          onRerunAll={props.onRerunAllActionJobs}
          logs={props.actionLogs}
          height={440}
        />
      </Show>

      {/* TAB 4: Enterprise Self-Hosted Admin, Auth & SSO Console */}
      <Show when={props.activeTab === "admin"}>
        <div
          style={{
            display: "flex",
            "align-items": "center",
            "justify-content": "space-between",
          }}
        >
          <div>
            <h1
              style={{
                "font-size": "var(--ds-font-size-lg)",
                "font-weight": 600,
              }}
            >
              Enterprise Self-Hosted Auth, SSO &amp; Site Admin Console
            </h1>
            <p
              style={{
                "font-size": "var(--ds-font-size-xs)",
                color: "var(--ds-fg-secondary)",
              }}
            >
              Built for corporate on-premise deployments: control invite-only
              registration, email domain whitelist, OIDC/LDAP SSO, 2FA, and
              organization roles with 0ms Optimistic UI.
            </p>
          </div>
          <Badge variant="accent" mono>
            <Icon name="lock" size={12} />
            <span>Single-Binary On-Premise Mode</span>
          </Badge>
        </div>

        <EnterpriseAdminView
          users={props.enterpriseUsers}
          onToggleRole={props.onToggleEnterpriseUserRole}
          onToggleStatus={props.onToggleEnterpriseUserStatus}
          onPolicyFeedback={props.onPolicyFeedback}
        />
      </Show>

      {/* TAB 5: 5,000-File Virtualized Monorepo File Tree */}
      <Show when={props.activeTab === "tree"}>
        <div
          style={{
            display: "flex",
            "align-items": "center",
            "justify-content": "space-between",
          }}
        >
          <div>
            <h1
              style={{
                "font-size": "var(--ds-font-size-lg)",
                "font-weight": 600,
              }}
            >
              Virtualized Monorepo File Tree (5,000 Files)
            </h1>
            <p
              style={{
                "font-size": "var(--ds-font-size-xs)",
                color: "var(--ds-fg-secondary)",
              }}
            >
              Instant client-side filtering and GPU-instanced SVG icons across
              5,000 Rust &amp; TypeScript source paths.
            </p>
          </div>
          <Badge variant="info" mono>
            <Icon name="file-code" size={12} />
            <span>{props.selectedFilePath}</span>
          </Badge>
        </div>

        <VirtualFileTree
          files={props.fileTreeNodes}
          activePath={props.selectedFilePath}
          onSelect={props.onSelectFileFromTree}
          height={440}
        />
      </Show>

      {/* TAB 6: Design Tokens, Vector Icon Sprite Sheet & Primitives Showcase */}
      <Show when={props.activeTab === "primitives"}>
        <PrimitivesTab />
      </Show>
    </>
  );
};
