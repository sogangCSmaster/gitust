import { DiffLine, FileTreeNode, IssueItem } from "../index";

export const INITIAL_ISSUES: IssueItem[] = [
  {
    id: 402,
    kind: "pr",
    title: "feat(git): implement zero-copy packfile tree traversal via gitoxide (gix)",
    status: "open",
    author: "yuseungjae",
    labels: [
      { name: "perf", variant: "accent" },
      { name: "gitoxide", variant: "info" },
    ],
    commentsCount: 14,
    updatedAt: "2m ago",
    branch: "feat/gix-zero-copy",
    ciStatus: "success",
  },
  {
    id: 401,
    kind: "issue",
    title: "design-system: enforce <16ms frame budget across VirtualDiffViewer and CommandPalette",
    status: "open",
    author: "superhuman-dev",
    labels: [
      { name: "design-system", variant: "merged" },
      { name: "priority:high", variant: "danger" },
    ],
    commentsCount: 8,
    updatedAt: "9m ago",
  },
  {
    id: 399,
    kind: "pr",
    title: "feat(ssh): embed async russh daemon for zero-dependency Git SSH transport",
    status: "merged",
    author: "rust-core",
    labels: [{ name: "ssh", variant: "success" }],
    commentsCount: 21,
    updatedAt: "1h ago",
    branch: "feat/russh-server",
    ciStatus: "success",
  },
  {
    id: 398,
    kind: "issue",
    title: "search: integrate Tantivy incremental indexing on post-receive hook",
    status: "open",
    author: "yuseungjae",
    labels: [{ name: "search", variant: "warning" }],
    commentsCount: 5,
    updatedAt: "3h ago",
  },
  {
    id: 395,
    kind: "pr",
    title: "refactor(db): replace dynamic ORM queries with compile-time verified SQLx macros",
    status: "open",
    author: "db-architect",
    labels: [
      { name: "database", variant: "info" },
      { name: "sqlx", variant: "neutral" },
    ],
    commentsCount: 11,
    updatedAt: "5h ago",
    branch: "refactor/sqlx-queries",
    ciStatus: "pending",
  },
  {
    id: 391,
    kind: "pr",
    title: "chore(actions): Gitea Actions YAML parser compatibility in crates/gitust-actions",
    status: "draft",
    author: "ci-bot",
    labels: [{ name: "ci-runner", variant: "neutral" }],
    commentsCount: 2,
    updatedAt: "1d ago",
    branch: "wip/actions-parser",
    ciStatus: "failed",
  },
  {
    id: 388,
    kind: "issue",
    title: "security: support SSH signature verification for commits alongside GPG",
    status: "closed",
    author: "sec-auditor",
    labels: [{ name: "security", variant: "danger" }],
    commentsCount: 19,
    updatedAt: "2d ago",
  },
];

export function generateLargeDiff(lineCount = 10000): DiffLine[] {
  const lines: DiffLine[] = [];
  let oldNum = 1;
  let newNum = 1;

  const sampleSnippets = [
    'pub async fn traverse_tree_zero_copy(repo: &gix::Repository, oid: gix::ObjectId) -> Result<TreeSnapshot, GitError> {',
    '    let object = repo.find_object(oid).map_err(GitError::ObjectNotFound)?;',
    '    let tree_ref = object.try_into_tree().map_err(GitError::InvalidTree)?;',
    '    let mut entries = Vec::with_capacity(128);',
    '    for entry_res in tree_ref.iter() {',
    '        let entry = entry_res.map_err(GitError::CorruptEntry)?;',
    '        entries.push(TreeEntryDto::from_gix_ref(entry));',
    '    }',
    '    Ok(TreeSnapshot { oid: oid.to_string(), entries })',
    '}',
  ];

  for (let i = 0; i < lineCount; i++) {
    if (i % 120 === 0) {
      lines.push({
        index: i,
        oldLineNumber: null,
        newLineNumber: null,
        type: "hunk",
        content: `@@ -${oldNum},60 +${newNum},68 @@ impl GitustRepository for GixEngine // hunk #${Math.floor(i / 120) + 1}`,
      });
      continue;
    }

    const mod = i % 14;
    const snippet = sampleSnippets[i % sampleSnippets.length] ?? "";

    if (mod === 3 || mod === 4) {
      lines.push({
        index: i,
        oldLineNumber: oldNum++,
        newLineNumber: null,
        type: "del",
        content: `    // Legacy subprocess git cat-file fallback removed (line ${i})`,
      });
    } else if (mod === 5 || mod === 6 || mod === 7) {
      lines.push({
        index: i,
        oldLineNumber: null,
        newLineNumber: newNum++,
        type: "add",
        content: `${snippet} // zero-alloc slice #${i}`,
      });
    } else {
      lines.push({
        index: i,
        oldLineNumber: oldNum++,
        newLineNumber: newNum++,
        type: "context",
        content: snippet,
      });
    }
  }

  return lines;
}

export function generateMonorepoFileTree(count = 5000): FileTreeNode[] {
  const crates = [
    "crates/gitust-server/src",
    "crates/gitust-api/src",
    "crates/gitust-core/src",
    "crates/gitust-db/src",
    "crates/gitust-git/src",
    "crates/gitust-search/src",
    "crates/gitust-actions/src",
    "packages/design-system/src",
    "packages/sync-client/src",
    "packages/web/src",
  ];

  const modules = [
    "repository",
    "commit_graph",
    "pull_request",
    "issue_tracker",
    "branch_protection",
    "webhook_dispatcher",
    "ssh_transport",
    "smart_http",
    "packfile_idx",
    "tantivy_indexer",
  ];

  const result: FileTreeNode[] = [];
  for (let i = 0; i < count; i++) {
    const base = crates[i % crates.length]!;
    const mod = modules[i % modules.length]!;
    const ext = base.startsWith("crates/") ? "rs" : "tsx";
    result.push({
      path: `${base}/${mod}_${i}.${ext}`,
      kind: i % 25 === 0 ? "dir" : "file",
      sizeBytes: 1240 + ((i * 7919) % 48000),
    });
  }
  return result;
}

export const INITIAL_ACTION_JOBS: import("../index").ActionJobStep[] = [
  {
    id: "job-check",
    name: "cargo fmt & clippy (--all-targets)",
    status: "success",
    duration: "14s",
    runner: "corp-runner-linux-01",
  },
  {
    id: "job-test",
    name: "cargo test --workspace (gitoxide + sqlx)",
    status: "success",
    duration: "38s",
    runner: "corp-runner-linux-02",
  },
  {
    id: "job-ui",
    name: "pnpm --filter @gitust/design-system build",
    status: "success",
    duration: "4s",
    runner: "corp-runner-linux-01",
  },
  {
    id: "job-docker",
    name: "Build Single-Binary Enterprise Container",
    status: "running",
    duration: "19s...",
    runner: "corp-runner-linux-03",
  },
];

export function generateActionLogs(count = 10000): import("../index").ActionLogLine[] {
  const sampleLines: { level: import("../index").ActionLogLine["level"]; text: string }[] = [
    { level: "info", text: "[runner:corp-runner-linux-01] Pulling internal registry image gitust/rust-builder:1.85..." },
    { level: "info", text: "   Compiling gitust-core v0.1.0 (/workspace/crates/gitust-core)" },
    { level: "info", text: "   Compiling gitust-git v0.1.0 (/workspace/crates/gitust-git) [gix zero-copy enabled]" },
    { level: "info", text: "   Compiling gitust-actions v0.1.0 (/workspace/crates/gitust-actions)" },
    { level: "info", text: "   Compiling gitust-server v0.1.0 (/workspace/crates/gitust-server)" },
    { level: "success", text: "test git_engine::packfile::zero_copy_tree_lookup ... ok (0.18ms)" },
    { level: "success", text: "test auth::enterprise_sso::verify_oidc_token ... ok (0.42ms)" },
    { level: "warn", text: "warning: rate-limit threshold near 85% on internal mirror cache" },
  ];

  const logs: import("../index").ActionLogLine[] = [];
  for (let i = 0; i < count; i++) {
    const item = sampleLines[i % sampleLines.length]!;
    const sec = Math.floor(i / 180)
      .toString()
      .padStart(2, "0");
    const ms = (i % 1000).toString().padStart(3, "0");
    logs.push({
      index: i,
      timestamp: `00:01:${sec}.${ms}`,
      level: item.level,
      text: `${item.text} (step #${i + 1})`,
    });
  }
  return logs;
}

export const INITIAL_ENTERPRISE_USERS: import("../index").EnterpriseUser[] = [
  {
    id: 1,
    username: "yuseungjae",
    email: "seungjae@company.internal",
    department: "Core Platform",
    role: "Site Admin",
    twoFactorEnabled: true,
    status: "active",
  },
  {
    id: 2,
    username: "sec-ops-lead",
    email: "security@company.internal",
    department: "InfoSec & Audit",
    role: "Site Admin",
    twoFactorEnabled: true,
    status: "active",
  },
  {
    id: 3,
    username: "infra-devops",
    email: "devops@company.internal",
    department: "CI/CD & Runners",
    role: "Maintainer",
    twoFactorEnabled: true,
    status: "active",
  },
  {
    id: 4,
    username: "backend-rust",
    email: "rust-dev@company.internal",
    department: "Product Engineering",
    role: "Developer",
    twoFactorEnabled: true,
    status: "active",
  },
  {
    id: 5,
    username: "contractor-temp",
    email: "ext.partner@company.internal",
    department: "External Vendor",
    role: "Developer",
    twoFactorEnabled: false,
    status: "suspended",
  },
];

export const INITIAL_AGENT_SWARM_TASKS: import("../index").AgentSandboxTask[] = [
  {
    id: 901,
    agent: "Claude Code",
    branch: "agent/claude/zero-copy-pack-delta",
    baseBranch: "main",
    forkLatencyMs: 0.34,
    promptSummary: "Implement O(1) memory-mapped packfile delta resolution in gitust-git",
    sandboxStatus: "verified",
    compileTimeMs: 78,
    testsPassed: 42,
    testsTotal: 42,
    filesChanged: 4,
    additions: 186,
    deletions: 29,
    updatedAt: "4s ago",
  },
  {
    id: 902,
    agent: "Cursor Agent",
    branch: "agent/cursor/mcp-batch-commit-rpc",
    baseBranch: "main",
    forkLatencyMs: 0.41,
    promptSummary: "Add single-roundtrip MCP tool for multi-file atomic commit + sandbox run",
    sandboxStatus: "verified",
    compileTimeMs: 92,
    testsPassed: 18,
    testsTotal: 18,
    filesChanged: 3,
    additions: 142,
    deletions: 11,
    updatedAt: "9s ago",
  },
  {
    id: 903,
    agent: "Claude Code",
    branch: "agent/claude/sandbox-cgroup-quota",
    baseBranch: "main",
    forkLatencyMs: 0.29,
    promptSummary: "Enforce per-agent CPU/memory limits and network egress firewall in gitust-sandbox",
    sandboxStatus: "compiling",
    compileTimeMs: 64,
    testsPassed: 12,
    testsTotal: 15,
    filesChanged: 5,
    additions: 230,
    deletions: 44,
    updatedAt: "just now",
  },
  {
    id: 904,
    agent: "Codex Swarm",
    branch: "agent/codex/sqlx-batch-loader",
    baseBranch: "main",
    forkLatencyMs: 0.38,
    promptSummary: "Eliminate N+1 queries in PR review comment resolver using SQLx DataLoader",
    sandboxStatus: "verified",
    compileTimeMs: 81,
    testsPassed: 31,
    testsTotal: 31,
    filesChanged: 2,
    additions: 94,
    deletions: 68,
    updatedAt: "18s ago",
  },
  {
    id: 905,
    agent: "Cursor Agent",
    branch: "agent/cursor/oidc-group-sync",
    baseBranch: "main",
    forkLatencyMs: 0.45,
    promptSummary: "Sync Okta/Keycloak SAML groups with Enterprise Team RBAC permissions",
    sandboxStatus: "failed",
    compileTimeMs: 105,
    testsPassed: 14,
    testsTotal: 16,
    filesChanged: 3,
    additions: 115,
    deletions: 19,
    updatedAt: "32s ago",
  },
  {
    id: 906,
    agent: "Internal Bot",
    branch: "agent/bot/deps-security-patch",
    baseBranch: "main",
    forkLatencyMs: 0.31,
    promptSummary: "Upgrade russh and ring cryptographic primitives for zero-CVE compliance",
    sandboxStatus: "merged",
    compileTimeMs: 71,
    testsPassed: 64,
    testsTotal: 64,
    filesChanged: 2,
    additions: 28,
    deletions: 28,
    updatedAt: "2m ago",
  },
];
