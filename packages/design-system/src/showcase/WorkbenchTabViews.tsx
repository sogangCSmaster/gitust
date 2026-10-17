import { Show, lazy, type Component } from "solid-js";
import {
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
import { PageHeading } from "../components/workspace/presentation";
import { Icon } from "../components/workspace/icon";
import type { LiveCommitSummary, LiveWorktreeReview } from "./live-repo-client";
import type { WorkbenchTab } from "./WorkbenchApp";
import { DeferredView } from "../components/workspace/deferred-view";
import { Alert, AlertDescription } from "../components/ui/alert";
const AgentSwarmView = lazy(() =>
  import("../patterns/AgentSwarmView").then((module) => ({
    default: module.AgentSwarmView,
  })),
);
const IssueListView = lazy(() =>
  import("../patterns/IssueListView").then((module) => ({
    default: module.IssueListView,
  })),
);
const ActionsPipelineView = lazy(() =>
  import("../patterns/ActionsPipelineView").then((module) => ({
    default: module.ActionsPipelineView,
  })),
);
const LiveRepositoryActionsView = lazy(() =>
  import("./LiveRepositoryActionsView").then((module) => ({
    default: module.LiveRepositoryActionsView,
  })),
);
const RepositoryChangesView = lazy(() =>
  import("./RepositoryChangesView").then((module) => ({
    default: module.RepositoryChangesView,
  })),
);
const RepositoryFilesView = lazy(() =>
  import("./RepositoryFilesView").then((module) => ({
    default: module.RepositoryFilesView,
  })),
);
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

import type { ConnectedRepository } from "./connected-repository-client";
const RepositoryBranchSelect = lazy(() =>
  import("./RepositoryBranchSelect").then((module) => ({
    default: module.RepositoryBranchSelect,
  })),
);
import type { RepositoryBranches } from "./live-branches";
import { RepositoryInitialCommit } from "./RepositoryInitialCommit";

export interface WorkbenchTabViewsProps {
  branches?: RepositoryBranches;
  activeTab: WorkbenchTab;
  liveMode: boolean;
  repository?: ConnectedRepository | null;
  repositoryEpoch?: number;
  canWorkbench?: boolean;
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
  selectedTreePath: string;
  blobLines: DiffLine[];
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
    <Show when={props.activeTab === "swarm" && props.canWorkbench !== false}>
      <PageHeading
        title="Worktrees"
        description="A separate space for every agent. Verify changes, then merge with confidence."
      />
      <DeferredView label="Worktrees">
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
      </DeferredView>
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
      <DeferredView label="Issues demo">
        <IssueListView
          items={props.issues}
          focusedIndex={props.focusedIssueIdx}
          onFocusIndexChange={props.onFocusedIssueIdxChange}
          onToggleStatus={props.onToggleIssueStatus}
          filterRef={props.filterRef}
          filterQuery={props.issueFilter}
          onFilterQueryChange={props.onIssueFilterChange}
        />
      </DeferredView>
    </Show>
    <Show
      when={
        props.activeTab === "diff" && (!props.liveMode || props.repository?.id)
      }
    >
      <PageHeading
        action={
          <Show when={props.liveMode && props.repository && props.branches}>
            <DeferredView label="Branches">
              <RepositoryBranchSelect
                repository={props.repository!}
                branches={props.branches!}
              />
            </DeferredView>
          </Show>
        }
        title="Changes"
        description={
          props.liveMode
            ? "Browse commits and inspect changes from the selected repository."
            : "Explore sample commits and a virtualized diff."
        }
      />
      <Show
        when={
          !props.liveMode ||
          props.repository?.headCommit !== null ||
          props.branches?.state().selected ||
          props.worktreeReview
        }
        fallback={
          <RepositoryInitialCommit
            branch={props.repository?.branch ?? ""}
            onRefresh={props.onRefreshSwarmTasks}
          />
        }
      >
        <DeferredView label="Changes">
          <RepositoryChangesView
            liveMode={props.liveMode}
            commits={props.liveCommits}
            activeCommitRev={props.activeCommitRev}
            onSelectCommit={props.onSelectCommitDiff}
            lookupMicros={props.lookupMicros}
            review={props.worktreeReview}
            selectedFilePath={props.selectedFilePath}
            lines={props.diffLines}
            files={props.liveDiffFiles}
            truncated={props.diffTruncated}
            onSelectFile={props.onSelectCommitDiffFile}
          />
        </DeferredView>
      </Show>
    </Show>
    <Show when={props.activeTab === "actions" && props.canWorkbench !== false}>
      <PageHeading
        title="Actions"
        description={
          props.liveMode
            ? "Read-only Checks diagnostics for this repository."
            : "Interface demo with generated logs and simulated jobs. Replay the sample pipeline to explore its states."
        }
      />
      <Show
        when={props.liveMode}
        fallback={
          <DeferredView label="Actions demo">
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
          </DeferredView>
        }
      >
        <DeferredView label="Repository Actions">
          <LiveRepositoryActionsView
            repositoryId={props.repository?.id}
            role={props.adminAuthProps?.liveSession?.role}
            isPrimary={props.canWorkbench === true}
            selectionEpoch={props.repositoryEpoch ?? 0}
          />
        </DeferredView>
      </Show>
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
    <Show
      when={
        props.activeTab === "tree" && (!props.liveMode || props.repository?.id)
      }
    >
      <PageHeading
        action={
          <Show when={props.liveMode && props.repository && props.branches}>
            <DeferredView label="Branches">
              <RepositoryBranchSelect
                repository={props.repository!}
                branches={props.branches!}
              />
            </DeferredView>
          </Show>
        }
        title="Repository files"
        description={
          props.liveMode
            ? "Search paths and open files from the selected repository."
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
      <Show
        when={
          !props.liveMode ||
          props.repository?.headCommit !== null ||
          props.branches?.state().selected
        }
        fallback={
          <RepositoryInitialCommit
            branch={props.repository?.branch ?? ""}
            onRefresh={props.onRefreshSwarmTasks}
          />
        }
      >
        <DeferredView label="Repository files">
          <RepositoryFilesView
            files={props.fileTreeNodes}
            selectedFilePath={props.selectedTreePath}
            lines={props.blobLines}
            onSelect={props.onSelectFileFromTree}
          />
        </DeferredView>
      </Show>
    </Show>
    <Show when={props.activeTab === "primitives"}>
      <DeferredView label="Components">
        <PrimitivesTab />
      </DeferredView>
    </Show>
  </>
);
