import { For, Show, lazy, type Component } from "solid-js";
import {
  ActionsPipelineView,
  AgentSwarmView,
  IssueListView,
  VirtualDiffViewer,
  VirtualFileTree,
  type ActionJobStep,
  type AgentSandboxTask,
  type DiffLine,
  type EnterpriseAdminViewProps,
  type EnterpriseUser,
  type FileTreeNode,
  type IssueItem,
  type ActionLogLine,
} from "../index";
import { Button } from "../components/ui/button";
import { Card, CardContent } from "../components/ui/card";
import { PageHeading, StatusBadge } from "../components/workspace/presentation";
import { Icon } from "../components/workspace/icon";
import type { LiveCommitSummary, LiveWorktreeReview } from "./live-repo-client";
import type { WorkbenchTab } from "./WorkbenchApp";
import { DeferredView } from "../components/workspace/deferred-view";
import { Alert, AlertDescription } from "../components/ui/alert";
const EnterpriseAdminView = lazy(() =>
  import("../patterns/EnterpriseAdminView").then((module) => ({
    default: module.EnterpriseAdminView,
  })),
);
const PrimitivesTab = lazy(() =>
  import("./PrimitivesTab").then((module) => ({
    default: module.PrimitivesTab,
  })),
);

export interface WorkbenchTabViewsProps {
  activeTab: WorkbenchTab;
  liveMode: boolean;
  swarmTasks: AgentSandboxTask[];
  focusedSwarmIdx: number;
  onFocusedSwarmIdxChange: (idx: number) => void;
  onApproveMergeSwarmTask: (id: number) => void;
  onDiscardSwarmTask: (id: number) => void;
  onPublishSwarmTask: (id: number) => void;
  onVerifySwarmTask: (id: number, command: string) => void;
  onRefreshSwarmTasks: () => void;
  onViewRunHistory: (worktreeId: string) => void;
  onReviewSwarmTask?: (id: number) => void;
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
  worktreeReview?: LiveWorktreeReview | null;
  activeCommitRev: string;
  onSelectCommitDiff: (sha: string) => void;
  lookupMicros: number | null;
  selectedFilePath: string;
  diffLines: DiffLine[];
  liveDiffFiles: string[];
  diffTruncated: boolean;
  treeTruncated: boolean;
  onSelectCommitDiffFile: (path: string) => void;
  actionJobs: ActionJobStep[];
  activeJobId: string;
  onSelectJob: (id: string) => void;
  onRerunAllActionJobs: () => void;
  actionLogs: ActionLogLine[];
  enterpriseUsers: EnterpriseUser[];
  onToggleEnterpriseUserRole: (id: number) => void;
  onToggleEnterpriseUserStatus: (id: number) => void;
  onPolicyFeedback: (msg: string) => void;
  adminAuthProps?: Partial<EnterpriseAdminViewProps>;
  fileTreeNodes: FileTreeNode[];
  onSelectFileFromTree: (node: { path: string; kind: "file" | "dir" }) => void;
}

export const WorkbenchTabViews: Component<WorkbenchTabViewsProps> = (props) => (
  <>
    <Show when={props.activeTab === "swarm"}>
      <PageHeading
        title="Worktrees"
        description="A separate space for every agent. Verify changes, then merge with confidence."
      />
      <AgentSwarmView
        tasks={props.swarmTasks}
        focusedIndex={props.focusedSwarmIdx}
        onFocusIndexChange={props.onFocusedSwarmIdxChange}
        onApproveMerge={props.onApproveMergeSwarmTask}
        onDiscardBranch={props.onDiscardSwarmTask}
        onPublishGithub={props.onPublishSwarmTask}
        onVerifyWorktree={props.onVerifySwarmTask}
        onRefreshWorktrees={props.onRefreshSwarmTasks}
        onViewRunHistory={props.onViewRunHistory}
        onReviewWorktree={props.onReviewSwarmTask}
        canRunVerification={
          props.adminAuthProps?.liveSession?.role === "SiteAdmin"
        }
        liveMode={props.liveMode}
        onSpawnBatchSwarm={props.onSpawnBatchAgentSwarm}
        onTriggerGithubSync={props.onTriggerGithubSync}
        filterQuery={props.swarmFilter}
        onFilterQueryChange={props.onSwarmFilterChange}
      />
    </Show>
    <Show when={props.activeTab === "issues"}>
      <PageHeading
        title="Issues & pull requests"
        description="Sample issues and pull requests. Filter, triage and explore the interface."
        action={
          <Button onClick={props.onCreateOptimisticIssue}>
            <Icon name="plus" />
            New demo issue
          </Button>
        }
      />
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
    <Show when={props.activeTab === "diff"}>
      <PageHeading
        title="Changes"
        description={
          props.liveMode
            ? "Browse commits and inspect changes from the connected repository."
            : "Explore sample commits and a virtualized diff."
        }
      />
      <Show when={props.worktreeReview}>
        {(review) => (
          <Card class="min-w-0">
            <CardContent class="space-y-2 pt-4 text-xs">
              <p class="font-medium">Committed worktree changes</p>
              <p class="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                <span class="break-all">Head {review().branch}</span>
                <code class="inline-block max-w-full break-all">{review().headSha ?? "Unresolved"}</code>
              </p>
              <p class="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                <span class="break-all">Base {review().baseBranch || "Unknown merge target"}</span>
                <code class="inline-block max-w-full break-all">{review().baseSha ?? "Unresolved"}</code>
              </p>
              <p class="text-muted-foreground">
                {review().source === "verified" ? "Requested verified head snapshot. " : "Committed branch comparison. "}
                Comparing against the merge target at review time. Uncommitted files are not included.
              </p>
              <Show when={review().state === "loading"}>
                <p role="status">Resolving committed revisions...</p>
              </Show>
              <Show when={review().error}>
                {(error) => <p role="status" class="text-error-foreground">{error()}</p>}
              </Show>
            </CardContent>
          </Card>
        )}
      </Show>
      <div class="flex flex-wrap gap-2">
        <For each={props.liveCommits}>
          {(commit) => (
            <Button
              variant={
                props.activeCommitRev === commit.sha ||
                props.activeCommitRev === commit.short_sha
                  ? "secondary"
                  : "outline"
              }
              size="sm"
              title={commit.summary}
              onClick={() => props.onSelectCommitDiff(commit.sha)}
            >
              <Icon name="git-commit" />
              <span>{commit.short_sha}</span>
              <span class="hidden max-w-52 truncate sm:inline">
                {commit.summary}
              </span>
            </Button>
          )}
        </For>
        <Show when={props.lookupMicros !== null}>
          <StatusBadge tone="success">
            Lookup: {props.lookupMicros}µs
          </StatusBadge>
        </Show>
      </div>
      <Show when={props.diffTruncated}>
        <Alert>
          <AlertDescription>
            This diff preview is incomplete because the server limit was
            reached. Some changed files or lines may be missing.
          </AlertDescription>
        </Alert>
      </Show>
      <div
        class={
          props.liveDiffFiles.length > 1
            ? "grid min-w-0 gap-4 lg:grid-cols-[minmax(14rem,1fr)_minmax(0,2fr)]"
            : "min-w-0"
        }
      >
        <Show when={props.liveDiffFiles.length > 1}>
          <VirtualFileTree
            label="Changed files"
            files={props.liveDiffFiles.map((path) => ({
              path,
              kind: "file" as const,
            }))}
            activePath={props.selectedFilePath}
            onSelect={(file) => props.onSelectCommitDiffFile(file.path)}
            height={240}
          />
        </Show>
        <VirtualDiffViewer
          filePath={props.selectedFilePath}
          lines={props.diffLines}
          height={480}
        />
      </div>
    </Show>
    <Show when={props.activeTab === "actions"}>
      <PageHeading
        title="Actions"
        description="Interface demo with generated logs and simulated jobs. Replay the sample pipeline to explore its states."
      />
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
    <Show when={props.activeTab === "admin"}>
      <PageHeading
        title="Access & accounts"
        description="Manage your local account, invitations and Git credentials. Planned security controls are labelled as previews."
      />
      <DeferredView label="Access & accounts">
        <EnterpriseAdminView
          users={props.enterpriseUsers}
          onToggleRole={props.onToggleEnterpriseUserRole}
          onToggleStatus={props.onToggleEnterpriseUserStatus}
          onPolicyFeedback={props.onPolicyFeedback}
          {...(props.adminAuthProps ?? {})}
        />
      </DeferredView>
    </Show>
    <Show when={props.activeTab === "tree"}>
      <PageHeading
        title="Repository files"
        description={
          props.liveMode
            ? "Search paths and open files from the connected repository."
            : "Browse a sample repository file tree."
        }
      />
      <Show when={props.treeTruncated}>
        <Alert>
          <AlertDescription>
            This repository path list is incomplete because the server limit was
            reached. Search only covers the listed paths.
          </AlertDescription>
        </Alert>
      </Show>
      <Show when={props.diffTruncated}>
        <Alert>
          <AlertDescription>
            This diff preview is incomplete because the server limit was
            reached.
          </AlertDescription>
        </Alert>
      </Show>
      <div class="grid min-w-0 gap-4 xl:grid-cols-[minmax(14rem,1fr)_minmax(0,2fr)]">
        <VirtualFileTree
          files={props.fileTreeNodes}
          activePath={props.selectedFilePath}
          onSelect={props.onSelectFileFromTree}
          height={400}
        />
        <VirtualDiffViewer
          filePath={props.selectedFilePath}
          lines={props.diffLines}
          height={400}
        />
      </div>
    </Show>
    <Show when={props.activeTab === "primitives"}>
      <DeferredView label="Components">
        <PrimitivesTab />
      </DeferredView>
    </Show>
  </>
);
