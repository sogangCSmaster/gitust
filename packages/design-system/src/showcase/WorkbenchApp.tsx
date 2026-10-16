import {
  Component,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
  Show,
} from "solid-js";
import {
  ActionJobStep,
  ActiveChordHUD,
  AgentProvider,
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
  TabItem,
  Tabs,
} from "../index";
import { createLiveAuthClient } from "./live-auth-client";
import { createLiveRepoClient, LiveCommitSummary } from "./live-repo-client";
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
import { ShortcutsReferenceModal, WorkbenchHeader } from "./WorkbenchHeader";
import { WorkbenchTabViews } from "./WorkbenchTabViews";

export type WorkbenchTab =
  | "swarm"
  | "issues"
  | "diff"
  | "actions"
  | "admin"
  | "tree"
  | "primitives";

export const WorkbenchApp: Component = () => {
  const [activeTab, setActiveTab] = createSignal<WorkbenchTab>("swarm");
  const [cmdOpen, setCmdOpen] = createSignal(false);
  const [shortcutsModalOpen, setShortcutsModalOpen] = createSignal(false);

  // Showcase Local-First Agent Swarm & Sandbox Fleet Store
  const [swarmTasks, setSwarmTasks] = createSignal<AgentSandboxTask[]>(
    INITIAL_AGENT_SWARM_TASKS
  );
  const [swarmFilter, setSwarmFilter] = createSignal("");
  const [focusedSwarmIdx, setFocusedSwarmIdx] = createSignal(0);
  const visibleSwarmTasks = createMemo(() =>
    filterAgentSwarmTasks(swarmTasks(), swarmFilter())
  );

  // Showcase Local-First / Optimistic Issue Store
  const [issues, setIssues] = createSignal<IssueItem[]>(INITIAL_ISSUES);
  const [issueFilter, setIssueFilter] = createSignal("");
  const [focusedIssueIdx, setFocusedIssueIdx] = createSignal(0);
  const visibleIssues = createMemo(() =>
    filterIssueItems(issues(), issueFilter())
  );

  const [optimisticToast, setOptimisticToast] = createSignal<string | null>(
    null
  );
  let toastTimer: ReturnType<typeof setTimeout> | null = null;

  // Showcase Gitust Actions CI/CD State
  const [actionJobs, setActionJobs] =
    createSignal<ActionJobStep[]>(INITIAL_ACTION_JOBS);
  const [activeJobId, setActiveJobId] = createSignal("job-docker");
  const actionLogs = createMemo(() => generateActionLogs(10000));

  // Showcase & Live Enterprise Self-Hosted Admin & IAM State
  const [enterpriseUsers, setEnterpriseUsers] = createSignal<EnterpriseUser[]>(
    INITIAL_ENTERPRISE_USERS
  );
  const [liveSession, setLiveSession] =
    createSignal<LiveAuthSessionInfo | null>(null);
  const [redeemMode, setRedeemMode] = createSignal<"invite" | "reset">(
    "invite"
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

  // Live Repository State (with 10,000 diff lines & 5,000 file tree paths as 0ms fallback)
  const fallbackDiffLines = createMemo(() => generateLargeDiff(10000));
  const fallbackFileTreeNodes = createMemo(() =>
    generateMonorepoFileTree(5000)
  );
  const [liveDiffLines, setLiveDiffLines] = createSignal<
    ReturnType<typeof generateLargeDiff> | null
  >(null);
  const [liveFileTreeNodes, setLiveFileTreeNodes] = createSignal<
    ReturnType<typeof generateMonorepoFileTree> | null
  >(null);
  const [liveCommits, setLiveCommits] = createSignal<LiveCommitSummary[]>([
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
      summary: "feat(mcp,oauth): Step 2-3 OAuth 2.1 PKCE S256 & MCP gateway",
      author_name: "yuseungjae",
    },
  ]);
  const [activeCommitRev, setActiveCommitRev] = createSignal("HEAD");
  const [lookupMicros, setLookupMicros] = createSignal<number>(180);
  const [selectedFilePath, setSelectedFilePath] = createSignal(
    "crates/gitust-git/src/browse.rs"
  );

  const diffLines = createMemo(() => liveDiffLines() ?? fallbackDiffLines());
  const fileTreeNodes = createMemo(
    () => liveFileTreeNodes() ?? fallbackFileTreeNodes()
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
    onTokenAuthenticated: () => void liveClient.hydrateFromLiveBackend(),
    onTokenCleared: () => {
      setLiveDiffLines(null);
      setLiveFileTreeNodes(null);
    },
    showQuickFeedback,
  });

  // Showcase Optimistic Demo Actions (0ms Local-First)
  const approveAndMergeSwarmTask = (id: number) => {
    const t0 = performance.now();
    setSwarmTasks((prev) =>
      prev.map((t) => {
        if (t.id !== id) return t;
        const dt = (performance.now() - t0).toFixed(2);
        showQuickFeedback(
          `Merged ${t.branch} (${t.agent}) into main & synced to GitHub in ${dt}ms (0 Cloud VM Leakage)`
        );
        return {
          ...t,
          sandboxStatus: "merged",
          githubSynced: true,
          updatedAt: "just now",
        };
      })
    );
  };

  const discardSwarmTask = (id: number) => {
    setSwarmTasks((prev) =>
      prev.map((t) => {
        if (t.id !== id) return t;
        showQuickFeedback(
          `Discarded CoW Worktree ${t.branch} & freed sandbox in 0.1ms`
        );
        return { ...t, sandboxStatus: "discarded", updatedAt: "just now" };
      })
    );
  };

  const spawnBatchAgentSwarm = () => {
    const t0 = performance.now();
    const providers: AgentProvider[] = [
      "Claude Code",
      "Cursor Agent",
      "Codex Swarm",
    ];
    const current = swarmTasks();
    const baseId =
      current.reduce((maxId, t) => (t.id > maxId ? t.id : maxId), 0) + 1;
    const batch: AgentSandboxTask[] = Array.from({ length: 10 }, (_, idx) => {
      const id = baseId + idx;
      const agent = providers[idx % providers.length]!;
      return {
        id,
        agent,
        branch: `agent/${agent.toLowerCase().split(" ")[0]}/parallel-task-${id}`,
        baseBranch: "main",
        forkLatencyMs: Number((0.25 + (idx % 4) * 0.04).toFixed(2)),
        promptSummary: `Parallel sandbox task #${id}: zero-copy refactor & unit test verification`,
        sandboxStatus: "verified",
        compileTimeMs: 68 + ((idx * 7) % 35),
        testsPassed: 24,
        testsTotal: 24,
        filesChanged: 2 + (idx % 3),
        additions: 48 + idx * 9,
        deletions: 12 + idx * 2,
        updatedAt: "just now",
      };
    });

    setSwarmTasks((prev) => [...batch, ...prev]);
    const dt = (performance.now() - t0).toFixed(2);
    showQuickFeedback(
      `Spawned +10 Zero-Copy Agent Branches & Verified Sandboxes in ${dt}ms`
    );
  };

  const toggleIssueStatus = (id: number) => {
    const t0 = performance.now();
    setIssues((prev) =>
      prev.map((item) => {
        if (item.id !== id) return item;
        const nextStatus = item.status === "open" ? "closed" : "open";
        const dt = (performance.now() - t0).toFixed(2);
        showQuickFeedback(
          `Optimistic UI: #${item.id} → ${nextStatus.toUpperCase()} (${dt}ms, 0 network wait)`
        );
        return { ...item, status: nextStatus, updatedAt: "just now" };
      })
    );
  };

  const createOptimisticIssue = () => {
    const current = issues();
    const newId =
      current.reduce((maxId, i) => (i.id > maxId ? i.id : maxId), 0) + 1;
    const newItem: IssueItem = {
      id: newId,
      kind: "issue",
      title: `feat: instant optimistic issue #${newId} created via 'c' shortcut`,
      status: "open",
      author: "yuseungjae",
      labels: [{ name: "optimistic-ui", variant: "accent" }],
      commentsCount: 0,
      updatedAt: "just now",
    };
    setIssues((prev) => [newItem, ...prev]);
    setActiveTab("issues");
    setFocusedIssueIdx(0);
    showQuickFeedback(`Created Issue #${newId} instantaneously (< 1ms)`);
  };

  const rerunAllActionJobs = () => {
    setActionJobs((prev) =>
      prev.map((j) => ({ ...j, status: "success", duration: "12s (cached)" }))
    );
    showQuickFeedback(
      "Gitust Actions: Dispatched workflow re-run to corp-runner-linux-01 (0ms Optimistic)"
    );
  };

  const toggleEnterpriseUserRole = (userId: number) => {
    const roles: EnterpriseUser["role"][] = [
      "Developer",
      "Maintainer",
      "Site Admin",
    ];
    setEnterpriseUsers((prev) =>
      prev.map((u) => {
        if (u.id !== userId) return u;
        const nextRole =
          roles[(roles.indexOf(u.role) + 1) % roles.length] ?? "Developer";
        showQuickFeedback(
          `IAM Role Updated: @${u.username} → ${nextRole} (0ms Optimistic)`
        );
        return { ...u, role: nextRole };
      })
    );
  };

  const toggleEnterpriseUserStatus = (userId: number) => {
    setEnterpriseUsers((prev) =>
      prev.map((u) => {
        if (u.id !== userId) return u;
        const nextStatus = u.status === "active" ? "suspended" : "active";
        showQuickFeedback(
          `Account Status Updated: @${u.username} → ${nextStatus.toUpperCase()} (0ms Optimistic)`
        );
        return { ...u, status: nextStatus };
      })
    );
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
      toggleIssueStatus,
      createOptimisticIssue,
      rerunAllActionJobs,
      focusFilterInput: () => queueMicrotask(() => filterInputRef?.focus()),
      showQuickFeedback,
    });

    onCleanup(() => {
      if (toastTimer) clearTimeout(toastTimer);
      cleanups.forEach((fn) => fn());
    });
  });

  const tabs = createMemo<TabItem<WorkbenchTab>[]>(() => [
    {
      id: "swarm",
      label: "Agent Swarm & Sandboxes",
      count: swarmTasks().length,
      shortcut: "g w",
      icon: <Icon name="bot" size={14} />,
    },
    {
      id: "issues",
      label: "Issues & PRs",
      count: issues().length,
      shortcut: "g i",
      icon: <Icon name="git-pull-request" size={14} />,
    },
    {
      id: "diff",
      label: "Diff (10k)",
      shortcut: "g d",
      icon: <Icon name="diff" size={14} />,
    },
    {
      id: "actions",
      label: "CI/CD Actions",
      count: actionJobs().length,
      shortcut: "g a",
      icon: <Icon name="play" size={14} />,
    },
    {
      id: "admin",
      label: "Enterprise Admin & Auth",
      shortcut: "g s",
      icon: <Icon name="shield-check" size={14} />,
    },
    {
      id: "tree",
      label: "File Tree (5k)",
      shortcut: "g t",
      icon: <Icon name="folder" size={14} />,
    },
    {
      id: "primitives",
      label: "Tokens & Icons",
      shortcut: "g p",
      icon: <Icon name="sliders" size={14} />,
    },
  ]);

  return (
    <div
      style={{
        "min-height": "100vh",
        display: "flex",
        "flex-direction": "column",
      }}
    >
      {/* Mount Global Zero-Overhead SVG <symbol> Sprite Sheet Once */}
      <IconSpriteSheet />

      <WorkbenchHeader
        onOpenCommandPalette={() => setCmdOpen(true)}
        onOpenShortcutsModal={() => setShortcutsModalOpen(true)}
      />

      {/* Sub-header Tabs */}
      <div style={{ "background-color": "var(--ds-bg-primary)" }}>
        <div style={{ "max-width": "1240px", margin: "0 auto" }}>
          <Tabs
            items={tabs()}
            activeId={activeTab()}
            onChange={(id) => setActiveTab(id)}
          />
        </div>
      </div>

      {/* Main Content Container */}
      <main
        style={{
          flex: 1,
          "max-width": "1240px",
          width: "100%",
          margin: "0 auto",
          padding: "var(--ds-space-5) var(--ds-space-4)",
          display: "flex",
          "flex-direction": "column",
          gap: "var(--ds-space-4)",
        }}
      >
        {/* Optimistic Feedback Banner (Zero Spinner) */}
        <Show when={optimisticToast()}>
          {(msg) => (
            <div
              style={{
                display: "flex",
                "align-items": "center",
                "justify-content": "space-between",
                padding: "var(--ds-space-2) var(--ds-space-3)",
                "background-color": "var(--ds-accent-subtle)",
                border: "1px solid var(--ds-accent-border)",
                "border-radius": "var(--ds-radius-sm)",
                "font-size": "var(--ds-font-size-xs)",
                color: "var(--ds-fg-primary)",
              }}
            >
              <span
                style={{
                  display: "inline-flex",
                  "align-items": "center",
                  gap: "var(--ds-space-1-5)",
                }}
              >
                <Icon name="zap" size={13} />
                <span>{msg()}</span>
              </span>
              <span class="ds-mono" style={{ color: "var(--ds-fg-secondary)" }}>
                Zero Spinner · Local-First Cache
              </span>
            </div>
          )}
        </Show>

        <WorkbenchTabViews
          activeTab={activeTab()}
          swarmTasks={swarmTasks()}
          focusedSwarmIdx={focusedSwarmIdx()}
          onFocusedSwarmIdxChange={setFocusedSwarmIdx}
          onApproveMergeSwarmTask={approveAndMergeSwarmTask}
          onDiscardSwarmTask={discardSwarmTask}
          onSpawnBatchAgentSwarm={spawnBatchAgentSwarm}
          onTriggerGithubSync={liveClient.triggerGithubSync}
          swarmFilter={swarmFilter()}
          onSwarmFilterChange={setSwarmFilter}
          issues={issues()}
          focusedIssueIdx={focusedIssueIdx()}
          onFocusedIssueIdxChange={setFocusedIssueIdx}
          onToggleIssueStatus={toggleIssueStatus}
          onCreateOptimisticIssue={createOptimisticIssue}
          filterRef={(el) => (filterInputRef = el)}
          issueFilter={issueFilter()}
          onIssueFilterChange={setIssueFilter}
          liveCommits={liveCommits()}
          activeCommitRev={activeCommitRev()}
          onSelectCommitDiff={(sha) => void liveClient.loadLiveCommitDiff(sha)}
          lookupMicros={lookupMicros()}
          selectedFilePath={selectedFilePath()}
          diffLines={diffLines()}
          actionJobs={actionJobs()}
          activeJobId={activeJobId()}
          onSelectJob={setActiveJobId}
          onRerunAllActionJobs={rerunAllActionJobs}
          actionLogs={actionLogs()}
          enterpriseUsers={enterpriseUsers()}
          onToggleEnterpriseUserRole={toggleEnterpriseUserRole}
          onToggleEnterpriseUserStatus={toggleEnterpriseUserStatus}
          onPolicyFeedback={showQuickFeedback}
          adminAuthProps={{
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

      {/* Global Command Palette (Cmd+K) */}
      <CommandPalette open={cmdOpen()} onClose={() => setCmdOpen(false)} />

      {/* Keyboard Shortcuts Reference Modal */}
      <ShortcutsReferenceModal
        open={shortcutsModalOpen()}
        onClose={() => setShortcutsModalOpen(false)}
      />

      {/* Floating Chord Indicator when 'g' is pressed */}
      <ActiveChordHUD />
    </div>
  );
};
