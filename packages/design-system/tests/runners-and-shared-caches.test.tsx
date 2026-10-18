// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IconSpriteSheet } from "../src/components/workspace/icon";
import { LiveWorkflowsView } from "../src/showcase/LiveWorkflowsView";
import {
  deleteRunnerNode,
  evictRepositoryCache,
  fetchRepositoryCaches,
  fetchRunnerFleet,
  registerRunnerNode,
  updateRunnerHeartbeat,
  type LiveRunnerNodeRecord,
  type LiveSharedCacheEntry,
} from "../src/showcase/live-runners-client";
import type { LiveRepositoryWorkflowsOverview } from "../src/showcase/live-workflows-client";

const REPO_ID = "11111111-1111-4111-8111-111111111111";
const RUN_ID = "aaaa1111-1111-4111-8111-111111111111";
const COMMIT_SHA = "d".repeat(40);

const makeRunner = (
  overrides: Partial<LiveRunnerNodeRecord> = {},
): LiveRunnerNodeRecord => ({
  id: "00000000-0000-0000-0000-000000000001",
  name: "built-in-local",
  labels: ["local", "linux-vm"],
  supported_tiers: ["host_os", "container_vm"],
  max_concurrency: 4,
  active_leases: 0,
  status: "online",
  registered_at: "2026-10-18T08:00:00Z",
  last_heartbeat_at: "2026-10-18T08:05:00Z",
  ...overrides,
});

const makeCacheEntry = (
  overrides: Partial<LiveSharedCacheEntry> = {},
): LiveSharedCacheEntry => ({
  key: "cargo-release-v1",
  digest_sha256: "e".repeat(64),
  size_bytes: 8192,
  files_count: 14,
  created_at_epoch_secs: 1792310400,
  source_job_id: "build",
  ...overrides,
});

const makeOverview = (): LiveRepositoryWorkflowsOverview => ({
  workflows: [
    {
      name: "Isolated Build Pipeline",
      on: ["push"],
      jobs: [
        {
          id: "build",
          name: "Isolated VM Build",
          needs: [],
          runs_on: "linux-vm",
          isolation_tier: "container_vm",
          matrix: null,
          secrets: [],
          cache_key: "cargo-release-v1",
          artifact_name: "vm-binary",
          steps_count: 3,
        },
      ],
    },
  ],
  runs: [
    {
      run_id: RUN_ID,
      repo_id: REPO_ID,
      workflow_name: "Isolated Build Pipeline",
      commit_sha: COMMIT_SHA,
      source_ref: "refs/heads/main",
      status: "passed",
      created_at: "2026-10-18T08:10:00Z",
      jobs: [
        {
          job_id: "build",
          display_name: "Isolated VM Build",
          needs: [],
          matrix_values: {},
          runner_id: "vm-runner-1",
          isolation_tier: "container_vm",
          status: "passed",
          cache_hit: true,
          artifacts: [],
          redacted_output: "Isolated container_vm build completed cleanly",
          duration_ms: 910,
        },
      ],
    },
  ],
});

const jsonResponse = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

describe("Slice 8: Stronger Execution Isolation Tier, Multi-Node Runner Scheduling & Shared Build Caches", () => {
  beforeEach(() => {
    window.localStorage.setItem("gitust_token", "gitust_ses_runner_admin");
    window.localStorage.setItem("gitust_csrf_token", "csrf_runner_token_555");
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    window.localStorage.clear();
  });

  it("calls runner fleet and shared build cache APIs and rejects malformed payloads", async () => {
    const calls: Array<{
      url: string;
      method: string;
      csrf: string | null;
      body?: unknown;
    }> = [];

    vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
      const url = typeof input === "string" ? input : input.url;
      const method = init?.method ?? "GET";
      const headers = new Headers(init?.headers);
      const csrf = headers.get("X-CSRF-Token");
      const body =
        typeof init?.body === "string" && init.body.length > 0
          ? JSON.parse(init.body)
          : undefined;
      calls.push({ url, method, csrf, body });

      if (url === "/api/v1/admin/runners" && method === "GET") {
        return jsonResponse({ runners: [makeRunner()] });
      }
      if (url === "/api/v1/admin/runners" && method === "POST") {
        const payload = body as {
          name: string;
          labels: string[];
          supported_tiers: Array<"host_os" | "container_vm">;
          max_concurrency: number;
        };
        return jsonResponse(
          makeRunner({
            id: "runner-vm-01",
            name: payload.name,
            labels: payload.labels,
            supported_tiers: payload.supported_tiers,
            max_concurrency: payload.max_concurrency,
          }),
          201,
        );
      }
      if (
        url === "/api/v1/admin/runners/runner-vm-01/heartbeat" &&
        method === "POST"
      ) {
        const payload = body as { status: "online" | "draining" | "offline" };
        return jsonResponse(
          makeRunner({
            id: "runner-vm-01",
            name: "vm-runner-1",
            status: payload.status,
          }),
        );
      }
      if (url === "/api/v1/admin/runners/runner-vm-01" && method === "DELETE") {
        return new Response(null, { status: 204 });
      }
      if (url === `/api/v1/repos/${REPO_ID}/caches` && method === "GET") {
        return jsonResponse({ caches: [makeCacheEntry()] });
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/caches/cargo-release-v1` &&
        method === "DELETE"
      ) {
        return new Response(null, { status: 204 });
      }
      return jsonResponse({}, 404);
    });

    const fleet = await fetchRunnerFleet();
    expect(fleet).toHaveLength(1);
    expect(fleet[0]?.supported_tiers).toContain("container_vm");

    const registered = await registerRunnerNode({
      name: "vm-runner-1",
      labels: ["linux-vm", "gpu"],
      supported_tiers: ["container_vm"],
      max_concurrency: 2,
    });
    expect(registered.id).toBe("runner-vm-01");
    expect(registered.name).toBe("vm-runner-1");

    const drained = await updateRunnerHeartbeat("runner-vm-01", "draining");
    expect(drained.status).toBe("draining");

    await deleteRunnerNode("runner-vm-01");

    const caches = await fetchRepositoryCaches(REPO_ID);
    expect(caches).toHaveLength(1);
    expect(caches[0]?.key).toBe("cargo-release-v1");

    await evictRepositoryCache(REPO_ID, "cargo-release-v1");

    expect(calls).toHaveLength(6);
    expect(calls[1]?.csrf).toBe("csrf_runner_token_555");
    expect(calls[2]?.csrf).toBe("csrf_runner_token_555");
    expect(calls[3]?.csrf).toBe("csrf_runner_token_555");
    expect(calls[5]?.csrf).toBe("csrf_runner_token_555");

    // Reject malformed runner fleet response
    vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
      jsonResponse({ runners: "not-an-array" }),
    );
    await expect(fetchRunnerFleet()).rejects.toThrow(/invalid runner/i);
  });

  it("renders isolation_tier, runs_on, and runner_id in LiveWorkflowsView and manages multi-node runners and shared caches", async () => {
    let runners: LiveRunnerNodeRecord[] = [makeRunner()];
    let caches: LiveSharedCacheEntry[] = [makeCacheEntry()];
    const calls: Array<{ url: string; method: string; body?: unknown }> = [];

    vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
      const url = typeof input === "string" ? input : input.url;
      const method = init?.method ?? "GET";
      const body =
        typeof init?.body === "string" && init.body.length > 0
          ? JSON.parse(init.body)
          : undefined;
      calls.push({ url, method, body });

      if (url === `/api/v1/repos/${REPO_ID}/workflows` && method === "GET") {
        return jsonResponse(makeOverview());
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/workflow-secrets` &&
        method === "GET"
      ) {
        return jsonResponse({ secrets: [] });
      }
      if (url === "/api/v1/admin/runners" && method === "GET") {
        return jsonResponse({ runners });
      }
      if (url === "/api/v1/admin/runners" && method === "POST") {
        const payload = body as {
          name: string;
          labels: string[];
          supported_tiers: Array<"host_os" | "container_vm">;
          max_concurrency: number;
        };
        const created = makeRunner({
          id: "runner-vm-1",
          name: payload.name,
          labels: payload.labels,
          supported_tiers: payload.supported_tiers,
          max_concurrency: payload.max_concurrency,
        });
        runners = [...runners, created];
        return jsonResponse(created, 201);
      }
      if (
        url === "/api/v1/admin/runners/runner-vm-1/heartbeat" &&
        method === "POST"
      ) {
        const payload = body as { status: "online" | "draining" | "offline" };
        runners = runners.map((r) =>
          r.id === "runner-vm-1" ? { ...r, status: payload.status } : r,
        );
        return jsonResponse(runners.find((r) => r.id === "runner-vm-1")!);
      }
      if (url === "/api/v1/admin/runners/runner-vm-1" && method === "DELETE") {
        runners = runners.filter((r) => r.id !== "runner-vm-1");
        return new Response(null, { status: 204 });
      }
      if (url === `/api/v1/repos/${REPO_ID}/caches` && method === "GET") {
        return jsonResponse({ caches });
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/caches/cargo-release-v1` &&
        method === "DELETE"
      ) {
        caches = caches.filter((c) => c.key !== "cargo-release-v1");
        return new Response(null, { status: 204 });
      }
      return jsonResponse({}, 404);
    });

    render(() => (
      <>
        <IconSpriteSheet />
        <LiveWorkflowsView
          repositoryId={REPO_ID}
          role="SiteAdmin"
          defaultRef="refs/heads/main"
          defaultCommit={COMMIT_SHA}
        />
      </>
    ));

    // Verify runs-on, isolation tier, runner_id, built-in runner, and shared cache render
    await waitFor(() => {
      expect(screen.getByText(/runs-on: linux-vm/i)).toBeTruthy();
      expect(screen.getAllByText(/tier: container_vm/i).length).toBeGreaterThanOrEqual(2);
      expect(screen.getByText(/runner: vm-runner-1/i)).toBeTruthy();
      expect(screen.getByText("built-in-local")).toBeTruthy();
      expect(
        screen.queryByRole("button", { name: /Delete runner built-in-local/i }),
      ).toBeNull();
      expect(screen.getByText("cargo-release-v1")).toBeTruthy();
      expect(screen.getByText(/8192 B/i)).toBeTruthy();
      expect(screen.getByText(/14 files/i)).toBeTruthy();
    });

    // Register a new container_vm runner node
    await fireEvent.input(screen.getByLabelText("Runner name"), {
      target: { value: "vm-runner-1" },
    });
    await fireEvent.input(screen.getByLabelText("Runner labels"), {
      target: { value: "linux-vm, high-mem" },
    });
    await fireEvent.change(screen.getByLabelText("Isolation tier"), {
      target: { value: "container_vm" },
    });
    await fireEvent.input(screen.getByLabelText("Max concurrency"), {
      target: { value: "2" },
    });
    await fireEvent.click(
      screen.getByRole("button", { name: /Register runner/i }),
    );

    await waitFor(() => {
      expect(screen.getByText(/labels: linux-vm, high-mem/i)).toBeTruthy();
    });

    // Toggle drain on vm-runner-1 via heartbeat
    await fireEvent.click(
      screen.getByRole("button", { name: /Toggle drain vm-runner-1/i }),
    );
    await waitFor(() => {
      expect(screen.getByText("draining")).toBeTruthy();
    });

    // Delete runner vm-runner-1
    await fireEvent.click(
      screen.getByRole("button", { name: /Delete runner vm-runner-1/i }),
    );
    await waitFor(() => {
      expect(screen.queryByText(/labels: linux-vm, high-mem/i)).toBeNull();
    });

    // Evict shared build cache cargo-release-v1
    await fireEvent.click(
      screen.getByRole("button", { name: /Evict cache cargo-release-v1/i }),
    );
    await waitFor(() => {
      expect(screen.queryByText("8192 B")).toBeNull();
    });
  });
});
