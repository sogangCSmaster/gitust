import {
  Component,
  createMemo,
  createEffect,
  createSignal,
  onCleanup,
  onMount,
  Show,
} from "solid-js";
import {
  ActionJobStep,
  ActiveChordHUD,
  Button,
  AgentSandboxTask,
  CommandPalette,
  EnterpriseUser,
  Icon,
  IconSpriteSheet,
  IssueItem,
  LiveAuthSessionInfo,
  LiveInviteItem,
  LivePatItem,
  LiveSshKeyItem,
} from "../index";
import { filterAgentSwarmTasks } from "../patterns/worktree-filter";
import { filterIssueItems } from "../patterns/issue-filter";
import { NewWorktreeDialog } from "./NewWorktreeDialog";
import { createNewWorktreeController } from "./new-worktree-controller";
import { createLiveAuthClient } from "./live-auth-client";
import { createLiveRepoClient, getLiveToken } from "./live-repo-client";
import { createLiveSwarmClient } from "./live-swarm-client";
import {
  generateActionLogs,
  INITIAL_ACTION_JOBS,
  INITIAL_AGENT_SWARM_TASKS,
  INITIAL_ENTERPRISE_USERS,
  INITIAL_ISSUES,
} from "./mock-data";
import { registerWorkbenchShortcuts } from "./register-workbench-shortcuts";
import { createShowcaseDemoActions } from "./showcase-demo-actions";
import { ShortcutsReferenceModal, WorkbenchHeader } from "./WorkbenchHeader";
import { WorkbenchTabViews } from "./WorkbenchTabViews";
import { LiveRunHistoryDialog } from "./LiveRunHistoryDialog";
import { Alert, AlertDescription } from "../components/ui/alert";
import { SidebarProvider, SidebarInset } from "../components/ui/sidebar";
import { WorkbenchNavigation, workbenchNavItems } from "./WorkbenchNavigation";
import { createWorkbenchRepositoryState } from "./workbench-repository-state";
import { readWorkbenchAccessLink } from "./workbench-access-link";
import {
  createRepositoryWorkspace,
  isRepositoryTab,
} from "./repository-workspace";
import { RepositoryLanding, RepositoryOverview } from "./RepositoryViews";

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

  const [issues, setIssues] = createSignal<IssueItem[]>(INITIAL_ISSUES);
  const [issueFilter, setIssueFilter] = createSignal("");
  const [focusedIssueIdx, setFocusedIssueIdx] = createSignal(0);
  const visibleIssues = createMemo(() =>
    filterIssueItems(issues(), issueFilter()),
  );

  const [optimisticToast, setOptimisticToast] = createSignal<string | null>(
    null,
  );
  let toastTimer: ReturnType<typeof setTimeout> | null = null;

  const [actionJobs, setActionJobs] =
    createSignal<ActionJobStep[]>(INITIAL_ACTION_JOBS);
  const [activeJobId, setActiveJobId] = createSignal("job-docker");
  const actionLogs = createMemo(() =>
    !liveMode() && activeTab() === "actions" ? generateActionLogs(10000) : [],
  );

  const [enterpriseUsers, setEnterpriseUsers] = createSignal<EnterpriseUser[]>(
    liveMode() ? [] : INITIAL_ENTERPRISE_USERS,
  );
  const [liveSession, setLiveSession] =
    createSignal<LiveAuthSessionInfo | null>(null);
  const [historyTask, setHistoryTask] = createSignal<AgentSandboxTask | null>(
    null,
  );
  const historyTarget = createMemo(() =>
    liveMode() &&
    liveSession()?.role === "SiteAdmin" &&
    swarmTasks().some((task) => task.liveId === historyTask()?.liveId)
      ? historyTask()
      : null,
  );
  createEffect(() => {
    if (!liveMode() || liveSession()?.role !== "SiteAdmin")
      setHistoryTask(null);
  });
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
  } = createWorkbenchRepositoryState(liveMode, activeTab);

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

  const liveClient = createLiveRepoClient({
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

  const liveSwarmClient = createLiveSwarmClient({
    getSwarmTasks: swarmTasks,
    setSwarmTasks,
    onRepoMutated: () => {
      void liveClient.hydrateFromLiveBackend();
      void connectedRepository.refresh();
    },
    showQuickFeedback,
  });

  const clearRepositoryView = () => {
    newWorktree.reset();
    setHistoryTask(null);
    liveClient.resetRepoEpoch();
    liveSwarmClient.resetSwarmEpoch();
    setSwarmFilter("");
    setFocusedSwarmIdx(0);
    if (!liveMode()) {
      setLiveDiffLines(null);
      setLiveFileTreeNodes(null);
    }
  };
  const connectedRepository = createRepositoryWorkspace({
    liveMode,
    setActiveTab,
    clearView: clearRepositoryView,
    hydrate: () => {
      setSwarmTasks([]);
      void liveClient.hydrateFromLiveBackend();
      void liveSwarmClient.hydrateSwarmWorktrees();
    },
  });
  const {
    state: repositoryState,
    selected: selectedRepository,
    open: openRepository,
    navigate,
  } = connectedRepository;

  const newWorktree = createNewWorktreeController({
    getRepository: selectedRepository,
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

  const liveAuthClient = createLiveAuthClient({
    setLiveSession,
    setEnterpriseUsers,
    setLiveInvites,
    setLastIssuedInviteUrl,
    setLivePats,
    setLastIssuedPatToken,
    setLiveSshKeys,
    setRedeemToken,
    fallbackUsers: INITIAL_ENTERPRISE_USERS,
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
    setEnterpriseUsers,
    showQuickFeedback,
  });

  const approveAndMergeSwarmTask = (id: number) => {
    if (!selectedRepository() || activeTab() !== "swarm") return;
    if (liveMode() || getLiveToken()) {
      void liveSwarmClient.approveLiveWorktree(id);
    } else {
      demoActions.approveDemoSwarmTask(id);
    }
  };

  const publishSwarmTask = (id: number) => {
    if (!selectedRepository() || activeTab() !== "swarm") return;
    void liveSwarmClient.publishLiveWorktree(id);
  };

  const discardSwarmTask = (id: number) => {
    if (!selectedRepository() || activeTab() !== "swarm") return;
    if (liveMode() || getLiveToken()) {
      void liveSwarmClient.discardLiveWorktree(id);
    } else {
      demoActions.discardDemoSwarmTask(id);
    }
  };

  const spawnBatchAgentSwarm = () => {
    if (!selectedRepository() || activeTab() !== "swarm") return;
    if (liveMode() || getLiveToken()) {
      if (!liveSession()) {
        showQuickFeedback(
          "Your account is still loading. Try again when it is ready.",
        );
        return;
      }
      newWorktree.open();
    } else {
      demoActions.spawnDemoBatchSwarm();
    }
  };

  const reviewSwarmTask = (id: number) => {
    if (!selectedRepository() || activeTab() !== "swarm") return;
    const task = swarmTasks().find((task) => task.id === id);
    if (!liveMode() || !getLiveToken() || !task?.liveId) return;
    setActiveTab("diff");
    void liveClient.loadLiveWorktreeDiff(task);
  };

  onMount(() => {
    const sessionChanged = (event: StorageEvent) => {
      if (event.key === "gitust_token" || event.key === null)
        void liveAuthClient.syncStoredSession();
    };
    window.addEventListener("storage", sessionChanged);
    void liveAuthClient.hydrateAuthData();
    void connectedRepository.refresh();
    const cleanups = registerWorkbenchShortcuts({
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

          <WorkbenchTabViews
            repositoryId={selectedRepository()?.id}
            activeTab={activeTab()}
            liveMode={liveMode()}
            swarmTasks={swarmTasks()}
            focusedSwarmIdx={focusedSwarmIdx()}
            onFocusedSwarmIdxChange={setFocusedSwarmIdx}
            onApproveMergeSwarmTask={approveAndMergeSwarmTask}
            onDiscardSwarmTask={discardSwarmTask}
            onPublishSwarmTask={publishSwarmTask}
            onReviewSwarmTask={reviewSwarmTask}
            onVerifySwarmTask={(id, command) => {
              if (selectedRepository() && activeTab() === "swarm")
                void liveSwarmClient.verifyLiveWorktree(id, command);
            }}
            onRefreshSwarmTasks={() =>
              void liveSwarmClient.hydrateSwarmWorktrees()
            }
            onViewRunHistory={(id) => {
              const task = swarmTasks().find((task) => task.liveId === id);
              if (
                task &&
                selectedRepository() &&
                liveMode() &&
                liveSession()?.role === "SiteAdmin"
              )
                setHistoryTask(task);
            }}
            onSpawnBatchAgentSwarm={spawnBatchAgentSwarm}
            onTriggerGithubSync={liveClient.triggerGithubSync}
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
            onSelectCommitDiffFile={(path) =>
              void liveClient.selectCommitDiffFile(path)
            }
            actionJobs={actionJobs()}
            activeJobId={activeJobId()}
            onSelectJob={setActiveJobId}
            onRerunAllActionJobs={demoActions.rerunAllActionJobs}
            actionLogs={actionLogs()}
            enterpriseUsers={enterpriseUsers()}
            onToggleEnterpriseUserRole={demoActions.toggleEnterpriseUserRole}
            onToggleEnterpriseUserStatus={
              demoActions.toggleEnterpriseUserStatus
            }
            onPolicyFeedback={showQuickFeedback}
            adminAuthProps={{
              liveMode: liveMode(),
              liveSession: liveSession(),
              onSignIn: (id, pwd) => void liveAuthClient.signIn(id, pwd),
              onSignOut: () => void liveAuthClient.signOut(),
              redeemMode: redeemMode(),
              onChangeRedeemMode: setRedeemMode,
              redeemToken: redeemToken(),
              onRedeemTokenInput: setRedeemToken,
              onRedeemInvite: (tok, user, pwd) =>
                void liveAuthClient.redeemInvite(tok, user, pwd),
              onRedeemPasswordReset: (tok, pwd) =>
                void liveAuthClient.redeemPasswordReset(tok, pwd),
              liveInvites: liveInvites(),
              lastIssuedInviteUrl: lastIssuedInviteUrl(),
              onIssueInvite: (email, role) =>
                void liveAuthClient.issueInvite(email, role),
              onRevokeInvite: (id) => void liveAuthClient.revokeInvite(id),
              onIssuePasswordReset: (uid) =>
                void liveAuthClient.issuePasswordReset(uid),
              onDeactivateLiveUser: (uid) =>
                void liveAuthClient.deactivateUser(uid),
              livePats: livePats(),
              lastIssuedPatToken: lastIssuedPatToken(),
              onCreatePat: (name) => void liveAuthClient.createPat(name),
              onRevokePat: (id) => void liveAuthClient.revokePat(id),
              liveSshKeys: liveSshKeys(),
              onRegisterSshKey: (title, key) =>
                void liveAuthClient.registerSshKey(title, key),
              onRevokeSshKey: (id) => void liveAuthClient.revokeSshKey(id),
            }}
            fileTreeNodes={fileTreeNodes()}
            onSelectFileFromTree={(node) =>
              void liveClient.selectFileFromTree(node)
            }
          />
        </main>
      </SidebarInset>

      <Show when={newWorktree.target()} keyed>
        {(repository) => (
          <NewWorktreeDialog
            repository={repository}
            submitting={newWorktree.submitting()}
            onSubmit={newWorktree.submit}
            onClose={newWorktree.close}
            onRefresh={() => void liveSwarmClient.hydrateSwarmWorktrees()}
          />
        )}
      </Show>

      {/* Global Command Palette (Cmd+K) */}
      <Show when={historyTarget()} keyed>
        {(task) => (
          <LiveRunHistoryDialog
            worktreeId={task.liveId!}
            branch={task.branch}
            onClose={() => setHistoryTask(null)}
          />
        )}
      </Show>
      <CommandPalette open={cmdOpen()} onClose={() => setCmdOpen(false)} />

      {/* Keyboard Shortcuts Reference Modal */}
      <ShortcutsReferenceModal
        open={shortcutsModalOpen()}
        onClose={() => setShortcutsModalOpen(false)}
      />

      {/* Floating Chord Indicator when 'g' is pressed */}
      <ActiveChordHUD />
    </SidebarProvider>
  );
};
