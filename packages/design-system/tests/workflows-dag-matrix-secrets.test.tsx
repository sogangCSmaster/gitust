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
import { LiveRepositoryActionsView } from "../src/showcase/LiveRepositoryActionsView";
import { LiveWorkflowsView } from "../src/showcase/LiveWorkflowsView";
import {
  deleteWorkflowSecret,
  fetchRepositoryWorkflows,
  fetchWorkflowArtifacts,
  fetchWorkflowSecrets,
  putWorkflowSecret,
  triggerWorkflowRun,
  type LiveRepositoryWorkflowsOverview,
  type LiveWorkflowArtifact,
  type LiveWorkflowRunRecord,
  type LiveWorkflowSecretItem,
} from "../src/showcase/live-workflows-client";
import { actionsPage } from "./actions-page-fixture";

const REPO_ID = "11111111-1111-4111-8111-111111111111";
const RUN_ID_1 = "aaaa1111-1111-4111-8111-111111111111";
const RUN_ID_2 = "bbbb2222-2222-4222-8222-222222222222";
const COMMIT_SHA = "c".repeat(40);

const makeArtifact = (
  overrides: Partial<LiveWorkflowArtifact> = {},
): LiveWorkflowArtifact => ({
  name: "dist-bundle",
  path: "target/release/gitust-bundle.tar.zst",
  size_bytes: 4096,
  content_preview: "bundle-manifest: sha256=9f86d081884c7d659a2feaa0c55ad015",
  ...overrides,
});

const makeRunRecord = (
  overrides: Partial<LiveWorkflowRunRecord> = {},
): LiveWorkflowRunRecord => ({
  run_id: RUN_ID_1,
  repo_id: REPO_ID,
  workflow_name: "CI Pipeline",
  commit_sha: COMMIT_SHA,
  source_ref: "refs/heads/main",
  status: "passed",
  created_at: "2026-10-18T07:00:00Z",
  jobs: [
    {
      job_id: "lint",
      display_name: "Lint & Format",
      needs: [],
      matrix_values: {},
      status: "passed",
      cache_hit: true,
      artifacts: [],
      redacted_output: "cargo fmt --check passed",
      duration_ms: 420,
    },
    {
      job_id: "build",
      display_name: "Build (ubuntu, stable)",
      needs: ["lint"],
      matrix_values: { os: "ubuntu", toolchain: "stable" },
      status: "passed",
      cache_hit: false,
      artifacts: [makeArtifact()],
      redacted_output: "Deploying with token *** and key ***",
      duration_ms: 1380,
    },
  ],
  ...overrides,
});

const makeOverview = (
  overrides: Partial<LiveRepositoryWorkflowsOverview> = {},
): LiveRepositoryWorkflowsOverview => ({
  workflows: [
    {
      name: "CI Pipeline",
      on: ["push", "pull_request"],
      jobs: [
        {
          id: "lint",
          name: "Lint & Format",
          needs: [],
          matrix: null,
          secrets: [],
          cache_key: "cargo-fmt-v1",
          artifact_name: null,
          steps_count: 2,
        },
        {
          id: "build",
          name: "Matrix Build",
          needs: ["lint"],
          matrix: { os: ["ubuntu", "macos"], toolchain: ["stable"] },
          secrets: ["DEPLOY_TOKEN"],
          cache_key: "cargo-target-v1",
          artifact_name: "dist-bundle",
          steps_count: 4,
        },
      ],
    },
  ],
  runs: [makeRunRecord()],
  ...overrides,
});

const jsonResponse = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

describe("Slice 7: GitHub Actions-Compatible Workflows (DAG, Matrix, Secrets, Artifacts & Caches)", () => {
  beforeEach(() => {
    window.localStorage.setItem("gitust_token", "gitust_ses_workflow_admin");
    window.localStorage.setItem("gitust_csrf_token", "csrf_workflow_token_888");
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    window.localStorage.clear();
  });

  it("calls workflows, workflow-runs, artifacts, and workflow-secrets APIs and rejects malformed payloads", async () => {
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

      if (url === `/api/v1/repos/${REPO_ID}/workflows` && method === "GET") {
        return jsonResponse(makeOverview());
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/workflow-runs` &&
        method === "POST"
      ) {
        return jsonResponse(makeRunRecord({ run_id: RUN_ID_2 }), 201);
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/workflow-runs/${RUN_ID_1}/artifacts` &&
        method === "GET"
      ) {
        return jsonResponse({ artifacts: [makeArtifact()] });
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/workflow-secrets` &&
        method === "GET"
      ) {
        return jsonResponse({
          secrets: [{ name: "DEPLOY_TOKEN", updated_at: "2026-10-18T06:00:00Z" }],
        });
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/workflow-secrets/NPM_TOKEN` &&
        method === "PUT"
      ) {
        return jsonResponse({
          name: "NPM_TOKEN",
          updated_at: "2026-10-18T07:10:00Z",
        });
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/workflow-secrets/DEPLOY_TOKEN` &&
        method === "DELETE"
      ) {
        return new Response(null, { status: 204 });
      }
      return jsonResponse({}, 404);
    });

    const overview = await fetchRepositoryWorkflows(REPO_ID);
    expect(overview.workflows).toHaveLength(1);
    expect(overview.workflows[0]?.jobs[1]?.needs).toEqual(["lint"]);
    expect(overview.runs[0]?.jobs[1]?.redacted_output).toContain("***");

    const triggered = await triggerWorkflowRun(REPO_ID, {
      expected_commit: COMMIT_SHA,
      source_ref: "refs/heads/main",
      workflow_name: "CI Pipeline",
    });
    expect(triggered.run_id).toBe(RUN_ID_2);

    const artifacts = await fetchWorkflowArtifacts(REPO_ID, RUN_ID_1);
    expect(artifacts).toHaveLength(1);
    expect(artifacts[0]?.name).toBe("dist-bundle");

    const secrets = await fetchWorkflowSecrets(REPO_ID);
    expect(secrets).toEqual([
      { name: "DEPLOY_TOKEN", updated_at: "2026-10-18T06:00:00Z" },
    ]);

    const upserted = await putWorkflowSecret(
      REPO_ID,
      "NPM_TOKEN",
      "super-secret-value-do-not-leak",
    );
    expect(upserted.name).toBe("NPM_TOKEN");

    await deleteWorkflowSecret(REPO_ID, "DEPLOY_TOKEN");

    expect(calls).toHaveLength(6);
    expect(calls[1]?.csrf).toBe("csrf_workflow_token_888");
    expect(calls[4]?.csrf).toBe("csrf_workflow_token_888");
    expect(calls[5]?.csrf).toBe("csrf_workflow_token_888");

    // Reject malformed workflow overview payload
    vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
      jsonResponse({ workflows: "invalid", runs: [] }),
    );
    await expect(fetchRepositoryWorkflows(REPO_ID)).rejects.toThrow(
      /invalid workflow/i,
    );
  });

  it("renders LiveWorkflowsView with DAG needs, matrix, cache hit/miss, redacted logs, artifact inspection, run trigger, and encrypted secret lifecycle", async () => {
    let currentOverview = makeOverview();
    let currentSecrets: LiveWorkflowSecretItem[] = [
      { name: "DEPLOY_TOKEN", updated_at: "2026-10-18T06:00:00Z" },
    ];
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
        return jsonResponse(currentOverview);
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/workflow-secrets` &&
        method === "GET"
      ) {
        return jsonResponse({ secrets: currentSecrets });
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/workflow-runs` &&
        method === "POST"
      ) {
        const newRun = makeRunRecord({
          run_id: RUN_ID_2,
          workflow_name: "CI Pipeline",
        });
        currentOverview = {
          ...currentOverview,
          runs: [newRun, ...currentOverview.runs],
        };
        return jsonResponse(newRun, 201);
      }
      if (
        url === `/api/v1/repos/${REPO_ID}/workflow-runs/${RUN_ID_1}/artifacts` &&
        method === "GET"
      ) {
        return jsonResponse({ artifacts: [makeArtifact()] });
      }
      if (
        url.startsWith(`/api/v1/repos/${REPO_ID}/workflow-secrets/`) &&
        method === "PUT"
      ) {
        const secretName = decodeURIComponent(url.split("/").pop()!);
        const item = {
          name: secretName,
          updated_at: "2026-10-18T07:30:00Z",
        };
        currentSecrets = [
          ...currentSecrets.filter((s) => s.name !== secretName),
          item,
        ];
        return jsonResponse(item);
      }
      if (
        url.startsWith(`/api/v1/repos/${REPO_ID}/workflow-secrets/`) &&
        method === "DELETE"
      ) {
        const secretName = decodeURIComponent(url.split("/").pop()!);
        currentSecrets = currentSecrets.filter((s) => s.name !== secretName);
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

    // Verify DAG needs, matrix axes, cache hit/miss, and redacted output render
    await waitFor(() => {
      expect(screen.getAllByText("CI Pipeline").length).toBeGreaterThanOrEqual(
        1,
      );
      expect(screen.getByText(/needs: lint/i)).toBeTruthy();
      expect(screen.getByText(/os: ubuntu, macos/i)).toBeTruthy();
      expect(screen.getByText("Cache Hit")).toBeTruthy();
      expect(screen.getByText("Cache Miss")).toBeTruthy();
      expect(
        screen.getByText(/Deploying with token \*\*\* and key \*\*\*/i),
      ).toBeTruthy();
    });

    // Inspect artifacts for RUN_ID_1
    await fireEvent.click(
      screen.getByRole("button", { name: /Inspect artifacts/i }),
    );
    await waitFor(() => {
      expect(
        screen.getByText(/target\/release\/gitust-bundle\.tar\.zst/i),
      ).toBeTruthy();
      expect(
        screen.getByText(/bundle-manifest: sha256=9f86d081884c7d659a2feaa0c55ad015/i),
      ).toBeTruthy();
    });

    // Trigger a workflow run
    await fireEvent.click(
      screen.getByRole("button", { name: /Run Workflow/i }),
    );
    await waitFor(() => {
      expect(screen.getByText(RUN_ID_2.slice(0, 8))).toBeTruthy();
    });
    const postRunCall = calls.find(
      (c) =>
        c.url === `/api/v1/repos/${REPO_ID}/workflow-runs` &&
        c.method === "POST",
    );
    expect(postRunCall?.body).toEqual({
      expected_commit: COMMIT_SHA,
      source_ref: "refs/heads/main",
      workflow_name: "CI Pipeline",
    });

    // Add an encrypted secret and verify plaintext is never rendered after save
    const secretPlaintext = "ghp_super_secret_never_shown_in_ui_999";
    await fireEvent.input(screen.getByLabelText("Secret name"), {
      target: { value: "RELEASE_SIGNING_KEY" },
    });
    await fireEvent.input(screen.getByLabelText("Secret value"), {
      target: { value: secretPlaintext },
    });
    await fireEvent.click(screen.getByRole("button", { name: /Save secret/i }));

    await waitFor(() => {
      expect(screen.getByText("RELEASE_SIGNING_KEY")).toBeTruthy();
    });
    expect(
      (screen.getByLabelText("Secret value") as HTMLInputElement).value,
    ).toBe("");
    expect(document.body.textContent).not.toContain(secretPlaintext);

    // Delete DEPLOY_TOKEN secret
    await fireEvent.click(
      screen.getByRole("button", { name: /Delete secret DEPLOY_TOKEN/i }),
    );
    await waitFor(() => {
      expect(screen.queryByText("DEPLOY_TOKEN")).toBeNull();
    });
  });

  it("exposes the Workflows tab inside LiveRepositoryActionsView and lazy-loads LiveWorkflowsView", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const url = typeof input === "string" ? input : input.url;
      if (url.includes("/repo/automatic-runs")) {
        return jsonResponse({ ...actionsPage(), repo_id: REPO_ID });
      }
      if (url === `/api/v1/repos/${REPO_ID}/workflows`) {
        return jsonResponse(makeOverview());
      }
      if (url === `/api/v1/repos/${REPO_ID}/workflow-secrets`) {
        return jsonResponse({ secrets: [] });
      }
      return jsonResponse([]);
    });

    render(() => (
      <>
        <IconSpriteSheet />
        <LiveRepositoryActionsView
          repositoryId={REPO_ID}
          role="SiteAdmin"
          isPrimary={true}
          selectionEpoch={1}
          canRunChecks={true}
          defaultRef="refs/heads/main"
          defaultCommit={COMMIT_SHA}
        />
      </>
    ));

    const workflowsTab = await screen.findByRole("tab", { name: "Workflows" });
    await fireEvent.click(workflowsTab);

    await waitFor(() => {
      expect(screen.getByText("Cache Hit")).toBeTruthy();
      expect(screen.getByText(/needs: lint/i)).toBeTruthy();
    });

    cleanup();

    // Verify secondary writable repository (isPrimary=false, canRunChecks=true) also exposes Local Checks and Workflows tabs
    render(() => (
      <>
        <IconSpriteSheet />
        <LiveRepositoryActionsView
          repositoryId={REPO_ID}
          role="SiteAdmin"
          isPrimary={false}
          selectionEpoch={2}
          canRunChecks={true}
          defaultRef="refs/heads/main"
          defaultCommit={COMMIT_SHA}
        />
      </>
    ));

    expect(screen.queryByRole("tab", { name: "Automatic" })).toBeNull();
    const secondaryWorkflowsTab = await screen.findByRole("tab", {
      name: "Workflows",
    });
    await fireEvent.click(secondaryWorkflowsTab);

    await waitFor(() => {
      expect(screen.getByText("Cache Hit")).toBeTruthy();
      expect(screen.getByText(/needs: lint/i)).toBeTruthy();
    });
  });
});
