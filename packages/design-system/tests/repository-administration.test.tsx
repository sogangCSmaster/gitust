// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@solidjs/testing-library";
import { createRoot } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IconSpriteSheet } from "../src/components/workspace/icon";
import { RepositoryAdministrationPanel } from "../src/patterns/RepositoryAdministrationPanel";
import {
  createConnectedRepositoryClient,
  type ConnectedRepository,
  type ConnectedRepositoryState,
} from "../src/showcase/connected-repository-client";
import {
  createHostedRepository,
  fetchRepositorySettings,
  updateRepositorySettings,
  type LiveRepositorySettings,
} from "../src/showcase/live-repo-admin-client";
import { createLiveSwarmSession } from "../src/showcase/live-swarm-session";
import { RepositoryOverview } from "../src/showcase/RepositoryOverview";
import { createRepositoryWorkspaceCore } from "../src/showcase/repository-workspace-core";

const REPO_ID = "11111111-1111-4111-8111-111111111111";
const TEAM_ID = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const HEAD_SHA = "b".repeat(40);

const makeCatalogRow = (overrides: Record<string, unknown> = {}) => ({
  id: REPO_ID,
  name: "core-engine",
  branch: "main",
  head_commit: HEAD_SHA,
  capability: "workbench",
  is_default: true,
  ...overrides,
});

const makeSettings = (
  overrides: Partial<LiveRepositorySettings> = {},
): LiveRepositorySettings => ({
  id: REPO_ID,
  name: "core-engine",
  description: "High-assurance Git engine and sandbox runtime.",
  default_branch: "main",
  changeable_branches: ["main", "release/v1"],
  is_private: true,
  archived: false,
  git_write: true,
  workbench_enabled: true,
  is_default: true,
  ...overrides,
});

const jsonResponse = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

describe("Slice 5: Repository Administration UI, Online Inventory Lifecycle & Worktree Routing", () => {
  beforeEach(() => {
    window.localStorage.setItem("gitust_token", "gitust_ses_admin_token");
    window.localStorage.setItem("gitust_csrf_token", "csrf_admin_token_999");
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    window.localStorage.clear();
  });

  it("parses can_spawn_worktree, archived, can_administer, and description in createConnectedRepositoryClient and rejects malformed types", async () => {
    const states: ConnectedRepositoryState[] = [];
    const client = createConnectedRepositoryClient((s) => states.push(s));

    vi.spyOn(globalThis, "fetch")
      .mockResolvedValueOnce(
        jsonResponse({
          repositories: [
            makeCatalogRow({
              can_spawn_worktree: true,
              archived: false,
              can_administer: true,
              description: "Primary workbench repository",
            }),
          ],
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse({
          repositories: [makeCatalogRow({ archived: "yes" })],
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse({
          repositories: [makeCatalogRow({ can_spawn_worktree: 1 })],
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse({
          repositories: [makeCatalogRow({ description: 123 })],
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse({
          repositories: [makeCatalogRow({ description: "x".repeat(2049) })],
        }),
      );

    await client.refresh();
    expect(states.at(-1)?.repositories[0]).toMatchObject({
      id: REPO_ID,
      canSpawnWorktree: true,
      archived: false,
      canAdminister: true,
      description: "Primary workbench repository",
    });

    for (let i = 0; i < 4; i++) {
      await client.refresh();
      expect(states.at(-1)?.repositories).toEqual([]);
      expect(states.at(-1)?.error).toMatch(/invalid repository metadata/i);
    }
  });

  it("calls GET/PATCH /api/v1/repos/{id}/settings and POST /api/v1/admin/repos via live-repo-admin-client", async () => {
    const calls: Array<{ url: string; method: string; body?: unknown }> = [];
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
      const url = typeof input === "string" ? input : input.url;
      const method = init?.method ?? "GET";
      const body =
        typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
      calls.push({ url, method, body });

      if (url === `/api/v1/repos/${REPO_ID}/settings` && method === "GET") {
        return jsonResponse(makeSettings());
      }
      if (url === `/api/v1/repos/${REPO_ID}/settings` && method === "PATCH") {
        return jsonResponse(
          makeSettings(body as Partial<LiveRepositorySettings>),
        );
      }
      if (url === "/api/v1/admin/repos" && method === "POST") {
        const payload = body as {
          name: string;
          description?: string;
          is_private: boolean;
          default_branch?: string;
          workbench_enabled?: boolean;
        };
        return jsonResponse(
          makeSettings({
            id: "33333333-3333-4333-8333-333333333333",
            name: payload.name,
            description: payload.description ?? null,
            is_private: payload.is_private,
            default_branch: payload.default_branch ?? "main",
            workbench_enabled: payload.workbench_enabled ?? false,
            is_default: false,
          }),
          201,
        );
      }
      return jsonResponse({}, 404);
    });

    const loaded = await fetchRepositorySettings(REPO_ID);
    expect(loaded.description).toBe(
      "High-assurance Git engine and sandbox runtime.",
    );

    const updated = await updateRepositorySettings(REPO_ID, {
      description: "Updated repo description",
      workbench_enabled: false,
      archived: true,
    });
    expect(updated.description).toBe("Updated repo description");
    expect(updated.workbench_enabled).toBe(false);
    expect(updated.archived).toBe(true);

    const created = await createHostedRepository({
      name: "online-provisioned-repo",
      owner_team_id: TEAM_ID,
      is_private: true,
      default_branch: "main",
      description: "Created online",
      workbench_enabled: true,
    });
    expect(created.name).toBe("online-provisioned-repo");
    expect(created.workbench_enabled).toBe(true);
    expect(calls).toHaveLength(3);
  });

  it("renders RepositoryOverview badges and administers repository settings and online provisioning", async () => {
    let currentSettings = makeSettings();
    let refreshCount = 0;
    const calls: Array<{ url: string; method: string; body?: unknown }> = [];

    vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
      const url = typeof input === "string" ? input : input.url;
      const method = init?.method ?? "GET";
      const body =
        typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
      calls.push({ url, method, body });

      if (url === `/api/v1/repos/${REPO_ID}/settings` && method === "GET") {
        return jsonResponse(currentSettings);
      }
      if (url === `/api/v1/repos/${REPO_ID}/settings` && method === "PATCH") {
        currentSettings = {
          ...currentSettings,
          ...(body as Partial<LiveRepositorySettings>),
        };
        return jsonResponse(currentSettings);
      }
      if (url === "/api/v1/admin/repos" && method === "POST") {
        const payload = body as {
          name: string;
          description?: string;
          is_private: boolean;
          default_branch?: string;
          workbench_enabled?: boolean;
        };
        return jsonResponse(
          makeSettings({
            id: "44444444-4444-4444-8444-444444444444",
            name: payload.name,
            description: payload.description ?? null,
            is_private: payload.is_private,
            default_branch: payload.default_branch ?? "main",
            workbench_enabled: payload.workbench_enabled ?? false,
            is_default: false,
          }),
          201,
        );
      }
      return jsonResponse({}, 404);
    });

    const repo: ConnectedRepository = {
      id: REPO_ID,
      name: "core-engine",
      branch: "main",
      headCommit: HEAD_SHA,
      capability: "workbench",
      isDefault: true,
      canAdminister: true,
      canSpawnWorktree: true,
      archived: true,
      description: "High-assurance Git engine and sandbox runtime.",
    };

    render(() => (
      <>
        <IconSpriteSheet />
        <RepositoryOverview
          repository={repo}
          liveMode={true}
          worktrees={2}
          onNavigate={() => {}}
          actions={{
            rename: {
              open: () => null,
              submit: async () => ({ kind: "stale" }),
              cancel: () => {},
            },
            refreshViews: () => {
              refreshCount++;
            },
            clone: () => null,
          }}
        />
      </>
    ));

    // Verify Archived and Workbench Enabled badges are visible on RepositoryOverview
    expect(screen.getByText("Archived")).toBeTruthy();
    expect(screen.getByText("Workbench Enabled")).toBeTruthy();
    expect(
      screen.getByText("High-assurance Git engine and sandbox runtime."),
    ).toBeTruthy();

    // Open the Administration & Lifecycle panel
    await fireEvent.click(
      screen.getByRole("button", { name: /Admin & lifecycle/i }),
    );

    await waitFor(() => {
      expect(screen.getByLabelText("Repository description")).toBeTruthy();
    });

    // Update description, toggle workbench_enabled and archived, and save settings
    await fireEvent.input(screen.getByLabelText("Repository description"), {
      target: { value: "Multi-repo worktree routing enabled." },
    });
    await fireEvent.click(
      screen.getByLabelText("Archive repository (read-only)"),
    );
    await fireEvent.click(
      screen.getByRole("button", { name: /Save repository settings/i }),
    );

    await waitFor(() => {
      const patchCall = calls.find(
        (c) =>
          c.url === `/api/v1/repos/${REPO_ID}/settings` &&
          c.method === "PATCH",
      );
      expect(patchCall).toBeTruthy();
      expect(patchCall?.body).toMatchObject({
        description: "Multi-repo worktree routing enabled.",
        archived: true,
      });
      expect(refreshCount).toBeGreaterThanOrEqual(1);
    });

    // Provision a new online repository via POST /api/v1/admin/repos
    await fireEvent.input(screen.getByLabelText("New repository name"), {
      target: { value: "secondary-workbench" },
    });
    await fireEvent.input(screen.getByLabelText("Owner team UUID"), {
      target: { value: TEAM_ID },
    });
    await fireEvent.click(
      screen.getByLabelText("Enable worktree execution on new repository"),
    );
    await fireEvent.click(
      screen.getByRole("button", { name: /Provision repository/i }),
    );

    await waitFor(() => {
      const postCall = calls.find(
        (c) => c.url === "/api/v1/admin/repos" && c.method === "POST",
      );
      expect(postCall).toBeTruthy();
      expect(postCall?.body).toEqual({
        name: "secondary-workbench",
        owner_team_id: TEAM_ID,
        is_private: true,
        default_branch: "main",
        description: "",
        workbench_enabled: true,
      });
      expect(refreshCount).toBeGreaterThanOrEqual(2);
    });
  });

  it("renders RepositoryAdministrationPanel standalone and handles errors gracefully", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
      new Response("Forbidden", { status: 403 }),
    );

    render(() => (
      <>
        <IconSpriteSheet />
        <RepositoryAdministrationPanel
          repositoryId={REPO_ID}
          repositoryName="core-engine"
        />
      </>
    ));

    await waitFor(() => {
      expect(
        screen.getByText(/Unable to load repository settings/i),
      ).toBeTruthy();
    });
  });

  it("disables workbench when primary repository is archived or canSpawnWorktree=false and scopes live swarm worktrees per selected repository", async () => {
    const SECONDARY_ID = "22222222-2222-4222-8222-222222222222";
    const WT_PRIMARY = "55555555-5555-4555-8555-555555555555";
    const WT_SECONDARY = "66666666-6666-4666-8666-666666666666";
    const WT_CREATED = "77777777-7777-4777-8777-777777777777";

    const hydratedIds: string[] = [];
    let activeTab = "repositories";

    const fetchSpy = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation(async (input, init) => {
        const url = typeof input === "string" ? input : input.url;
        const method = init?.method ?? "GET";
        if (url === "/api/v1/repos") {
          return jsonResponse({
            repositories: [
              makeCatalogRow({
                id: REPO_ID,
                name: "primary-archived",
                capability: "workbench",
                is_default: true,
                archived: true,
                can_spawn_worktree: false,
              }),
              makeCatalogRow({
                id: SECONDARY_ID,
                name: "secondary-workbench",
                capability: "browse",
                is_default: false,
                archived: false,
                can_spawn_worktree: true,
              }),
            ],
          });
        }
        if (url === "/api/v1/swarm/worktrees" && method === "GET") {
          return jsonResponse([
            {
              id: WT_PRIMARY,
              repo_id: REPO_ID,
              agent: "CodexSwarm",
              branch: "agent/primary-wt",
              base_branch: "main",
              base_commit: HEAD_SHA,
              prompt_summary: "Primary repo worktree",
              materialize_micros: 120,
              status: "pending",
              last_sandbox_duration_ms: 0,
              tests_passed: 0,
              tests_total: 0,
              files_changed: 0,
              additions: 0,
              deletions: 0,
              github_synced: false,
              created_at: "2026-10-18T00:00:00Z",
            },
            {
              id: WT_SECONDARY,
              repo_id: SECONDARY_ID,
              agent: "ClaudeCode",
              branch: "agent/secondary-wt",
              base_branch: "main",
              base_commit: HEAD_SHA,
              prompt_summary: "Secondary repo worktree",
              materialize_micros: 150,
              status: "pending",
              last_sandbox_duration_ms: 0,
              tests_passed: 0,
              tests_total: 0,
              files_changed: 0,
              additions: 0,
              deletions: 0,
              github_synced: false,
              created_at: "2026-10-18T00:00:00Z",
            },
          ]);
        }
        if (url === "/api/v1/swarm/worktrees" && method === "POST") {
          const payload = JSON.parse(String(init?.body ?? "{}"));
          return jsonResponse(
            {
              id: WT_CREATED,
              repo_id: payload.repo_id,
              agent: payload.agent,
              branch: payload.branch,
              base_branch: payload.base_branch,
              base_commit: HEAD_SHA,
              prompt_summary: payload.prompt_summary,
              materialize_micros: 90,
              status: "pending",
              last_sandbox_duration_ms: 0,
              tests_passed: 0,
              tests_total: 0,
              files_changed: 0,
              additions: 0,
              deletions: 0,
              github_synced: false,
              created_at: "2026-10-18T00:00:00Z",
            },
            201,
          );
        }
        return jsonResponse({}, 404);
      });

    let dispose = () => {};
    const core = createRoot((d) => {
      dispose = d;
      return createRepositoryWorkspaceCore({
        liveMode: () => true,
        setActiveTab: (tab) => {
          activeTab = tab;
        },
        clearView: () => {},
        hydrate: (repo) => {
          hydratedIds.push(repo.id);
        },
      });
    });

    await core.refresh();
    const repos = core.state().repositories;
    expect(repos).toHaveLength(2);

    // Primary repository is archived & canSpawnWorktree=false -> canWorkbench() MUST be false and browseOnly() MUST be true
    core.open(repos[0]!, "swarm");
    expect(core.selected()?.id).toBe(REPO_ID);
    expect(core.canWorkbench()).toBe(false);
    expect(core.browseOnly()).toBe(true);
    expect(activeTab).toBe("overview");

    // Secondary repository has canSpawnWorktree=true & archived=false -> canWorkbench() MUST be true and browseOnly() MUST be false
    core.open(repos[1]!, "swarm");
    expect(core.selected()?.id).toBe(SECONDARY_ID);
    expect(core.canWorkbench()).toBe(true);
    expect(core.browseOnly()).toBe(false);
    expect(activeTab).toBe("swarm");
    expect(hydratedIds).toEqual([REPO_ID, SECONDARY_ID]);

    // Verify liveSwarmSession scopes worktree hydration and worktree spawn to the selected repository
    let tasks: ReturnType<typeof core.state>["repositories"] extends unknown
      ? import("../src/patterns/AgentSwarmView").AgentSandboxTask[]
      : never = [];
    const swarm = createLiveSwarmSession({
      getRepositoryId: () => core.selected()?.id,
      getSwarmTasks: () => tasks,
      setSwarmTasks: (updater) => {
        tasks = typeof updater === "function" ? updater(tasks) : updater;
      },
      showQuickFeedback: () => {},
    });

    await swarm.hydrateSwarmWorktrees();
    expect(tasks.map((t) => t.liveId)).toEqual([WT_SECONDARY]);

    const spawnResult = await swarm.spawnLiveWorktree({
      repoId: "",
      agent: "ClaudeCode",
      branch: "agent/new-secondary-task",
      baseBranch: "main",
      promptSummary: "Spawned on secondary repo",
    });
    expect(spawnResult).toMatchObject({
      kind: "created",
      worktreeId: WT_CREATED,
    });
    const postCall = fetchSpy.mock.calls.find(
      ([u, i]) => u === "/api/v1/swarm/worktrees" && i?.method === "POST",
    );
    expect(JSON.parse(String(postCall?.[1]?.body))).toMatchObject({
      repo_id: SECONDARY_ID,
      branch: "agent/new-secondary-task",
    });
    dispose();
  });
});
