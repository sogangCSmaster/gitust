import { Show, createMemo, lazy, type Component } from "solid-js";
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
import { PageHeading } from "../components/workspace/page-heading";
import { Icon } from "../components/workspace/icon";
import type {
  LiveCommitSummary,
  LiveRepoSessionController,
  LiveWorktreeReview,
} from "./live-repo-session";
import type { LiveSwarmSessionController } from "./live-swarm-session";
import type { WorkbenchTab } from "./WorkbenchApp";
import { DeferredView } from "../components/workspace/deferred-view";
import { Alert, AlertDescription } from "../components/ui/alert";
import {
  generateActionLogs,
  generateLargeDiff,
  generateMonorepoFileTree,
  INITIAL_ACTION_JOBS,
  INITIAL_ENTERPRISE_USERS,
  INITIAL_ISSUES,
} from "./mock-generators";
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

import type { RepositoryActionsContext } from "./repository-workspace-core";
import type { ConnectedRepository } from "./connected-repository-session";
const RepositoryBranchSelect = lazy(() =>
  import("./RepositoryBranchSelect").then((module) => ({
    default: module.RepositoryBranchSelect,
  })),
);
import type { RepositoryBranches } from "./live-branches";
import {
  buildAdminAuthProps,
  type AdminAuthPropsSource,
} from "./live-auth-client";
import { extendLiveRepoClient } from "./live-repo-client";
import { extendLiveSwarmClient } from "./live-swarm-client";
import { RepositoryInitialCommit } from "./RepositoryInitialCommit";

export interface WorkbenchTabViewsProps {
  branches?: RepositoryBranches;
  repoSession?: LiveRepoSessionController;
  swarmSession?: LiveSwarmSessionController;
  activeTab: WorkbenchTab;
  liveMode: boolean;
  repository?: ConnectedRepository | null;
  repositoryEpoch?: number;
  repositoryActions?: RepositoryActionsContext;
  canWorkbench?: boolean;
  swarmTasks: AgentSandboxTask[];
  focusedSwarmIdx: number;
  onFocusedSwarmIdxChange: (idx: number) => void;
  onApproveMergeSwarmTask: (id: number) => void;
  onDiscardSwarmTask: (id: number) => void;
  onPublishSwarmTask: (id: number) => void;
  onVerifySwarmTask?: (id: number, command: string) => void;
  onRefreshSwarmTasks: () => void;
  onViewRunHistory: (worktreeId: string) => void;
  onReviewSwarmTask?: (id: number) => void;
  onSpawnBatchAgentSwarm: () => void;
  onTriggerGithubSync?: () => void;
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
  diffLines: DiffLine[] | null;
  selectedTreePath: string;
  blobLines: DiffLine[];
  liveDiffFiles: string[];
  diffTruncated: boolean;
  treeTruncated: boolean;
  onSelectCommitDiffFile?: (path: string) => void;
  actionJobs: ActionJobStep[];
  activeJobId: string;
  onSelectJob: (id: string) => void;
  onRerunAllActionJobs: () => void;
  actionLogs?: ActionLogLine[];
  enterpriseUsers: EnterpriseUser[];
  setEnterpriseUsers?: (
    updater: EnterpriseUser[] | ((prev: EnterpriseUser[]) => EnterpriseUser[]),
  ) => void;
  onToggleEnterpriseUserRole?: (id: number) => void;
  onToggleEnterpriseUserStatus?: (id: number) => void;
  onPolicyFeedback: (msg: string) => void;
  adminAuthProps?: Partial<EnterpriseAdminViewProps>;
  adminAuthSource?: AdminAuthPropsSource;
  fileTreeNodes: FileTreeNode[] | null;
  onSelectFileFromTree?: (node: { path: string; kind: "file" | "dir" }) => void;
  onSeedDemoData?: (data: {
    issues?: IssueItem[];
    actionJobs?: ActionJobStep[];
  }) => void;
}

export const WorkbenchTabViews: Component<WorkbenchTabViewsProps> = (props) => {
  const repoClient = createMemo(() =>
    props.repoSession ? extendLiveRepoClient(props.repoSession) : undefined,
  );
  const swarmClient = createMemo(() =>
    props.swarmSession ? extendLiveSwarmClient(props.swarmSession) : undefined,
  );
  const adminAuthProps = createMemo(
    () =>
      props.adminAuthProps ??
      (props.adminAuthSource
        ? buildAdminAuthProps(props.adminAuthSource)
        : undefined),
  );
  const diffLines = createMemo(
    () =>
      props.diffLines ??
      (!props.liveMode && props.activeTab === "diff"
        ? generateLargeDiff(10000)
        : []),
  );
  const fileTreeNodes = createMemo(
    () =>
      props.fileTreeNodes ??
      (!props.liveMode && props.activeTab === "tree"
        ? generateMonorepoFileTree(5000)
        : []),
  );
  const actionLogs = createMemo(
    () =>
      props.actionLogs ??
      (!props.liveMode && props.activeTab === "actions"
        ? generateActionLogs(10000)
        : []),
  );
  const demoIssues = createMemo(() =>
    !props.liveMode && props.issues.length === 0
      ? INITIAL_ISSUES
      : props.issues,
  );
  const demoActionJobs = createMemo(() =>
    !props.liveMode && props.actionJobs.length === 0
      ? INITIAL_ACTION_JOBS
      : props.actionJobs,
  );
  const demoEnterpriseUsers = createMemo(() =>
    !props.liveMode && props.enterpriseUsers.length === 0
      ? INITIAL_ENTERPRISE_USERS
      : props.enterpriseUsers,
  );
  const toggleEnterpriseUserRole = (userId: number) => {
    if (props.onToggleEnterpriseUserRole) {
      props.onToggleEnterpriseUserRole(userId);
      return;
    }
    const roles: EnterpriseUser["role"][] = [
      "Developer",
      "Maintainer",
      "Site Admin",
    ];
    props.setEnterpriseUsers?.((prev) =>
      (prev.length ? prev : INITIAL_ENTERPRISE_USERS).map((u) => {
        if (u.id !== userId) return u;
        const nextRole =
          roles[(roles.indexOf(u.role) + 1) % roles.length] ?? "Developer";
        props.onPolicyFeedback(
          `Demo role changed: @${u.username} -> ${nextRole}`,
        );
        return { ...u, role: nextRole };
      }),
    );
  };
  const toggleEnterpriseUserStatus = (userId: number) => {
    if (props.onToggleEnterpriseUserStatus) {
      props.onToggleEnterpriseUserStatus(userId);
      return;
    }
    props.setEnterpriseUsers?.((prev) =>
      (prev.length ? prev : INITIAL_ENTERPRISE_USERS).map((u) => {
        if (u.id !== userId) return u;
        const nextStatus = u.status === "active" ? "suspended" : "active";
        props.onPolicyFeedback(
          `Demo account status changed: @${u.username} -> ${nextStatus.toUpperCase()}`,
        );
        return { ...u, status: nextStatus };
      }),
    );
  };
  if (
    !props.liveMode &&
    (props.issues.length === 0 || props.actionJobs.length === 0)
  ) {
    props.onSeedDemoData?.({
      issues: INITIAL_ISSUES,
      actionJobs: INITIAL_ACTION_JOBS,
    });
  }
  return (
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
            onVerifyWorktree={(id, command) => {
              if (props.onVerifySwarmTask) props.onVerifySwarmTask(id, command);
              else if (props.canWorkbench && props.activeTab === "swarm")
                void swarmClient()?.verifyLiveWorktree(id, command);
            }}
            onRefreshWorktrees={props.onRefreshSwarmTasks}
            onViewRunHistory={props.onViewRunHistory}
            onReviewWorktree={props.onReviewSwarmTask}
            canRunVerification={
              adminAuthProps()?.liveSession?.role === "SiteAdmin"
            }
            liveMode={props.liveMode}
            onSpawnBatchSwarm={props.onSpawnBatchAgentSwarm}
            onTriggerGithubSync={() =>
              props.onTriggerGithubSync
                ? props.onTriggerGithubSync()
                : void repoClient()?.triggerGithubSync()
            }
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
            items={demoIssues()}
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
              repositoryId={props.repository?.id}
              defaultBranch={props.repository?.branch}
              selectedBranch={props.branches?.state().selected?.name}
              selectionEpoch={
                props.repositoryActions?.selectionEpoch() ??
                props.repositoryEpoch ??
                0
              }
              commits={props.liveCommits}
              activeCommitRev={props.activeCommitRev}
              onSelectCommit={props.onSelectCommitDiff}
              lookupMicros={props.lookupMicros}
              review={props.worktreeReview}
              selectedFilePath={props.selectedFilePath}
              lines={diffLines()}
              files={props.liveDiffFiles}
              truncated={props.diffTruncated}
              onSelectFile={(path) =>
                props.onSelectCommitDiffFile
                  ? props.onSelectCommitDiffFile(path)
                  : void repoClient()?.selectCommitDiffFile(path)
              }
            />
          </DeferredView>
        </Show>
      </Show>
      <Show
        when={
          props.activeTab === "actions" &&
          (props.canWorkbench !== false ||
            Boolean(
              props.repository?.canViewLocalChecksAutomatic ||
                props.repository?.canRunLocalChecks,
            ))
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
          title="Actions"
          description={
            props.liveMode
              ? "Checks diagnostics for this repository."
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
                jobs={demoActionJobs()}
                activeJobId={props.activeJobId}
                onSelectJob={props.onSelectJob}
                onRerunAll={props.onRerunAllActionJobs}
                logs={actionLogs()}
                height={440}
              />
            </DeferredView>
          }
        >
          <DeferredView label="Repository Actions">
            <LiveRepositoryActionsView
              repositoryId={props.repository?.id}
              role={adminAuthProps()?.liveSession?.role}
              isPrimary={props.canWorkbench === true}
              selectionEpoch={
                props.repositoryActions?.selectionEpoch() ??
                props.repositoryEpoch ??
                0
              }
              canRunChecks={props.repository?.canRunLocalChecks === true}
              canViewLocalChecksAutomatic={
                props.repository?.canViewLocalChecksAutomatic === true
              }
              selectedBranch={props.branches?.state().selected?.name}
              defaultRef={
                props.branches?.state().selected?.ref_name ??
                (props.repository
                  ? `refs/heads/${props.repository.branch}`
                  : undefined)
              }
              defaultCommit={
                props.branches?.state().selected?.commit_sha ??
                props.repository?.headCommit
              }
              refreshRepository={props.repositoryActions?.refresh}
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
            users={demoEnterpriseUsers()}
            onToggleRole={toggleEnterpriseUserRole}
            onToggleStatus={toggleEnterpriseUserStatus}
            onPolicyFeedback={props.onPolicyFeedback}
            {...(adminAuthProps() ?? {})}
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
              This repository path list is incomplete because the server limit
              was reached. Search only covers the listed paths.
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
              files={fileTreeNodes()}
              selectedFilePath={props.selectedTreePath}
              lines={props.blobLines}
              liveMode={props.liveMode}
              repositoryId={props.repository?.id}
              revision={
                props.branches?.state().selected?.name ??
                props.repository?.branch
              }
              onSelect={(node) =>
                props.onSelectFileFromTree
                  ? props.onSelectFileFromTree(node)
                  : void repoClient()?.selectFileFromTree(node)
              }
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
};
