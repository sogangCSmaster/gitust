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
  ActionsPipelineView,
  ActiveChordHUD,
  Avatar,
  Badge,
  Button,
  CommandPalette,
  Dialog,
  EnterpriseAdminView,
  EnterpriseUser,
  getTheme,
  ICON_DEFS,
  Icon,
  IconName,
  IconSpriteSheet,
  Input,
  IssueItem,
  IssueListView,
  Kbd,
  shortcutManager,
  TabItem,
  Tabs,
  toggleTheme,
  VirtualDiffViewer,
  VirtualFileTree,
} from "../index";
import {
  generateActionLogs,
  generateLargeDiff,
  generateMonorepoFileTree,
  INITIAL_ACTION_JOBS,
  INITIAL_ENTERPRISE_USERS,
  INITIAL_ISSUES,
} from "./mock-data";

type WorkbenchTab =
  | "issues"
  | "diff"
  | "actions"
  | "admin"
  | "tree"
  | "primitives";

const ALL_ICON_NAMES = Object.keys(ICON_DEFS) as IconName[];

export const WorkbenchApp: Component = () => {
  const [activeTab, setActiveTab] = createSignal<WorkbenchTab>("issues");
  const [cmdOpen, setCmdOpen] = createSignal(false);
  const [shortcutsModalOpen, setShortcutsModalOpen] = createSignal(false);

  // Superhuman-style Local-First / Optimistic Issue Store
  const [issues, setIssues] = createSignal<IssueItem[]>(INITIAL_ISSUES);
  const [focusedIssueIdx, setFocusedIssueIdx] = createSignal(0);
  const [optimisticToast, setOptimisticToast] = createSignal<string | null>(
    null
  );

  // Gitust Actions CI/CD State
  const [actionJobs, setActionJobs] =
    createSignal<ActionJobStep[]>(INITIAL_ACTION_JOBS);
  const [activeJobId, setActiveJobId] = createSignal("job-docker");
  const actionLogs = createMemo(() => generateActionLogs(10000));

  // Enterprise Self-Hosted Admin & IAM State
  const [enterpriseUsers, setEnterpriseUsers] = createSignal<EnterpriseUser[]>(
    INITIAL_ENTERPRISE_USERS
  );

  let filterInputRef: HTMLInputElement | undefined;

  // Pre-generate 10,000 diff lines & 5,000 file tree paths
  const diffLines = createMemo(() => generateLargeDiff(10000));
  const fileTreeNodes = createMemo(() => generateMonorepoFileTree(5000));
  const [selectedFilePath, setSelectedFilePath] = createSignal(
    "crates/gitust-git/src/repository_4.rs"
  );

  const showQuickFeedback = (msg: string) => {
    setOptimisticToast(msg);
    setTimeout(() => {
      setOptimisticToast((cur) => (cur === msg ? null : cur));
    }, 2000);
  };

  // Optimistic 0ms status toggle
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
    const newId = Math.max(...issues().map((i) => i.id)) + 1;
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
    const cleanups = [
      shortcutManager.register({
        id: "cmd.palette",
        title: "Open Command Palette",
        category: "Actions",
        keys: "mod+k",
        description: "Search all commands, files, and navigation actions",
        allowInInput: true,
        handler: () => setCmdOpen((v) => !v),
      }),
      shortcutManager.register({
        id: "nav.issues",
        title: "Go to Issues & Pull Requests",
        category: "Navigation",
        keys: "g i",
        description: "Superhuman list view with j/k navigation",
        handler: () => setActiveTab("issues"),
      }),
      shortcutManager.register({
        id: "nav.diff",
        title: "Go to 10,000-Line Virtualized Diff Viewer",
        category: "Navigation",
        keys: "g d",
        description: "120fps O(1) DOM sliced PR diff engine",
        handler: () => setActiveTab("diff"),
      }),
      shortcutManager.register({
        id: "nav.actions",
        title: "Go to Gitust Actions CI/CD & Live Log Stream",
        category: "Navigation",
        keys: "g a",
        description: "GitHub/Gitea Actions workflow pipeline & 10k-line console",
        handler: () => setActiveTab("actions"),
      }),
      shortcutManager.register({
        id: "nav.admin",
        title: "Go to Enterprise Auth, SSO & Site Admin Console",
        category: "Navigation",
        keys: "g s",
        description: "Self-hosted on-premise IAM, domain lock, and user roles",
        handler: () => setActiveTab("admin"),
      }),
      shortcutManager.register({
        id: "nav.tree",
        title: "Go to 5,000-File Monorepo Tree",
        category: "Navigation",
        keys: "g t",
        description: "Instant file finder and virtualized tree",
        handler: () => setActiveTab("tree"),
      }),
      shortcutManager.register({
        id: "nav.primitives",
        title: "Go to Design Tokens & UI Primitives",
        category: "Navigation",
        keys: "g p",
        description: "Inspect tokens, buttons, badges, icons, and typography",
        handler: () => setActiveTab("primitives"),
      }),
      shortcutManager.register({
        id: "list.down",
        title: "Move Focus Down",
        category: "Issues & PRs",
        keys: "j",
        description: "Select next row in active list",
        handler: () => {
          if (activeTab() === "issues") {
            setFocusedIssueIdx((i) => Math.min(issues().length - 1, i + 1));
          }
        },
      }),
      shortcutManager.register({
        id: "list.up",
        title: "Move Focus Up",
        category: "Issues & PRs",
        keys: "k",
        description: "Select previous row in active list",
        handler: () => {
          if (activeTab() === "issues") {
            setFocusedIssueIdx((i) => Math.max(0, i - 1));
          }
        },
      }),
      shortcutManager.register({
        id: "issue.toggle",
        title: "Toggle Focused Issue Open/Closed (Optimistic)",
        category: "Issues & PRs",
        keys: "e",
        description: "Instant 0ms state mutation without spinner",
        handler: () => {
          const item = issues()[focusedIssueIdx()];
          if (item) toggleIssueStatus(item.id);
        },
      }),
      shortcutManager.register({
        id: "issue.create",
        title: "Create Instant Issue",
        category: "Issues & PRs",
        keys: "c",
        description: "Insert a new issue with 0ms optimistic feedback",
        handler: () => createOptimisticIssue(),
      }),
      shortcutManager.register({
        id: "actions.rerun",
        title: "Re-run CI/CD Workflow Jobs",
        category: "Actions",
        keys: "r",
        description: "Trigger instant re-run on self-hosted runner",
        handler: () => rerunAllActionJobs(),
      }),
      shortcutManager.register({
        id: "filter.focus",
        title: "Focus Instant Filter Bar",
        category: "Actions",
        keys: "/",
        description: "Filter issues and PRs as you type",
        handler: () => {
          setActiveTab("issues");
          queueMicrotask(() => filterInputRef?.focus());
        },
      }),
      shortcutManager.register({
        id: "view.theme",
        title: "Toggle Dark / Light High-Contrast Theme",
        category: "View & Theme",
        keys: "shift+t",
        description: "Zero-runtime CSS Custom Property theme switch",
        handler: () => {
          const next = toggleTheme();
          showQuickFeedback(`Theme switched to ${next.toUpperCase()} (0ms)`);
        },
      }),
      shortcutManager.register({
        id: "view.help",
        title: "Show Keyboard Shortcuts Reference",
        category: "View & Theme",
        keys: "?",
        description: "Inspect all registered single & chord shortcuts",
        handler: () => setShortcutsModalOpen((v) => !v),
      }),
    ];

    onCleanup(() => cleanups.forEach((fn) => fn()));
  });

  const tabs = createMemo<TabItem<WorkbenchTab>[]>(() => [
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

      {/* Top Global Navigation & Speed Telemetry Header */}
      <header
        style={{
          display: "flex",
          "align-items": "center",
          "justify-content": "space-between",
          height: "48px",
          padding: "0 var(--ds-space-4)",
          "background-color": "var(--ds-bg-primary)",
          "border-bottom": "1px solid var(--ds-border-default)",
          position: "sticky",
          top: 0,
          "z-index": 20,
        }}
      >
        <div
          style={{
            display: "flex",
            "align-items": "center",
            gap: "var(--ds-space-3)",
          }}
        >
          <div
            style={{
              display: "flex",
              "align-items": "center",
              gap: "var(--ds-space-2)",
              "font-weight": 700,
              "font-size": "var(--ds-font-size-md)",
            }}
          >
            <span
              style={{
                display: "inline-flex",
                "align-items": "center",
                "justify-content": "center",
                width: "22px",
                height: "22px",
                "border-radius": "var(--ds-radius-xs)",
                "background-color": "var(--ds-accent-primary)",
                color: "#fff",
              }}
            >
              <Icon name="repo" size={13} />
            </span>
            <span>gitust</span>
            <span style={{ color: "var(--ds-fg-muted)", "font-weight": 400 }}>
              /
            </span>
            <span
              class="ds-mono"
              style={{ "font-size": "var(--ds-font-size-sm)" }}
            >
              enterprise-self-hosted
            </span>
          </div>

          <Badge variant="accent" dot>
            Private On-Premise
          </Badge>
          <Badge variant="success" mono>
            <Icon name="zap" size={11} />
            <span>&lt;16ms Frame</span>
          </Badge>
        </div>

        {/* Live Keystroke Latency HUD & Global Actions */}
        <div
          style={{
            display: "flex",
            "align-items": "center",
            gap: "var(--ds-space-2)",
          }}
        >
          <Show when={shortcutManager.lastTelemetry()}>
            {(t) => (
              <div
                class="ds-mono"
                style={{
                  display: "inline-flex",
                  "align-items": "center",
                  gap: "var(--ds-space-1-5)",
                  padding: "2px 8px",
                  "border-radius": "var(--ds-radius-sm)",
                  "background-color": "var(--ds-bg-secondary)",
                  border: "1px solid var(--ds-border-subtle)",
                  "font-size": "var(--ds-font-size-2xs)",
                  color: "var(--ds-fg-secondary)",
                }}
              >
                <span
                  style={{
                    color: "var(--ds-success-fg)",
                    display: "inline-flex",
                  }}
                >
                  <Icon name="zap" size={12} />
                </span>
                <span>{t().title}</span>
                <span
                  style={{
                    color: "var(--ds-success-fg)",
                    "font-weight": 600,
                  }}
                >
                  {t().dispatchLatencyMs}ms
                </span>
              </div>
            )}
          </Show>

          <Button
            variant="secondary"
            size="sm"
            icon={<Icon name="terminal" size={13} />}
            shortcut="mod+k"
            onClick={() => setCmdOpen(true)}
          >
            Command Palette
          </Button>

          <Button
            variant="ghost"
            size="sm"
            icon={<Icon name="keyboard" size={14} />}
            shortcut="?"
            onClick={() => setShortcutsModalOpen(true)}
          >
            Shortcuts
          </Button>

          <Button
            variant="ghost"
            size="sm"
            icon={
              <Icon
                name={getTheme() === "dark" ? "sun" : "moon"}
                size={14}
              />
            }
            onClick={() => toggleTheme()}
            title="Toggle Dark/Light Theme"
          >
            {getTheme() === "dark" ? "Light" : "Dark"}
          </Button>
        </div>
      </header>

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

        {/* TAB 1: Superhuman Issue & PR List */}
        <Show when={activeTab() === "issues"}>
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
                Keyboard-First Issues & Pull Requests
              </h1>
              <p
                style={{
                  "font-size": "var(--ds-font-size-xs)",
                  color: "var(--ds-fg-secondary)",
                }}
              >
                Use <Kbd keys="j" /> / <Kbd keys="k" /> to move focus,{" "}
                <Kbd keys="e" /> to toggle Open/Closed in 0ms, <Kbd keys="c" />{" "}
                to create an instant issue, or <Kbd keys="/" /> to filter.
              </p>
            </div>
            <Button
              variant="primary"
              size="sm"
              icon={<Icon name="plus" size={13} />}
              shortcut="c"
              onClick={createOptimisticIssue}
            >
              New Issue
            </Button>
          </div>

          <IssueListView
            items={issues()}
            focusedIndex={focusedIssueIdx()}
            onFocusIndexChange={setFocusedIssueIdx}
            onToggleStatus={toggleIssueStatus}
            filterRef={(el) => (filterInputRef = el)}
          />
        </Show>

        {/* TAB 2: 10,000-Line Virtualized Diff Viewer */}
        <Show when={activeTab() === "diff"}>
          <div>
            <h1
              style={{
                "font-size": "var(--ds-font-size-lg)",
                "font-weight": 600,
              }}
            >
              Virtualized Code & Diff Viewer (10,000 Lines Stress Test)
            </h1>
            <p
              style={{
                "font-size": "var(--ds-font-size-xs)",
                color: "var(--ds-fg-secondary)",
                "margin-top": "2px",
              }}
            >
              Renders 10,000 diff lines using O(1) viewport slicing. Only ~45
              DOM nodes exist at any moment, guaranteeing 120fps scrolling.
            </p>
          </div>

          <VirtualDiffViewer
            filePath="crates/gitust-git/src/gix_engine.rs"
            lines={diffLines()}
            height={480}
          />
        </Show>

        {/* TAB 3: Gitust Actions CI/CD Pipeline & 10,000-Line Log Streamer */}
        <Show when={activeTab() === "actions"}>
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
                Gitust Actions — Self-Hosted CI/CD Pipeline & Live Console
              </h1>
              <p
                style={{
                  "font-size": "var(--ds-font-size-xs)",
                  color: "var(--ds-fg-secondary)",
                }}
              >
                100% compatible with <code class="ds-mono">.github/workflows/*.yml</code>{" "}
                & Gitea <code class="ds-mono">act_runner</code>. Streams 10,000+
                console lines at 120fps. Press <Kbd keys="r" /> to re-run jobs.
              </p>
            </div>
            <Badge variant="success" mono>
              <Icon name="server" size={12} />
              <span>Private Runner Pool: Active</span>
            </Badge>
          </div>

          <ActionsPipelineView
            workflowName="ci-enterprise-release.yml"
            commitSha="5072e0a"
            branch="main"
            jobs={actionJobs()}
            activeJobId={activeJobId()}
            onSelectJob={setActiveJobId}
            onRerunAll={rerunAllActionJobs}
            logs={actionLogs()}
            height={440}
          />
        </Show>

        {/* TAB 4: Enterprise Self-Hosted Admin, Auth & SSO Console */}
        <Show when={activeTab() === "admin"}>
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
                Enterprise Self-Hosted Auth, SSO & Site Admin Console
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
            users={enterpriseUsers()}
            onToggleRole={toggleEnterpriseUserRole}
            onToggleStatus={toggleEnterpriseUserStatus}
            onPolicyFeedback={showQuickFeedback}
          />
        </Show>

        {/* TAB 5: 5,000-File Virtualized Monorepo File Tree */}
        <Show when={activeTab() === "tree"}>
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
                5,000 Rust & TypeScript source paths.
              </p>
            </div>
            <Badge variant="info" mono>
              <Icon name="file-code" size={12} />
              <span>{selectedFilePath()}</span>
            </Badge>
          </div>

          <VirtualFileTree
            files={fileTreeNodes()}
            activePath={selectedFilePath()}
            onSelect={(node) => setSelectedFilePath(node.path)}
            height={440}
          />
        </Show>

        {/* TAB 6: Design Tokens, Vector Icon Sprite Sheet & Primitives Showcase */}
        <Show when={activeTab() === "primitives"}>
          <div
            style={{
              display: "grid",
              "grid-template-columns": "repeat(auto-fit, minmax(360px, 1fr))",
              gap: "var(--ds-space-4)",
            }}
          >
            {/* Card 1: GPU-Instanced SVG Sprite Icon System */}
            <section
              style={{
                padding: "var(--ds-space-4)",
                "background-color": "var(--ds-bg-primary)",
                border: "1px solid var(--ds-border-default)",
                "border-radius": "var(--ds-radius-md)",
                display: "flex",
                "flex-direction": "column",
                gap: "var(--ds-space-3)",
                "grid-column": "1 / -1",
              }}
            >
              <div
                style={{
                  display: "flex",
                  "align-items": "center",
                  "justify-content": "space-between",
                }}
              >
                <div>
                  <h2 style={{ "font-size": "var(--ds-font-size-md)" }}>
                    Zero-Dependency SVG &lt;symbol&gt; Sprite Icon System
                  </h2>
                  <p
                    style={{
                      "font-size": "var(--ds-font-size-xs)",
                      color: "var(--ds-fg-secondary)",
                    }}
                  >
                    Parsed once in memory via &lt;defs&gt; and rendered via GPU
                    instanced &lt;use href="#ds-icon-..." /&gt; with 0 external
                    runtime dependencies.
                  </p>
                </div>
                <Badge variant="success" mono>
                  {ALL_ICON_NAMES.length} Icons · ~1.3 KB Gzip
                </Badge>
              </div>

              <div
                style={{
                  display: "grid",
                  "grid-template-columns":
                    "repeat(auto-fill, minmax(140px, 1fr))",
                  gap: "var(--ds-space-2)",
                }}
              >
                <For each={ALL_ICON_NAMES}>
                  {(iconName) => (
                    <div
                      class="ds-mono"
                      style={{
                        display: "flex",
                        "align-items": "center",
                        gap: "var(--ds-space-2)",
                        padding: "var(--ds-space-2) var(--ds-space-2-5)",
                        "background-color": "var(--ds-bg-secondary)",
                        border: "1px solid var(--ds-border-subtle)",
                        "border-radius": "var(--ds-radius-sm)",
                        "font-size": "var(--ds-font-size-2xs)",
                        color: "var(--ds-fg-secondary)",
                      }}
                    >
                      <span
                        style={{
                          color: "var(--ds-fg-primary)",
                          display: "inline-flex",
                        }}
                      >
                        <Icon name={iconName} size={15} />
                      </span>
                      <span
                        style={{
                          overflow: "hidden",
                          "text-overflow": "ellipsis",
                          "white-space": "nowrap",
                        }}
                      >
                        {iconName}
                      </span>
                    </div>
                  )}
                </For>
              </div>
            </section>

            {/* Card 2: Buttons */}
            <section
              style={{
                padding: "var(--ds-space-4)",
                "background-color": "var(--ds-bg-primary)",
                border: "1px solid var(--ds-border-default)",
                "border-radius": "var(--ds-radius-md)",
                display: "flex",
                "flex-direction": "column",
                gap: "var(--ds-space-3)",
              }}
            >
              <h2 style={{ "font-size": "var(--ds-font-size-md)" }}>
                Buttons & Shortcut Pills
              </h2>
              <div
                style={{
                  display: "flex",
                  "flex-wrap": "wrap",
                  gap: "var(--ds-space-2)",
                  "align-items": "center",
                }}
              >
                <Button
                  variant="primary"
                  icon={<Icon name="git-merge" size={14} />}
                  shortcut="mod+enter"
                >
                  Merge Pull Request
                </Button>
                <Button
                  variant="secondary"
                  icon={<Icon name="git-branch" size={14} />}
                  shortcut="c"
                >
                  New Branch
                </Button>
                <Button
                  variant="ghost"
                  icon={<Icon name="issue-opened" size={14} />}
                  shortcut="g i"
                >
                  Issues
                </Button>
                <Button variant="danger" icon={<Icon name="x" size={14} />}>
                  Delete Repo
                </Button>
              </div>
              <div
                style={{
                  display: "flex",
                  "flex-wrap": "wrap",
                  gap: "var(--ds-space-2)",
                  "align-items": "center",
                }}
              >
                <Button size="xs" variant="secondary">
                  Size XS (22px)
                </Button>
                <Button size="sm" variant="secondary">
                  Size SM (26px)
                </Button>
                <Button size="md" variant="secondary">
                  Size MD (30px)
                </Button>
                <Button size="lg" variant="secondary">
                  Size LG (36px)
                </Button>
              </div>
            </section>

            {/* Card 3: Git Domain Badges */}
            <section
              style={{
                padding: "var(--ds-space-4)",
                "background-color": "var(--ds-bg-primary)",
                border: "1px solid var(--ds-border-default)",
                "border-radius": "var(--ds-radius-md)",
                display: "flex",
                "flex-direction": "column",
                gap: "var(--ds-space-3)",
              }}
            >
              <h2 style={{ "font-size": "var(--ds-font-size-md)" }}>
                Semantic & Git Domain Badges
              </h2>
              <div
                style={{
                  display: "flex",
                  "flex-wrap": "wrap",
                  gap: "var(--ds-space-2)",
                }}
              >
                <Badge variant="success">
                  <Icon name="git-pull-request" size={12} />
                  <span>Open</span>
                </Badge>
                <Badge variant="merged">
                  <Icon name="git-merge" size={12} />
                  <span>Merged</span>
                </Badge>
                <Badge variant="danger">
                  <Icon name="issue-closed" size={12} />
                  <span>Closed</span>
                </Badge>
                <Badge variant="warning">
                  <Icon name="clock" size={12} />
                  <span>CI Pending</span>
                </Badge>
                <Badge variant="info">
                  <Icon name="issue-draft" size={12} />
                  <span>Draft PR</span>
                </Badge>
                <Badge variant="neutral" mono>
                  <Icon name="shield-check" size={12} />
                  <span>a94f8c1 (Verified)</span>
                </Badge>
              </div>
              <div
                style={{
                  display: "flex",
                  "align-items": "center",
                  gap: "var(--ds-space-2)",
                }}
              >
                <Avatar name="Yu Seungjae" size="xs" />
                <Avatar name="Gitoxide Core" size="sm" />
                <Avatar name="Superhuman UX" size="md" />
                <span style={{ color: "var(--ds-fg-secondary)" }}>
                  Zero-image deterministic initials avatars
                </span>
              </div>
            </section>

            {/* Card 4: Inputs & Chord Keybindings */}
            <section
              style={{
                padding: "var(--ds-space-4)",
                "background-color": "var(--ds-bg-primary)",
                border: "1px solid var(--ds-border-default)",
                "border-radius": "var(--ds-radius-md)",
                display: "flex",
                "flex-direction": "column",
                gap: "var(--ds-space-3)",
              }}
            >
              <h2 style={{ "font-size": "var(--ds-font-size-md)" }}>
                High-Density Inputs & Chords
              </h2>
              <Input
                icon={<Icon name="search" size={14} />}
                placeholder="Search commits by SHA, author, or message..."
                shortcut="mod+f"
              />
              <div
                style={{
                  display: "flex",
                  "flex-wrap": "wrap",
                  gap: "var(--ds-space-3)",
                  "align-items": "center",
                  "font-size": "var(--ds-font-size-xs)",
                  color: "var(--ds-fg-secondary)",
                }}
              >
                <span>
                  Issues: <Kbd keys="g i" />
                </span>
                <span>
                  Diff: <Kbd keys="g d" />
                </span>
                <span>
                  Actions: <Kbd keys="g a" />
                </span>
                <span>
                  Admin: <Kbd keys="g s" />
                </span>
              </div>
            </section>
          </div>
        </Show>
      </main>

      {/* Global Command Palette (Cmd+K) */}
      <CommandPalette open={cmdOpen()} onClose={() => setCmdOpen(false)} />

      {/* Keyboard Shortcuts Reference Modal */}
      <Dialog
        open={shortcutsModalOpen()}
        onClose={() => setShortcutsModalOpen(false)}
        title="Keyboard Shortcuts Reference (Superhuman Mode)"
      >
        <div
          style={{
            display: "flex",
            "flex-direction": "column",
            gap: "var(--ds-space-2)",
          }}
        >
          <For each={shortcutManager.shortcuts()}>
            {(sc) => (
              <div
                style={{
                  display: "flex",
                  "align-items": "center",
                  "justify-content": "space-between",
                  padding: "var(--ds-space-1-5) 0",
                  "border-bottom": "1px solid var(--ds-border-subtle)",
                }}
              >
                <div>
                  <div style={{ "font-weight": 500 }}>{sc.title}</div>
                  <div
                    style={{
                      "font-size": "var(--ds-font-size-xs)",
                      color: "var(--ds-fg-muted)",
                    }}
                  >
                    {sc.description}
                  </div>
                </div>
                <Kbd keys={sc.keys} />
              </div>
            )}
          </For>
        </div>
      </Dialog>

      {/* Floating Chord Indicator when 'g' is pressed */}
      <ActiveChordHUD />
    </div>
  );
};
