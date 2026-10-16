import {
  Component,
  createMemo,
  createSignal,
  For,
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
  filterAgentSwarmTasks,
  filterIssueItems,
  Icon,
  IconSpriteSheet,
  IssueItem,
  LiveAuthSessionInfo,
  LiveInviteItem,
  LivePatItem,
  LiveSshKeyItem,
} from "../index";
import { createLiveAuthClient } from "./live-auth-client";
import {
  createLiveRepoClient,
  getLiveToken,
  LiveCommitSummary,
} from "./live-repo-client";
import { createLiveSwarmClient } from "./live-swarm-client";
import {
  generateActionLogs,
  generateLargeDiff,
  generateMonorepoFileTree,
  INITIAL_ACTION_JOBS,
  INITIAL_AGENT_SWARM_TASKS,
  INITIAL_ENTERPRISE_USERS,
  INITIAL_ISSUES,
} from "./mock-data";
import { registerWorkbenchShortcuts } from "./register-workbench-shortcuts";
import { createShowcaseDemoActions } from "./showcase-demo-actions";
import { ShortcutsReferenceModal, WorkbenchHeader } from "./WorkbenchHeader";
import { WorkbenchTabViews } from "./WorkbenchTabViews";
import { Alert, AlertDescription } from "../components/ui/alert";
import { SidebarProvider, SidebarInset } from "../components/ui/sidebar";
import {
  WorkbenchNavigation,
  type WorkbenchNavItem,
} from "./WorkbenchNavigation";

export type WorkbenchTab =
  "swarm" | "issues" | "diff" | "actions" | "admin" | "tree" | "primitives";

export const WorkbenchApp: Component = () => {
  const [liveMode, setLiveMode] = createSignal(Boolean(getLiveToken()));
  const [activeTab, setActiveTab] = createSignal<WorkbenchTab>("swarm");
  const [cmdOpen, setCmdOpen] = createSignal(false);
  const [shortcutsModalOpen, setShortcutsModalOpen] = createSignal(false);

  // Showcase Local-First Agent Swarm & Sandbox Fleet Store
  const [swarmTasks, setSwarmTasks] = createSignal<AgentSandboxTask[]>(
    liveMode() ? [] : INITIAL_AGENT_SWARM_TASKS,
  );
  const [swarmFilter, setSwarmFilter] = createSignal("");
  const [focusedSwarmIdx, setFocusedSwarmIdx] = createSignal(0);
  const visibleSwarmTasks = createMemo(() =>
    filterAgentSwarmTasks(swarmTasks(), swarmFilter()),
  );

  // Showcase Local-First / Optimistic Issue Store
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

  // Showcase Gitust Actions CI/CD State
  const [actionJobs, setActionJobs] =
    createSignal<ActionJobStep[]>(INITIAL_ACTION_JOBS);
  const [activeJobId, setActiveJobId] = createSignal("job-docker");
  const actionLogs = createMemo(() => generateActionLogs(10000));

  // Showcase & Live Enterprise Self-Hosted Admin & IAM State
  const [enterpriseUsers, setEnterpriseUsers] = createSignal<EnterpriseUser[]>(
    liveMode() ? [] : INITIAL_ENTERPRISE_USERS,
  );
  const [liveSession, setLiveSession] =
    createSignal<LiveAuthSessionInfo | null>(null);
  const [redeemMode, setRedeemMode] = createSignal<"invite" | "reset">(
    "invite",
  );
  const [redeemToken, setRedeemToken] = createSignal("");
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

  // Demo data is only available in the explicitly labelled showcase mode.
  const fallbackDiffLines = createMemo(() =>
    liveMode() ? [] : generateLargeDiff(10000),
  );
  const fallbackFileTreeNodes = createMemo(() =>
    liveMode() ? [] : generateMonorepoFileTree(5000),
  );
  const [liveDiffLines, setLiveDiffLines] = createSignal<ReturnType<
    typeof generateLargeDiff
  > | null>(null);
  const [liveFileTreeNodes, setLiveFileTreeNodes] = createSignal<ReturnType<
    typeof generateMonorepoFileTree
  > | null>(null);
  const [liveCommits, setLiveCommits] = createSignal<LiveCommitSummary[]>(
    liveMode()
      ? []
      : [
          {
            sha: "554fb6c",
            short_sha: "554fb6c",
            summary:
              "feat(git,server): Step 2-4 gix commit/tree/blob/diff & GitHub mirror sync",
            author_name: "yuseungjae",
          },
          {
            sha: "5edd70a",
            short_sha: "5edd70a",
            summary:
              "feat(mcp,oauth): Step 2-3 OAuth 2.1 PKCE S256 & MCP gateway",
            author_name: "yuseungjae",
          },
        ],
  );
  const [activeCommitRev, setActiveCommitRev] = createSignal("HEAD");
  const [lookupMicros, setLookupMicros] = createSignal<number | null>(null);
  const [selectedFilePath, setSelectedFilePath] = createSignal(
    liveMode() ? "Repository preview" : "crates/gitust-git/src/browse.rs",
  );

  const diffLines = createMemo(() => liveDiffLines() ?? fallbackDiffLines());
  const fileTreeNodes = createMemo(
    () => liveFileTreeNodes() ?? fallbackFileTreeNodes(),
  );

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

  // Dedicated Live Backend Client (`/api/v1/repo/*` & `/api/v1/sync/github`)
  const liveClient = createLiveRepoClient({
    setActiveCommitRev,
    setLookupMicros,
    setSelectedFilePath,
    setLiveDiffLines,
    setLiveFileTreeNodes,
    setLiveCommits,
    getActiveCommitRev: activeCommitRev,
    showQuickFeedback,
  });

  // Dedicated Live Agent Swarm Client (`/api/v1/swarm/worktrees/*`)
  const liveSwarmClient = createLiveSwarmClient({
    getSwarmTasks: swarmTasks,
    setSwarmTasks,
    onRepoMutated: () => void liveClient.hydrateFromLiveBackend(),
    showQuickFeedback,
  });

  // Dedicated Live Auth & Credential Client (`/api/v1/auth/*` & `/api/v1/admin/*`)
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
      setLiveMode(true);
      setEnterpriseUsers([]);
      setLiveCommits([]);
      setLiveDiffLines([]);
      setLiveFileTreeNodes([]);
      setLookupMicros(null);
      liveSwarmClient.resetSwarmEpoch();
      setSwarmTasks([]);
      void liveClient.hydrateFromLiveBackend();
      void liveSwarmClient.hydrateSwarmWorktrees();
    },
    onTokenCleared: () => {
      setLiveMode(false);
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
    setActiveTab,
    setFocusedIssueIdx,
    setActionJobs,
    setEnterpriseUsers,
    showQuickFeedback,
  });

  const approveAndMergeSwarmTask = (id: number) => {
    if (getLiveToken()) {
      void liveSwarmClient.approveLiveWorktree(id);
    } else {
      demoActions.approveDemoSwarmTask(id);
    }
  };

  const publishSwarmTask = (id: number) => {
    void liveSwarmClient.publishLiveWorktree(id);
  };

  const discardSwarmTask = (id: number) => {
    if (getLiveToken()) {
      void liveSwarmClient.discardLiveWorktree(id);
    } else {
      demoActions.discardDemoSwarmTask(id);
    }
  };

  const spawnBatchAgentSwarm = () => {
    if (getLiveToken()) {
      void liveSwarmClient.spawnLiveWorktree();
    } else {
      demoActions.spawnDemoBatchSwarm();
    }
  };

  onMount(() => {
    if (typeof window !== "undefined") {
      const path = window.location.pathname;
      const params = new URLSearchParams(window.location.search);
      const urlToken = (params.get("token") ?? "").trim();
      if (
        path.startsWith("/auth/accept-invite") ||
        urlToken.startsWith("gitust_inv_")
      ) {
        setActiveTab("admin");
        setRedeemMode("invite");
        if (urlToken) setRedeemToken(urlToken);
        window.history.replaceState({}, "", path);
      } else if (
        path.startsWith("/auth/reset-password") ||
        urlToken.startsWith("gitust_rst_")
      ) {
        setActiveTab("admin");
        setRedeemMode("reset");
        if (urlToken) setRedeemToken(urlToken);
        window.history.replaceState({}, "", path);
      }
    }
    void liveAuthClient.hydrateAuthData();
    void liveClient.hydrateFromLiveBackend();
    void liveSwarmClient.hydrateSwarmWorktrees();
    const cleanups = registerWorkbenchShortcuts({
      getActiveTab: activeTab,
      setActiveTab,
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
      rerunAllActionJobs: demoActions.rerunAllActionJobs,
      focusFilterInput: () => queueMicrotask(() => filterInputRef?.focus()),
      showQuickFeedback,
    });

    onCleanup(() => {
      if (toastTimer) clearTimeout(toastTimer);
      cleanups.forEach((fn) => fn());
    });
  });

  const tabs = createMemo<WorkbenchNavItem[]>(() => [
    {
      id: "swarm",
      label: "Worktrees",
      count: swarmTasks().length,
      icon: <Icon name="bot" size={14} />,
    },
    {
      id: "issues",
      label: "Issues · Demo",
      icon: <Icon name="git-pull-request" size={14} />,
    },
    {
      id: "diff",
      label: liveMode() ? "Diff" : "Diff · Demo",
      icon: <Icon name="diff" size={14} />,
    },
    {
      id: "actions",
      label: "Actions · Demo",
      icon: <Icon name="play" size={14} />,
    },
    {
      id: "admin",
      label: "Access",
      icon: <Icon name="shield-check" size={14} />,
    },
    {
      id: "tree",
      label: liveMode() ? "Files" : "Files · Demo",
      icon: <Icon name="folder" size={14} />,
    },
    {
      id: "primitives",
      label: "Components",
      icon: <Icon name="sliders" size={14} />,
    },
  ]);

  return (
    <SidebarProvider>
      <IconSpriteSheet />
      <WorkbenchNavigation
        items={tabs()}
        active={activeTab()}
        onSelect={setActiveTab}
      />
      <SidebarInset class="min-w-0">
        <WorkbenchHeader
          onOpenCommandPalette={() => setCmdOpen(true)}
          onOpenShortcutsModal={() => setShortcutsModalOpen(true)}
        />
        <div class="mx-auto w-full max-w-6xl space-y-5 p-4 sm:p-5 lg:p-6">
          <div class="flex flex-wrap items-center gap-x-4 gap-y-2 text-xs text-muted-foreground">
            <span class="inline-flex items-center gap-2 text-foreground">
              <span class="size-1.5 rounded-full bg-foreground" />
              {liveMode() ? "Live workspace" : "Demo workspace"}
            </span>
            <span>
              {liveMode()
                ? "Issues and Actions use sample data."
                : "Explore with sample data, or connect your local server."}
            </span>
            <Show when={!liveMode()}>
              <Button
                size="sm"
                variant="ghost"
                class="-ml-2"
                onClick={() => setActiveTab("admin")}
              >
                Sign in <Icon name="chevron-right" size={12} />
              </Button>
            </Show>
          </div>
          <Show when={optimisticToast()}>
            {(message) => (
              <Alert>
                <AlertDescription>{message()}</AlertDescription>
              </Alert>
            )}
          </Show>

          <WorkbenchTabViews
            activeTab={activeTab()}
            liveMode={liveMode()}
            swarmTasks={swarmTasks()}
            focusedSwarmIdx={focusedSwarmIdx()}
            onFocusedSwarmIdxChange={setFocusedSwarmIdx}
            onApproveMergeSwarmTask={approveAndMergeSwarmTask}
            onDiscardSwarmTask={discardSwarmTask}
            onPublishSwarmTask={publishSwarmTask}
            onVerifySwarmTask={(id, command) =>
              void liveSwarmClient.verifyLiveWorktree(id, command)
            }
            onRefreshSwarmTasks={() =>
              void liveSwarmClient.hydrateSwarmWorktrees()
            }
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
            activeCommitRev={activeCommitRev()}
            onSelectCommitDiff={(sha) =>
              void liveClient.loadLiveCommitDiff(sha)
            }
            lookupMicros={lookupMicros()}
            selectedFilePath={selectedFilePath()}
            diffLines={diffLines()}
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
        </div>
      </SidebarInset>

      {/* Global Command Palette (Cmd+K) */}
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
