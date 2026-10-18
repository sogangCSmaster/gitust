import {
  Component,
  createMemo,
  createEffect,
  createSignal,
  lazy,
  onCleanup,
  onMount,
  Show,
  Suspense,
} from "solid-js";
import { IconSpriteSheet } from "../components/workspace/icon";
import type {
  ActionJobStep,
  AgentSandboxTask,
  EnterpriseUser,
  IssueItem,
  LiveAuthSessionInfo,
  LiveInviteItem,
  LivePatItem,
  LiveSshKeyItem,
} from "../index";
import { filterAgentSwarmTasks } from "../patterns/worktree-filter";
import { filterIssueItems } from "../patterns/issue-filter";
import { createNewWorktreeController } from "./new-worktree-controller";
import { createPrimaryWorktreeActions } from "./primary-worktree-actions";
import { createLiveAuthSession } from "./live-auth-session";
import { createLiveRepoSession } from "./live-repo-session";
import { getLiveToken } from "./live-repo-context";
import { createLiveSwarmSession } from "./live-swarm-session";
import { INITIAL_AGENT_SWARM_TASKS } from "./mock-data";
import { registerWorkbenchShortcuts } from "./register-workbench-shortcuts";
import { createShowcaseDemoActions } from "./showcase-demo-actions";
import { WorkbenchHeader } from "./WorkbenchHeader";
import { ActiveChordHUD } from "../patterns/ActiveChordHUD";
import { Alert, AlertDescription } from "../components/ui/alert";
import { SidebarProvider, SidebarInset } from "../components/ui/sidebar";
import { WorkbenchNavigation, workbenchNavItems } from "./WorkbenchNavigation";
import { createWorkbenchRepositoryState } from "./workbench-repository-state";
import { readWorkbenchAccessLink } from "./workbench-access-link";
import {
  createRepositoryWorkspaceCore,
  isRepositoryTab,
} from "./repository-workspace-core";
import { RepositoryLanding, RepositoryOverview } from "./RepositoryViews";
import { RepositoryInitialCommit } from "./RepositoryInitialCommit";

const WorkbenchTabViews = lazy(() =>
  import("./WorkbenchTabViews").then((m) => ({ default: m.WorkbenchTabViews })),
);
const WorkbenchOverlays = lazy(() =>
  import("./WorkbenchOverlays").then((m) => ({ default: m.WorkbenchOverlays })),
);

export type WorkbenchTab =
  | "repositories"
  | "overview"
  | "swarm"
  | "issues"
  | "diff"
  | "actions"
  | "admin"
  | "tree"
  | "primitives";

export const WorkbenchApp: Component = () => {
  let accessLink = readWorkbenchAccessLink();
  const [liveMode, setLiveMode] = createSignal(Boolean(getLiveToken()));
  const [activeTab, setActiveTab] = createSignal<WorkbenchTab>(
    accessLink ? "admin" : "repositories",
  );
  const [cmdOpen, setCmdOpen] = createSignal(false);
  const [shortcutsModalOpen, setShortcutsModalOpen] = createSignal(false);
  const [swarmTasks, setSwarmTasks] = createSignal<AgentSandboxTask[]>(
    liveMode() ? [] : INITIAL_AGENT_SWARM_TASKS,
  );
  const [swarmFilter, setSwarmFilter] = createSignal("");
  const [focusedSwarmIdx, setFocusedSwarmIdx] = createSignal(0);
  const visibleSwarmTasks = createMemo(() =>
    filterAgentSwarmTasks(swarmTasks(), swarmFilter()),
  );
  const [issues, setIssues] = createSignal<IssueItem[]>([]);
  const [issueFilter, setIssueFilter] = createSignal("");
  const [focusedIssueIdx, setFocusedIssueIdx] = createSignal(0);
  const visibleIssues = createMemo(() =>
    filterIssueItems(issues(), issueFilter()),
  );
  const [optimisticToast, setOptimisticToast] = createSignal<string | null>(
    null,
  );
  let toastTimer: ReturnType<typeof setTimeout> | null = null;
  const [actionJobs, setActionJobs] = createSignal<ActionJobStep[]>([]);
  const [activeJobId, setActiveJobId] = createSignal("job-docker");
  const [enterpriseUsers, setEnterpriseUsers] = createSignal<EnterpriseUser[]>(
    [],
  );
  const [liveSession, setLiveSession] =
    createSignal<LiveAuthSessionInfo | null>(null);
  const [historyTask, setHistoryTask] = createSignal<AgentSandboxTask | null>(
    null,
  );
  const [redeemMode, setRedeemMode] = createSignal<"invite" | "reset">(
    accessLink?.mode ?? "invite",
  );
  const [redeemToken, setRedeemToken] = createSignal(accessLink?.token ?? "");
  const [liveInvites, setLiveInvites] = createSignal<LiveInviteItem[]>([]);
  const [lastIssuedInviteUrl, setLastIssuedInviteUrl] = createSignal<
    string | null
  >(null);
  const [livePats, setLivePats] = createSignal<LivePatItem[]>([]);
  const [lastIssuedPatToken, setLastIssuedPatToken] = createSignal<
    string | null
  >(null);
  const [liveSshKeys, setLiveSshKeys] = createSignal<LiveSshKeyItem[]>([]);
  let filterInputRef: HTMLInputElement | undefined;

  const {
    setActiveCommitRev,
    setLookupMicros,
    setSelectedFilePath,
    setLiveDiffLines,
    setSelectedTreePath,
    setLiveBlobLines,
    setLiveFileTreeNodes,
    setLiveCommits,
    activeCommitRev,
    setLiveDiffFiles,
    setDiffTruncated,
    setTreeTruncated,
    setWorktreeReview,
    liveCommits,
    worktreeReview,
    lookupMicros,
    selectedFilePath,
    diffLines,
    liveDiffFiles,
    diffTruncated,
    treeTruncated,
    fileTreeNodes,
    selectedTreePath,
    blobLines,
  } = createWorkbenchRepositoryState(liveMode);

  const showQuickFeedback = (msg: string) => {
    if (toastTimer) {
      clearTimeout(toastTimer);
    }
    setOptimisticToast(msg);
    toastTimer = setTimeout(() => {
      setOptimisticToast((cur) => (cur === msg ? null : cur));
      toastTimer = null;
    }, 2200);
  };

  const liveClient = createLiveRepoSession({
    getRepositoryId: () => connectedRepository.selected()?.id,
    getRepositoryHead: () => connectedRepository.selected()?.headCommit,
    onRepositoryChanged: () => connectedRepository.refreshViews(),
    canWorkbench: () => connectedRepository.canWorkbench(),
    onRepositoryUnavailable: (id) => connectedRepository.unavailable(id),
    setActiveCommitRev,
    setLookupMicros,
    setSelectedFilePath,
    setLiveDiffLines,
    setSelectedTreePath,
    setLiveBlobLines,
    setLiveFileTreeNodes,
    setLiveCommits,
    getActiveCommitRev: activeCommitRev,
    setLiveDiffFiles,
    setDiffTruncated,
    setTreeTruncated,
    setWorktreeReview,
    showQuickFeedback,
  });
  const liveSwarmClient = createLiveSwarmSession({
    getRepositoryId: () => connectedRepository.selected()?.id,
    getSwarmTasks: swarmTasks,
    setSwarmTasks,
    onRepoMutated: () => {
      if (!connectedRepository.canWorkbench()) return;
      void connectedRepository.refresh();
    },
    showQuickFeedback,
  });

  const clearRepositoryView = () => {
    newWorktree.reset();
    setHistoryTask(null);
    if (liveMode()) setSwarmTasks([]);
    liveClient.resetRepoEpoch();
    liveSwarmClient.resetSwarmEpoch();
    setSwarmFilter("");
    setFocusedSwarmIdx(0);
    if (!liveMode()) {
      setLiveDiffLines(null);
      setLiveFileTreeNodes(null);
    }
  };
  const connectedRepository = createRepositoryWorkspaceCore({
    liveMode,
    setActiveTab,
    clearView: clearRepositoryView,
    hydrate: () => {
      void liveClient.hydrateFromLiveBackend();
      if (connectedRepository.canWorkbench())
        void liveSwarmClient.hydrateSwarmWorktrees();
    },
  });
  const {
    state: repositoryState,
    selected: selectedRepository,
    open: openRepository,
    navigate,
  } = connectedRepository;

  const historyTarget = createMemo(() =>
    connectedRepository.canWorkbench() &&
    liveMode() &&
    liveSession()?.role === "SiteAdmin" &&
    swarmTasks().some((task) => task.liveId === historyTask()?.liveId)
      ? historyTask()
      : null,
  );
  createEffect(() => {
    if (
      !connectedRepository.canWorkbench() ||
      !liveMode() ||
      liveSession()?.role !== "SiteAdmin"
    )
      setHistoryTask(null);
  });
  const newWorktree = createNewWorktreeController({
    getRepository: () =>
      connectedRepository.canWorkbench() ? selectedRepository() : null,
    liveMode,
    sessionReady: () => Boolean(liveSession()),
    create: liveSwarmClient.spawnLiveWorktree,
    onCreated: (_id, worktreeId) => {
      setSwarmFilter("");
      queueMicrotask(() => {
        const index = swarmTasks().findIndex(
          (task) => task.liveId === worktreeId,
        );
        if (selectedRepository() && index >= 0) setFocusedSwarmIdx(index);
      });
    },
  });

  const liveAuthClient = createLiveAuthSession({
    setLiveSession,
    setEnterpriseUsers,
    setLiveInvites,
    setLastIssuedInviteUrl,
    setLivePats,
    setLastIssuedPatToken,
    setLiveSshKeys,
    setRedeemToken,
    onTokenAuthenticated: () => {
      accessLink = null;
      clearRepositoryView();
      connectedRepository.reset();
      setLiveMode(true);
      setOptimisticToast(null);
      setActiveCommitRev("HEAD");
      setSelectedFilePath("Repository preview");
      setEnterpriseUsers([]);
      setLiveCommits([]);
      setLiveDiffLines([]);
      setLiveFileTreeNodes([]);
      setLookupMicros(null);
      setSwarmTasks([]);
      void connectedRepository.refresh();
    },
    onTokenCleared: () => {
      clearRepositoryView();
      connectedRepository.reset();
      if (accessLink) {
        setActiveTab("admin");
        setRedeemToken(accessLink.token);
      }
      setLiveMode(false);
      setOptimisticToast(null);
      setActiveCommitRev("HEAD");
      setSelectedFilePath("crates/gitust-git/src/browse.rs");
      setLiveCommits([]);
      setLookupMicros(null);
      liveSwarmClient.resetSwarmEpoch();
      setLiveDiffLines(null);
      setLiveFileTreeNodes(null);
      setSwarmTasks(INITIAL_AGENT_SWARM_TASKS);
    },
    showQuickFeedback,
  });

  const demoActions = createShowcaseDemoActions({
    getSwarmTasks: swarmTasks,
    setSwarmTasks,
    getIssues: issues,
    setIssues,
    setActiveTab: navigate,
    setFocusedIssueIdx,
    setActionJobs,
    showQuickFeedback,
  });

  const {
    approveAndMergeSwarmTask,
    publishSwarmTask,
    discardSwarmTask,
    spawnBatchAgentSwarm,
    reviewSwarmTask,
  } = createPrimaryWorktreeActions({
    canWorkbench: connectedRepository.canWorkbench,
    activeTab,
    liveMode,
    sessionReady: () => Boolean(liveSession()),
    feedback: showQuickFeedback,
    liveApprove: liveSwarmClient.approveLiveWorktree,
    liveDiscard: liveSwarmClient.discardLiveWorktree,
    livePublish: liveSwarmClient.publishLiveWorktree,
    demoApprove: demoActions.approveDemoSwarmTask,
    demoDiscard: demoActions.discardDemoSwarmTask,
    demoSpawn: demoActions.spawnDemoBatchSwarm,
    openNewWorktree: newWorktree.open,
    tasks: swarmTasks,
    openChanges: () => setActiveTab("diff"),
    loadWorktreeDiff: liveClient.loadLiveWorktreeDiff,
  });

  onMount(() => {
    const sessionChanged = (event: StorageEvent) => {
      if (event.key === "gitust_token" || event.key === null)
        void liveAuthClient.syncStoredSession();
    };
    window.addEventListener("storage", sessionChanged);
    void liveAuthClient.hydrateAuthData();
    void connectedRepository.refresh();
    const cleanups = registerWorkbenchShortcuts({
      isBrowseOnly: connectedRepository.browseOnly,
      getActiveTab: activeTab,
      setActiveTab: navigate,
      toggleCommandPalette: () => setCmdOpen((v) => !v),
      toggleShortcutsModal: () => setShortcutsModalOpen((v) => !v),
      getVisibleSwarmTasks: visibleSwarmTasks,
      getFocusedSwarmIdx: focusedSwarmIdx,
      setFocusedSwarmIdx,
      getVisibleIssues: visibleIssues,
      getFocusedIssueIdx: focusedIssueIdx,
      setFocusedIssueIdx,
      approveAndMergeSwarmTask,
      discardSwarmTask,
      spawnBatchAgentSwarm,
      toggleIssueStatus: demoActions.toggleIssueStatus,
      createOptimisticIssue: demoActions.createOptimisticIssue,
      rerunAllActionJobs: () => {
        if (!liveMode() && !getLiveToken()) demoActions.rerunAllActionJobs();
      },
      focusFilterInput: () => queueMicrotask(() => filterInputRef?.focus()),
      showQuickFeedback,
    });

    onCleanup(() => {
      window.removeEventListener("storage", sessionChanged);
      connectedRepository.reset();
      clearRepositoryView();
      if (toastTimer) clearTimeout(toastTimer);
      cleanups.forEach((fn) => fn());
    });
  });

  const tabs = createMemo(() =>
    workbenchNavItems(liveMode(), swarmTasks().length),
  );

  return (
    <SidebarProvider>
      <IconSpriteSheet />
      <WorkbenchNavigation
        items={tabs()}
        active={activeTab()}
        onSelect={navigate}
        repositoryName={selectedRepository()?.name}
        browseOnly={connectedRepository.browseOnly()}
        canActions={connectedRepository.canViewActions()}
        liveMode={liveMode()}
        username={liveSession()?.username}
      />
      <SidebarInset class="min-w-0">
        <WorkbenchHeader
          title={
            tabs().find((item) => item.id === activeTab())?.label ??
            "Repositories"
          }
          repositoryName={
            isRepositoryTab(activeTab(), liveMode())
              ? selectedRepository()?.name
              : undefined
          }
          onBackToRepositories={() => navigate("repositories")}
          onOpenCommandPalette={() => setCmdOpen(true)}
          onOpenShortcutsModal={() => setShortcutsModalOpen(true)}
        />
        <main class="mx-auto w-full max-w-7xl min-w-0 space-y-4 p-3 sm:p-5 lg:p-6">
          <div class="flex min-w-0 items-center gap-2 text-[11px] text-muted-foreground">
            <span class="size-1.5 shrink-0 rounded-full bg-primary" />
            <span>{liveMode() ? "Live workspace" : "Demo workspace"}</span>
            <Show when={!liveMode()}>
              <span class="truncate">Sample data</span>
            </Show>
          </div>
          <Show when={activeTab() === "repositories"}>
            <RepositoryLanding
              state={repositoryState()}
              liveMode={liveMode()}
              onOpen={openRepository}
              onRefresh={() => void connectedRepository.refresh()}
              onSignIn={() => navigate("admin")}
            />
          </Show>
          <Show when={activeTab() === "overview" && selectedRepository()}>
            {(repo) => (
              <RepositoryOverview
                repository={repo()}
                liveMode={liveMode()}
                worktrees={swarmTasks().length}
                onNavigate={navigate}
                actions={connectedRepository}
              />
            )}
          </Show>
          <Show when={optimisticToast()}>
            {(message) => (
              <Alert>
                <AlertDescription>{message()}</AlertDescription>
              </Alert>
            )}
          </Show>

          <Show
            when={
              activeTab() !== "repositories" && activeTab() !== "overview"
            }
          >
            <Suspense
              fallback={
                <Show
                  when={
                    (activeTab() === "diff" || activeTab() === "tree") &&
                    liveMode() &&
                    selectedRepository() &&
                    selectedRepository()?.headCommit === null &&
                    !liveClient.branches.state().selected &&
                    !(activeTab() === "diff" && worktreeReview())
                  }
                >
                  <RepositoryInitialCommit
                    branch={selectedRepository()?.branch ?? ""}
                    onRefresh={() => {
                      void connectedRepository.refreshViews();
                      if (connectedRepository.canWorkbench())
                        void liveSwarmClient.hydrateSwarmWorktrees();
                    }}
                  />
                </Show>
              }
            >
              <WorkbenchTabViews
                branches={liveClient.branches}
                repoSession={liveClient}
                swarmSession={liveSwarmClient}
                repository={selectedRepository()}
                repositoryActions={connectedRepository.actions}
                canWorkbench={connectedRepository.canWorkbench()}
                activeTab={activeTab()}
                liveMode={liveMode()}
                swarmTasks={swarmTasks()}
                focusedSwarmIdx={focusedSwarmIdx()}
                onFocusedSwarmIdxChange={setFocusedSwarmIdx}
                onApproveMergeSwarmTask={approveAndMergeSwarmTask}
                onDiscardSwarmTask={discardSwarmTask}
                onPublishSwarmTask={publishSwarmTask}
                onReviewSwarmTask={reviewSwarmTask}
                onRefreshSwarmTasks={() => {
                  void connectedRepository.refreshViews();
                  if (connectedRepository.canWorkbench())
                    void liveSwarmClient.hydrateSwarmWorktrees();
                }}
                onViewRunHistory={(id) => {
                  const task = swarmTasks().find((task) => task.liveId === id);
                  if (
                    task &&
                    connectedRepository.canWorkbench() &&
                    liveMode() &&
                    liveSession()?.role === "SiteAdmin"
                  )
                    setHistoryTask(task);
                }}
                onSpawnBatchAgentSwarm={spawnBatchAgentSwarm}
                swarmFilter={swarmFilter()}
                onSwarmFilterChange={setSwarmFilter}
                issues={issues()}
                focusedIssueIdx={focusedIssueIdx()}
                onFocusedIssueIdxChange={setFocusedIssueIdx}
                onToggleIssueStatus={demoActions.toggleIssueStatus}
                onCreateOptimisticIssue={demoActions.createOptimisticIssue}
                filterRef={(el) => (filterInputRef = el)}
                issueFilter={issueFilter()}
                onIssueFilterChange={setIssueFilter}
                liveCommits={liveCommits()}
                worktreeReview={worktreeReview()}
                activeCommitRev={activeCommitRev()}
                onSelectCommitDiff={(sha) =>
                  void liveClient.loadLiveCommitDiff(sha)
                }
                lookupMicros={lookupMicros()}
                selectedFilePath={selectedFilePath()}
                diffLines={diffLines()}
                selectedTreePath={selectedTreePath()}
                blobLines={blobLines()}
                liveDiffFiles={liveDiffFiles()}
                diffTruncated={diffTruncated()}
                treeTruncated={treeTruncated()}
                actionJobs={actionJobs()}
                activeJobId={activeJobId()}
                onSelectJob={setActiveJobId}
                onRerunAllActionJobs={demoActions.rerunAllActionJobs}
                enterpriseUsers={enterpriseUsers()}
                setEnterpriseUsers={setEnterpriseUsers}
                onPolicyFeedback={showQuickFeedback}
                adminAuthSource={{
                  liveMode: liveMode(),
                  liveSession: liveSession(),
                  redeemMode: redeemMode(),
                  setRedeemMode,
                  redeemToken: redeemToken(),
                  setRedeemToken,
                  liveInvites: liveInvites(),
                  lastIssuedInviteUrl: lastIssuedInviteUrl(),
                  livePats: livePats(),
                  lastIssuedPatToken: lastIssuedPatToken(),
                  liveSshKeys: liveSshKeys(),
                  session: liveAuthClient,
                }}
                fileTreeNodes={fileTreeNodes()}
                onSeedDemoData={(data) => {
                  if (data.issues && issues().length === 0)
                    setIssues(data.issues);
                  if (data.actionJobs && actionJobs().length === 0)
                    setActionJobs(data.actionJobs);
                }}
              />
            </Suspense>
          </Show>
        </main>
      </SidebarInset>
      <Show
        when={Boolean(
          newWorktree.target() ||
            historyTarget() ||
            cmdOpen() ||
            shortcutsModalOpen(),
        )}
      >
        <Suspense>
          <WorkbenchOverlays
            newWorktree={newWorktree}
            onRefreshWorktrees={() => {
              if (connectedRepository.canWorkbench())
                void liveSwarmClient.hydrateSwarmWorktrees();
            }}
            historyTarget={historyTarget()}
            onCloseHistory={() => setHistoryTask(null)}
            cmdOpen={cmdOpen()}
            onCloseCmd={() => setCmdOpen(false)}
            shortcutsModalOpen={shortcutsModalOpen()}
            onCloseShortcutsModal={() => setShortcutsModalOpen(false)}
          />
        </Suspense>
      </Show>
      <ActiveChordHUD />
    </SidebarProvider>
  );
};
